use crate::config::Config;
use crate::utils::{run_command, run_command_quiet, run_command_with_output_silent, vercmp_silent};
use crate::{blog, vlog};
use colored::Colorize;
use std::fs;
use std::path::{Path, PathBuf};

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
        if matches_name {
            if let Some(version) = trimmed.strip_prefix("version = ") {
                return Some(version.trim().trim_matches('"').to_string());
            }
        }
    }
    None
}

/// Fetch the latest version from raw GitHub Cargo.toml
fn fetch_latest_version(raw_url: &str) -> Result<String, String> {
    vlog!("Fetching latest version from {}...", raw_url);
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
    vlog!("Fetched latest version in {:?}", start.elapsed());
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
    run_command_quiet("pacman", &["-Q", pkg], None::<&str>).is_ok()
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

fn find_pkg_artifact(aur_dir: &Path, pkg: &str) -> Result<PathBuf, String> {
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
        if name.starts_with(&format!("{pkg}-"))
            && name.ends_with(".pkg.tar.zst")
            && !name.contains("-debug-")
        {
            matches.push(path);
        }
    }
    matches.sort();
    matches
        .pop()
        .ok_or_else(|| format!("no built package artifact for {pkg} in {}", aur_dir.display()))
}

fn run_pacman_self_update(repo_dir: &Path) -> Result<(), String> {
    let aur_dir = repo_dir.join("aur");
    if !aur_dir.join("PKGBUILD").exists() {
        return Err(format!(
            "aur/PKGBUILD not found in {} (expected Arch packaging layout)",
            repo_dir.display()
        ));
    }

    blog!("Building pacman packages from {}...", aur_dir.display());
    run_command(
        "makepkg",
        &["-Csr", "--noconfirm"],
        Some(&aur_dir),
    )?;

    let to_install = pacman_packages_to_upgrade();
    let mut artifacts = Vec::new();
    for pkg in &to_install {
        artifacts.push(find_pkg_artifact(&aur_dir, pkg)?);
    }

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

fn sync_source_repo(config: &Config) -> Result<PathBuf, String> {
    let packages_path = config.paths.packages_path.clone();
    let abs_dir = PathBuf::from(&packages_path).join("abs");

    let mut repo_ok = false;
    if abs_dir.exists() && abs_dir.join(".git").exists() {
        blog!("Updating ABS repository in {}...", abs_dir.display());
        if run_command("git", &["fetch", "--depth=1"], Some(&abs_dir)).is_ok()
            && run_command("git", &["reset", "--hard", "origin/master"], Some(&abs_dir)).is_ok()
        {
            repo_ok = true;
        } else {
            vlog!("Failed to update existing repository. Cleaning up for a fresh clone...");
            let _ = fs::remove_dir_all(&abs_dir);
        }
    } else if abs_dir.exists() {
        let _ = fs::remove_dir_all(&abs_dir);
    }

    if !repo_ok {
        blog!(
            "Cloning latest repository source from {}...",
            config.self_update_github_url
        );
        fs::create_dir_all(&packages_path)
            .map_err(|e| format!("Failed to create packages directory: {}", e))?;
        run_command(
            "git",
            &[
                "clone",
                "--depth=1",
                &format!("{}.git", config.self_update_github_url),
                abs_dir.to_str().unwrap(),
            ],
            None::<&str>,
        )?;
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

    let repo_dir = sync_source_repo(config)?;

    if should_use_pacman_update(config) {
        match run_pacman_self_update(&repo_dir) {
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
