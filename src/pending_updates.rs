//! List pending pacman/AUR upgrades and install them (repo all-at-once, AUR one-by-one).

use crate::config::Config;
use crate::package_pattern::package_matches_any_pattern;
use crate::system::{
    SystemUpdateMode, packages_ignored_during_system_update, transform_system_update_argv,
};
use crate::utils::{
    apply_gui_nested_sudo_askpass, command_exists, pacman_query_version, pacman_sync_version,
    parse_command_argv, prime_sudo_for_session, run_argv_command, spawn_sudo_keepalive, vercmp,
};
use crate::{die, vlog};
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::{self, Write};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateHelper {
    Pacman,
    Yay,
    Paru,
    Pikaur,
}

impl UpdateHelper {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pacman => "pacman",
            Self::Yay => "yay",
            Self::Paru => "paru",
            Self::Pikaur => "pikaur",
        }
    }

    pub fn is_aur_helper(self) -> bool {
        !matches!(self, Self::Pacman)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingPkg {
    pub name: String,
    pub old: String,
    pub new: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedPkg {
    pub name: String,
    pub old: String,
    pub new: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingUpdates {
    pub helper: String,
    pub repo: Vec<PendingPkg>,
    pub aur: Vec<PendingPkg>,
    /// Watched `manual_update_packages` with an upstream/PKGBUILD newer than installed.
    #[serde(default)]
    pub manual: Vec<PendingPkg>,
    pub skipped: Vec<SkippedPkg>,
}

#[cfg(test)]
impl PendingUpdates {
    pub fn has_work(&self) -> bool {
        !self.repo.is_empty() || !self.aur.is_empty() || !self.manual.is_empty()
    }
}

/// First real binary in a system-update command (skip `sudo`).
pub fn helper_from_update_command(cmd: &str) -> UpdateHelper {
    let Ok(argv) = parse_command_argv(cmd) else {
        return UpdateHelper::Pacman;
    };
    helper_from_argv(&argv)
}

fn helper_from_argv(argv: &[String]) -> UpdateHelper {
    let mut i = 0;
    if argv
        .first()
        .is_some_and(|c| c == "sudo" || c.ends_with("/sudo"))
    {
        i = 1;
    }
    let Some(bin) = argv.get(i) else {
        return UpdateHelper::Pacman;
    };
    match bin.rsplit('/').next().unwrap_or(bin) {
        "yay" => UpdateHelper::Yay,
        "paru" => UpdateHelper::Paru,
        "pikaur" => UpdateHelper::Pikaur,
        _ => UpdateHelper::Pacman,
    }
}

/// Parse a `name old -> new` upgrade line (pacman/checkupdates/yay/paru).
pub fn parse_upgrade_line(line: &str) -> Option<(String, String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with("::") || line.starts_with("=>") {
        return None;
    }
    let (left, right) = line.split_once("->")?;
    let new = right.split_whitespace().next()?.to_string();
    let mut parts = left.split_whitespace();
    let name = parts.next()?.to_string();
    let old = parts.next()?.to_string();
    if name.is_empty() || old.is_empty() || new.is_empty() {
        return None;
    }
    Some((name, old, new))
}

fn parse_upgrade_text(text: &str) -> Vec<PendingPkg> {
    text.lines()
        .filter_map(parse_upgrade_line)
        .map(|(name, old, new)| PendingPkg { name, old, new })
        .collect()
}

pub fn skip_reason(config: &Config, name: &str) -> Option<&'static str> {
    if crate::pgo::active_pipeline_hold_packages(config)
        .iter()
        .any(|p| p == name)
    {
        return Some("pgo_pipeline");
    }
    if crate::held::is_held(config, name) {
        return Some("held_packages");
    }
    if package_matches_any_pattern(name, &config.system_update.ignore_packages) {
        return Some("ignore_packages");
    }
    if package_matches_any_pattern(name, &config.manual_update_packages) {
        return Some("manual_update_packages");
    }
    if package_matches_any_pattern(name, &config.skip_install_packages) {
        return Some("skip_install_packages");
    }
    None
}

pub fn classify_pending(
    upgrades: Vec<PendingPkg>,
    config: &Config,
) -> (Vec<PendingPkg>, Vec<SkippedPkg>) {
    let mut keep = Vec::new();
    let mut skipped = Vec::new();
    for pkg in upgrades {
        match skip_reason(config, &pkg.name) {
            Some(reason) => skipped.push(SkippedPkg {
                name: pkg.name,
                old: pkg.old,
                new: pkg.new,
                reason: reason.to_string(),
            }),
            None => keep.push(pkg),
        }
    }
    (keep, skipped)
}

/// Move `manual_update_packages` out of skipped into a pending ABS list, and add any
/// watched packages whose sync version is newer than installed (even if checkupdates
/// did not list them, e.g. because they are already `--ignore`d).
fn collect_manual_pending(
    config: &Config,
    skipped: Vec<SkippedPkg>,
) -> (Vec<PendingPkg>, Vec<SkippedPkg>) {
    let (mut manual, rest, mut seen) = promote_manual_skipped(skipped);
    for name in crate::package_pattern::expand_package_patterns(&config.manual_update_packages) {
        if !seen.insert(name.clone()) {
            continue;
        }
        if matches!(
            skip_reason(config, &name),
            Some("pgo_pipeline" | "held_packages" | "ignore_packages")
        ) {
            continue;
        }
        let Ok(Some(old)) = pacman_query_version(&name) else {
            continue;
        };
        let Ok(Some(new)) = pacman_sync_version(&name) else {
            continue;
        };
        if vercmp(&new, &old).ok().is_some_and(|c| c > 0) {
            manual.push(PendingPkg { name, old, new });
        }
    }
    (manual, rest)
}

fn promote_manual_skipped(
    skipped: Vec<SkippedPkg>,
) -> (Vec<PendingPkg>, Vec<SkippedPkg>, HashSet<String>) {
    let mut manual = Vec::new();
    let mut rest = Vec::new();
    let mut seen = HashSet::new();
    for pkg in skipped {
        if pkg.reason == "manual_update_packages" {
            if seen.insert(pkg.name.clone()) {
                manual.push(PendingPkg {
                    name: pkg.name,
                    old: pkg.old,
                    new: pkg.new,
                });
            }
        } else {
            rest.push(pkg);
        }
    }
    (manual, rest, seen)
}

fn capture_stdout(cmd: &str, args: &[&str]) -> Result<String, String> {
    vlog!("$ {} {}", cmd, args.join(" "));
    let output = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Failed to execute '{cmd}': {e}"))?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn gui_mode() -> bool {
    std::env::var_os("ABS_GUI").is_some()
}

fn maybe_noconfirm(argv: &mut Vec<String>) {
    crate::system::apply_noninteractive_update_flags(argv, gui_mode());
}

fn append_ignore_flags(argv: &mut Vec<String>, config: &Config) {
    for pkg in packages_ignored_during_system_update(config) {
        argv.push(config.system_update.ignore_flag.clone());
        argv.push(pkg);
    }
}

fn prepare_privileged_update() -> Result<(), String> {
    apply_gui_nested_sudo_askpass();
    if gui_mode() {
        prime_sudo_for_session()?;
        spawn_sudo_keepalive();
    }
    Ok(())
}

fn refuse_if_pgo(config: &Config) -> Result<(), String> {
    let active = crate::pgo::active_pipelines(config);
    if active.is_empty() {
        return Ok(());
    }
    crate::system::warn_system_update_blocked_during_pgo(
        &active,
        SystemUpdateMode::PerformUpdateWithRefresh,
    );
    Err("system update blocked while a kernel PGO pipeline is active".into())
}

/// List pending upgrades. `json` prints one object to stdout.
pub fn print_pending(config: &Config, json: bool) {
    match gather(config) {
        Ok(pending) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&pending).unwrap_or_else(|_| "{}".into())
                );
            } else {
                print_human(&pending);
            }
        }
        Err(e) => die!("{e}"),
    }
}

pub fn gather(config: &Config) -> Result<PendingUpdates, String> {
    let helper = helper_from_update_command(&config.system_update.command_to_perform_system_update);
    let repo_raw = gather_repo(config)?;
    let (repo, mut skipped) = classify_pending(repo_raw, config);
    let repo_names: HashSet<String> = repo.iter().map(|p| p.name.clone()).collect();
    let aur_raw = gather_aur(helper, &repo_names)?;
    let (aur, skipped_aur) = classify_pending(aur_raw, config);
    skipped.extend(skipped_aur);
    let (manual, skipped) = collect_manual_pending(config, skipped);
    Ok(PendingUpdates {
        helper: helper.as_str().to_string(),
        repo,
        aur,
        manual,
        skipped,
    })
}

fn gather_repo(config: &Config) -> Result<Vec<PendingPkg>, String> {
    if command_exists("checkupdates") {
        let out = capture_stdout("checkupdates", &[])?;
        return Ok(parse_upgrade_text(&out));
    }
    vlog!("checkupdates not found; syncing repos then pacman -Qu");
    prepare_privileged_update()?;
    let _ = crate::system::run_system_update(config, SystemUpdateMode::UpdateRepositories);
    let out = capture_stdout("pacman", &["-Qu"])?;
    Ok(parse_upgrade_text(&out))
}

fn gather_aur(
    helper: UpdateHelper,
    repo_names: &HashSet<String>,
) -> Result<Vec<PendingPkg>, String> {
    let mut pkgs = if helper.is_aur_helper() && command_exists(helper.as_str()) {
        let out = capture_stdout(helper.as_str(), &["-Qua"]).unwrap_or_default();
        let parsed = parse_upgrade_text(&out);
        if parsed.is_empty() && out.trim().is_empty() {
            gather_aur_rpc()?
        } else {
            parsed
        }
    } else {
        gather_aur_rpc()?
    };
    pkgs.retain(|p| !repo_names.contains(&p.name));
    Ok(pkgs)
}

fn gather_aur_rpc() -> Result<Vec<PendingPkg>, String> {
    let out = capture_stdout("pacman", &["-Qm"])?;
    let mut foreign = Vec::new();
    for line in out.lines() {
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else {
            continue;
        };
        let Some(ver) = parts.next() else {
            continue;
        };
        foreign.push((name.to_string(), ver.to_string()));
    }
    if foreign.is_empty() {
        return Ok(Vec::new());
    }
    let names: Vec<String> = foreign.iter().map(|(n, _)| n.clone()).collect();
    let remote = crate::aur_rpc::fetch_aur_packages_info(&names).unwrap_or_default();
    let mut out = Vec::new();
    for (name, old) in foreign {
        let Some(new) = remote.get(&name) else {
            continue;
        };
        if vercmp(new, &old).ok().is_some_and(|c| c > 0) {
            out.push(PendingPkg {
                name,
                old,
                new: new.clone(),
            });
        }
    }
    Ok(out)
}

fn print_human(pending: &PendingUpdates) {
    println!(
        "{} Official repos ({})  helper={}",
        "==>".green().bold(),
        pending.repo.len(),
        pending.helper
    );
    if pending.repo.is_empty() {
        println!("    {}", "(none)".dimmed());
    } else {
        for p in &pending.repo {
            println!("    {}  {} -> {}", p.name.bold(), p.old, p.new.green());
        }
    }
    println!("{} AUR ({})", "==>".green().bold(), pending.aur.len());
    if pending.aur.is_empty() {
        println!("    {}", "(none)".dimmed());
    } else {
        for p in &pending.aur {
            println!("    {}  {} -> {}", p.name.bold(), p.old, p.new.green());
        }
    }
    println!(
        "{} ABS watched ({})",
        "==>".green().bold(),
        pending.manual.len()
    );
    if pending.manual.is_empty() {
        println!("    {}", "(none)".dimmed());
    } else {
        for p in &pending.manual {
            println!("    {}  {} -> {}", p.name.bold(), p.old, p.new.green());
        }
    }
    if !pending.skipped.is_empty() {
        println!(
            "{} Skipped, ABS-managed ({})",
            "==>".yellow().bold(),
            pending.skipped.len()
        );
        for p in &pending.skipped {
            println!(
                "    {}  {} -> {}  ({})",
                p.name,
                p.old,
                p.new,
                p.reason.dimmed()
            );
        }
    }
    let _ = io::stdout().flush();
}

fn aur_helper_bin(config: &Config) -> Result<UpdateHelper, String> {
    let configured =
        helper_from_update_command(&config.system_update.command_to_perform_system_update);
    if configured.is_aur_helper() && command_exists(configured.as_str()) {
        return Ok(configured);
    }
    for helper in [UpdateHelper::Yay, UpdateHelper::Paru, UpdateHelper::Pikaur] {
        if command_exists(helper.as_str()) {
            return Ok(helper);
        }
    }
    Err("No AUR helper found. Install yay, paru, or pikaur, or set \
         command_to_perform_system_update to one of them in abs.toml."
        .into())
}

/// Install every pending official-repo package in one transaction.
/// `names` empty means re-query; otherwise install exactly those names (minus ABS ignores).
pub fn install_repo_updates(config: &Config, names: &[String]) -> Result<(), String> {
    refuse_if_pgo(config)?;
    prepare_privileged_update()?;
    let helper = helper_from_update_command(&config.system_update.command_to_perform_system_update);
    let pkgs: Vec<String> = if names.is_empty() {
        gather(config)?.repo.into_iter().map(|p| p.name).collect()
    } else {
        names
            .iter()
            .filter(|n| skip_reason(config, n).is_none())
            .cloned()
            .collect()
    };
    if pkgs.is_empty() {
        return Err("No official-repo packages to update.".into());
    }

    let mut argv = match helper {
        UpdateHelper::Pacman => {
            let mut v = vec!["pacman".into(), "-S".into(), "--needed".into()];
            maybe_noconfirm(&mut v);
            v.extend(pkgs);
            transform_system_update_argv(v, crate::system::is_root())
        }
        other => {
            let mut v = vec![
                other.as_str().to_string(),
                "-S".into(),
                "--repo".into(),
                "--needed".into(),
            ];
            maybe_noconfirm(&mut v);
            v.extend(pkgs);
            v
        }
    };
    append_ignore_flags(&mut argv, config);
    vlog!("Installing repo updates: {}", argv.join(" "));
    run_argv_command(&argv, None::<&str>)
}

/// Install one AUR package with the configured (or discovered) helper.
pub fn install_aur(config: &Config, package: &str) -> Result<(), String> {
    let package = package.trim();
    if package.is_empty() {
        return Err("--install-aur requires a package name".into());
    }
    if skip_reason(config, package).is_some() {
        return Err(format!(
            "{package} is managed by ABS (held, ignored, or watched) and is not installed here"
        ));
    }
    refuse_if_pgo(config)?;
    prepare_privileged_update()?;
    let helper = aur_helper_bin(config)?;
    let mut argv = vec![helper.as_str().to_string(), "-S".into()];
    maybe_noconfirm(&mut argv);
    argv.push(package.to_string());
    vlog!("Installing AUR package: {}", argv.join(" "));
    run_argv_command(&argv, None::<&str>)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BuildConfig, Config, PathsConfig, SystemUpdateConfig};

    fn minimal_config(manual: Vec<&str>, ignore: Vec<&str>) -> Config {
        Config {
            config_version: 1,
            paths: PathsConfig {
                packages_path: "/tmp/p".into(),
                chroot_base_path: "/tmp/c".into(),
                ready_made_packages_path: "/tmp/r".into(),
                chroot_makepkg_conf: None,
            },
            build: BuildConfig {
                default_environment: "local".into(),
                ignore_compilation_failures: false,
                compile_first_install_after: false,
                clean_install_by_default: false,
                ignore_already_made_packages: false,
                concurrent_repos_downloads_limit: 1,
                concurrent_compilations_limit: 1,
                fast_aur_rpc_update_checks: false,
                system_update_first: false,
                clean_chroot_after_compilation: true,
                global_cpu_threads_mode: "strict".into(),
                global_cpu_threads_cap: None,
                maximum_cpu_threads_cap: None,
                default_compilation_threads: None,
                default_compiler: None,
                check_for_update_on_startup: None,
                auto_update_on_startup: None,
                self_update_at_updates: None,
                self_update_install_path: None,
                install_testing_phase_archlinux_packages: None,
            },
            system_update: SystemUpdateConfig {
                command_to_update_repositories: "pacman -Sy".into(),
                command_to_perform_system_update: "yay -Syu".into(),
                command_to_perform_system_update_no_refresh: None,
                ignore_flag: "--ignore".into(),
                ignore_packages: ignore.into_iter().map(String::from).collect(),
            },
            repositories: Default::default(),
            manual_update_packages: manual.into_iter().map(String::from).collect(),
            skip_install_packages: vec![],
            skip_install_packages_after_compilation: None,
            held_packages: Default::default(),
            packages: Default::default(),
            check_for_update_on_startup: false,
            auto_update_on_startup: false,
            self_update_install_path: String::new(),
            self_update_use_pacman: None,
            self_update_at_updates: false,
            install_absgui: None,
            install_testing_phase_archlinux_packages: false,
            compilers: Default::default(),
            ramdisk: Default::default(),
            lang: None,
        }
    }

    #[test]
    fn helper_from_yay_syu() {
        assert_eq!(helper_from_update_command("yay -Syu"), UpdateHelper::Yay);
    }

    #[test]
    fn helper_from_sudo_pacman() {
        assert_eq!(
            helper_from_update_command("sudo pacman -Syu"),
            UpdateHelper::Pacman
        );
    }

    #[test]
    fn helper_from_paru_noconfirm() {
        assert_eq!(
            helper_from_update_command("paru -Syu --noconfirm"),
            UpdateHelper::Paru
        );
    }

    #[test]
    fn helper_from_pikaur_path() {
        assert_eq!(
            helper_from_update_command("/usr/bin/pikaur -Syu"),
            UpdateHelper::Pikaur
        );
    }

    #[test]
    fn parse_checkupdates_line() {
        assert_eq!(
            parse_upgrade_line("linux 6.8.1.arch1-1 -> 6.8.2.arch1-1"),
            Some((
                "linux".into(),
                "6.8.1.arch1-1".into(),
                "6.8.2.arch1-1".into()
            ))
        );
        assert_eq!(parse_upgrade_line(":: Synchronising"), None);
        assert_eq!(parse_upgrade_line(""), None);
        assert_eq!(
            parse_upgrade_line("  extra/firefox 1.0-1 -> 1.1-1 extra"),
            Some(("extra/firefox".into(), "1.0-1".into(), "1.1-1".into()))
        );
    }

    #[test]
    fn classify_moves_ignored_to_skipped() {
        let config = minimal_config(vec!["linux-cachyos"], vec!["held-via-ignore"]);
        let upgrades = vec![
            PendingPkg {
                name: "firefox".into(),
                old: "1-1".into(),
                new: "2-1".into(),
            },
            PendingPkg {
                name: "linux-cachyos".into(),
                old: "1-1".into(),
                new: "2-1".into(),
            },
            PendingPkg {
                name: "held-via-ignore".into(),
                old: "1-1".into(),
                new: "2-1".into(),
            },
        ];
        let (keep, skipped) = classify_pending(upgrades, &config);
        assert_eq!(keep.len(), 1);
        assert_eq!(keep[0].name, "firefox");
        assert!(
            skipped
                .iter()
                .any(|s| s.name == "linux-cachyos" && s.reason == "manual_update_packages")
        );
        assert!(
            skipped
                .iter()
                .any(|s| s.name == "held-via-ignore" && s.reason == "ignore_packages")
        );
        let (manual, rest, _) = promote_manual_skipped(skipped);
        assert_eq!(manual.len(), 1);
        assert_eq!(manual[0].name, "linux-cachyos");
        assert!(
            rest.iter()
                .any(|s| s.name == "held-via-ignore" && s.reason == "ignore_packages")
        );
        assert!(!rest.iter().any(|s| s.name == "linux-cachyos"));
    }

    #[test]
    fn has_work_includes_manual_and_ignores_skipped() {
        let empty = PendingUpdates {
            helper: "yay".into(),
            repo: vec![],
            aur: vec![],
            manual: vec![],
            skipped: vec![SkippedPkg {
                name: "held".into(),
                old: "1".into(),
                new: "2".into(),
                reason: "held_packages".into(),
            }],
        };
        assert!(!empty.has_work());
        let watched = PendingUpdates {
            helper: "yay".into(),
            repo: vec![],
            aur: vec![],
            manual: vec![PendingPkg {
                name: "linux-cachyos".into(),
                old: "1".into(),
                new: "2".into(),
            }],
            skipped: vec![],
        };
        assert!(watched.has_work());
    }
}
