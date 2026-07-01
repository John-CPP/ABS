use crate::vlog;
use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Repo dirs with a live `.PKGBUILD.emerge_backup` that still needs restoring. Because `die!`
/// calls `process::exit` (skipping `Drop`/`PkgbuildGuard`), a fatal error mid-build would otherwise
/// leave the user's git tree on a bumped/overridden PKGBUILD. [`restore_pending_pkgbuilds`] flushes
/// this set from the `die!` path so the working tree is always returned to its upstream state.
static PENDING_PKGBUILD_RESTORES: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

fn pending_restores() -> &'static Mutex<HashSet<PathBuf>> {
    PENDING_PKGBUILD_RESTORES.get_or_init(|| Mutex::new(HashSet::new()))
}

fn register_pending_restore(repo_dir: &Path) {
    if let Ok(mut set) = pending_restores().lock() {
        set.insert(repo_dir.to_path_buf());
    }
}

fn unregister_pending_restore(repo_dir: &Path) {
    if let Ok(mut set) = pending_restores().lock() {
        set.remove(repo_dir);
    }
}

/// Restore every PKGBUILD with an outstanding backup. Safe to call multiple times and from the
/// `die!` path (only does file renames; no panics, no sudo).
pub fn restore_pending_pkgbuilds() {
    let pending: Vec<PathBuf> = match pending_restores().lock() {
        Ok(mut set) => set.drain().collect(),
        Err(_) => return,
    };
    for repo_dir in pending {
        let original = repo_dir.join("PKGBUILD");
        let backup = repo_dir.join(".PKGBUILD.emerge_backup");
        if backup.exists() {
            let _ = fs::rename(&backup, &original);
        }
    }
}

/// Match Bash `tr -d '"'\'' '` on the pkgrel value: drop quotes and all whitespace.
fn bash_strip_pkgrel_value(raw: &str) -> String {
    raw.chars()
        .filter(|c| *c != '"' && *c != '\'' && !c.is_whitespace())
        .collect()
}

fn extract_pkgrel_stripped(pkgbuild_text: &str) -> Option<String> {
    let line_re = Regex::new(r"(?m)^pkgrel=(.*)$").unwrap();
    let caps = line_re.captures(pkgbuild_text)?;
    let raw_value = caps.get(1).map(|m| m.as_str()).unwrap_or("");
    let no_comment = raw_value.split('#').next().unwrap_or("").trim();
    let stripped = bash_strip_pkgrel_value(no_comment);
    (!stripped.is_empty()).then_some(stripped)
}

/// One Bash-style bump step from a **baseline** pkgrel string (used with the session backup).
fn compute_next_pkgrel(baseline: &str) -> String {
    debug_assert!(!baseline.is_empty());

    let re_suffix = Regex::new(r"^(.*)\.([0-9]+)$").unwrap();
    if let Some(caps) = re_suffix.captures(baseline) {
        let base = caps.get(1).unwrap().as_str();
        let suffix: u32 = caps.get(2).unwrap().as_str().parse().unwrap_or(0);
        if suffix >= 2 {
            return format!("{}.{}", base, suffix + 1);
        }
    }

    format!("{}.2", baseline)
}

/// Apply `pkgrel={next}` to live PKGBUILD text (same line replacement as Bash `sed`).
fn replace_all_pkgrel_lines(content: &str, next: &str) -> String {
    replace_pkgbuild_field(content, "pkgrel", next)
}

/// Replace the first `^key=...` line or append `key=value` when missing.
pub fn replace_pkgbuild_field(content: &str, key: &str, value: &str) -> String {
    let replace_re = Regex::new(&format!(r"(?m)^{}=.*$", regex::escape(key))).unwrap();
    if replace_re.is_match(content) {
        let replacement = format!("{key}={value}");
        replace_re
            .replace_all(content, regex::NoExpand(&replacement))
            .to_string()
    } else {
        let mut out = content.to_string();
        if !out.ends_with('\n') && !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("{key}={value}\n"));
        out
    }
}

/// Apply CLI `[key=value,...]` overrides to the working `PKGBUILD`.
pub fn apply_pkgbuild_overrides(repo_dir: &Path, overrides: &std::collections::HashMap<String, String>) {
    if overrides.is_empty() {
        return;
    }

    let pkgbuild_path = repo_dir.join("PKGBUILD");
    if !pkgbuild_path.exists() {
        vlog!("PKGBUILD not found, skipping overrides");
        return;
    }

    let mut content = fs::read_to_string(&pkgbuild_path).unwrap_or_default();
    for (key, value) in overrides {
        content = replace_pkgbuild_field(&content, key, value);
        vlog!("PKGBUILD override: {}={}", key, value);
    }

    if let Err(e) = fs::write(&pkgbuild_path, content) {
        vlog!("Failed to apply PKGBUILD overrides: {}", e);
    }
}

/// Bump `pkgrel` in the working `PKGBUILD`.
///
/// - After a **clean** run, `restore_pkgbuild` puts the tree back; the next run’s backup matches
///   live again, so we bump **once from upstream** (e.g. `1` → `1.2` every time).
/// - If the process was **stopped before restore** (Ctrl+Z, kill, crash), live `PKGBUILD` still
///   carries the last bumped `pkgrel` while the backup still holds upstream. We detect
///   `live != backup` and **chain** one more step from live (`1.2` → `1.3` → …).
pub fn bump_pkgrel(repo_dir: &Path) {
    let pkgbuild_path = repo_dir.join("PKGBUILD");
    let backup_path = repo_dir.join(".PKGBUILD.emerge_backup");

    if !pkgbuild_path.exists() {
        vlog!("PKGBUILD not found, skipping pkgrel bump");
        return;
    }

    let live_text = fs::read_to_string(&pkgbuild_path).unwrap_or_default();

    let backup_text = if backup_path.exists() {
        fs::read_to_string(&backup_path).unwrap_or_default()
    } else {
        vlog!(
            "No PKGBUILD backup found; using live PKGBUILD as bump baseline"
        );
        String::new()
    };

    let line_re = Regex::new(r"(?m)^pkgrel=(.*)$").unwrap();
    let live_has_pkgrel = line_re.is_match(&live_text);
    let backup_has_pkgrel = !backup_text.is_empty() && line_re.is_match(&backup_text);
    if !live_has_pkgrel && !backup_has_pkgrel {
        let mut out = live_text;
        if !out.ends_with('\n') && !out.is_empty() {
            out.push('\n');
        }
        out.push_str("pkgrel=1.2\n");
        if let Err(e) = fs::write(&pkgbuild_path, out) {
            vlog!("Failed to append pkgrel: {}", e);
        }
        return;
    }

    let live_pkgrel = extract_pkgrel_stripped(&live_text);
    let backup_pkgrel = if backup_has_pkgrel {
        extract_pkgrel_stripped(&backup_text)
    } else {
        None
    };

    let bump_from = match (&live_pkgrel, &backup_pkgrel) {
        (Some(live), Some(bak)) if live != bak => {
            vlog!(
                "PKGBUILD still bumped vs backup (no restore yet); chaining pkgrel from {}",
                live
            );
            live.clone()
        }
        (_, Some(bak)) => bak.clone(),
        (Some(live), None) => live.clone(),
        _ => String::new(),
    };

    if bump_from.is_empty() {
        let mut out = live_text;
        if !out.ends_with('\n') && !out.is_empty() {
            out.push('\n');
        }
        out.push_str("pkgrel=1.2\n");
        if let Err(e) = fs::write(&pkgbuild_path, out) {
            vlog!("Failed to append pkgrel: {}", e);
        }
        return;
    }

    let next = compute_next_pkgrel(&bump_from);
    let replaced = replace_all_pkgrel_lines(&live_text, &next);

    if let Err(e) = fs::write(&pkgbuild_path, replaced) {
        vlog!("Failed to bump pkgrel: {}", e);
    }
}

/// Extract `validpgpkeys` fingerprints from a PKGBUILD (for makepkg source signature checks).
pub fn parse_validpgpkeys(pkgbuild_text: &str) -> Vec<String> {
    let block_re = match Regex::new(r"(?s)validpgpkeys=\((.*?)\)") {
        Ok(re) => re,
        Err(_) => return Vec::new(),
    };
    let Some(caps) = block_re.captures(pkgbuild_text) else {
        return Vec::new();
    };
    let key_re = match Regex::new(r"[0-9A-Fa-f]{16,40}") {
        Ok(re) => re,
        Err(_) => return Vec::new(),
    };
    let mut keys = Vec::new();
    let mut seen = HashSet::new();
    for m in key_re.find_iter(&caps[1]) {
        let key = m.as_str().to_ascii_uppercase();
        if seen.insert(key.clone()) {
            keys.push(key);
        }
    }
    keys
}

/// Snapshot `PKGBUILD` before any emerge edits to the package tree.
///
/// Call order must match Bash `process_package` intent: **right after** `prepare_repo` and
/// **before** `PRE_UPDATE_COMMANDS` / `pre_update_command`, then sums/bump/build (see `build.rs`).
///
/// If a backup already exists (e.g. last run stopped before restore), it is **not** overwritten
/// so we keep the true upstream baseline for `bump_pkgrel`.
pub fn backup_pkgbuild(repo_dir: &Path) {
    let original = repo_dir.join("PKGBUILD");
    let backup = repo_dir.join(".PKGBUILD.emerge_backup");

    if !original.exists() {
        return;
    }
    if backup.exists() {
        // A leftover backup (process stopped before restore) is still pending restore.
        register_pending_restore(repo_dir);
        return;
    }
    if fs::copy(&original, &backup).is_ok() {
        register_pending_restore(repo_dir);
    }
}

pub fn restore_pkgbuild(repo_dir: &Path) {
    let original = repo_dir.join("PKGBUILD");
    let backup = repo_dir.join(".PKGBUILD.emerge_backup");

    if backup.exists() {
        let _ = fs::rename(&backup, &original);
    }
    unregister_pending_restore(repo_dir);
}

pub fn inject_compiler_env(repo_dir: &Path, cc: &str, cxx: &str) -> Result<(), String> {
    let pkgbuild_path = repo_dir.join("PKGBUILD");
    if !pkgbuild_path.exists() {
        return Err("PKGBUILD not found".to_string());
    }
    let original = fs::read_to_string(&pkgbuild_path)
        .map_err(|e| format!("Failed to read PKGBUILD: {}", e))?;

    let mut modified = format!(
        "export CC={}\nexport CXX={}\n",
        crate::utils::sh_single_quote(cc),
        crate::utils::sh_single_quote(cxx)
    );
    modified.push_str(&original);

    fs::write(&pkgbuild_path, modified)
        .map_err(|e| format!("Failed to write PKGBUILD: {}", e))?;
    Ok(())
}

pub fn update_pkgsums(repo_dir: &Path) -> bool {
    vlog!("==> Updating checksums (updpkgsums)...");
    if let Err(e) = crate::utils::run_command("updpkgsums", &[], Some(repo_dir)) {
        vlog!("Failed to run updpkgsums: {}", e);
        false
    } else {
        true
    }
}

/// Download source tarballs with `updpkgsums` before ramdisk setup (PGO stage 1 only, on disk).
/// When compilation uses tmpfs (`w`) but the git tree stays on disk, store tarballs in
/// `.makepkg-src/` on disk so they survive ramdisk teardown between PGO stages.
pub fn prefetch_pgo_sources(
    repo_dir: &Path,
    targets: &crate::ramdisk::RamdiskTargets,
) -> bool {
    if targets.build_workdir && !targets.packages {
        let srcdest = crate::ramdisk::srcdest_for_repo(repo_dir);
        if let Err(e) = std::fs::create_dir_all(&srcdest) {
            crate::vlog!("Failed to create SRCDEST {}: {e}", srcdest.display());
            return false;
        }
        let cmd = format!(
            "SRCDEST={} updpkgsums",
            crate::utils::sh_single_quote(&srcdest.to_string_lossy())
        );
        if let Err(e) = crate::utils::run_command("sh", &["-c", &cmd], Some(repo_dir)) {
            crate::vlog!("updpkgsums prefetch failed: {e}");
            false
        } else {
            true
        }
    } else {
        update_pkgsums(repo_dir)
    }
}

fn extract_base_package_name(dep: &str) -> String {
    let dep = dep.trim();
    let cleaned = dep.split(['<', '>', '=', ':']).next().unwrap_or(dep).trim();
    cleaned.to_string()
}

pub fn parse_pkg_dependencies(pkg_dir: &Path) -> Vec<String> {
    use crate::utils::run_command_with_output;
    let mut deps = Vec::new();
    let srcinfo_path = pkg_dir.join(".SRCINFO");
    let srcinfo_text = if srcinfo_path.is_file() {
        fs::read_to_string(&srcinfo_path).ok()
    } else {
        run_command_with_output("makepkg", &["--printsrcinfo"], Some(pkg_dir)).ok()
    };

    if let Some(text) = srcinfo_text {
        for line in text.lines() {
            let trimmed = line.trim();
            if let Some((key, value)) = trimmed.split_once('=') {
                let key = key.trim();
                if key == "depends" || key == "makedepends" {
                    let dep_name = extract_base_package_name(value);
                    if !dep_name.is_empty() {
                        deps.push(dep_name);
                    }
                }
            }
        }
    } else {
        let pkgbuild_path = pkg_dir.join("PKGBUILD");
        if let Ok(content) = fs::read_to_string(&pkgbuild_path) {
            let dep_array_re = Regex::new(r#"(?s)(depends|makedepends)=\((.*?)\)"#).unwrap();
            for caps in dep_array_re.captures_iter(&content) {
                let array_content = &caps[2];
                for word in array_content.split_whitespace() {
                    let clean_word = word.trim_matches(|c| c == '\'' || c == '"');
                    let dep_name = extract_base_package_name(clean_word);
                    if !dep_name.is_empty() {
                        deps.push(dep_name);
                    }
                }
            }
        }
    }

    deps.sort();
    deps.dedup();
    deps
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn replace_pkgbuild_field_preserves_dollar_signs() {
        let content = "pkgver=1.0\n";
        let out = replace_pkgbuild_field(content, "pkgver", "foo$bar");
        assert_eq!(out, "pkgver=foo$bar\n");
    }

    #[test]
    fn restore_pending_restores_backed_up_pkgbuild() {
        let temp = std::env::temp_dir().join(format!(
            "abs_pending_restore_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp).unwrap();
        let pkgbuild = temp.join("PKGBUILD");
        fs::write(&pkgbuild, "pkgver=1.0\npkgrel=1\n").unwrap();

        backup_pkgbuild(&temp);
        // Simulate a mid-build edit (pkgrel bump / override).
        fs::write(&pkgbuild, "pkgver=1.0\npkgrel=1.2\n").unwrap();

        restore_pending_pkgbuilds();

        let restored = fs::read_to_string(&pkgbuild).unwrap();
        assert_eq!(restored, "pkgver=1.0\npkgrel=1\n");
        assert!(!temp.join(".PKGBUILD.emerge_backup").exists());

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn parse_validpgpkeys_extracts_fingerprints() {
        let sample = r#"pkgname=curl
validpgpkeys=('27EDEAF22F3ABCEB50DB9A125CC908FDB71E12C2') # Daniel Stenberg
source=("git+https://github.com/curl/curl.git?signed#tag=curl-${pkgver//./_}")
"#;
        let keys = parse_validpgpkeys(sample);
        assert_eq!(keys, vec!["27EDEAF22F3ABCEB50DB9A125CC908FDB71E12C2"]);
    }

    #[test]
    fn parse_validpgpkeys_multiline_array() {
        let sample = r#"validpgpkeys=(
  'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'
  'BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB'
)"#;
        let keys = parse_validpgpkeys(sample);
        assert_eq!(keys.len(), 2);
    }

    #[test]
    fn test_inject_compiler_env() {
        let temp = std::env::temp_dir().join(format!("abs_test_pkgbuild_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(&temp).unwrap();
        let pkgbuild = temp.join("PKGBUILD");
        fs::write(&pkgbuild, "pkgname=foo\nbuild() {\n  make\n}\n").unwrap();

        inject_compiler_env(&temp, "clang", "clang++").unwrap();

        let content = fs::read_to_string(&pkgbuild).unwrap();
        assert!(content.contains("export CC='clang'\nexport CXX='clang++'\n"));
        assert!(content.contains("pkgname=foo\n"));

        let _ = fs::remove_dir_all(&temp);
    }
}
