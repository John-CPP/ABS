use crate::utils::{check_sudo_removal, run_command};
use crate::{blog, die, ewarn};
use colored::Colorize;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

fn collect_pkgbuild_dirs(root: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if name == ".git" || name == "pkg" || name == "src" {
                continue;
            }
            collect_pkgbuild_dirs(&path, out);
        } else if path.file_name().and_then(|n| n.to_str()) == Some("PKGBUILD")
            && let Some(parent) = path.parent()
        {
            out.push(parent.to_path_buf());
        }
    }
}

fn find_pkg_dir(repo_dir: &Path, pkg_name: &str) -> Option<PathBuf> {
    let mut dirs = Vec::new();
    collect_pkgbuild_dirs(repo_dir, &mut dirs);

    if let Some(exact) = dirs.iter().find(|d| {
        d.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n == pkg_name)
            .unwrap_or(false)
    }) {
        return Some(exact.clone());
    }

    let pkg_name_lower = pkg_name.to_lowercase();
    dirs.into_iter().find(|d| {
        d.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.to_lowercase().contains(&pkg_name_lower))
            .unwrap_or(false)
    })
}

pub fn prepare_repo(
    pkg_name: &str,
    base_pkg_name: &str,
    repo_name: &str,
    repo_url: &str,
    packages_path: &str,
    clean: bool,
    force_update: bool,
) -> PathBuf {
    if repo_name == "arch" {
        let repo_dir = PathBuf::from(packages_path)
            .join("arch")
            .join(base_pkg_name);
        if clean && repo_dir.exists() {
            blog!("Cleaning old repository directory for {}", base_pkg_name);
            if let Err(e) = check_sudo_removal(&repo_dir) {
                die!("Failed to clean repository directory: {}", e);
            }
        }

        if !repo_dir.exists() {
            let clone_url = format!("{}/{}.git", repo_url.trim_end_matches('/'), base_pkg_name);
            blog!("Cloning arch package repo {}...", base_pkg_name);
            if let Err(e) = run_command(
                "git",
                &["clone", &clone_url, repo_dir.to_string_lossy().as_ref()],
                None::<&str>,
            ) {
                die!("Failed to clone repository {}: {}", clone_url, e);
            }
        }
        return repo_dir;
    }

    let repo_dir = PathBuf::from(packages_path).join(repo_name);
    if clean && repo_dir.exists() {
        blog!("Cleaning old repository directory for {}", repo_name);
        if let Err(e) = check_sudo_removal(&repo_dir) {
            die!("Failed to clean repository directory: {}", e);
        }
    }

    if !repo_dir.join(".git").exists() {
        blog!("Cloning repository '{}'...", repo_name);
        if let Err(e) = run_command(
            "git",
            &["clone", repo_url, repo_dir.to_string_lossy().as_ref()],
            None::<&str>,
        ) {
            die!("Failed to clone repository {}: {}", repo_url, e);
        }
    } else if force_update {
        blog!("Updating repo for {} (R flag used)", repo_name);
        loop {
            if run_command("git", &["pull", "--ff-only"], Some(&repo_dir)).is_ok() {
                break;
            }

            println!();
            ewarn!("Failed to update repository: {:?}", &repo_dir);
            println!("This is often caused by local modifications to PKGBUILDs.");
            println!("Options:");
            println!("  [r] Retry update (after you manually fix it in another terminal)");
            println!("  [d] Delete repository and re-clone (Warning: loses uncommitted changes!)");
            println!("  [a] Abort completely");

            print!("Choice [r/d/a]: ");
            io::stdout().flush().unwrap();

            let mut input = String::new();
            if io::stdin().read_line(&mut input).is_err() {
                die!("Failed to read from terminal");
            }

            match input.trim().to_lowercase().as_str() {
                "r" => {
                    println!("Retrying...");
                }
                "d" => {
                    println!("Deleting and re-cloning...");
                    if let Err(e) = check_sudo_removal(&repo_dir) {
                        die!("Failed to clean repository directory: {}", e);
                    }
                    if let Err(e) = run_command(
                        "git",
                        &["clone", repo_url, repo_dir.to_string_lossy().as_ref()],
                        None::<&str>,
                    ) {
                        die!("Failed to clone repository {}: {}", repo_url, e);
                    }
                    break;
                }
                "a" => {
                    die!("Update aborted by user.");
                }
                _ => {
                    println!("Invalid choice.");
                }
            }
        }
    }

    match find_pkg_dir(&repo_dir, base_pkg_name) {
        Some(path) => path,
        None => die!("Package {} not found in repository {}", pkg_name, repo_name),
    }
}
