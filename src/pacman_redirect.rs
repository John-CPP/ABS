//! Point pacman / yay / paru at the ramdisk for a system update or package build,
//! then put the original configs back. A journal on disk survives a crash.

use crate::config::Config;
use crate::ramdisk::{self, RamdiskTargets};
use crate::utils::{parse_command_argv, run_command};
use crate::{blog, ewarn};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

struct RedirectState {
    depth: u32,
}

static STATE: Mutex<RedirectState> = Mutex::new(RedirectState { depth: 0 });

pub struct Guard {
    active: bool,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mut state = STATE.lock().unwrap();
        state.depth = state.depth.saturating_sub(1);
        if state.depth == 0
            && let Err(e) = restore_files(&journal_dir())
        {
            ewarn!("Failed to restore package-manager configs: {e}");
        }
    }
}

/// Fix configs left pointed at the ramdisk by a crashed run. No-op when there is no journal.
pub fn restore_pending() {
    let dir = journal_dir();
    if !dir.join("manifest.json").is_file() {
        return;
    }
    let _state = STATE.lock().unwrap();
    if let Err(e) = restore_files(&dir) {
        ewarn!("Failed to restore package-manager configs: {e}");
    }
}

pub fn system_update_wants_redirect(config: &Config, cli_ramdisk: Option<&str>) -> bool {
    if let Some(code) = cli_ramdisk {
        if ramdisk::is_ramdisk_disabled(code) {
            return false;
        }
        return ramdisk::parse_ramdisk_targets(code)
            .map(|t| t.pacman_cache)
            .unwrap_or(false);
    }
    config.ramdisk.enabled && config.ramdisk.pacman_cache
}

/// Host pacman.conf only. Used while ABS runs makepkg.
pub fn begin_build(config: &Config) -> Result<Guard, String> {
    begin(config, None)
}

/// Host pacman.conf, plus yay or paru when `command` runs that helper.
pub fn begin_system_update(config: &Config, command: &str) -> Result<Guard, String> {
    begin(config, Some(command))
}

fn begin(config: &Config, command: Option<&str>) -> Result<Guard, String> {
    if crate::is_dry_run_mode() {
        blog!("[DRY RUN] would point package-manager configs at the ramdisk");
        return Ok(Guard { active: false });
    }

    let mut state = STATE.lock().unwrap();
    if state.depth == 0 {
        apply(config, command)?;
    }
    state.depth += 1;
    Ok(Guard { active: true })
}

fn apply(config: &Config, command: Option<&str>) -> Result<(), String> {
    let journal = journal_dir();
    restore_files(&journal)?;

    let targets = RamdiskTargets {
        pacman_cache: true,
        ..RamdiskTargets::default()
    };
    ramdisk::ensure_for_targets(config, &targets)?;
    let mount = ramdisk::session_mount_point()
        .ok_or_else(|| "ramdisk session missing after mount".to_string())?;
    let cache = mount.join("pacman");
    let pkgdest_dir = mount.join("pkgdest");
    fs::create_dir_all(&cache).map_err(|e| format!("create {}: {e}", cache.display()))?;
    fs::create_dir_all(&pkgdest_dir)
        .map_err(|e| format!("create {}: {e}", pkgdest_dir.display()))?;

    let pkgdest = if config.ramdisk.save_built_packages {
        config.paths.ready_made_packages_path.clone()
    } else {
        pkgdest_dir.to_string_lossy().into_owned()
    };
    let cache_s = cache.to_string_lossy().into_owned();
    let makepkg_conf = mount.join("abs-pkgdest.conf");

    let mut edits = Vec::new();
    let pacman_conf = PathBuf::from("/etc/pacman.conf");
    let original = read_file(&pacman_conf)?;
    let text = String::from_utf8_lossy(&original);
    edits.push(Edit {
        path: pacman_conf,
        new_contents: prepend_cache_dir(&text, &cache_s).into_bytes(),
    });

    if let Some(helper) = command.and_then(command_helper) {
        match helper.as_str() {
            "yay" => {
                let path = config_dir()?.join("yay/config.json");
                let (existing_makepkg, body) = yay_edit(&path, &mount.join("yay"), &makepkg_conf)?;
                write_makepkg_conf(&makepkg_conf, &pkgdest, existing_makepkg.as_deref())?;
                edits.push(Edit {
                    path,
                    new_contents: body.into_bytes(),
                });
            }
            "paru" => {
                let path = config_dir()?.join("paru/paru.conf");
                let (existing_makepkg, body) =
                    paru_edit(&path, &mount.join("paru"), &makepkg_conf)?;
                write_makepkg_conf(&makepkg_conf, &pkgdest, existing_makepkg.as_deref())?;
                edits.push(Edit {
                    path,
                    new_contents: body.into_bytes(),
                });
            }
            _ => {}
        }
    }

    blog!(
        "Pointing package-manager configs at {} (restore when this step finishes)",
        cache.display()
    );
    if let Err(e) = redirect_files(&journal, &edits) {
        let _ = restore_files(&journal);
        return Err(e);
    }
    Ok(())
}

fn command_helper(command: &str) -> Option<String> {
    let argv = parse_command_argv(command).ok()?;
    let mut i = 0;
    if argv
        .first()
        .is_some_and(|c| c == "sudo" || c.ends_with("/sudo"))
    {
        i = 1;
    }
    let bin = argv.get(i)?;
    let name = bin.rsplit('/').next().unwrap_or(bin);
    match name {
        "yay" | "paru" => Some(name.to_string()),
        _ => None,
    }
}

fn config_dir() -> Result<PathBuf, String> {
    dirs::config_dir().ok_or_else(|| "could not resolve the user config directory".to_string())
}

fn journal_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("abs")
        .join("ramdisk-redirect")
}

fn yay_edit(
    path: &Path,
    build_dir: &Path,
    makepkg_conf: &Path,
) -> Result<(Option<String>, String), String> {
    fs::create_dir_all(build_dir).map_err(|e| format!("create {}: {e}", build_dir.display()))?;
    let existing = if path.is_file() {
        Some(fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?)
    } else {
        None
    };
    let mut value: serde_json::Value = match &existing {
        Some(text) if !text.trim().is_empty() => serde_json::from_str(text)
            .map_err(|e| format!("{} is not JSON ({e})", path.display()))?,
        _ => serde_json::json!({}),
    };
    let previous = value
        .get("makepkgconf")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let Some(obj) = value.as_object_mut() else {
        return Err(format!("{} is not a JSON object", path.display()));
    };
    obj.insert(
        "buildDir".into(),
        serde_json::Value::String(build_dir.to_string_lossy().into_owned()),
    );
    obj.insert(
        "makepkgconf".into(),
        serde_json::Value::String(makepkg_conf.to_string_lossy().into_owned()),
    );
    let body =
        serde_json::to_string_pretty(&value).map_err(|e| format!("encode yay config: {e}"))?;
    Ok((previous, body))
}

fn paru_edit(
    path: &Path,
    clone_dir: &Path,
    makepkg_conf: &Path,
) -> Result<(Option<String>, String), String> {
    fs::create_dir_all(clone_dir).map_err(|e| format!("create {}: {e}", clone_dir.display()))?;
    let existing = if path.is_file() {
        fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?
    } else {
        String::new()
    };
    let previous = conf_option(&existing, "MakepkgConf");
    let mut body = set_conf_option(&existing, "CloneDir", &clone_dir.to_string_lossy());
    body = set_conf_option(&body, "MakepkgConf", &makepkg_conf.to_string_lossy());
    Ok((previous, body))
}

fn write_makepkg_conf(path: &Path, pkgdest: &str, existing: Option<&str>) -> Result<(), String> {
    let body = match existing {
        Some(base) => format!(
            "# Generated by ABS\n[[ -r {base} ]] && source {base}\nPKGDEST={dest}\n",
            base = crate::utils::sh_single_quote(base),
            dest = crate::utils::sh_single_quote(pkgdest),
        ),
        None => crate::build_env::makepkg_conf_with_trailer(&format!(
            "PKGDEST={}\n",
            crate::utils::sh_single_quote(pkgdest)
        )),
    };
    fs::write(path, body).map_err(|e| format!("write {}: {e}", path.display()))
}

/// First uncommented `CacheDir`, if any.
pub fn first_cache_dir(conf: &str) -> Option<String> {
    for line in conf.lines() {
        let t = line.trim();
        if t.starts_with('#') {
            continue;
        }
        let Some(rest) = strip_key(t, "CacheDir") else {
            continue;
        };
        if !rest.is_empty() {
            return Some(rest.to_string());
        }
    }
    None
}

/// Insert `ram` as the first CacheDir and keep every existing CacheDir.
pub fn prepend_cache_dir(conf: &str, ram: &str) -> String {
    if first_cache_dir(conf).as_deref() == Some(ram) {
        return conf.to_string();
    }
    let line = format!("CacheDir = {ram}\n");
    if let Some(idx) = first_key_line_start(conf, "CacheDir") {
        let mut out = String::with_capacity(conf.len() + line.len());
        out.push_str(&conf[..idx]);
        out.push_str(&line);
        out.push_str(&conf[idx..]);
        return out;
    }
    if let Some(idx) = conf.find("[options]") {
        let after = idx + "[options]".len();
        let nl = conf[after..]
            .find('\n')
            .map(|i| after + i + 1)
            .unwrap_or(conf.len());
        let mut out = String::with_capacity(conf.len() + line.len());
        out.push_str(&conf[..nl]);
        out.push_str(&line);
        out.push_str(&conf[nl..]);
        return out;
    }
    format!("{line}{conf}")
}

fn first_key_line_start(conf: &str, key: &str) -> Option<usize> {
    let mut offset = 0;
    for line in conf.split_inclusive('\n') {
        let t = line.trim();
        if !t.starts_with('#') && strip_key(t, key).is_some() {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

fn strip_key<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line
        .get(..key.len())
        .filter(|head| head.eq_ignore_ascii_case(key))
        .map(|_| line[key.len()..].trim_start())?;
    let rest = rest.strip_prefix('=')?.trim();
    Some(rest)
}

pub fn conf_option(text: &str, key: &str) -> Option<String> {
    let mut in_options = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_options = t.eq_ignore_ascii_case("[options]");
            continue;
        }
        if in_options
            && !t.starts_with('#')
            && let Some(rest) = strip_key(t, key)
            && !rest.is_empty()
        {
            return Some(rest.to_string());
        }
    }
    None
}

/// Set `key` under `[options]`, creating the section when it is missing.
pub fn set_conf_option(text: &str, key: &str, value: &str) -> String {
    let assignment = format!("{key} = {value}");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut in_options = false;
    let mut options_at = None;
    let mut replaced = false;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.starts_with('[') {
            if in_options {
                break;
            }
            in_options = t.eq_ignore_ascii_case("[options]");
            if in_options {
                options_at = Some(i);
            }
            continue;
        }
        if in_options && !t.starts_with('#') && strip_key(t, key).is_some() {
            lines[i] = assignment.clone();
            replaced = true;
            break;
        }
    }
    if !replaced {
        if let Some(at) = options_at {
            lines.insert(at + 1, assignment);
        } else {
            let mut out = format!("[options]\n{assignment}\n");
            if !text.is_empty() {
                out.push('\n');
                out.push_str(text);
            }
            return out;
        }
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') || text.is_empty() {
        out.push('\n');
    }
    out
}

struct Edit {
    path: PathBuf,
    new_contents: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    entries: Vec<ManifestEntry>,
}

#[derive(Serialize, Deserialize)]
struct ManifestEntry {
    path: String,
    existed: bool,
    backup: String,
}

fn redirect_files(journal: &Path, edits: &[Edit]) -> Result<(), String> {
    fs::create_dir_all(journal).map_err(|e| format!("create {}: {e}", journal.display()))?;
    let mut entries = Vec::new();
    for (i, edit) in edits.iter().enumerate() {
        let backup = format!("{i}.bak");
        let existed = edit.path.is_file();
        if existed {
            let bytes = read_file(&edit.path)?;
            fs::write(journal.join(&backup), bytes)
                .map_err(|e| format!("write backup {}: {e}", backup))?;
        }
        entries.push(ManifestEntry {
            path: edit.path.to_string_lossy().into_owned(),
            existed,
            backup,
        });
    }
    write_manifest(journal, &Manifest { entries })?;
    for edit in edits {
        write_bytes(&edit.path, &edit.new_contents)?;
    }
    Ok(())
}

fn write_manifest(journal: &Path, manifest: &Manifest) -> Result<(), String> {
    let raw = serde_json::to_vec_pretty(manifest).map_err(|e| format!("encode journal: {e}"))?;
    let tmp = journal.join("manifest.json.tmp");
    {
        let mut file = fs::File::create(&tmp).map_err(|e| format!("create journal: {e}"))?;
        file.write_all(&raw)
            .map_err(|e| format!("write journal: {e}"))?;
        file.sync_all().ok();
    }
    fs::rename(&tmp, journal.join("manifest.json")).map_err(|e| format!("commit journal: {e}"))
}

fn restore_files(journal: &Path) -> Result<(), String> {
    let manifest_path = journal.join("manifest.json");
    if !manifest_path.is_file() {
        return Ok(());
    }
    let raw = fs::read(&manifest_path).map_err(|e| format!("read journal: {e}"))?;
    let manifest: Manifest =
        serde_json::from_slice(&raw).map_err(|e| format!("parse journal: {e}"))?;
    for entry in &manifest.entries {
        let path = PathBuf::from(&entry.path);
        if entry.existed {
            let bytes = fs::read(journal.join(&entry.backup))
                .map_err(|e| format!("read backup {}: {e}", entry.backup))?;
            write_bytes(&path, &bytes)?;
        } else {
            remove_file(&path)?;
        }
    }
    let _ = fs::remove_dir_all(journal);
    if !manifest.entries.is_empty() {
        blog!("Restored package-manager configs");
    }
    Ok(())
}

fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    match fs::write(path, bytes) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => sudo_install(path, bytes),
        Err(e) => Err(format!("write {}: {e}", path.display())),
    }
}

fn sudo_install(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = std::env::temp_dir().join(format!(
        "abs-redirect-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::write(&tmp, bytes).map_err(|e| format!("write temp config: {e}"))?;
    let result = run_command(
        "sudo",
        &[
            "cp",
            tmp.to_string_lossy().as_ref(),
            path.to_string_lossy().as_ref(),
        ],
        None::<&str>,
    );
    let _ = fs::remove_file(&tmp);
    result.map_err(|e| format!("install {}: {e}", path.display()))
}

fn remove_file(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => run_command(
            "sudo",
            &["rm", "-f", path.to_string_lossy().as_ref()],
            None::<&str>,
        )
        .map_err(|err| format!("remove {}: {err}", path.display())),
        Err(e) => Err(format!("remove {}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepend_keeps_the_existing_cache_dir() {
        let conf = "[options]\nCacheDir = /var/cache/pacman/pkg\n";
        let out = prepend_cache_dir(conf, "/run/abs-ram/pacman");
        assert_eq!(
            first_cache_dir(&out).as_deref(),
            Some("/run/abs-ram/pacman")
        );
        assert!(out.contains("CacheDir = /var/cache/pacman/pkg"));
        assert_eq!(prepend_cache_dir(&out, "/run/abs-ram/pacman"), out);
    }

    #[test]
    fn prepend_inserts_under_options_when_unset() {
        let out = prepend_cache_dir("[options]\nColor\n", "/run/abs-ram/pacman");
        assert!(out.starts_with("[options]\nCacheDir = /run/abs-ram/pacman\n"));
    }

    #[test]
    fn set_conf_option_replaces_and_creates() {
        let out = set_conf_option("[options]\nCloneDir = /old\n", "CloneDir", "/ram/paru");
        assert_eq!(conf_option(&out, "CloneDir").as_deref(), Some("/ram/paru"));
        let created = set_conf_option("", "CloneDir", "/ram/paru");
        assert!(created.starts_with("[options]\nCloneDir = /ram/paru\n"));
    }

    #[test]
    fn journal_restores_edited_and_created_files() {
        let root = std::env::temp_dir().join(format!(
            "abs-redirect-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let journal = root.join("journal");
        let conf = root.join("pacman.conf");
        let original = b"[options]\nCacheDir = /disk\n".to_vec();
        fs::write(&conf, &original).unwrap();
        let created = root.join("yay.json");
        redirect_files(
            &journal,
            &[
                Edit {
                    path: conf.clone(),
                    new_contents: b"[options]\nCacheDir = /ram\nCacheDir = /disk\n".to_vec(),
                },
                Edit {
                    path: created.clone(),
                    new_contents: b"{}\n".to_vec(),
                },
            ],
        )
        .unwrap();
        assert!(created.is_file());
        assert!(fs::read_to_string(&conf).unwrap().contains("/ram"));
        restore_files(&journal).unwrap();
        assert_eq!(fs::read(&conf).unwrap(), original);
        assert!(!created.exists());
        assert!(!journal.exists());
        let _ = fs::remove_dir_all(&root);
    }
}
