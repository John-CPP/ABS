use crate::vlog;
use regex::Regex;
use std::fs;
use std::path::Path;

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
        replace_re
            .replace_all(content, format!("{key}={value}"))
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
        return;
    }
    let _ = fs::copy(&original, &backup);
}

pub fn restore_pkgbuild(repo_dir: &Path) {
    let original = repo_dir.join("PKGBUILD");
    let backup = repo_dir.join(".PKGBUILD.emerge_backup");

    if backup.exists() {
        let _ = fs::rename(&backup, &original);
    }
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

fn extract_base_package_name(dep: &str) -> String {
    let dep = dep.trim();
    let cleaned = dep.split(|c| c == '<' || c == '>' || c == '=' || c == ':').next().unwrap_or(dep).trim();
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
