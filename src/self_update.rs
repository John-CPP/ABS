use crate::config::Config;
use crate::utils::{run_command, run_command_quiet, run_command_with_output_silent, vercmp_silent};
use crate::{blog, vlog};
use colored::Colorize;
use std::fs;
use std::path::{Path, PathBuf};

const OFFICIAL_REPOSITORY_URL: &str = "https://github.com/John-CPP/ABS.git";
/// `HEAD` resolves to the remote's default branch, so updates keep working across branch renames.
const OFFICIAL_REPOSITORY_BRANCH: &str = "HEAD";

/// Parse `version` for the workspace `abs` package from raw Cargo.toml text.
fn parse_cargo_toml_version(text: &str) -> Option<String> {
    parse_cargo_toml_package_version(text, "abs")
}

fn parse_cargo_toml_package_version(text: &str, package: &str) -> Option<String> {
    let mut in_package = false;
    let mut matches_name = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "[package]" {
            in_package = true;
            matches_name = false;
            continue;
        }
        if trimmed.starts_with('[') {
            in_package = false;
            matches_name = false;
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(name) = trimmed.strip_prefix("name = ") {
            let name = name.trim().trim_matches('"');
            matches_name = name == package;
            continue;
        }
        if matches_name
            && let Some(version) = trimmed.strip_prefix("version = ")
        {
            return Some(version.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// Fetch the latest version from raw GitHub Cargo.toml
fn fetch_latest_version(raw_url: &str) -> Result<String, String> {
    vlog!("Checking upstream ABS release at {}...", raw_url);
    let start = std::time::Instant::now();
    let out = run_command_with_output_silent(
        "curl",
        &[
            "-fsSL",
            "--compressed",
            "-m", "5", // 5 seconds timeout
            raw_url,
        ],
        None::<&str>,
    )?;
    vlog!("Upstream ABS version check finished in {:?}", start.elapsed());
    parse_cargo_toml_version(&out)
        .ok_or_else(|| "Failed to parse version from remote Cargo.toml".to_string())
}

/// Perform update check and return if a new version is available along with the version string
pub fn check_for_update(raw_url: &str) -> Result<(bool, String), String> {
    let latest = fetch_latest_version(raw_url)?;
    let current = env!("CARGO_PKG_VERSION");
    let is_newer = vercmp_silent(&latest, current)? > 0;
    Ok((is_newer, latest))
}

fn pacman_installed(pkg: &str) -> bool {
    // Silent even at normal verbosity: this is an internal probe, not user-facing output.
    run_command_with_output_silent("pacman", &["-Q", pkg], None::<&str>).is_ok()
}

fn should_use_pacman_update(config: &Config) -> bool {
    match config.self_update_use_pacman {
        Some(true) => true,
        Some(false) => false,
        None => pacman_installed("abs") || pacman_installed("absgui") || pacman_installed("abs-full"),
    }
}

fn pacman_packages_to_upgrade() -> Vec<&'static str> {
    if pacman_installed("abs-full") {
        return vec!["abs", "absgui", "abs-full"];
    }
    let mut pkgs = Vec::new();
    if pacman_installed("abs") {
        pkgs.push("abs");
    }
    if pacman_installed("absgui") {
        pkgs.push("absgui");
    }
    if pkgs.is_empty() {
        pkgs.extend(["abs", "absgui"]);
    }
    pkgs
}

/// True when `filename` is a non-debug package for `pkg` at `pkgver` (any pkgrel/arch).
/// e.g. `abs-1.3.7-1-x86_64.pkg.tar.zst` for pkg=`abs`, pkgver=`1.3.7`.
fn is_pkg_artifact_for_version(filename: &str, pkg: &str, pkgver: &str) -> bool {
    filename.starts_with(&format!("{pkg}-{pkgver}-"))
        && filename.ends_with(".pkg.tar.zst")
        && !filename.contains("-debug-")
}

fn find_pkg_artifacts_for_version(aur_dir: &Path, pkg: &str, pkgver: &str) -> Result<Vec<PathBuf>, String> {
    let mut matches = Vec::new();
    for entry in fs::read_dir(aur_dir).map_err(|e| format!("read {}: {e}", aur_dir.display()))? {
        let entry = entry.map_err(|e| format!("read dir entry: {e}"))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if is_pkg_artifact_for_version(name, pkg, pkgver) {
            matches.push(path);
        }
    }
    matches.sort();
    Ok(matches)
}

fn find_pkg_artifact(aur_dir: &Path, pkg: &str, pkgver: &str) -> Result<PathBuf, String> {
    find_pkg_artifacts_for_version(aur_dir, pkg, pkgver)?
        .pop()
        .ok_or_else(|| {
            format!(
                "no built package artifact for {pkg} {pkgver} in {}",
                aur_dir.display()
            )
        })
}

/// Split-package leftovers for this `pkgver` block `makepkg` without `-f`.
/// Remove them only when we are about to rebuild (not when reusing a complete set).
fn remove_pkg_artifacts_for_version(aur_dir: &Path, pkgver: &str) -> Result<(), String> {
    for pkg in ["abs", "absgui", "abs-full"] {
        for path in find_pkg_artifacts_for_version(aur_dir, pkg, pkgver)? {
            fs::remove_file(&path).map_err(|e| {
                format!("failed to remove leftover {}: {e}", path.display())
            })?;
            vlog!("Removed leftover package artifact {}", path.display());
        }
    }
    Ok(())
}

fn try_ready_pkg_artifacts(
    aur_dir: &Path,
    pkgs: &[&str],
    pkgver: &str,
) -> Option<Vec<PathBuf>> {
    let mut artifacts = Vec::with_capacity(pkgs.len());
    for pkg in pkgs {
        match find_pkg_artifact(aur_dir, pkg, pkgver) {
            Ok(path) => artifacts.push(path),
            Err(_) => return None,
        }
    }
    Some(artifacts)
}

fn run_pacman_self_update(repo_dir: &Path, expected_version: &str) -> Result<(), String> {
    let aur_dir = repo_dir.join("aur");
    if !aur_dir.join("PKGBUILD").exists() {
        return Err(format!(
            "aur/PKGBUILD not found in {} (expected Arch packaging layout)",
            repo_dir.display()
        ));
    }

    let to_install = pacman_packages_to_upgrade();
    let artifacts = if let Some(ready) = try_ready_pkg_artifacts(&aur_dir, &to_install, expected_version)
    {
        blog!(
            "Using already-built pacman packages for {}...",
            expected_version
        );
        ready
    } else {
        // Partial same-version artifacts make makepkg exit with "already been built".
        remove_pkg_artifacts_for_version(&aur_dir, expected_version)?;
        blog!("Building pacman packages from {}...", aur_dir.display());
        run_command(
            "makepkg",
            &["-Csr", "--noconfirm"],
            Some(&aur_dir),
        )?;
        let mut built = Vec::new();
        for pkg in &to_install {
            built.push(find_pkg_artifact(&aur_dir, pkg, expected_version)?);
        }
        built
    };

    blog!(
        "Installing pacman package(s): {}",
        to_install.join(", ")
    );

    let mut args = vec!["-U".to_string(), "--noconfirm".to_string()];
    for artifact in &artifacts {
        args.push(artifact.to_string_lossy().into_owned());
    }

    let install_res = run_command_quiet("pacman", &args.iter().map(String::as_str).collect::<Vec<_>>(), Some(&aur_dir));
    if install_res.is_err() {
        vlog!("Non-root pacman failed; retrying with sudo...");
        let mut sudo_args = vec!["pacman".to_string()];
        sudo_args.extend(args);
        run_command(
            "sudo",
            &sudo_args.iter().map(String::as_str).collect::<Vec<_>>(),
            Some(&aur_dir),
        )?;
    }

    Ok(())
}

fn source_version_matches(repo_dir: &Path, expected_version: &str) -> Result<bool, String> {
    let cargo_toml = repo_dir.join("Cargo.toml");
    let text = fs::read_to_string(&cargo_toml)
        .map_err(|e| format!("read {}: {e}", cargo_toml.display()))?;
    Ok(parse_cargo_toml_version(&text).as_deref() == Some(expected_version))
}

fn remote_is_official(repo_dir: &Path) -> bool {
    run_command_with_output_silent("git", &["remote", "get-url", "origin"], Some(repo_dir))
        .map(|url| url.trim() == OFFICIAL_REPOSITORY_URL)
        .unwrap_or(false)
}

fn sync_source_repo(config: &Config, expected_version: &str) -> Result<PathBuf, String> {
    let packages_path = config.paths.packages_path.clone();
    let abs_dir = PathBuf::from(&packages_path).join("abs");

    let mut repo_ok = false;
    if abs_dir.exists() && abs_dir.join(".git").exists() && remote_is_official(&abs_dir) {
        blog!("Updating ABS repository in {}...", abs_dir.display());
        if run_command(
            "git",
            &["fetch", "--depth=1", "origin", OFFICIAL_REPOSITORY_BRANCH],
            Some(&abs_dir),
        )
        .is_ok()
            && run_command("git", &["reset", "--hard", "FETCH_HEAD"], Some(&abs_dir)).is_ok()
        {
            repo_ok = true;
        } else {
            vlog!("Failed to update existing repository. Cleaning up for a fresh clone...");
            let _ = fs::remove_dir_all(&abs_dir);
        }
    } else if abs_dir.exists() {
        vlog!("Existing self-update checkout has an unexpected origin. Re-cloning the official repository...");
        let _ = fs::remove_dir_all(&abs_dir);
    }

    if !repo_ok {
        blog!(
            "Cloning latest repository source from {}...",
            OFFICIAL_REPOSITORY_URL
        );
        fs::create_dir_all(&packages_path)
            .map_err(|e| format!("Failed to create packages directory: {}", e))?;
        run_command(
            "git",
            &[
                "clone",
                "--depth=1",
                OFFICIAL_REPOSITORY_URL,
                abs_dir.to_str().unwrap(),
            ],
            None::<&str>,
        )?;
    }

    if !source_version_matches(&abs_dir, expected_version)? {
        return Err(format!(
            "official repository source version does not match checked update version {expected_version}"
        ));
    }

    Ok(abs_dir)
}

fn run_binary_self_update(config: &Config, repo_dir: &Path) -> Result<(), String> {
    blog!("Compiling latest release...");
    run_command(
        "cargo",
        &["build", "--release"],
        Some(repo_dir),
    )?;

    let new_binary = repo_dir.join("target").join("release").join("abs");
    if !new_binary.exists() {
        return Err("Compiled binary not found in target/release/abs".into());
    }

    let install_path = &config.self_update_install_path;
    blog!("Installing executable to {}...", install_path);

    let new_str = new_binary.to_string_lossy();
    let install_res = run_command_quiet(
        "install",
        &["-Dm755", new_str.as_ref(), install_path.as_ref()],
        None::<&str>,
    );

    if install_res.is_err() {
        vlog!("Standard install failed. Retrying with sudo...");
        run_command(
            "sudo",
            &["install", "-Dm755", new_str.as_ref(), install_path.as_ref()],
            None::<&str>,
        )?;
    }

    Ok(())
}

/// Run self update (explicitly called by CLI or auto-update on startup)
pub fn run_self_update(config: &Config, is_auto: bool) -> Result<bool, String> {
    if !is_auto {
        blog!("Checking for updates...");
    }

    let (is_newer, latest) = match check_for_update(&config.self_update_raw_url) {
        Ok(res) => res,
        Err(e) => {
            if is_auto {
                return Ok(false); // Fail silently on auto-update
            } else {
                return Err(format!("Update check failed: {}", e));
            }
        }
    };

    if !is_newer {
        if !is_auto {
            let current = env!("CARGO_PKG_VERSION");
            match vercmp_silent(current, &latest) {
                Ok(cmp) if cmp > 0 => {
                    blog!(
                        "ABS {} is newer than published upstream {} (local or manual install).",
                        current.green(),
                        latest.yellow()
                    );
                }
                Ok(_) => {
                    blog!(
                        "ABS is up-to-date (current: {}, upstream: {}).",
                        current.green(),
                        latest
                    );
                }
                Err(e) => {
                    blog!(
                        "ABS is up-to-date (current version: {}). (Could not compare with upstream: {e})",
                        current.green()
                    );
                }
            }
        }
        return Ok(false);
    }

    blog!(
        "New version available: {} (current version: {}). Starting update...",
        latest.green(),
        env!("CARGO_PKG_VERSION").yellow()
    );

    let repo_dir = sync_source_repo(config, &latest)?;

    if should_use_pacman_update(config) {
        match run_pacman_self_update(&repo_dir, &latest) {
            Ok(()) => {
                blog!(
                    "ABS successfully updated to version {} via pacman!",
                    latest.green()
                );
                return Ok(true);
            }
            Err(e) => {
                if config.self_update_use_pacman == Some(true) {
                    return Err(format!("Pacman self-update failed: {e}"));
                }
                eprintln!(
                    "{} Pacman self-update failed ({e}); falling back to binary install.",
                    "==> WARNING:".yellow()
                );
            }
        }
    }

    run_binary_self_update(config, &repo_dir)?;
    blog!("ABS successfully updated to version {}!", latest.green());
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_remote_cargo_version() {
        let text = r#"[package]
name = "abs"
version = "1.3.4"
"#;
        assert_eq!(parse_cargo_toml_version(text).as_deref(), Some("1.3.4"));
    }

    #[test]
    fn detects_ready_pkg_artifact_for_version() {
        assert!(is_pkg_artifact_for_version(
            "abs-1.3.7-1-x86_64.pkg.tar.zst",
            "abs",
            "1.3.7"
        ));
        assert!(is_pkg_artifact_for_version(
            "absgui-1.3.7-1-x86_64.pkg.tar.zst",
            "absgui",
            "1.3.7"
        ));
        assert!(!is_pkg_artifact_for_version(
            "abs-1.3.6-1-x86_64.pkg.tar.zst",
            "abs",
            "1.3.7"
        ));
        assert!(!is_pkg_artifact_for_version(
            "abs-1.3.7-debug-1-x86_64.pkg.tar.zst",
            "abs",
            "1.3.7"
        ));
        // Prefix collision: absgui must not match pkg "abs"
        assert!(!is_pkg_artifact_for_version(
            "absgui-1.3.7-1-x86_64.pkg.tar.zst",
            "abs",
            "1.3.7"
        ));
    }

    /// A stale absgui version breaks `cargo build --locked` in the AUR PKGBUILD after a
    /// release bump, which makes pacman self-updates fail for every user.
    #[test]
    fn workspace_member_versions_stay_in_sync() {
        let gui_manifest = include_str!("../absgui/Cargo.toml");
        let gui_version = parse_cargo_toml_package_version(gui_manifest, "absgui")
            .expect("absgui Cargo.toml has a version");
        assert_eq!(gui_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn parse_remote_cargo_version_ignores_other_workspace_members() {
        let text = r#"[package]
name = "absgui"
version = "1.3.3"

[package]
name = "abs"
version = "1.3.4"
"#;
        assert_eq!(parse_cargo_toml_version(text).as_deref(), Some("1.3.4"));
    }
}
