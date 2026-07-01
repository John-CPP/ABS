use crate::config::{self, Config};
use crate::blog;
use crate::ramdisk;
use crate::utils::{check_sudo_removal, init_deletable_roots, run_command};
use colored::Colorize;
use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemovalKind {
    UserTree,
    SudoTree,
    SudoFile,
}

#[derive(Debug, Clone)]
struct PurgeTarget {
    path: PathBuf,
    kind: RemovalKind,
    description: &'static str,
}

/// Remove ABS from the system: installed binaries, config, state, cache, and build data.
pub fn run(yes: bool) {
    ramdisk::shutdown();

    let config = Config::try_load_existing();
    let mut targets = collect_targets(config.as_ref());

    if targets.is_empty() {
        blog!("No ABS files found to remove.");
        return;
    }

    targets.sort_by(|a, b| a.path.cmp(&b.path));

    blog!("The following ABS-related paths will be removed:");
    for target in &targets {
        println!("  {} ({})", target.path.display(), target.description);
    }

    if crate::is_dry_run_mode() {
        blog!("Dry run — no files were removed.");
        return;
    }

    if !yes && !confirm_purge() {
        blog!("Purge cancelled.");
        return;
    }

    if let Some(ref cfg) = config {
        prepare_deletable_roots(cfg, &targets);
        remove_chroot_tree(cfg);
    }

    let mut removed = 0usize;
    let mut failed = 0usize;
    for target in &targets {
        match remove_target(target) {
            Ok(true) => removed += 1,
            Ok(false) => {}
            Err(e) => {
                failed += 1;
                crate::ewarn!("{}: {e}", target.path.display());
            }
        }
    }

    if failed == 0 {
        blog!("Purge complete ({removed} path(s) removed).");
    } else {
        crate::ewarn!("Purge finished with {failed} error(s) ({removed} path(s) removed).");
    }
}

fn confirm_purge() -> bool {
    print!("Proceed with removal? [y/N]: ");
    let _ = io::stdout().flush();
    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

fn collect_targets(config: Option<&Config>) -> Vec<PurgeTarget> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();

    for path in standard_install_files() {
        push_file(&mut seen, &mut out, path, "installed file", RemovalKind::SudoFile);
    }

    push_tree(
        &mut seen,
        &mut out,
        PathBuf::from("/usr/share/abs"),
        "installed share data",
        RemovalKind::SudoTree,
    );
    push_tree(
        &mut seen,
        &mut out,
        PathBuf::from("/etc/abs"),
        "system config",
        RemovalKind::SudoTree,
    );

    if let Some(config_dir) = dirs::config_dir() {
        push_tree(
            &mut seen,
            &mut out,
            config_dir.join("abs"),
            "user config and PGO state",
            RemovalKind::UserTree,
        );
        push_tree(
            &mut seen,
            &mut out,
            config_dir.join(".cache/abs"),
            "default package/chroot cache",
            RemovalKind::UserTree,
        );
    }
    if let Some(state_dir) = dirs::state_dir() {
        push_tree(
            &mut seen,
            &mut out,
            state_dir.join("abs"),
            "PGO event logs",
            RemovalKind::UserTree,
        );
    }
    if let Some(data_dir) = dirs::data_local_dir() {
        push_tree(
            &mut seen,
            &mut out,
            data_dir.join("abs"),
            "bundled benchmark script copy",
            RemovalKind::UserTree,
        );
    }

    if let Some(cfg) = config {
        push_config_path(
            &mut seen,
            &mut out,
            &cfg.self_update_install_path,
            "abs binary (from config)",
            RemovalKind::SudoFile,
        );
        push_config_path(
            &mut seen,
            &mut out,
            &cfg.paths.packages_path,
            "packages_path",
            RemovalKind::SudoTree,
        );
        push_config_path(
            &mut seen,
            &mut out,
            &cfg.paths.chroot_base_path,
            "chroot_base_path",
            RemovalKind::SudoTree,
        );
        push_config_path(
            &mut seen,
            &mut out,
            &cfg.paths.ready_made_packages_path,
            "ready_made_packages_path",
            RemovalKind::SudoTree,
        );
        if let Some(seed) = &cfg.ramdisk.seed_chroot_from {
            push_config_path(
                &mut seen,
                &mut out,
                seed,
                "ramdisk seed chroot",
                RemovalKind::SudoTree,
            );
        }
        if cfg.ramdisk.enabled {
            push_config_path(
                &mut seen,
                &mut out,
                &cfg.ramdisk.mount_point,
                "ramdisk mount tree",
                RemovalKind::SudoTree,
            );
        }
        for pkg in cfg.packages.values() {
            if let Some(pgo) = &pkg.pgo
                && let Some(archive) = &pgo.profiles_archive_dir
            {
                push_config_path(
                    &mut seen,
                    &mut out,
                    archive,
                    "PGO profiles archive",
                    RemovalKind::SudoTree,
                );
            }
        }
    }

    out.retain(|t| t.path.exists());
    out
}

fn standard_install_files() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/usr/bin/abs"),
        PathBuf::from("/usr/bin/absgui"),
        PathBuf::from("/usr/share/abs/pgo-benchmark.sh"),
        PathBuf::from("/usr/share/applications/absgui.desktop"),
        PathBuf::from("/usr/share/icons/hicolor/256x256/apps/absgui.png"),
    ]
}

fn push_config_path(
    seen: &mut HashSet<PathBuf>,
    out: &mut Vec<PurgeTarget>,
    raw: &str,
    description: &'static str,
    kind: RemovalKind,
) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return;
    }
    let path = config::expand_user_path(trimmed);
    if kind == RemovalKind::SudoFile {
        push_file(seen, out, path, description, kind);
    } else {
        push_tree(seen, out, path, description, kind);
    }
}

fn push_file(
    seen: &mut HashSet<PathBuf>,
    out: &mut Vec<PurgeTarget>,
    path: PathBuf,
    description: &'static str,
    kind: RemovalKind,
) {
    if seen.insert(path.clone()) {
        out.push(PurgeTarget {
            path,
            kind,
            description,
        });
    }
}

fn push_tree(
    seen: &mut HashSet<PathBuf>,
    out: &mut Vec<PurgeTarget>,
    path: PathBuf,
    description: &'static str,
    kind: RemovalKind,
) {
    push_file(seen, out, path, description, kind);
}

fn prepare_deletable_roots(cfg: &Config, targets: &[PurgeTarget]) {
    let extra: Vec<PathBuf> = targets
        .iter()
        .filter(|t| t.kind == RemovalKind::SudoTree)
        .map(|t| t.path.clone())
        .collect();
    if init_deletable_roots(
        &cfg.paths.packages_path,
        &cfg.paths.chroot_base_path,
        &cfg.paths.ready_made_packages_path,
        &extra,
    )
    .is_err()
    {
        crate::ewarn!("Some configured paths could not be registered for safe deletion; sudo removals may be skipped.");
    }
}

fn remove_chroot_tree(cfg: &Config) {
    let master = PathBuf::from(&cfg.paths.chroot_base_path).join("base");
    if master.exists()
        && let Err(e) = check_sudo_removal(&master)
    {
        crate::ewarn!("Failed to remove chroot {}: {e}", master.display());
    }
}

fn remove_target(target: &PurgeTarget) -> Result<bool, String> {
    if !target.path.exists() {
        return Ok(false);
    }
    match target.kind {
        RemovalKind::UserTree => remove_user_tree(&target.path),
        RemovalKind::SudoTree => check_sudo_removal(&target.path).map(|_| true),
        RemovalKind::SudoFile => remove_sudo_file(&target.path),
    }
}

fn remove_user_tree(path: &Path) -> Result<bool, String> {
    if path.is_dir() {
        fs::remove_dir_all(path).map_err(|e| format!("remove_dir_all: {e}"))?;
    } else {
        fs::remove_file(path).map_err(|e| format!("remove_file: {e}"))?;
    }
    Ok(true)
}

fn remove_sudo_file(path: &Path) -> Result<bool, String> {
    if !is_safe_install_path(path) {
        return Err(format!(
            "refusing to remove unexpected install path: {}",
            path.display()
        ));
    }
    run_command(
        "sudo",
        &["rm", "-f", path.to_string_lossy().as_ref()],
        None::<&str>,
    )
    .map(|_| true)
}

fn is_safe_install_path(path: &Path) -> bool {
    const WHITELIST: &[&str] = &[
        "/usr/bin/abs",
        "/usr/bin/absgui",
        "/usr/share/abs/pgo-benchmark.sh",
        "/usr/share/applications/absgui.desktop",
        "/usr/share/icons/hicolor/256x256/apps/absgui.png",
    ];
    if WHITELIST.iter().any(|p| path == Path::new(p)) {
        return true;
    }
    path.parent() == Some(Path::new("/usr/bin"))
        && path.file_name().is_some_and(|n| n == "abs" || n == "absgui")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_install_paths_are_whitelisted() {
        assert!(is_safe_install_path(Path::new("/usr/bin/abs")));
        assert!(is_safe_install_path(Path::new("/usr/bin/absgui")));
        assert!(!is_safe_install_path(Path::new("/usr/bin/pacman")));
    }

    #[test]
    fn collect_includes_standard_dirs_without_config() {
        let targets = collect_targets(None);
        let paths: HashSet<_> = targets.iter().map(|t| t.path.clone()).collect();
        if let Some(config_dir) = dirs::config_dir() {
            assert!(paths.contains(&config_dir.join("abs")));
        }
    }
}
