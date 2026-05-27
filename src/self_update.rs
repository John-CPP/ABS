use crate::config::Config;
use crate::utils::{run_command, run_command_with_output};
use crate::{blog, vlog};
use colored::Colorize;
use regex::Regex;
use std::fs;

/// Parse version from raw Cargo.toml string
fn parse_cargo_toml_version(text: &str) -> Option<String> {
    let re = Regex::new(r#"(?m)^version\s*=\s*"([^"]+)""#).ok()?;
    let caps = re.captures(text)?;
    Some(caps[1].to_string())
}

/// Fetch the latest version from raw GitHub Cargo.toml
fn fetch_latest_version(raw_url: &str) -> Result<String, String> {
    vlog!("Fetching latest version from {}...", raw_url);
    let out = run_command_with_output(
        "curl",
        &[
            "-fsSL",
            "-m", "5", // 5 seconds timeout
            raw_url,
        ],
        None::<&str>,
    )?;
    parse_cargo_toml_version(&out)
        .ok_or_else(|| "Failed to parse version from remote Cargo.toml".to_string())
}

/// Perform update check and return if a new version is available along with the version string
pub fn check_for_update(raw_url: &str) -> Result<(bool, String), String> {
    let latest = fetch_latest_version(raw_url)?;
    let current = env!("CARGO_PKG_VERSION");
    let is_newer = crate::utils::vercmp(&latest, current)? > 0;
    Ok((is_newer, latest))
}

/// Run self update (explicitly called by CLI or auto-update on startup)
pub fn run_self_update(config: &Config, is_auto: bool) -> Result<(), String> {
    if !is_auto {
        blog!("Checking for updates...");
    }

    let (is_newer, latest) = match check_for_update(&config.self_update_raw_url) {
        Ok(res) => res,
        Err(e) => {
            if is_auto {
                return Ok(()); // Fail silently on auto-update
            } else {
                return Err(format!("Update check failed: {}", e));
            }
        }
    };

    if !is_newer {
        if !is_auto {
            blog!("ABS is up-to-date (current version: {}).", env!("CARGO_PKG_VERSION").green());
        }
        return Ok(());
    }

    blog!(
        "New version available: {} (current version: {}). Starting update...",
        latest.green(),
        env!("CARGO_PKG_VERSION").yellow()
    );



    let tmp_dir = std::env::temp_dir().join(format!("abs_self_update_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()));
    if tmp_dir.exists() {
        let _ = fs::remove_dir_all(&tmp_dir);
    }
    fs::create_dir_all(&tmp_dir)
        .map_err(|e| format!("Failed to create temporary directory: {}", e))?;

    blog!("Cloning latest repository source from {}...", config.self_update_github_url);
    run_command(
        "git",
        &[
            "clone",
            "--depth=1",
            &format!("{}.git", config.self_update_github_url),
            tmp_dir.to_str().unwrap(),
        ],
        None::<&str>,
    )?;

    blog!("Compiling latest release...");
    run_command(
        "cargo",
        &["build", "--release"],
        Some(&tmp_dir),
    )?;

    let new_binary = tmp_dir.join("target").join("release").join("abs");
    if !new_binary.exists() {
        return Err("Compiled binary not found in target/release/abs".into());
    }

    let install_path = &config.self_update_install_path;
    blog!("Installing executable to {}...", install_path);

    let new_str = new_binary.to_string_lossy();
    let install_res = run_command(
        "install",
        &["-Dm755", new_str.as_ref(), install_path.as_ref()],
        None::<&str>,
    );

    if let Err(_) = install_res {
        vlog!("Standard install failed. Retrying with sudo...");
        run_command(
            "sudo",
            &["install", "-Dm755", new_str.as_ref(), install_path.as_ref()],
            None::<&str>,
        )?;
    }

    // Clean up
    let _ = fs::remove_dir_all(&tmp_dir);

    blog!("ABS successfully updated to version {}!", latest.green());
    Ok(())
}
