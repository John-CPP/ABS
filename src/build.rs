use crate::cli::Cli;
use crate::config::Config;
use crate::git::prepare_repo;
use crate::pkgbuild::{backup_pkgbuild, bump_pkgrel, restore_pkgbuild, update_pkgsums};
use crate::utils::{remove_stale_pkgs_in_pkgdest, run_command, run_shell_in_dir_with_tee};
use crate::{blog, die, ewarn};
use colored::Colorize;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Resolve a `[repositories]` entry to a clone URL. Values may be a URL, or another key
/// (e.g. `default = "arch"` then `arch = "https://..."`).
fn repository_url(repos: &HashMap<String, String>, start: &str) -> Option<String> {
    let mut key = start.to_string();
    for _ in 0..8 {
        let v = repos.get(&key)?;
        if v.contains("://") || v.starts_with("git@") {
            return Some(v.clone());
        }
        key = v.clone();
    }
    None
}

pub struct PkgbuildGuard<'a> {
    pub repo_dir: &'a Path,
}

impl<'a> Drop for PkgbuildGuard<'a> {
    fn drop(&mut self) {
        restore_pkgbuild(self.repo_dir);
    }
}

fn run_build_with_key_retry(build_cmd: &str, repo_dir: &Path, verbose: bool) -> Result<(), String> {
    let key_re = Regex::new(r"(?i)unknown public key ([0-9A-F]+)")
        .map_err(|e| format!("Failed to compile missing-key regex: {}", e))?;
    let mut seen_keys: HashSet<String> = HashSet::new();

    loop {
        match run_shell_in_dir_with_tee(repo_dir, build_cmd) {
            Ok(()) => return Ok(()),
            Err(err) => {
                let mut newly_found = Vec::new();
                for caps in key_re.captures_iter(&err) {
                    let key = caps[1].to_uppercase();
                    if seen_keys.insert(key.clone()) {
                        newly_found.push(key);
                    }
                }
                if newly_found.is_empty() {
                    return Err(err);
                }

                for key in newly_found {
                    crate::vlog!(verbose, "Importing missing key: {}", key);
                    if let Err(gpg_err) = run_command(
                        "gpg",
                        &[
                            "--keyserver",
                            "hkps://keyserver.ubuntu.com",
                            "--recv-keys",
                            &key,
                        ],
                        None::<&str>,
                    ) {
                        return Err(format!(
                            "Build failed and key import also failed for {}: {}\nOriginal build error:\n{}",
                            key, gpg_err, err
                        ));
                    }
                }
                crate::vlog!(verbose, "Retrying build after importing keys...");
            }
        }
    }
}

pub fn process_package(pkg: &str, cli: &Cli, config: &Config) -> bool {
    let pkg_config = config.packages.get(pkg);

    // Determine the source repository
    let mut repo_name = config
        .repositories
        .get("default")
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            die!("Missing [repositories] entry: default = \"<repo-key>\" (see emerge.toml.example)")
        });
    if let Some(r) = &cli.repo {
        repo_name = r.to_string();
    } else if let Some(pc) = pkg_config
        && let Some(src) = &pc.source
    {
        repo_name = src.to_string();
    }

    let repo_url_string = match repository_url(&config.repositories, &repo_name) {
        Some(url) => url,
        None => {
            ewarn!(
                "Repository '{}' not found in config. Using default.",
                repo_name
            );
            let default_key = config
                .repositories
                .get("default")
                .map(|s| s.as_str())
                .unwrap_or("arch");
            repository_url(&config.repositories, default_key).unwrap_or_else(|| {
                die!(
                    "Could not resolve a repository URL (check [repositories] for '{}' and 'default')",
                    repo_name
                )
            })
        }
    };
    let repo_url = repo_url_string.as_str();

    let base_pkg_name = if let Some(pc) = pkg_config {
        pc.alias.as_deref().unwrap_or(pkg)
    } else {
        pkg
    };

    if cli.install_only {
        blog!("Install-only mode, searching for existing artifacts...");
        crate::install::install_from_ready_dir(pkg, base_pkg_name, config, cli.verbose);
        return true;
    }

    if cli.download_only {
        blog!("Downloading sources for {}...", pkg);
        let _ = prepare_repo(
            pkg,
            base_pkg_name,
            &repo_name,
            repo_url,
            &config.paths.packages_path,
            cli.clean,
            true,
        );
        return true;
    }

    // Actual build flow
    let repo_dir_path = prepare_repo(
        pkg,
        base_pkg_name,
        &repo_name,
        repo_url,
        &config.paths.packages_path,
        cli.clean,
        cli.force_repo_update,
    );
    let repo_dir = repo_dir_path.as_path();

    // Bash `process_package` order: `prepare_repo` → `PRE_UPDATE_COMMANDS` → `prepare_sums_pkgrel` → build …
    // Rust mirrors that **except** we snapshot `PKGBUILD` here first (Bash has no separate backup file).
    // This **must** run before `pre_update_command` (TOML `pre_update_command` / Bash `PRE_UPDATE_COMMANDS`)
    // so those hooks can edit `PKGBUILD` and we can still restore the pre-hook tree on exit.
    // If `.PKGBUILD.emerge_backup` already exists (e.g. last run stopped before restore), we do not
    // overwrite it — keep the upstream baseline for bump logic.
    backup_pkgbuild(repo_dir);
    let _guard = PkgbuildGuard { repo_dir };

    if let Some(pc) = pkg_config
        && let Some(cmd) = &pc.pre_update_command
    {
        blog!("Running pre-update command...");
        if let Err(e) = run_command("sh", &["-c", cmd], Some(repo_dir)) {
            die!("Pre-update command failed: {}", e);
        }
    }

    // Match Bash `prepare_sums_pkgrel`: `prepare_pkgsums` (updpkgsums only if -u), then always `bump_pkgrel`.
    if cli.update_sums && !update_pkgsums(repo_dir, cli.verbose) {
        ewarn!("updpkgsums failed, continuing...");
    }
    bump_pkgrel(repo_dir, cli.verbose);

    // Drop older PKGDEST artifacts for this base name so install prompts do not list stale builds.
    remove_stale_pkgs_in_pkgdest(
        &config.paths.ready_made_packages_path,
        base_pkg_name,
        cli.verbose,
    );

    let mut build_env = config.build.default_environment.clone();
    if let Some(pc) = pkg_config
        && let Some(env) = &pc.build_env
    {
        build_env = env.to_string();
    }

    if cli.local_build {
        build_env = "local".to_string();
    } else if cli.chroot_build {
        build_env = "chroot".to_string();
    }

    let mut custom_cmd = None;
    if let Some(pc) = pkg_config {
        if build_env == "local" {
            custom_cmd = pc.custom_local_build_command.clone();
        } else {
            custom_cmd = pc.custom_chroot_build_command.clone();
        }
    }

    if let Some(cmd) = custom_cmd {
        blog!("Executing custom build command...");
        if let Err(e) = run_build_with_key_retry(&cmd, repo_dir, cli.verbose) {
            die!("Custom build command failed: {}", e);
        }
    } else {
        let mut tests_enabled = true;
        if let Some(pc) = pkg_config
            && let Some(t) = pc.tests
        {
            tests_enabled = t;
        }
        let skip_tests = cli.no_check || !tests_enabled;

        if build_env == "local" {
            blog!("Building locally with makepkg...");

            let mut build_cmd = format!(
                "PKGDEST=\"{}\" makepkg --syncdeps --noconfirm --needed -f",
                config.paths.ready_made_packages_path
            );
            if cli.clean {
                build_cmd.push_str(" -c");
            }

            if skip_tests {
                build_cmd.push_str(" --nocheck");
            }

            if let Err(e) = run_build_with_key_retry(&build_cmd, repo_dir, cli.verbose) {
                die!("makepkg failed for {}: {}", pkg, e);
            }
        } else {
            blog!("Building in chroot with makechrootpkg...");
            let master_chroot = PathBuf::from(&config.paths.chroot_base_path).join("base");
            let mut build_cmd = format!(
                "PKGDEST=\"{}\" makechrootpkg -c -r \"{}\" -d \"{}\"",
                config.paths.ready_made_packages_path,
                master_chroot.to_string_lossy(),
                repo_dir.to_string_lossy()
            );
            if skip_tests {
                build_cmd.push_str(" -- --nocheck");
            }
            if let Err(e) = run_build_with_key_retry(&build_cmd, repo_dir, cli.verbose) {
                die!("makechrootpkg failed for {}: {}", pkg, e);
            }
        }
    }

    // Bash: install then post-update (both only if not `-o`). Hooks still see the bumped PKGBUILD.
    if !cli.compile_only {
        crate::install::install_artifacts(pkg, base_pkg_name, Some(repo_dir), config, cli.verbose);

        if let Some(pc) = pkg_config
            && let Some(cmd) = &pc.post_update_command
        {
            blog!("Running post-update command...");
            if let Err(e) = run_command("sh", &["-c", cmd], Some(repo_dir)) {
                ewarn!("Post-update command failed: {}", e);
            }
        }
    }

    // Build (and optional install) are done — no more compilation. Restore upstream PKGBUILD now
    // instead of only at scope end; `Drop` becomes a no-op once backup is consumed.
    restore_pkgbuild(repo_dir);

    true
}

#[cfg(test)]
mod tests {
    use super::repository_url;
    use std::collections::HashMap;

    fn sample_repos() -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("default".into(), "arch".into());
        m.insert("arch".into(), "https://gitlab.example/pkg".into());
        m.insert(
            "cachyos".into(),
            "https://github.com/example/cachy.git".into(),
        );
        m
    }

    #[test]
    fn repository_url_direct_https() {
        let m = sample_repos();
        assert_eq!(
            repository_url(&m, "cachyos").as_deref(),
            Some("https://github.com/example/cachy.git")
        );
    }

    #[test]
    fn repository_url_follows_default_chain() {
        let m = sample_repos();
        assert_eq!(
            repository_url(&m, "default").as_deref(),
            Some("https://gitlab.example/pkg")
        );
    }

    #[test]
    fn repository_url_git_ssh() {
        let mut m = HashMap::new();
        m.insert("priv".into(), "git@github.com:org/repo.git".into());
        assert_eq!(
            repository_url(&m, "priv").as_deref(),
            Some("git@github.com:org/repo.git")
        );
    }

    #[test]
    fn repository_url_unknown_returns_none() {
        let m = sample_repos();
        assert!(repository_url(&m, "missing").is_none());
    }
}
