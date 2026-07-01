use crate::utils::{check_sudo_removal, run_command};
use crate::{die, ewarn, vlog};
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

fn dir_name(d: &Path) -> Option<&str> {
    d.file_name().and_then(|n| n.to_str())
}

fn find_pkg_dir_in_list(dirs: &[PathBuf], pkg_name: &str) -> Option<PathBuf> {
    // 1. Exact directory name match (case-sensitive).
    if let Some(exact) = dirs.iter().find(|d| dir_name(d) == Some(pkg_name)) {
        return Some(exact.clone());
    }

    let pkg_name_lower = pkg_name.to_lowercase();

    // 2. Case-insensitive exact match.
    if let Some(ci_exact) = dirs
        .iter()
        .find(|d| dir_name(d).map(|n| n.eq_ignore_ascii_case(pkg_name)).unwrap_or(false))
    {
        return Some(ci_exact.clone());
    }

    // 3. Substring fallback. Multiple package dirs can contain the query as a substring
    // (e.g. `mesa` is in `lib32-mesa` and `mesa-utils`). Prefer the closest by name length to
    // avoid silently building the wrong package, and warn so the choice is visible.
    let mut candidates: Vec<&PathBuf> = dirs
        .iter()
        .filter(|d| {
            dir_name(d)
                .map(|n| n.to_lowercase().contains(&pkg_name_lower))
                .unwrap_or(false)
        })
        .collect();
    candidates.sort_by_key(|d| dir_name(d).map(|n| n.len()).unwrap_or(usize::MAX));

    let chosen = candidates.first()?;
    ewarn!(
        "No exact PKGBUILD directory for '{}'; using closest match '{}'",
        pkg_name,
        dir_name(chosen).unwrap_or_default()
    );
    Some((*chosen).clone())
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

/// Repositories where each package lives in its own git clone (`{base}/{repo}/{pkg}.git`).
pub fn is_per_package_repo(repo_name: &str) -> bool {
    matches!(
        repo_name.to_ascii_lowercase().as_str(),
        "arch" | "aur"
    )
}

fn per_package_repo_key(repo_name: &str) -> String {
    repo_name.to_ascii_lowercase()
}

/// Whether [`prepare_repo`] cloned or updated the git tree on this call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoSyncAction {
    Unchanged,
    Cloned,
    Updated,
}

#[derive(Debug, Clone)]
pub struct PrepareRepoResult {
    pub pkg_dir: PathBuf,
    pub sync_action: RepoSyncAction,
}

impl PrepareRepoResult {
    pub fn synced(&self) -> bool {
        matches!(
            self.sync_action,
            RepoSyncAction::Cloned | RepoSyncAction::Updated
        )
    }
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
) -> PrepareRepoResult {
    let git_run = |c: &str, a: &[&str], d: Option<&Path>| run_command(c, a, d);
    let mut sync_action = RepoSyncAction::Unchanged;
    if is_per_package_repo(repo_name) {
        let repo_key = per_package_repo_key(repo_name);
        let repo_dir = PathBuf::from(packages_path)
            .join(&repo_key)
            .join(base_pkg_name);
        if clean && repo_dir.exists() {
            vlog!("Cleaning old repository directory for {}", base_pkg_name);
            if let Err(e) = check_sudo_removal(&repo_dir) {
                die!("Failed to clean repository directory: {}", e);
            }
        }

        if !repo_dir.exists() {
            let clone_url = format!("{}/{}.git", repo_url.trim_end_matches('/'), base_pkg_name);
            vlog!(
                "Cloning {} package repo {}...",
                repo_key.to_uppercase(),
                base_pkg_name
            );
            if let Err(e) = git_run(
                "git",
                &["clone", &clone_url, repo_dir.to_string_lossy().as_ref()],
                None,
            ) {
                die!("Failed to clone repository {}: {}", clone_url, e);
            }
            sync_action = RepoSyncAction::Cloned;
            shared_repo_remote_note_updated(&repo_dir);
        } else if force_update
            && repo_dir.join(".git").exists()
            && !shared_repo_remote_already_updated(&repo_dir)
        {
            vlog!(
                "Updating {} package repo {}...",
                repo_key.to_uppercase(),
                base_pkg_name
            );
            match git_run("git", &["pull", "--ff-only"], Some(repo_dir.as_path())) {
                Ok(()) => {
                    sync_action = RepoSyncAction::Updated;
                    shared_repo_remote_note_updated(&repo_dir);
                }
                Err(e) => {
                    ewarn!("git pull failed for {}: {}", base_pkg_name, e);
                }
            }
        }
        return PrepareRepoResult {
            pkg_dir: repo_dir,
            sync_action,
        };
    }

    let repo_dir = PathBuf::from(packages_path).join(repo_name);
    if clean && repo_dir.exists() {
        vlog!("Cleaning old repository directory for {}", repo_name);
        if let Err(e) = check_sudo_removal(&repo_dir) {
            die!("Failed to clean repository directory: {}", e);
        }
    }

    if !repo_dir.join(".git").exists() {
        vlog!("Cloning repository '{}'...", repo_name);
        if let Err(e) = git_run(
            "git",
            &["clone", repo_url, repo_dir.to_string_lossy().as_ref()],
            None,
        ) {
            die!("Failed to clone repository {}: {}", repo_url, e);
        }
        sync_action = RepoSyncAction::Cloned;
        shared_repo_remote_note_updated(&repo_dir);
    } else if force_update && !shared_repo_remote_already_updated(&repo_dir) {
        loop {
            if git_run("git", &["pull", "--ff-only"], Some(repo_dir.as_path())).is_ok() {
                sync_action = RepoSyncAction::Updated;
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
                    sync_action = RepoSyncAction::Cloned;
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
    let pkg_dir = match found {
        Some(path) => path,
        None => die!("Package {} not found in repository {}", pkg_name, repo_name),
    };
    PrepareRepoResult {
        pkg_dir,
        sync_action,
    }
}

#[cfg(test)]
mod tests {
    use super::find_pkg_dir_in_list;
    use std::path::PathBuf;

    fn dirs(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(|n| PathBuf::from("/repo").join(n)).collect()
    }

    #[test]
    fn exact_match_wins_over_substrings() {
        let list = dirs(&["lib32-mesa", "mesa", "mesa-utils"]);
        let found = find_pkg_dir_in_list(&list, "mesa").unwrap();
        assert_eq!(found.file_name().unwrap(), "mesa");
    }

    #[test]
    fn substring_fallback_prefers_shortest_name() {
        let list = dirs(&["lib32-mesa", "mesa-extra-utils"]);
        let found = find_pkg_dir_in_list(&list, "mesa").unwrap();
        // Both contain "mesa"; the shorter directory name is the closer match.
        assert_eq!(found.file_name().unwrap(), "lib32-mesa");
    }

    #[test]
    fn case_insensitive_exact_beats_substring() {
        let list = dirs(&["lib32-mesa", "Mesa"]);
        let found = find_pkg_dir_in_list(&list, "mesa").unwrap();
        assert_eq!(found.file_name().unwrap(), "Mesa");
    }

    #[test]
    fn no_match_returns_none() {
        let list = dirs(&["firefox", "vim"]);
        assert!(find_pkg_dir_in_list(&list, "mesa").is_none());
    }
}
