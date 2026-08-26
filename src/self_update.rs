use crate::config::Config;
use crate::utils::{
    ABSGUI_ICON_SIZES, absgui_hicolor_icon_path, is_package_artifact, run_command,
    run_command_quiet, run_command_with_output_silent, sh_single_quote, vercmp_silent,
};
use crate::{blog, vlog};
use colored::Colorize;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

const OFFICIAL_REPOSITORY_URL: &str = "https://github.com/John-CPP/ABS.git";
/// `HEAD` resolves to the remote's default branch, so updates keep working across branch renames.
const OFFICIAL_REPOSITORY_BRANCH: &str = "HEAD";
const OFFICIAL_CARGO_TOML_URL: &str =
    "https://raw.githubusercontent.com/John-CPP/ABS/HEAD/Cargo.toml";
const OFFICIAL_SOURCE_ARCHIVE_URL: &str = "https://github.com/John-CPP/ABS/archive/HEAD.tar.gz";
// Root `[package]` must keep a literal `version = "x.y.z"`. Installs from
// before 2.0.2 cannot parse `version.workspace = true`.
const SOURCE_SYNC_ATTEMPTS: u32 = 4;

fn retry<T>(attempts: u32, op: impl FnMut(u32) -> Result<T, String>) -> Result<T, String> {
    retry_with_delay(attempts, std::time::Duration::from_millis(400), op)
}

fn retry_with_delay<T>(
    attempts: u32,
    delay: std::time::Duration,
    mut op: impl FnMut(u32) -> Result<T, String>,
) -> Result<T, String> {
    let attempts = attempts.max(1);
    let mut last = String::from("retry: no attempts");
    for i in 1..=attempts {
        match op(i) {
            Ok(v) => return Ok(v),
            Err(e) => {
                last = e;
                if i < attempts && !delay.is_zero() {
                    std::thread::sleep(delay.saturating_mul(i));
                }
            }
        }
    }
    Err(last)
}

fn extract_github_tarball(archive: &Path, dest: &Path) -> Result<(), String> {
    let Some(parent) = dest.parent() else {
        return Err(format!("cannot extract archive to {}", dest.display()));
    };
    let staging = parent.join(format!(
        ".{}.new",
        dest.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("abs-src")
    ));
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|e| format!("remove {}: {e}", staging.display()))?;
    }
    fs::create_dir_all(&staging).map_err(|e| format!("create {}: {e}", staging.display()))?;
    let archive_s = archive.to_string_lossy().into_owned();
    let staging_s = staging.to_string_lossy().into_owned();
    let extract = run_command(
        "tar",
        &[
            "-xzf",
            archive_s.as_str(),
            "-C",
            staging_s.as_str(),
            "--strip-components=1",
        ],
        None::<&str>,
    );
    if extract.is_err() || !staging.join("Cargo.toml").is_file() {
        let _ = fs::remove_dir_all(&staging);
        return Err(extract.err().unwrap_or_else(|| {
            format!(
                "source archive did not contain Cargo.toml after extract into {}",
                staging.display()
            )
        }));
    }
    if dest.exists() {
        fs::remove_dir_all(dest).map_err(|e| format!("remove {}: {e}", dest.display()))?;
    }
    fs::rename(&staging, dest).map_err(|e| {
        let _ = fs::remove_dir_all(&staging);
        format!("move extracted source to {}: {e}", dest.display())
    })
}

fn download_url_to_file(url: &str, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let dest_s = dest.to_string_lossy().into_owned();
    let mut args = crate::utils::curl_base_args();
    args.extend([
        "-m".into(),
        "60".into(),
        "-o".into(),
        dest_s,
        url.to_string(),
    ]);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_command("curl", &arg_refs, None::<&str>)
}

fn install_official_archive(dest: &Path) -> Result<(), String> {
    blog!(
        "Downloading ABS source archive from {}...",
        OFFICIAL_SOURCE_ARCHIVE_URL
    );
    let tmp_dir = dest.with_file_name(format!(".abs-archive-{}", std::process::id()));
    fs::create_dir_all(&tmp_dir).map_err(|e| format!("create {}: {e}", tmp_dir.display()))?;
    let archive = tmp_dir.join("HEAD.tar.gz");
    let result = (|| {
        retry(SOURCE_SYNC_ATTEMPTS, |i| {
            if i > 1 {
                blog!(
                    "Source archive download failed (attempt {i}/{SOURCE_SYNC_ATTEMPTS}); retrying..."
                );
            }
            download_url_to_file(OFFICIAL_SOURCE_ARCHIVE_URL, &archive)
        })?;
        extract_github_tarball(&archive, dest)
    })();
    let _ = fs::remove_dir_all(&tmp_dir);
    result
}

fn git_fetch_official(repo_dir: &Path) -> Result<(), String> {
    retry(SOURCE_SYNC_ATTEMPTS, |i| {
        if i > 1 {
            blog!("Git fetch failed (attempt {i}/{SOURCE_SYNC_ATTEMPTS}); retrying...");
        }
        run_command(
            "git",
            &["fetch", "--depth=1", "origin", OFFICIAL_REPOSITORY_BRANCH],
            Some(repo_dir),
        )?;
        run_command("git", &["reset", "--hard", "FETCH_HEAD"], Some(repo_dir))
    })
}

fn git_clone_official(abs_dir: &Path) -> Result<(), String> {
    let dest = abs_dir
        .to_str()
        .ok_or_else(|| "ABS checkout path is not valid UTF-8".to_string())?
        .to_string();
    retry(SOURCE_SYNC_ATTEMPTS, |i| {
        if i > 1 {
            blog!("Git clone failed (attempt {i}/{SOURCE_SYNC_ATTEMPTS}); retrying...");
            let _ = fs::remove_dir_all(abs_dir);
        }
        run_command(
            "git",
            &["clone", "--depth=1", OFFICIAL_REPOSITORY_URL, dest.as_str()],
            None::<&str>,
        )
    })
}

/// Parse `version` for the workspace `abs` package from raw Cargo.toml text.
fn parse_cargo_toml_version(text: &str) -> Option<String> {
    parse_cargo_toml_package_version(text, "abs")
}

fn toml_quoted_value(rest: &str) -> Option<String> {
    let v = rest.trim().trim_matches('"');
    if v.is_empty() || v == "true" || v == "false" {
        None
    } else {
        Some(v.to_string())
    }
}

fn parse_toml_section_key(text: &str, section: &str, key: &str) -> Option<String> {
    let header = format!("[{section}]");
    let prefix = format!("{key} = ");
    let mut in_section = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == header {
            in_section = true;
            continue;
        }
        if trimmed.starts_with('[') {
            in_section = false;
            continue;
        }

        if in_section && let Some(rest) = trimmed.strip_prefix(&prefix) {
            return toml_quoted_value(rest);
        }
    }
    None
}

fn parse_cargo_toml_package_version(text: &str, package: &str) -> Option<String> {
    let workspace_version = parse_toml_section_key(text, "workspace.package", "version");
    let mut in_package = false;
    let mut matches_name = false;
    let mut version = None;
    let mut inherit = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "[package]" {
            if matches_name {
                break;
            }
            in_package = true;
            matches_name = false;
            version = None;
            inherit = false;
            continue;
        }
        if trimmed.starts_with('[') {
            if matches_name {
                break;
            }
            in_package = false;
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(name) = trimmed.strip_prefix("name = ") {
            matches_name = name.trim().trim_matches('"') == package;
            continue;
        }
        if trimmed.starts_with("version.workspace") {
            inherit = true;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("version = ") {
            version = toml_quoted_value(rest);
        }
    }
    if !matches_name {
        return None;
    }
    if inherit { workspace_version } else { version }
}

/// Fetch the latest version from the official raw GitHub Cargo.toml
fn fetch_latest_version() -> Result<String, String> {
    vlog!(
        "Checking upstream ABS release at {}...",
        OFFICIAL_CARGO_TOML_URL
    );
    let start = std::time::Instant::now();
    let mut args = crate::utils::curl_base_args();
    args.extend([
        "--compressed".into(),
        "-m".into(),
        "5".into(),
        OFFICIAL_CARGO_TOML_URL.to_string(),
    ]);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = run_command_with_output_silent("curl", &arg_refs, None::<&str>)?;
    vlog!(
        "Upstream ABS version check finished in {:?}",
        start.elapsed()
    );
    parse_cargo_toml_version(&out)
        .ok_or_else(|| "Failed to parse version from remote Cargo.toml".to_string())
}

/// Perform update check and return if a new version is available along with the version string
pub fn check_for_update() -> Result<(bool, String), String> {
    let latest = fetch_latest_version()?;
    let current = env!("CARGO_PKG_VERSION");
    let is_newer = vercmp_silent(&latest, current)? > 0;
    Ok((is_newer, latest))
}

fn pacman_installed(pkg: &str) -> bool {
    // Silent even at normal verbosity: this is an internal probe, not user-facing output.
    run_command_with_output_silent("pacman", &["-Q", pkg], None::<&str>).is_ok()
}

fn should_use_pacman_update(config: &Config) -> bool {
    // Default (unset / None) and explicit true both use pacman; only false forces binary install.
    !matches!(config.self_update_use_pacman, Some(false))
}

fn pacman_packages_to_upgrade(want_gui: bool) -> Vec<&'static str> {
    if want_gui {
        vec!["abs", "absgui", "abs-full"]
    } else {
        vec!["abs"]
    }
}

pub(crate) fn wants_absgui(config: &Config) -> bool {
    crate::config::resolve_install_absgui(
        config.install_absgui,
        crate::config::load_install_absgui_pref(),
        pacman_installed("absgui") || pacman_installed("abs-full"),
    )
}

/// True when `filename` is a non-debug package for `pkg` at `pkgver` (any pkgrel/arch).
/// e.g. `abs-1.3.7-1-x86_64.pkg.tar.zst` for pkg=`abs`, pkgver=`1.3.7`.
fn is_pkg_artifact_for_version(filename: &str, pkg: &str, pkgver: &str) -> bool {
    is_package_artifact(filename)
        && filename.starts_with(&format!("{pkg}-{pkgver}-"))
        && !filename.contains("-debug-")
}

fn find_pkg_artifacts_for_version(
    dir: &Path,
    pkg: &str,
    pkgver: &str,
) -> Result<Vec<PathBuf>, String> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut matches = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
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

fn find_pkg_artifact(dir: &Path, pkg: &str, pkgver: &str) -> Result<PathBuf, String> {
    find_pkg_artifacts_for_version(dir, pkg, pkgver)?
        .pop()
        .ok_or_else(|| {
            format!(
                "no built package artifact for {pkg} {pkgver} in {}",
                dir.display()
            )
        })
}

/// Split-package leftovers for this `pkgver` block `makepkg` without `-f`.
/// Remove them only when we are about to rebuild (not when reusing a complete set).
fn remove_pkg_artifacts_for_version(dir: &Path, pkgver: &str) -> Result<(), String> {
    for pkg in ["abs", "absgui", "abs-full"] {
        for path in find_pkg_artifacts_for_version(dir, pkg, pkgver)? {
            fs::remove_file(&path)
                .map_err(|e| format!("failed to remove leftover {}: {e}", path.display()))?;
            vlog!("Removed leftover package artifact {}", path.display());
        }
    }
    Ok(())
}

fn remove_pkg_artifacts_for_version_in_dirs(dirs: &[PathBuf], pkgver: &str) -> Result<(), String> {
    for dir in dirs {
        remove_pkg_artifacts_for_version(dir, pkgver)?;
    }
    Ok(())
}

fn try_ready_pkg_artifacts(dir: &Path, pkgs: &[&str], pkgver: &str) -> Option<Vec<PathBuf>> {
    let mut artifacts = Vec::with_capacity(pkgs.len());
    for pkg in pkgs {
        match find_pkg_artifact(dir, pkg, pkgver) {
            Ok(path) => artifacts.push(path),
            Err(_) => return None,
        }
    }
    Some(artifacts)
}

fn try_ready_pkg_artifacts_in_dirs(
    dirs: &[PathBuf],
    pkgs: &[&str],
    pkgver: &str,
) -> Option<(PathBuf, Vec<PathBuf>)> {
    for dir in dirs {
        if let Some(found) = try_ready_pkg_artifacts(dir, pkgs, pkgver) {
            return Some((dir.clone(), found));
        }
    }
    None
}

fn push_unique_dir(dirs: &mut Vec<PathBuf>, path: PathBuf) {
    if !path.as_os_str().is_empty() && !dirs.iter().any(|d| d == &path) {
        dirs.push(path);
    }
}

/// Directories that may already hold `abs` / `absgui` / `abs-full` packages.
/// Prefer `ready_made_packages_path` (shared PKGDEST); `aur/` is a legacy fallback.
fn self_update_artifact_dirs(config: &Config, repo_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    push_unique_dir(
        &mut dirs,
        PathBuf::from(&config.paths.ready_made_packages_path),
    );
    if let Some(repo) = repo_dir {
        push_unique_dir(&mut dirs, repo.join("aur"));
    } else {
        push_unique_dir(
            &mut dirs,
            PathBuf::from(&config.paths.packages_path)
                .join("abs")
                .join("aur"),
        );
    }
    dirs
}

fn install_pacman_artifacts(artifacts: &[PathBuf], pkg_names: &[&str]) -> Result<(), String> {
    blog!("Installing pacman package(s): {}", pkg_names.join(", "));

    let mut args = vec!["-U".to_string(), "--noconfirm".to_string()];
    for artifact in artifacts {
        args.push(artifact.to_string_lossy().into_owned());
    }

    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    if run_command_quiet("pacman", &arg_refs, None::<&str>).is_err() {
        vlog!("Non-root pacman failed; retrying with sudo...");
        let mut sudo_args = vec!["pacman".to_string()];
        sudo_args.extend(args);
        run_command(
            "sudo",
            &sudo_args.iter().map(String::as_str).collect::<Vec<_>>(),
            None::<&str>,
        )?;
    }
    Ok(())
}

fn run_pacman_self_update(
    config: &Config,
    repo_dir: &Path,
    expected_version: &str,
) -> Result<(), String> {
    let aur_dir = repo_dir.join("aur");
    if !aur_dir.join("PKGBUILD").exists() {
        return Err(format!(
            "aur/PKGBUILD not found in {} (expected Arch packaging layout)",
            repo_dir.display()
        ));
    }

    let pkgdest = config.paths.ready_made_packages_path.as_str();
    let to_install = pacman_packages_to_upgrade(wants_absgui(config));
    let search_dirs = self_update_artifact_dirs(config, Some(repo_dir));
    let artifacts = if let Some((dir, ready)) =
        try_ready_pkg_artifacts_in_dirs(&search_dirs, &to_install, expected_version)
    {
        blog!(
            "Using already-built pacman packages for {} from {}...",
            expected_version,
            dir.display()
        );
        ready
    } else {
        // Partial same-version artifacts make makepkg exit with "already been built".
        remove_pkg_artifacts_for_version_in_dirs(&search_dirs, expected_version)?;
        fs::create_dir_all(pkgdest)
            .map_err(|e| format!("Failed to create ready_made_packages_path {}: {e}", pkgdest))?;
        blog!(
            "Building pacman packages from {} into {}...",
            aur_dir.display(),
            pkgdest
        );
        let cmdline = format!(
            "PKGDEST={} makepkg -Csr --noconfirm",
            sh_single_quote(pkgdest)
        );
        run_command("sh", &["-c", &cmdline], Some(&aur_dir))?;
        let mut built = Vec::new();
        for pkg in &to_install {
            built.push(
                find_pkg_artifact(Path::new(pkgdest), pkg, expected_version)
                    .or_else(|_| find_pkg_artifact(&aur_dir, pkg, expected_version))?,
            );
        }
        built
    };

    install_pacman_artifacts(&artifacts, &to_install)
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

/// Directory of the official ABS git checkout used by `--self-update`.
pub fn abs_install_checkout_dir(config: &Config) -> PathBuf {
    PathBuf::from(&config.paths.packages_path).join("abs")
}

fn sync_source_repo(config: &Config, expected_version: &str) -> Result<PathBuf, String> {
    let packages_path = config.paths.packages_path.clone();
    let abs_dir = abs_install_checkout_dir(config);

    let official_checkout =
        abs_dir.exists() && abs_dir.join(".git").exists() && remote_is_official(&abs_dir);

    if official_checkout {
        blog!("Updating ABS repository in {}...", abs_dir.display());
        if git_fetch_official(&abs_dir).is_err() {
            blog!(
                "Git fetch failed; downloading source archive instead of deleting the existing checkout..."
            );
            if let Err(e) = install_official_archive(&abs_dir) {
                return Err(format!(
                    "Failed to update existing ABS checkout (kept at {}): {e}",
                    abs_dir.display()
                ));
            }
        }
    } else {
        if abs_dir.exists() {
            vlog!(
                "Existing self-update checkout has an unexpected origin. Re-cloning the official repository..."
            );
            let _ = fs::remove_dir_all(&abs_dir);
        }
        blog!(
            "Cloning latest repository source from {}...",
            OFFICIAL_REPOSITORY_URL
        );
        fs::create_dir_all(&packages_path)
            .map_err(|e| format!("Failed to create packages directory: {}", e))?;
        if let Err(clone_err) = git_clone_official(&abs_dir) {
            blog!("Git clone failed; downloading source archive...");
            if let Err(archive_err) = install_official_archive(&abs_dir) {
                return Err(format!(
                    "{clone_err}; archive fallback also failed: {archive_err}"
                ));
            }
        }
    }

    if !source_version_matches(&abs_dir, expected_version)? {
        return Err(format!(
            "official repository source version does not match checked update version {expected_version}"
        ));
    }

    Ok(abs_dir)
}

/// Install `src` to `dest` with `install -DmMODE`, retrying via sudo on failure.
fn install_file(src: &Path, dest: &Path, mode: &str) -> Result<(), String> {
    let src_str = src.to_string_lossy();
    let dest_str = dest.to_string_lossy();
    let mode_flag = format!("-Dm{mode}");
    let install_res = run_command_quiet(
        "install",
        &[mode_flag.as_str(), src_str.as_ref(), dest_str.as_ref()],
        None::<&str>,
    );
    if install_res.is_err() {
        vlog!(
            "Standard install failed for {}. Retrying with sudo...",
            dest.display()
        );
        run_command(
            "sudo",
            &[
                "install",
                mode_flag.as_str(),
                src_str.as_ref(),
                dest_str.as_ref(),
            ],
            None::<&str>,
        )?;
    }
    Ok(())
}

/// Derive `…/absgui` from `self_update_install_path` (e.g. `/usr/bin/abs` → `/usr/bin/absgui`).
fn absgui_install_path(abs_install_path: &str) -> PathBuf {
    Path::new(abs_install_path).with_file_name("absgui")
}

fn parse_abs_cli_version(stdout: &str) -> Option<String> {
    let line = stdout.lines().next()?.trim();
    line.strip_prefix("abs ")
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

fn abs_binaries_from_path_env(path_env: &OsStr) -> Vec<PathBuf> {
    std::env::split_paths(path_env)
        .map(|dir| dir.join("abs"))
        .filter(|p| p.is_file())
        .collect()
}

fn paths_refer_to_same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

fn unique_shadowing_abs_paths(installed: &Path, candidates: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for p in candidates {
        if !p.is_file() {
            continue;
        }
        if paths_refer_to_same_file(p, installed) {
            continue;
        }
        if out.iter().any(|e| paths_refer_to_same_file(e, p)) {
            continue;
        }
        out.push(p.clone());
    }
    out
}

fn authoritative_abs_path(config: &Config) -> PathBuf {
    if should_use_pacman_update(config) {
        PathBuf::from("/usr/bin/abs")
    } else {
        PathBuf::from(&config.self_update_install_path)
    }
}

fn read_abs_binary_version(path: &Path) -> Option<String> {
    let path_s = path.to_str()?;
    let out = run_command_with_output_silent(path_s, &["--version"], None::<&str>).ok()?;
    parse_abs_cli_version(&out)
}

fn version_covers_latest(installed: &str, latest: &str) -> bool {
    vercmp_silent(installed, latest)
        .map(|c| c >= 0)
        .unwrap_or(false)
}

fn shadowing_abs_candidates(installed: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        candidates.push(exe);
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(abs_binaries_from_path_env(&path));
    }
    unique_shadowing_abs_paths(installed, &candidates)
}

/// Replace `dest` even if it is the currently running binary (unlink + rename).
fn replace_file(src: &Path, dest: &Path, mode: &str) -> Result<(), String> {
    let dest_name = dest.file_name().unwrap_or_default().to_string_lossy();
    let tmp = dest.with_file_name(format!(".{}.abs-new-{}", dest_name, std::process::id()));
    install_file(src, &tmp, mode)?;
    if fs::rename(&tmp, dest).is_ok() {
        return Ok(());
    }
    vlog!(
        "Rename to {} failed; retrying with sudo mv...",
        dest.display()
    );
    let tmp_s = tmp.to_string_lossy().into_owned();
    let dest_s = dest.to_string_lossy().into_owned();
    if run_command(
        "sudo",
        &["mv", "-f", tmp_s.as_str(), dest_s.as_str()],
        None::<&str>,
    )
    .is_err()
    {
        let _ = fs::remove_file(&tmp);
        let _ = run_command_quiet("sudo", &["rm", "-f", tmp_s.as_str()], None::<&str>);
        return Err(format!("failed to replace {}", dest.display()));
    }
    Ok(())
}

fn refresh_shadowing_abs_installs(installed: &Path, want_gui: bool) -> Result<(), String> {
    let shadows = shadowing_abs_candidates(installed);
    if shadows.is_empty() {
        return Ok(());
    }
    if !installed.is_file() {
        return Err(format!(
            "updated abs was not found at {}",
            installed.display()
        ));
    }
    for dest in shadows {
        blog!(
            "Replacing leftover abs at {} with {}...",
            dest.display(),
            installed.display()
        );
        replace_file(installed, &dest, "755")?;
        if want_gui {
            let src_gui = installed.with_file_name("absgui");
            let dest_gui = dest.with_file_name("absgui");
            if src_gui.is_file() && dest_gui.is_file() {
                blog!("Replacing leftover absgui at {}...", dest_gui.display());
                replace_file(&src_gui, &dest_gui, "755")?;
            }
        }
    }
    Ok(())
}

fn warn_if_refresh_failed(result: Result<(), String>, installed: &Path) {
    if let Err(e) = result {
        eprintln!(
            "{} Updated {} but could not replace leftover abs on PATH: {e}",
            "==> WARNING:".yellow(),
            installed.display()
        );
    }
}

fn run_binary_self_update(config: &Config, repo_dir: &Path) -> Result<(), String> {
    blog!("Compiling latest release...");
    run_command("cargo", &["build", "--release"], Some(repo_dir))?;

    let release_dir = repo_dir.join("target").join("release");
    let abs_binary = release_dir.join("abs");
    if !abs_binary.exists() {
        return Err("Compiled binary not found in target/release/abs".into());
    }

    let abs_install = PathBuf::from(&config.self_update_install_path);
    blog!("Installing executable to {}...", abs_install.display());
    install_file(&abs_binary, &abs_install, "755")?;

    let gui_binary = release_dir.join("absgui");
    if wants_absgui(config) && gui_binary.exists() {
        let gui_install = absgui_install_path(&config.self_update_install_path);
        blog!("Installing executable to {}...", gui_install.display());
        install_file(&gui_binary, &gui_install, "755")?;

        // Match README / pacman package layout when installing under /usr/bin.
        if abs_install
            .parent()
            .is_some_and(|p| p == Path::new("/usr/bin"))
        {
            let desktop = repo_dir.join("absgui").join("absgui.desktop");
            let icons_dir = repo_dir.join("absgui").join("assets").join("icons");
            if desktop.exists() {
                install_file(
                    &desktop,
                    Path::new("/usr/share/applications/absgui.desktop"),
                    "644",
                )?;
            }
            for size in ABSGUI_ICON_SIZES {
                let src = icons_dir.join(format!("icon_{size}.png"));
                if src.exists() {
                    install_file(&src, &absgui_hicolor_icon_path(*size), "644")?;
                }
            }
        }
    } else if wants_absgui(config) {
        eprintln!(
            "{} Compiled absgui not found in target/release; left existing GUI install unchanged.",
            "==> WARNING:".yellow()
        );
    }

    Ok(())
}

/// Run self update (explicitly called by CLI or auto-update on startup)
pub fn run_self_update(config: &Config, is_auto: bool) -> Result<bool, String> {
    if !is_auto {
        blog!("Checking for updates...");
    }

    let (is_newer, latest) = match check_for_update() {
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

    let install_path = authoritative_abs_path(config);
    let want_gui = wants_absgui(config);
    if read_abs_binary_version(&install_path).is_some_and(|v| version_covers_latest(&v, &latest)) {
        blog!(
            "Newer abs {} is already installed at {}; updating leftover copies on PATH...",
            latest,
            install_path.display()
        );
        refresh_shadowing_abs_installs(&install_path, want_gui)?;
        blog!("ABS successfully updated to version {}!", latest.green());
        return Ok(true);
    }

    if should_use_pacman_update(config) {
        let to_install = pacman_packages_to_upgrade(want_gui);
        let search_dirs = self_update_artifact_dirs(config, None);
        if let Some((dir, ready)) =
            try_ready_pkg_artifacts_in_dirs(&search_dirs, &to_install, &latest)
        {
            blog!(
                "Using already-built pacman packages for {} from {}...",
                latest,
                dir.display()
            );
            install_pacman_artifacts(&ready, &to_install)
                .map_err(|e| format!("Pacman self-update failed: {e}"))?;
            warn_if_refresh_failed(
                refresh_shadowing_abs_installs(&install_path, want_gui),
                &install_path,
            );
            blog!(
                "ABS successfully updated to version {} via pacman!",
                latest.green()
            );
            return Ok(true);
        }

        let repo_dir = sync_source_repo(config, &latest)?;
        run_pacman_self_update(config, &repo_dir, &latest)
            .map_err(|e| format!("Pacman self-update failed: {e}"))?;
        warn_if_refresh_failed(
            refresh_shadowing_abs_installs(&install_path, want_gui),
            &install_path,
        );
        blog!(
            "ABS successfully updated to version {} via pacman!",
            latest.green()
        );
        return Ok(true);
    }

    let repo_dir = sync_source_repo(config, &latest)?;
    run_binary_self_update(config, &repo_dir)?;
    warn_if_refresh_failed(
        refresh_shadowing_abs_installs(&install_path, want_gui),
        &install_path,
    );
    blog!("ABS successfully updated to version {}!", latest.green());
    Ok(true)
}

/// True when a failed `--self-update` already found a newer version and then clone/build/install failed.
pub fn is_retryable_self_update_error(err: &str) -> bool {
    !err.starts_with("Update check failed:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_succeeds_on_a_later_attempt() {
        let mut n = 0;
        let result = retry_with_delay(3, std::time::Duration::ZERO, |_| {
            n += 1;
            if n < 3 {
                Err(format!("fail {n}"))
            } else {
                Ok(n)
            }
        });
        assert_eq!(result, Ok(3));
        assert_eq!(n, 3);
    }

    #[test]
    fn retry_returns_the_last_error() {
        let result: Result<(), String> =
            retry_with_delay(3, std::time::Duration::ZERO, |i| Err(format!("fail {i}")));
        assert_eq!(result, Err("fail 3".into()));
    }

    #[test]
    fn extract_github_tarball_strips_top_level_directory() {
        let root = std::env::temp_dir().join(format!(
            "abs_tarball_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let packed = root.join("packed");
        let inner = packed.join("ABS-HEAD");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join("Cargo.toml"), "[package]\nname = \"abs\"\n").unwrap();
        fs::write(inner.join("README.md"), "ok").unwrap();
        let archive = root.join("HEAD.tar.gz");
        std::process::Command::new("tar")
            .args([
                "-czf",
                archive.to_str().unwrap(),
                "-C",
                packed.to_str().unwrap(),
                "ABS-HEAD",
            ])
            .status()
            .unwrap();
        let dest = root.join("dest");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("stale"), "old").unwrap();
        extract_github_tarball(&archive, &dest).unwrap();
        assert!(dest.join("Cargo.toml").is_file());
        assert!(dest.join("README.md").is_file());
        assert!(!dest.join("ABS-HEAD").exists());
        assert!(!dest.join("stale").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parse_remote_cargo_version() {
        let text = r#"[package]
name = "abs"
version = "1.3.4"
"#;
        assert_eq!(parse_cargo_toml_version(text).as_deref(), Some("1.3.4"));
    }

    #[test]
    fn parse_workspace_inherited_package_version() {
        let text = r#"[workspace.package]
version = "2.0.1"

[package]
name = "abs"
version.workspace = true
"#;
        assert_eq!(parse_cargo_toml_version(text).as_deref(), Some("2.0.1"));
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
        assert!(is_pkg_artifact_for_version(
            "abs-1.3.7-1-x86_64.pkg.tar.xz",
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

    #[test]
    fn missing_artifact_dir_is_empty_not_error() {
        let missing = PathBuf::from("/no/such/abs-self-update-dir");
        assert!(
            find_pkg_artifacts_for_version(&missing, "abs", "1.6.0")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn try_ready_pkg_artifacts_requires_complete_set() {
        let dir = std::env::temp_dir().join(format!(
            "abs_self_update_ready_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("abs-1.6.0-1-x86_64.pkg.tar.zst"), b"x").unwrap();
        assert!(try_ready_pkg_artifacts(&dir, &["abs", "absgui"], "1.6.0").is_none());
        fs::write(dir.join("absgui-1.6.0-1-x86_64.pkg.tar.zst"), b"x").unwrap();
        let found = try_ready_pkg_artifacts(&dir, &["abs", "absgui"], "1.6.0").unwrap();
        assert_eq!(found.len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_ready_pkg_artifacts_in_dirs_uses_first_complete_set() {
        let root = std::env::temp_dir().join(format!(
            "abs_self_update_dirs_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let pkgdest = root.join("ready");
        let aur = root.join("aur");
        fs::create_dir_all(&pkgdest).unwrap();
        fs::create_dir_all(&aur).unwrap();
        fs::write(pkgdest.join("abs-1.6.0-1-x86_64.pkg.tar.zst"), b"a").unwrap();
        fs::write(pkgdest.join("absgui-1.6.0-1-x86_64.pkg.tar.zst"), b"b").unwrap();
        fs::write(aur.join("abs-1.6.0-1-x86_64.pkg.tar.zst"), b"old").unwrap();
        let (dir, files) =
            try_ready_pkg_artifacts_in_dirs(&[pkgdest.clone(), aur], &["abs", "absgui"], "1.6.0")
                .unwrap();
        assert_eq!(dir, pkgdest);
        assert_eq!(files.len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn self_update_artifact_dirs_prefers_ready_made_packages_path() {
        let config: crate::config::Config = toml::from_str(
            r#"
manual_update_packages = []
skip_install_packages = []
[paths]
packages_path = "/mnt/share/packages"
chroot_base_path = "/mnt/share/chroot"
ready_made_packages_path = "/mnt/share/ready"
[build]
default_environment = "local"
[system_update]
command_to_update_repositories = "pacman -Sy"
command_to_perform_system_update = "pacman -Syu"
ignore_flag = "--ignore"
ignore_packages = []
[repositories]
default = "arch"
[packages]
"#,
        )
        .unwrap();
        let dirs = self_update_artifact_dirs(&config, None);
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/mnt/share/ready"),
                PathBuf::from("/mnt/share/packages/abs/aur"),
            ]
        );
        let repo = PathBuf::from("/mnt/share/packages/abs");
        let dirs = self_update_artifact_dirs(&config, Some(&repo));
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/mnt/share/ready"),
                PathBuf::from("/mnt/share/packages/abs/aur"),
            ]
        );
    }

    /// Parser shipped in abs <= 2.0.1: only `version = "..."` under `[package]`.
    /// Installed `--self-update` fetches GitHub `Cargo.toml`; that format must stay parseable.
    fn parse_legacy_package_version(text: &str) -> Option<String> {
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
                matches_name = name.trim().trim_matches('"') == "abs";
                continue;
            }
            if matches_name && let Some(version) = trimmed.strip_prefix("version = ") {
                return Some(version.trim().trim_matches('"').to_string());
            }
        }
        None
    }

    #[test]
    fn legacy_self_update_parser_cannot_read_workspace_inherited_version() {
        let text = r#"[workspace.package]
version = "2.0.2"

[package]
name = "abs"
version.workspace = true
"#;
        assert_eq!(parse_legacy_package_version(text), None);
        assert_eq!(parse_cargo_toml_version(text).as_deref(), Some("2.0.2"));
    }

    #[test]
    fn root_cargo_toml_stays_parseable_by_pre_workspace_self_update_clients() {
        let root = include_str!("../Cargo.toml");
        let version = parse_legacy_package_version(root).expect(
            "root [package] must keep version = \"x.y.z\" so installed abs --self-update can parse GitHub Cargo.toml",
        );
        assert_eq!(version, env!("CARGO_PKG_VERSION"));
    }

    /// A stale member version breaks `cargo build --locked` after a release bump.
    /// Members inherit `[workspace.package] version` from the root Cargo.toml.
    #[test]
    fn workspace_member_versions_stay_in_sync() {
        let root = include_str!("../Cargo.toml");
        let gui = include_str!("../absgui/Cargo.toml");
        let i18n = include_str!("../abs-i18n/Cargo.toml");
        let version = parse_cargo_toml_version(root).expect("root Cargo.toml has a version");
        assert_eq!(version, env!("CARGO_PKG_VERSION"));
        assert!(
            gui.contains("version.workspace = true"),
            "absgui must inherit workspace.package.version"
        );
        assert!(
            i18n.contains("version.workspace = true"),
            "abs-i18n must inherit workspace.package.version"
        );
    }

    #[test]
    fn abs_install_checkout_is_packages_path_abs() {
        let config: crate::config::Config = toml::from_str(
            r#"
manual_update_packages = []
skip_install_packages = []
[paths]
packages_path = "/mnt/share/packages"
chroot_base_path = "/mnt/share/chroot"
ready_made_packages_path = "/mnt/share/ready"
[build]
default_environment = "local"
[system_update]
command_to_update_repositories = "pacman -Sy"
command_to_perform_system_update = "pacman -Syu"
ignore_flag = "--ignore"
ignore_packages = []
[repositories]
default = "arch"
[packages]
"#,
        )
        .unwrap();
        assert_eq!(
            abs_install_checkout_dir(&config),
            PathBuf::from("/mnt/share/packages/abs")
        );
    }

    #[test]
    fn retryable_self_update_error_skips_update_check() {
        assert!(!is_retryable_self_update_error(
            "Update check failed: curl timed out"
        ));
        assert!(is_retryable_self_update_error(
            "official repository source version does not match checked update version 1.6.0"
        ));
        assert!(is_retryable_self_update_error(
            "Pacman self-update failed: makepkg"
        ));
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

    #[test]
    fn absgui_path_siblings_abs_install_path() {
        assert_eq!(
            absgui_install_path("/usr/bin/abs"),
            PathBuf::from("/usr/bin/absgui")
        );
        assert_eq!(
            absgui_install_path("/home/user/.local/bin/abs"),
            PathBuf::from("/home/user/.local/bin/absgui")
        );
    }

    #[test]
    fn pacman_packages_follow_absgui_choice() {
        assert_eq!(
            pacman_packages_to_upgrade(true),
            vec!["abs", "absgui", "abs-full"]
        );
        assert_eq!(pacman_packages_to_upgrade(false), vec!["abs"]);
    }

    #[test]
    fn parse_abs_cli_version_from_clap() {
        assert_eq!(
            parse_abs_cli_version("abs 1.5.0\n").as_deref(),
            Some("1.5.0")
        );
        assert_eq!(parse_abs_cli_version("abs 2.0.3").as_deref(), Some("2.0.3"));
        assert_eq!(parse_abs_cli_version("not-abs"), None);
    }

    #[test]
    fn abs_binaries_from_path_env_lists_existing_files() {
        let root = std::env::temp_dir().join(format!(
            "abs_path_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let first = root.join("first");
        let second = root.join("second");
        let empty = root.join("empty");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::create_dir_all(&empty).unwrap();
        fs::write(first.join("abs"), "old").unwrap();
        fs::write(second.join("abs"), "new").unwrap();
        let path = std::env::join_paths([&first, &empty, &second]).unwrap();
        let found = abs_binaries_from_path_env(&path);
        let _ = fs::remove_dir_all(&root);
        assert_eq!(found, vec![first.join("abs"), second.join("abs")]);
    }

    #[test]
    fn shadowing_abs_paths_skip_the_install_target_and_dedupe() {
        let root = std::env::temp_dir().join(format!(
            "abs_shadow_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let usr = root.join("usr");
        let local = root.join("local");
        fs::create_dir_all(&usr).unwrap();
        fs::create_dir_all(&local).unwrap();
        let installed = usr.join("abs");
        let leftover = local.join("abs");
        fs::write(&installed, "new").unwrap();
        fs::write(&leftover, "old").unwrap();
        let got = unique_shadowing_abs_paths(
            &installed,
            &[
                leftover.clone(),
                installed.clone(),
                leftover.clone(),
                local.join("missing"),
            ],
        );
        let _ = fs::remove_dir_all(&root);
        assert_eq!(got, vec![leftover]);
    }
}
