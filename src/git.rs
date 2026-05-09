use crate::utils::{check_sudo_removal, run_command};
use crate::{blog, die, ewarn};
use colored::Colorize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// One `abs` process may call `prepare_repo(..., force_update: true)` many times for the same
/// shared clone (e.g. several `manual_update_packages` on `cachyos`). Skip redundant `git pull`s.
static SHARED_REPO_REMOTE_UP_TO_DATE: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

fn shared_repo_remote_cache() -> &'static Mutex<HashSet<PathBuf>> {
    SHARED_REPO_REMOTE_UP_TO_DATE.get_or_init(|| Mutex::new(HashSet::new()))
}

fn shared_repo_remote_already_updated(repo_dir: &Path) -> bool {
    shared_repo_remote_cache()
        .lock()
        .map(|g| g.contains(repo_dir))
        .unwrap_or(false)
}

fn shared_repo_remote_note_updated(repo_dir: &Path) {
    if let Ok(mut g) = shared_repo_remote_cache().lock() {
        g.insert(repo_dir.to_path_buf());
    }
}

/// Caches `collect_pkgbuild_dirs` per shared repo root so scanning a large tree (e.g. CachyOS)
/// happens once per `report_manual_update_versions` pass, not once per package.
pub type PkgbuildDirCache = HashMap<PathBuf, Vec<PathBuf>>;

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

fn list_pkgbuild_parent_dirs(repo_root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    collect_pkgbuild_dirs(repo_root, &mut dirs);
    dirs
}

fn find_pkg_dir_in_list(dirs: &[PathBuf], pkg_name: &str) -> Option<PathBuf> {
    if let Some(exact) = dirs.iter().find(|d| {
        d.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n == pkg_name)
            .unwrap_or(false)
    }) {
        return Some(exact.clone());
    }

    let pkg_name_lower = pkg_name.to_lowercase();
    dirs.iter()
        .find(|d| {
            d.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.to_lowercase().contains(&pkg_name_lower))
                .unwrap_or(false)
        })
        .cloned()
}

fn find_pkg_dir(repo_dir: &Path, pkg_name: &str) -> Option<PathBuf> {
    let dirs = list_pkgbuild_parent_dirs(repo_dir);
    find_pkg_dir_in_list(&dirs, pkg_name)
}

pub fn find_pkg_dir_cached(
    repo_dir: &Path,
    pkg_name: &str,
    cache: &mut PkgbuildDirCache,
) -> Option<PathBuf> {
    if !cache.contains_key(repo_dir) {
        cache.insert(repo_dir.to_path_buf(), list_pkgbuild_parent_dirs(repo_dir));
    }
    let dirs = cache.get(repo_dir).unwrap();
    find_pkg_dir_in_list(dirs, pkg_name)
}

#[allow(clippy::too_many_arguments)]
pub fn prepare_repo(
    pkg_name: &str,
    base_pkg_name: &str,
    repo_name: &str,
    repo_url: &str,
    packages_path: &str,
    clean: bool,
    force_update: bool,
    pkgbuild_cache: Option<&mut PkgbuildDirCache>,
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
            shared_repo_remote_note_updated(&repo_dir);
        } else if force_update
            && repo_dir.join(".git").exists()
            && !shared_repo_remote_already_updated(&repo_dir)
        {
            blog!("Updating arch package repo {}...", base_pkg_name);
            match run_command("git", &["pull", "--ff-only"], Some(&repo_dir)) {
                Ok(()) => shared_repo_remote_note_updated(&repo_dir),
                Err(e) => {
                    ewarn!("git pull failed for {}: {}", base_pkg_name, e);
                }
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
        shared_repo_remote_note_updated(&repo_dir);
    } else if force_update && !shared_repo_remote_already_updated(&repo_dir) {
        loop {
            if run_command("git", &["pull", "--ff-only"], Some(&repo_dir)).is_ok() {
                shared_repo_remote_note_updated(&repo_dir);
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
                    shared_repo_remote_note_updated(&repo_dir);
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

    let found = match pkgbuild_cache {
        Some(cache) => find_pkg_dir_cached(&repo_dir, base_pkg_name, cache),
        None => find_pkg_dir(&repo_dir, base_pkg_name),
    };
    match found {
        Some(path) => path,
        None => die!("Package {} not found in repository {}", pkg_name, repo_name),
    }
}
