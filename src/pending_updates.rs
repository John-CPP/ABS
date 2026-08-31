//! List pending pacman/AUR upgrades and install them (repo all-at-once, AUR one-by-one).

use crate::config::Config;
use crate::package_pattern::package_matches_any_pattern;
use crate::system::{
    SystemUpdateMode, packages_ignored_during_system_update, transform_system_update_argv,
};
use crate::utils::{
    apply_gui_nested_sudo_askpass, command_exists, pacman_query_version, pacman_sync_version,
    parse_command_argv, run_argv_command, vercmp,
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
    /// Pacman sync repo (`extra`, `cachyos-extra`, …), `aur`, or an ABS `[repositories]` key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedPkg {
    pub name: String,
    pub old: String,
    pub new: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PgoPipelineHold {
    pub package: String,
    pub stage_label: String,
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
    /// In-progress kernel PGO pipelines (not `Done` / `Aborted`). Empty means none.
    #[serde(default)]
    pub pgo_pipelines: Vec<PgoPipelineHold>,
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
/// `repo/name` prefixes (yay/paru) become `repository` + package name.
pub fn parse_upgrade_line(line: &str) -> Option<PendingPkg> {
    let line = line.trim();
    if line.is_empty() || line.starts_with("::") || line.starts_with("=>") {
        return None;
    }
    let (left, right) = line.split_once("->")?;
    let new = right.split_whitespace().next()?.to_string();
    let mut parts = left.split_whitespace();
    let raw_name = parts.next()?.to_string();
    let old = parts.next()?.to_string();
    if raw_name.is_empty() || old.is_empty() || new.is_empty() {
        return None;
    }
    let (repository, name) = split_repo_qualified_name(&raw_name);
    Some(PendingPkg {
        name,
        old,
        new,
        repository,
    })
}

fn split_repo_qualified_name(raw: &str) -> (Option<String>, String) {
    match raw.split_once('/') {
        Some((repo, name)) if !repo.is_empty() && !name.is_empty() => {
            (Some(repo.to_string()), name.to_string())
        }
        _ => (None, raw.to_string()),
    }
}

fn parse_upgrade_text(text: &str) -> Vec<PendingPkg> {
    text.lines().filter_map(parse_upgrade_line).collect()
}

#[derive(Debug, Clone)]
struct PacmanSiPkg {
    name: String,
    repository: String,
    version: String,
}

fn parse_pacman_si_packages(out: &str) -> Vec<PacmanSiPkg> {
    let mut entries = Vec::new();
    let mut repository = String::new();
    let mut name = String::new();
    let mut version = String::new();

    let flush = |entries: &mut Vec<PacmanSiPkg>, repository: &str, name: &str, version: &str| {
        if !repository.is_empty() && !name.is_empty() && !version.is_empty() {
            entries.push(PacmanSiPkg {
                name: name.to_string(),
                repository: repository.to_string(),
                version: version.to_string(),
            });
        }
    };

    for line in out.lines() {
        let line = line.trim();
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        let val = val.trim();
        match key.trim() {
            "Repository" => {
                flush(&mut entries, &repository, &name, &version);
                repository = val.to_string();
                name.clear();
                version.clear();
            }
            "Name" => name = val.to_string(),
            "Version" => version = val.to_string(),
            _ => {}
        }
    }
    flush(&mut entries, &repository, &name, &version);
    entries
}

fn apply_si_repositories(pkgs: &mut [PendingPkg], si_output: &str) {
    let entries = parse_pacman_si_packages(si_output);
    for pkg in pkgs {
        if pkg
            .repository
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            continue;
        }
        let matches: Vec<&PacmanSiPkg> = entries.iter().filter(|e| e.name == pkg.name).collect();
        let Some(chosen) = matches
            .iter()
            .find(|e| e.version == pkg.new)
            .copied()
            .or_else(|| matches.first().copied())
        else {
            continue;
        };
        pkg.repository = Some(chosen.repository.clone());
    }
}

fn repo_pending_query(helper: UpdateHelper) -> (&'static str, &'static [&'static str]) {
    match helper {
        UpdateHelper::Pacman => ("pacman", &["-Qu"]),
        other => (other.as_str(), &["-Qu", "--repo"]),
    }
}

fn fill_missing_sync_repositories(pkgs: &mut [PendingPkg], helper: UpdateHelper) {
    let names: Vec<String> = pkgs
        .iter()
        .filter(|p| p.repository.as_deref().is_none_or(str::is_empty))
        .map(|p| p.name.clone())
        .collect();
    if names.is_empty() {
        return;
    }
    let bin = helper.as_str();
    for chunk in names.chunks(64) {
        let mut args = Vec::with_capacity(1 + chunk.len());
        args.push("-Si");
        args.extend(chunk.iter().map(String::as_str));
        let Ok(out) = capture_stdout(bin, &args) else {
            continue;
        };
        apply_si_repositories(pkgs, &out);
    }
}

fn abs_package_source(config: &Config, name: &str) -> Option<String> {
    config
        .packages
        .get(name)
        .and_then(|p| p.source.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
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
            manual.push(PendingPkg {
                name,
                old,
                new,
                repository: None,
            });
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
                    repository: None,
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
    // Askpass only — do not prime `sudo -v` first. GUI sudo capture runs each sudo
    // on its own pty, so a separate prime would prompt for the password twice.
    // Optional session cache (`ABS_SUDO_CACHE`) is filled by the first askpass.
    apply_gui_nested_sudo_askpass();
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
    let repos_helper =
        helper_from_update_command(&config.system_update.command_to_update_repositories);
    let repo_raw = gather_repo(config, repos_helper)?;
    let (mut repo, mut skipped) = classify_pending(repo_raw, config);
    fill_missing_sync_repositories(&mut repo, repos_helper);
    let repo_names: HashSet<String> = repo.iter().map(|p| p.name.clone()).collect();
    let aur_raw = gather_aur(helper, &repo_names)?;
    let (mut aur, skipped_aur) = classify_pending(aur_raw, config);
    skipped.extend(skipped_aur);
    for pkg in &mut aur {
        if pkg.repository.as_deref().is_none_or(str::is_empty) {
            pkg.repository = Some("aur".into());
        }
    }
    let (mut manual, skipped) = collect_manual_pending(config, skipped);
    for pkg in &mut manual {
        if pkg.repository.as_deref().is_none_or(str::is_empty) {
            pkg.repository = abs_package_source(config, &pkg.name);
        }
    }
    fill_missing_sync_repositories(&mut manual, repos_helper);
    Ok(attach_pgo_pipelines(
        PendingUpdates {
            helper: helper.as_str().to_string(),
            repo,
            aur,
            manual,
            skipped,
            pgo_pipelines: vec![],
        },
        config,
    ))
}

fn attach_pgo_pipelines(mut pending: PendingUpdates, config: &Config) -> PendingUpdates {
    pending.pgo_pipelines = crate::pgo::active_pipelines(config)
        .into_iter()
        .map(|p| PgoPipelineHold {
            package: p.package,
            stage_label: p.stage_label,
        })
        .collect();
    pending
}

fn gather_repo(config: &Config, repos_helper: UpdateHelper) -> Result<Vec<PendingPkg>, String> {
    prepare_privileged_update()?;
    let _ = crate::system::run_system_update(config, SystemUpdateMode::UpdateRepositories);
    let (bin, args) = repo_pending_query(repos_helper);
    let out = capture_stdout(bin, args)?;
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
                repository: None,
            });
        }
    }
    Ok(out)
}

fn human_skip_reason(reason: &str) -> &str {
    match reason {
        "pgo_pipeline" => "PGO pipeline not finished",
        other => other,
    }
}

fn human_report(pending: &PendingUpdates) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    if !pending.pgo_pipelines.is_empty() {
        let _ = writeln!(
            out,
            "{} {}",
            "==> PGO IN PROGRESS — SYSTEM UPDATE PAUSED".red().bold(),
            "(no packages are installed while a kernel PGO pipeline is not finished)"
                .yellow()
                .bold()
        );
        for pipeline in &pending.pgo_pipelines {
            let _ = writeln!(
                out,
                "    {} {} — {}",
                "•".yellow().bold(),
                pipeline.package.yellow().bold(),
                pipeline.stage_label.yellow()
            );
        }
        let _ = writeln!(
            out,
            "    {} Finish with {} or abandon with {} before running system updates.",
            "Hint:".bold(),
            "`abs --pgo-resume PKG`".cyan(),
            "`abs --pgo-abort PKG`".cyan()
        );
        let _ = writeln!(out);
    }
    let _ = writeln!(
        out,
        "{} Official repos ({})  helper={}",
        "==>".green().bold(),
        pending.repo.len(),
        pending.helper
    );
    if pending.repo.is_empty() {
        let _ = writeln!(out, "    {}", "(none)".dimmed());
    } else {
        for p in &pending.repo {
            let _ = writeln!(out, "{}", format_pending_pkg(p));
        }
    }
    let _ = writeln!(out, "{} AUR ({})", "==>".green().bold(), pending.aur.len());
    if pending.aur.is_empty() {
        let _ = writeln!(out, "    {}", "(none)".dimmed());
    } else {
        for p in &pending.aur {
            let _ = writeln!(out, "{}", format_pending_pkg(p));
        }
    }
    let _ = writeln!(
        out,
        "{} ABS watched ({})",
        "==>".green().bold(),
        pending.manual.len()
    );
    if pending.manual.is_empty() {
        let _ = writeln!(out, "    {}", "(none)".dimmed());
    } else {
        for p in &pending.manual {
            let _ = writeln!(out, "{}", format_pending_pkg(p));
        }
    }
    if !pending.skipped.is_empty() {
        let _ = writeln!(
            out,
            "{} Skipped, ABS-managed ({})",
            "==>".yellow().bold(),
            pending.skipped.len()
        );
        for p in &pending.skipped {
            let _ = writeln!(
                out,
                "    {}  {} -> {}  ({})",
                p.name,
                p.old,
                p.new,
                human_skip_reason(&p.reason).dimmed()
            );
        }
    }
    out
}

fn print_human(pending: &PendingUpdates) {
    print!("{}", human_report(pending));
    let _ = io::stdout().flush();
}

fn format_pending_pkg(p: &PendingPkg) -> String {
    match p
        .repository
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(repo) => format!(
            "    {} [{}]  {} -> {}",
            p.name.bold(),
            repo,
            p.old,
            p.new.green()
        ),
        None => format!("    {}  {} -> {}", p.name.bold(), p.old, p.new.green()),
    }
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

/// Install the distro's prebuilt package (`pacman -S`), even if ABS holds/ignores it.
pub fn install_os_package(package: &str) -> Result<(), String> {
    let argv = os_package_install_argv(package.trim(), crate::system::is_root())?;
    // Askpass only — do not prime `sudo -v` first. GUI sudo capture runs each sudo
    // on its own pty, so a separate prime would prompt for the password twice.
    apply_gui_nested_sudo_askpass();
    crate::blog!("Installing OS-provided package: {}", argv.join(" "));
    run_argv_command(&argv, None::<&str>)
}

fn valid_pacman_pkg_name(name: &str) -> bool {
    let b = name.as_bytes();
    if b.is_empty() || b.len() > 128 {
        return false;
    }
    if b.iter().all(|&c| c == b'.') {
        return false;
    }
    b.iter().all(|c| {
        matches!(
            c,
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'@' | b'.' | b'_' | b'+' | b'-'
        )
    })
}

/// `pacman -S PKG` (with `sudo` when not root). Reinstalls if already present so this
/// can replace an ABS-built kernel with the distro binary. No ABS ignore/hold flags.
fn os_package_install_argv(package: &str, is_root: bool) -> Result<Vec<String>, String> {
    if !valid_pacman_pkg_name(package) {
        return Err(format!("invalid package name: {package}"));
    }
    let mut v = vec!["pacman".into(), "-S".into(), "--noconfirm".into()];
    v.push(package.to_string());
    Ok(transform_system_update_argv(v, is_root))
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
                auto_refresh_delay: 0,
                remember_sudo: false,
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
    fn repo_pending_query_uses_helper_from_update_repositories_command() {
        assert_eq!(
            repo_pending_query(helper_from_update_command("yay -Sy")),
            ("yay", &["-Qu", "--repo"] as &[&str])
        );
        assert_eq!(
            repo_pending_query(helper_from_update_command("sudo pacman -Sy")),
            ("pacman", &["-Qu"] as &[&str])
        );
        assert_eq!(
            repo_pending_query(helper_from_update_command("paru -Sy --quiet")),
            ("paru", &["-Qu", "--repo"] as &[&str])
        );
    }

    fn pending(name: &str, old: &str, new: &str) -> PendingPkg {
        PendingPkg {
            name: name.into(),
            old: old.into(),
            new: new.into(),
            repository: None,
        }
    }

    #[test]
    fn parse_checkupdates_line() {
        assert_eq!(
            parse_upgrade_line("linux 6.8.1.arch1-1 -> 6.8.2.arch1-1"),
            Some(pending("linux", "6.8.1.arch1-1", "6.8.2.arch1-1"))
        );
        assert_eq!(parse_upgrade_line(":: Synchronising"), None);
        assert_eq!(parse_upgrade_line(""), None);
        assert_eq!(
            parse_upgrade_line("  extra/firefox 1.0-1 -> 1.1-1 extra"),
            Some(PendingPkg {
                name: "firefox".into(),
                old: "1.0-1".into(),
                new: "1.1-1".into(),
                repository: Some("extra".into()),
            })
        );
        assert_eq!(
            parse_upgrade_line("aur/yay 12.0-1 -> 12.1-1"),
            Some(PendingPkg {
                name: "yay".into(),
                old: "12.0-1".into(),
                new: "12.1-1".into(),
                repository: Some("aur".into()),
            })
        );
    }

    #[test]
    fn apply_si_repositories_picks_repo_matching_new_version() {
        let mut pkgs = vec![
            pending("firefox", "1.0-1", "1.2-1"),
            PendingPkg {
                name: "linux".into(),
                old: "6.8-1".into(),
                new: "6.9-1".into(),
                repository: Some("core".into()),
            },
        ];
        let si = "\
Repository      : extra
Name            : firefox
Version         : 1.1-1

Repository      : cachyos-extra
Name            : firefox
Version         : 1.2-1

Repository      : core
Name            : linux
Version         : 6.9-1
";
        apply_si_repositories(&mut pkgs, si);
        assert_eq!(pkgs[0].repository.as_deref(), Some("cachyos-extra"));
        assert_eq!(
            pkgs[1].repository.as_deref(),
            Some("core"),
            "already-set repository must not be overwritten"
        );
    }

    #[test]
    fn apply_si_repositories_picks_znver4_when_pkgrel_dot_suffix_is_newer() {
        let mut pkgs = vec![pending("libreoffice-still", "26.2.5-2", "26.2.5-2.1")];
        let si = "\
Repository      : extra
Name            : libreoffice-still
Version         : 26.2.5-2

Repository      : cachyos-extra-znver4
Name            : libreoffice-still
Version         : 26.2.5-2.1
";
        apply_si_repositories(&mut pkgs, si);
        assert_eq!(
            pkgs[0].repository.as_deref(),
            Some("cachyos-extra-znver4"),
            "the .1 pkgrel from the higher-priority znver4 repo is what pacman installs"
        );
    }

    #[test]
    fn classify_moves_ignored_to_skipped() {
        let config = minimal_config(vec!["linux-cachyos"], vec!["held-via-ignore"]);
        let upgrades = vec![
            pending("firefox", "1-1", "2-1"),
            pending("linux-cachyos", "1-1", "2-1"),
            pending("held-via-ignore", "1-1", "2-1"),
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
            pgo_pipelines: vec![],
        };
        assert!(!empty.has_work());
        let watched = PendingUpdates {
            helper: "yay".into(),
            repo: vec![],
            aur: vec![],
            manual: vec![pending("linux-cachyos", "1", "2")],
            skipped: vec![],
            pgo_pipelines: vec![],
        };
        assert!(watched.has_work());
    }

    fn pending_with_pgo() -> PendingUpdates {
        PendingUpdates {
            helper: "yay".into(),
            repo: vec![],
            aur: vec![],
            manual: vec![],
            skipped: vec![],
            pgo_pipelines: vec![PgoPipelineHold {
                package: "linux-cachyos".into(),
                stage_label: "Waiting for reboot (boot stage-2 kernel)".into(),
            }],
        }
    }

    #[test]
    fn json_includes_active_pgo_pipelines_even_when_lists_are_empty() {
        let json = serde_json::to_value(&pending_with_pgo()).unwrap();
        assert_eq!(json["repo"], serde_json::json!([]));
        assert_eq!(
            json["pgo_pipelines"][0]["package"],
            serde_json::json!("linux-cachyos")
        );
        assert_eq!(
            json["pgo_pipelines"][0]["stage_label"],
            serde_json::json!("Waiting for reboot (boot stage-2 kernel)")
        );
    }

    #[test]
    fn human_report_explains_unfinished_pgo_when_nothing_is_installable() {
        let text = human_report(&pending_with_pgo());
        assert!(
            text.contains("PGO") && text.contains("linux-cachyos"),
            "empty pending list must still say PGO is why updates are paused: {text}"
        );
        assert!(
            text.to_lowercase().contains("not finished")
                || text.to_lowercase().contains("in progress"),
            "must say the pipeline is unfinished, not only list the package: {text}"
        );
    }

    #[test]
    fn human_skip_reason_explains_pgo_pipeline() {
        assert_eq!(
            human_skip_reason("pgo_pipeline"),
            "PGO pipeline not finished"
        );
    }

    #[test]
    fn attach_pgo_pipelines_reads_active_state_file() {
        use crate::config::{PackageConfig, PgoConfig};
        use crate::pgo::{PgoStageId, PgoState};

        let dir = std::env::temp_dir().join(format!(
            "abs-pending-pgo-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let state_path = dir.join("linux-cachyos.json");
        let state = PgoState {
            package: "linux-cachyos".into(),
            repo_dir: "/tmp/repo".into(),
            current_stage: PgoStageId::WaitReboot2,
            started_at: 0,
            updated_at: 0,
            expected_kernel_uname: None,
            expected_package_base: None,
            stage_history: vec![],
            compare_run_dir: None,
        };
        std::fs::write(&state_path, serde_json::to_string_pretty(&state).unwrap()).unwrap();

        let mut config = minimal_config(vec![], vec![]);
        config.packages.insert(
            "linux-cachyos".into(),
            PackageConfig {
                pgo: Some(PgoConfig {
                    enabled: true,
                    preset: "cachyos-kernel".into(),
                    profiles_archive_dir: Some(dir.to_string_lossy().into_owned()),
                    save_kernels_dir: None,
                    profile_scratch_dir: "auto".into(),
                    perf_data_on_ram: true,
                    propeller_profiles_on_ram: true,
                    convert_relocate: "force".into(),
                    benchmark_command: None,
                    benchmark_workdir: None,
                    benchmark_preset: "kernel".into(),
                    compare_preset: "auto".into(),
                    kernel_workload_seconds: 0,
                    profiling_quality: "sweet".into(),
                    build_user: None,
                    perf_event_args: "auto".into(),
                    perf_extra_args: crate::config::PERF_EXTRA_ARGS_STANDARD.into(),
                    sysctl_command: None,
                    vmlinux: "auto".into(),
                    afdo_tool: "llvm-profgen".into(),
                    propeller_tool: "create_llvm_prof".into(),
                    afdo_profile_name: "kernel-compilation.afdo".into(),
                    verify_boot: true,
                    select_boot_kernel: true,
                    auto_restart: false,
                    reboot_before_start: false,
                    shutdown_after_finish: false,
                    reuse_afdo_profile: false,
                    reuse_propeller_profile: false,
                    skip_propeller: false,
                    compare_current: false,
                    compare_debug: false,
                    compare_debug_clean: false,
                    compare_autofdo: false,
                    compare_autofdo_clean: false,
                    compare_final: false,
                    state_file: Some(state_path.to_string_lossy().into_owned()),
                }),
                ..Default::default()
            },
        );

        let pending = attach_pgo_pipelines(
            PendingUpdates {
                helper: "yay".into(),
                repo: vec![],
                aur: vec![],
                manual: vec![],
                skipped: vec![],
                pgo_pipelines: vec![],
            },
            &config,
        );
        assert_eq!(pending.pgo_pipelines.len(), 1);
        assert_eq!(pending.pgo_pipelines[0].package, "linux-cachyos");
        assert!(!pending.pgo_pipelines[0].stage_label.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn os_package_install_uses_pacman_s_for_the_named_kernel() {
        let argv = super::os_package_install_argv("linux-cachyos", true).unwrap();
        assert_eq!(argv[0], "pacman");
        assert!(argv.contains(&"-S".to_string()));
        assert!(
            argv.contains(&"--noconfirm".to_string()),
            "one-shot OS install must not wait for [Y/n]: {argv:?}"
        );
        assert!(
            !argv.contains(&"--needed".to_string()),
            "must reinstall a distro kernel that is already present: {argv:?}"
        );
        assert!(argv.contains(&"linux-cachyos".to_string()));
        assert!(!argv.iter().any(|a| a == "--ignore"));
    }

    #[test]
    fn os_package_install_adds_sudo_when_not_root() {
        let argv = super::os_package_install_argv("linux-cachyos-lto", false).unwrap();
        assert_eq!(argv[0], "sudo");
        assert_eq!(argv[1], "pacman");
        assert!(argv.contains(&"--noconfirm".to_string()), "{argv:?}");
        assert!(!argv.contains(&"--needed".to_string()), "{argv:?}");
        assert!(argv.contains(&"linux-cachyos-lto".to_string()));
    }

    #[test]
    fn os_package_install_rejects_injection() {
        assert!(super::os_package_install_argv("", true).is_err());
        assert!(super::os_package_install_argv("linux-cachyos;id", true).is_err());
        assert!(super::os_package_install_argv("../evil", true).is_err());
        assert!(super::os_package_install_argv("foo bar", true).is_err());
    }
}
