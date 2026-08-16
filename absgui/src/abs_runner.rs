use iced::futures::channel::mpsc;
use iced::futures::FutureExt;
use iced::futures::SinkExt;
use iced::futures::Stream;
use iced::futures::StreamExt;
use iced::stream;
use serde::Deserialize;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PgoStatus {
    #[allow(dead_code)]
    pub package: String,
    pub stage: String,
    pub stage_label: String,
    pub expected_kernel_uname: Option<String>,
    pub reboot_required: bool,
    #[serde(default)]
    pub boot_ready: bool,
    pub next_action: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PendingPkg {
    pub name: String,
    pub old: String,
    pub new: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SkippedPkg {
    pub name: String,
    pub old: String,
    pub new: String,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PendingUpdates {
    #[serde(default)]
    pub helper: String,
    #[serde(default)]
    pub repo: Vec<PendingPkg>,
    #[serde(default)]
    pub aur: Vec<PendingPkg>,
    #[serde(default)]
    pub manual: Vec<PendingPkg>,
    #[serde(default)]
    pub skipped: Vec<SkippedPkg>,
}

impl PendingUpdates {
    pub fn has_work(&self) -> bool {
        !self.repo.is_empty() || !self.aur.is_empty() || !self.manual.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct AbsRunOutput {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub event_log: Option<PathBuf>,
    pub user_aborted: bool,
}

#[derive(Debug)]
pub enum AbsPgoStreamItem {
    Lines(Vec<String>),
    Finished(Result<AbsRunOutput, String>),
}

#[derive(Debug, Clone, Copy)]
pub enum PgoAction {
    /// Fresh pipeline from stage 1 (`--pgo-restart`).
    Restart,
    /// Continue at a chosen stage (`--pgo-resume --pgo-stage …`).
    Resume,
    /// One-shot kernel build applying the user's kernel options, no PGO pipeline.
    KernelBuild,
}

enum PgoStreamEvent {
    Lines(Vec<String>),
    Done(Result<AbsRunOutput, String>),
}

/// Shared handle for the abs child spawned by a PGO run; used to stop compilation on Abort.
#[derive(Clone, Default)]
pub struct PgoRunHandle {
    pid: Arc<Mutex<Option<u32>>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    user_aborted: Arc<AtomicBool>,
}

impl PgoRunHandle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&self) {
        self.user_aborted.store(false, Ordering::SeqCst);
        *self.pid.lock().unwrap() = None;
        self.clear_stdin();
    }

    pub fn user_aborted(&self) -> bool {
        self.user_aborted.load(Ordering::SeqCst)
    }

    fn set_pid(&self, pid: u32) {
        *self.pid.lock().unwrap() = Some(pid);
    }

    fn clear_pid(&self) {
        *self.pid.lock().unwrap() = None;
    }

    fn set_stdin(&self, stdin: ChildStdin) {
        *self.stdin.lock().unwrap() = Some(stdin);
    }

    fn clear_stdin(&self) {
        *self.stdin.lock().unwrap() = None;
    }

    pub fn stdin_open(&self) -> bool {
        self.stdin.lock().unwrap().is_some()
    }

    /// Write bytes to the running abs stdin (PTY via `script`). Does not close the pipe.
    pub fn write_stdin(&self, data: &str) -> Result<(), String> {
        let mut guard = self.stdin.lock().unwrap();
        let Some(stdin) = guard.as_mut() else {
            return Err(abs_i18n::t("gui.log.stdin_idle").into());
        };
        stdin
            .write_all(data.as_bytes())
            .and_then(|_| stdin.flush())
            .map_err(|e| abs_i18n::tf("gui.log.stdin_error", &[("e", &e.to_string())]))
    }

    /// Send stop signals to the tracked in-app abs child and/or an external-terminal abs PID file.
    pub fn stop_running_build(&self, external_pid_file: Option<&Path>) {
        self.user_aborted.store(true, Ordering::SeqCst);
        self.clear_stdin();
        if let Some(pid) = *self.pid.lock().unwrap() {
            terminate_process_group(pid);
        }
        kill_pid_from_file(external_pid_file);
    }

    /// Stop builds then run `abs --pgo-abort` / `--ramdisk-shutdown` cleanup.
    pub fn abort(
        &self,
        package: &str,
        run_pgo_abort: bool,
        external_pid_file: Option<&Path>,
    ) -> Result<String, String> {
        self.stop_running_build(external_pid_file);
        let mut out = String::new();
        let mut errors = Vec::new();
        if run_pgo_abort {
            match run_abs_abort(package) {
                Ok(msg) => out.push_str(&msg),
                Err(e) => errors.push(e),
            }
        }
        match run_ramdisk_shutdown() {
            Ok(msg) => out.push_str(&msg),
            Err(e) => errors.push(e),
        }
        if errors.is_empty() {
            Ok(out)
        } else if out.trim().is_empty() {
            Err(errors.join("; "))
        } else {
            Ok(format!("{}\nWarning: {}", out.trim(), errors.join("; ")))
        }
    }
}

pub fn abs_binary() -> String {
    std::env::var("ABS_BINARY").unwrap_or_else(|_| "abs".into())
}

pub fn verify_abs_binary() -> Result<(), String> {
    let bin = abs_binary();
    let output = Command::new(&bin).arg("--version").output().map_err(|e| {
        abs_i18n::tf(
            "gui.msg.cannot_run_abs",
            &[("bin", &bin), ("e", &e.to_string())],
        )
    })?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(abs_i18n::tf(
            "gui.msg.abs_version_failed",
            &[("bin", &bin), ("err", err.as_ref())],
        ));
    }
    Ok(())
}

pub fn fetch_pgo_status(package: &str) -> Result<PgoStatus, String> {
    let output = Command::new(abs_binary())
        .args(["--pgo-status", package, "--json"])
        .output()
        .map_err(|e| format!("spawn abs: {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("abs --pgo-status failed: {err}"));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json_start = stdout
        .find('{')
        .ok_or_else(|| format!("parse status JSON: no JSON object in abs output: {stdout}"))?;
    serde_json::from_str(&stdout[json_start..]).map_err(|e| format!("parse status JSON: {e}"))
}

/// Run `abs --hold-check` (optional package filter) and return combined stdout/stderr text.
pub fn run_hold_check(packages: &[String]) -> Result<String, String> {
    let mut cmd = Command::new(abs_binary());
    cmd.arg("--hold-check").arg("--no-wait");
    for pkg in packages {
        if !pkg.trim().is_empty() {
            cmd.arg(pkg);
        }
    }
    let output = cmd
        .output()
        .map_err(|e| format!("spawn abs --hold-check: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut text = String::new();
    if !stdout.trim().is_empty() {
        text.push_str(stdout.trim());
    }
    if !stderr.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(stderr.trim());
    }
    if !output.status.success() && text.is_empty() {
        return Err(format!(
            "abs --hold-check failed with status {}",
            output.status
        ));
    }
    if text.is_empty() {
        Ok("(no held packages)".into())
    } else {
        Ok(text)
    }
}

const AUR_PKGBUILD_MAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct AurPkgbuild {
    pub name: String,
    pub version: String,
    pub text: String,
    /// Unified diff vs the last known PKGBUILD. `None` if there is no previous copy.
    pub delta: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AurRpcInfoResponse {
    results: Vec<AurRpcInfo>,
}

#[derive(Debug, Deserialize)]
struct AurRpcInfo {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Version")]
    version: String,
    #[serde(rename = "PackageBase")]
    package_base: String,
}

pub(crate) fn valid_aur_pkg_name(name: &str) -> bool {
    let b = name.as_bytes();
    if b.is_empty() || b.len() > 128 {
        return false;
    }
    if b.iter().all(|&c| c == b'.') {
        return false;
    }
    b.iter().all(|c| {
        matches!(
            c,
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'@' | b'.' | b'_' | b'+' | b'-'
        )
    })
}

fn aur_url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for byte in input.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn curl_get(url: &str) -> Result<(u16, String), String> {
    let output = Command::new("curl")
        .args([
            "-sS",
            "-L",
            "--compressed",
            "--max-time",
            "20",
            "-A",
            concat!("absgui/", env!("CARGO_PKG_VERSION")),
            "-w",
            "\n%{http_code}",
            "--",
            url,
        ])
        .output()
        .map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                abs_i18n::t("gui.pkgbuild.curl_missing").to_string()
            } else {
                format!("curl: {e}")
            }
        })?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        if err.trim().is_empty() {
            return Err(format!("curl exited {}", output.status));
        }
        return Err(err.trim().to_string());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let (body, status_s) = stdout
        .rsplit_once('\n')
        .ok_or_else(|| "curl: missing HTTP status".to_string())?;
    let status: u16 = status_s
        .trim()
        .parse()
        .map_err(|_| format!("curl: unexpected HTTP status {}", status_s.trim()))?;
    if body.len() > AUR_PKGBUILD_MAX_BYTES {
        return Err(abs_i18n::t("gui.pkgbuild.too_large").to_string());
    }
    Ok((status, body.to_string()))
}

/// Live PKGBUILD from aur.archlinux.org for the current AUR revision (not a local clone).
pub fn fetch_aur_pkgbuild(name: &str) -> Result<AurPkgbuild, String> {
    let name = name.trim();
    if !valid_aur_pkg_name(name) {
        return Err(abs_i18n::t("gui.pkgbuild.invalid_name").to_string());
    }
    let encoded = aur_url_encode(name);
    let rpc_url = format!("https://aur.archlinux.org/rpc/?v=5&type=info&arg[]={encoded}");
    let (status, body) = curl_get(&rpc_url)?;
    if status != 200 {
        return Err(abs_i18n::tf(
            "gui.pkgbuild.http_error",
            &[("code", &status.to_string()), ("name", name)],
        ));
    }
    let parsed: AurRpcInfoResponse = serde_json::from_str(&body).map_err(|e| {
        abs_i18n::tf(
            "gui.pkgbuild.parse_error",
            &[("name", name), ("e", &e.to_string())],
        )
    })?;
    let info = parsed
        .results
        .into_iter()
        .next()
        .ok_or_else(|| abs_i18n::tf("gui.pkgbuild.not_found", &[("name", name)]))?;
    if !valid_aur_pkg_name(&info.package_base) {
        return Err(abs_i18n::t("gui.pkgbuild.invalid_name").to_string());
    }
    let pkg_url = format!(
        "https://aur.archlinux.org/cgit/aur.git/plain/PKGBUILD?h={}",
        aur_url_encode(&info.package_base)
    );
    let (status, text) = curl_get(&pkg_url)?;
    if status == 404 {
        return Err(abs_i18n::tf("gui.pkgbuild.not_found", &[("name", name)]));
    }
    if status != 200 {
        return Err(abs_i18n::tf(
            "gui.pkgbuild.http_error",
            &[("code", &status.to_string()), ("name", name)],
        ));
    }
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return Err(abs_i18n::tf("gui.pkgbuild.empty", &[("name", name)]));
    }
    if trimmed.starts_with("<!DOCTYPE") || trimmed.starts_with("<html") {
        return Err(abs_i18n::tf("gui.pkgbuild.not_found", &[("name", name)]));
    }
    Ok(AurPkgbuild {
        name: info.name,
        version: info.version,
        text,
        delta: None,
    })
}

fn expand_packages_path(path: &str) -> PathBuf {
    let p = path.trim();
    if let Some(rest) = p.strip_prefix("$XDG_CACHE_HOME") {
        let cache = dirs::cache_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
        return cache.join(rest.trim_start_matches('/'));
    }
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(p)
}

fn preview_cache_path(name: &str) -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("abs")
        .join("pkgbuild-preview")
        .join(format!("{name}.pkgbuild"))
}

fn read_text_file_capped(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    if bytes.len() > AUR_PKGBUILD_MAX_BYTES {
        return None;
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn read_preview_cache(name: &str) -> Option<String> {
    read_text_file_capped(&preview_cache_path(name))
}

fn write_preview_cache(name: &str, text: &str) {
    let path = preview_cache_path(name);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, text);
}

/// Last PKGBUILD we can compare against: last preview cache, else a local clone.
pub fn previous_pkgbuild(name: &str, packages_path: &str) -> Option<String> {
    read_preview_cache(name).or_else(|| find_local_pkgbuild(packages_path, name))
}

fn find_local_pkgbuild(packages_path: &str, name: &str) -> Option<String> {
    if !valid_aur_pkg_name(name) {
        return None;
    }
    let root = expand_packages_path(packages_path);
    let mut candidates = vec![root.join("aur").join(name), root.join(name)];
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                candidates.push(path.join(name));
            }
        }
    }
    for dir in candidates {
        let backup = dir.join(".PKGBUILD.emerge_backup");
        if let Some(text) = read_text_file_capped(&backup) {
            return Some(text);
        }
        let live = dir.join("PKGBUILD");
        if let Some(text) = read_text_file_capped(&live) {
            return Some(text);
        }
    }
    None
}

/// Fetch the live AUR PKGBUILD and attach a delta vs the last known copy.
pub fn fetch_aur_pkgbuild_preview(name: &str, packages_path: &str) -> Result<AurPkgbuild, String> {
    let mut pkg = fetch_aur_pkgbuild(name)?;
    let previous = previous_pkgbuild(&pkg.name, packages_path)
        .or_else(|| previous_pkgbuild(name, packages_path));
    pkg.delta = previous
        .as_ref()
        .map(|old| crate::pkgbuild_diff::unified_diff(old, &pkg.text));
    write_preview_cache(&pkg.name, &pkg.text);
    if name != pkg.name {
        write_preview_cache(name, &pkg.text);
    }
    Ok(pkg)
}

/// Installed version from `pacman -Q`, or `None` if not installed.
pub fn pacman_query_version(pkg: &str) -> Option<String> {
    let output = Command::new("pacman").args(["-Q", pkg]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&output.stdout);
    let line = line.lines().next()?.trim();
    line.split_once(char::is_whitespace)
        .map(|(_, v)| v.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

pub fn format_abs_pgo_command(
    action: PgoAction,
    package: &str,
    event_log: Option<&Path>,
    pgo_stage: Option<&str>,
    pgo_once: bool,
    pgo_auto: bool,
) -> String {
    let mut parts = vec![shell_quote(&abs_binary())];
    match action {
        PgoAction::Restart => {
            parts.push("--pgo-restart".into());
            parts.push(shell_quote(package));
        }
        PgoAction::Resume => {
            parts.push("--pgo-resume".into());
            parts.push(shell_quote(package));
        }
        PgoAction::KernelBuild => {
            parts.push("--kernel-build".into());
            parts.push(shell_quote(package));
        }
    }
    if let Some(stage) = pgo_stage {
        parts.push("--pgo-stage".into());
        parts.push(shell_quote(stage));
    }
    if pgo_once {
        parts.push("--pgo-once".into());
    }
    if pgo_auto {
        parts.push("--pgo-auto".into());
    }
    if let Some(path) = event_log {
        parts.push("--event-log".into());
        parts.push(shell_quote(&path.display().to_string()));
    }
    parts.join(" ")
}

/// Shell-quoted `abs -RU` (refresh watched packages, compile what qualifies, then system update).
pub fn format_abs_system_update_command() -> String {
    format!("{} -RU", shell_quote(&abs_binary()))
}

pub fn format_install_repo_updates(names: &[String]) -> String {
    let mut parts = vec![
        shell_quote(&abs_binary()),
        "--install-repo-updates".into(),
        "--no-wait".into(),
    ];
    for name in names {
        parts.push(shell_quote(name));
    }
    parts.join(" ")
}

pub fn format_install_aur(package: &str) -> String {
    format!(
        "{} --install-aur {} --no-wait",
        shell_quote(&abs_binary()),
        shell_quote(package)
    )
}

pub fn require_gui_askpass() -> Result<(), String> {
    ensure_askpass_helper()
        .map(|_| ())
        .ok_or_else(|| abs_i18n::t("gui.msg.require_askpass").into())
}

pub fn fetch_pending_updates() -> Result<PendingUpdates, String> {
    let mut cmd = Command::new(abs_binary());
    cmd.args(["--pending-updates", "--json", "--no-wait"]);
    apply_gui_sudo_env(&mut cmd);
    cmd.stdin(Stdio::null());
    let output = cmd
        .output()
        .map_err(|e| format!("spawn abs --pending-updates: {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let out = String::from_utf8_lossy(&output.stdout);
        let msg = if !err.trim().is_empty() {
            err.into_owned()
        } else {
            out.into_owned()
        };
        return Err(format!("abs --pending-updates failed: {msg}"));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json_start = stdout.find('{').ok_or_else(|| {
        format!("parse pending-updates JSON: no JSON object in abs output: {stdout}")
    })?;
    serde_json::from_str(&stdout[json_start..])
        .map_err(|e| format!("parse pending-updates JSON: {e}"))
}

#[derive(Debug, Clone, Deserialize)]
pub struct WizardForm {
    pub path: String,
    pub reconfigure: bool,
    pub steps: Vec<WizardStep>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WizardStep {
    #[allow(dead_code)]
    pub id: String,
    pub title: String,
    pub blurb: String,
    pub fields: Vec<WizardField>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WizardField {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub explanation: String,
    pub current: serde_json::Value,
    pub suggested: serde_json::Value,
    pub prefer_current: bool,
    #[allow(dead_code)]
    pub optional: bool,
    #[serde(default)]
    pub options: Option<Vec<WizardChoice>>,
    #[serde(default)]
    pub visible_if: Option<serde_json::Value>,
    #[allow(dead_code)]
    #[serde(default)]
    pub usize_min: Option<usize>,
    #[serde(default)]
    pub path_pick: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WizardChoice {
    pub value: String,
    pub label: String,
    #[serde(default)]
    pub help: String,
    #[allow(dead_code)]
    pub suggested: bool,
}

#[derive(Debug, Deserialize)]
struct WizardJsonStatus {
    #[serde(default)]
    ok: Option<bool>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

fn parse_json_object(stdout: &str, stderr: &str, what: &str) -> Result<serde_json::Value, String> {
    let blob = if stdout.contains('{') { stdout } else { stderr };
    let json_start = blob.find('{').ok_or_else(|| {
        let extra = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        format!("parse {what} JSON: no JSON object in abs output: {extra}")
    })?;
    serde_json::from_str(&blob[json_start..]).map_err(|e| format!("parse {what} JSON: {e}"))
}

fn run_wizard_json(
    flag: &str,
    stdin_json: Option<&serde_json::Value>,
) -> Result<(bool, serde_json::Value, String), String> {
    let mut cmd = Command::new(abs_binary());
    cmd.args([flag, "--no-wait"]);
    if stdin_json.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("spawn abs {flag}: {e}"))?;
    if let Some(body) = stdin_json {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("abs {flag}: stdin not piped"))?;
        stdin
            .write_all(body.to_string().as_bytes())
            .map_err(|e| format!("abs {flag}: write stdin: {e}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("abs {flag}: wait: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let value = parse_json_object(&stdout, &stderr, flag)?;
    Ok((output.status.success(), value, stderr.into_owned()))
}

pub fn fetch_wizard_form() -> Result<WizardForm, String> {
    let (ok, value, stderr) = run_wizard_json("--config-wizard-form", None)?;
    if !ok {
        if let Some(err) = value.get("error").and_then(|v| v.as_str()) {
            return Err(err.to_string());
        }
        return Err(if stderr.trim().is_empty() {
            "abs --config-wizard-form failed".into()
        } else {
            stderr
        });
    }
    serde_json::from_value(value).map_err(|e| format!("parse wizard form: {e}"))
}

pub fn wizard_check(
    id: &str,
    value: &serde_json::Value,
    answers: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    let body = serde_json::json!({
        "id": id,
        "value": value,
        "answers": answers,
    });
    let (ok, parsed, stderr) = run_wizard_json("--config-wizard-check", Some(&body))?;
    let status: WizardJsonStatus =
        serde_json::from_value(parsed).map_err(|e| format!("parse wizard check: {e}"))?;
    if ok && status.ok.unwrap_or(true) {
        Ok(())
    } else {
        Err(status.error.filter(|s| !s.is_empty()).unwrap_or_else(|| {
            if stderr.trim().is_empty() {
                "invalid value".into()
            } else {
                stderr
            }
        }))
    }
}

pub fn wizard_apply(
    answers: &serde_json::Map<String, serde_json::Value>,
) -> Result<String, String> {
    let body = serde_json::json!({ "answers": answers });
    let (ok, parsed, stderr) = run_wizard_json("--config-wizard-apply", Some(&body))?;
    let status: WizardJsonStatus =
        serde_json::from_value(parsed).map_err(|e| format!("parse wizard apply: {e}"))?;
    if ok && status.ok.unwrap_or(true) {
        Ok(status.path.unwrap_or_default())
    } else {
        Err(status.error.filter(|s| !s.is_empty()).unwrap_or_else(|| {
            if stderr.trim().is_empty() {
                "abs --config-wizard-apply failed".into()
            } else {
                stderr
            }
        }))
    }
}

/// Launch `command` in a new terminal-emulator window so the build gets a real TTY: sudo can prompt
/// for a password, all output is visible, and the process is fully interactive. Returns the name of
/// the terminal program used. The window stays open after the command finishes so output and errors
/// remain readable.
///
/// When `pid_file` is set, the abs child PID is written there so [`PgoRunHandle::abort`] can stop
/// builds started in the external terminal.
pub fn launch_in_terminal(command: &str, pid_file: Option<&Path>) -> Result<String, String> {
    let script = if let Some(path) = pid_file {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let pid_q = shell_quote(&path.display().to_string());
        // Foreground in a real terminal: drop GUI askpass vars so `sudo` uses the TTY.
        format!(
            "unset SUDO_ASKPASS ABS_GUI 2>/dev/null; export ABS_NO_EXIT_PAUSE=1; \
             printf '%s\\n' \"$$\" > {pid_q}; trap 'rm -f {pid_q}' EXIT INT TERM; \
             {command}; status=$?; \
             echo; printf '[abs finished with exit %s — press Enter to close this window]\\n' \"$status\"; read -r _"
        )
    } else {
        format!(
            "export ABS_NO_EXIT_PAUSE=1; {command}; status=$?; echo; printf '[abs finished with exit %s — press Enter to close this window]\\n' \"$status\"; read -r _",
        )
    };

    // (binary, args that precede the `bash -lc <script>` we append).
    let candidates = terminal_candidates();

    let has_setsid = command_exists("setsid");
    let mut tried = Vec::new();
    for (bin, before) in candidates {
        if !command_exists(&bin) {
            continue;
        }
        tried.push(bin.clone());
        let mut argv: Vec<String> = Vec::new();
        if has_setsid {
            // Fully detach so the terminal outlives absgui and never lingers as a zombie.
            argv.push("setsid".into());
            argv.push("-f".into());
        }
        argv.push(bin.clone());
        argv.extend(before);
        argv.push("bash".into());
        argv.push("-lc".into());
        argv.push(script.clone());

        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match cmd.spawn() {
            Ok(mut child) => {
                if has_setsid {
                    // `setsid -f` exits immediately; reap it so it isn't left as a zombie.
                    let _ = child.wait();
                } else {
                    // Reap the terminal in the background so it doesn't become a zombie.
                    thread::spawn(move || {
                        let _ = child.wait();
                    });
                }
                return Ok(bin);
            }
            Err(_) => continue,
        }
    }
    if tried.is_empty() {
        Err(abs_i18n::t("gui.msg.no_terminal").into())
    } else {
        Err(abs_i18n::tf(
            "gui.msg.terminal_launch_failed",
            &[("tried", &tried.join(", "))],
        ))
    }
}

fn command_exists(name: &str) -> bool {
    let path = Path::new(name);
    if path.components().count() > 1 {
        return path.is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
}

/// Ordered terminal launch candidates: ABSGUI_TERMINAL, xdg-terminal-exec, COSMIC’s cosmic-term,
/// then the known-emulator list (which also includes cosmic-term).
fn terminal_candidates() -> Vec<(String, Vec<String>)> {
    terminal_candidates_from(
        std::env::var("ABSGUI_TERMINAL").ok(),
        desktop_env_is_cosmic(
            &std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
            &std::env::var("XDG_SESSION_DESKTOP").unwrap_or_default(),
            &std::env::var("DESKTOP_SESSION").unwrap_or_default(),
        ),
    )
}

fn terminal_candidates_from(
    absgui_terminal: Option<String>,
    cosmic: bool,
) -> Vec<(String, Vec<String>)> {
    let mut candidates: Vec<(String, Vec<String>)> = Vec::new();
    let mut push = |bin: &str, before: &[&str]| {
        if candidates.iter().any(|(b, _)| b == bin) {
            return;
        }
        candidates.push((
            bin.to_string(),
            before.iter().map(|s| (*s).to_string()).collect(),
        ));
    };

    if let Some(t) = absgui_terminal {
        let t = t.trim();
        if !t.is_empty() {
            let args = terminal_exec_args(t.rsplit('/').next().unwrap_or(t));
            push(t, args);
        }
    }
    push("xdg-terminal-exec", terminal_exec_args("xdg-terminal-exec"));
    if cosmic {
        push("cosmic-term", terminal_exec_args("cosmic-term"));
    }
    for (bin, before) in KNOWN_TERMINALS {
        push(bin, before);
    }
    candidates
}

const KNOWN_TERMINALS: &[(&str, &[&str])] = &[
    ("kitty", &[]),
    ("alacritty", &["-e"]),
    ("wezterm", &["start", "--"]),
    ("foot", &[]),
    ("ghostty", &["-e"]),
    ("cosmic-term", &["-e"]),
    ("konsole", &["-e"]),
    ("gnome-terminal", &["--"]),
    ("tilix", &["-e"]),
    ("xfce4-terminal", &["-x"]),
    ("mate-terminal", &["-x"]),
    ("lxterminal", &["-e"]),
    ("st", &["-e"]),
    ("urxvt", &["-e"]),
    ("xterm", &["-e"]),
    ("x-terminal-emulator", &["-e"]),
];

fn terminal_exec_args(bin: &str) -> &'static [&'static str] {
    KNOWN_TERMINALS
        .iter()
        .find(|(name, _)| *name == bin)
        .map(|(_, args)| *args)
        .unwrap_or_else(|| match bin {
            "xdg-terminal-exec" => &["--"],
            _ => &["-e"],
        })
}

/// True when a desktop-identity string contains a `cosmic` token (colon/semicolon-separated).
fn desktop_env_is_cosmic(current: &str, session: &str, desktop_session: &str) -> bool {
    [current, session, desktop_session]
        .into_iter()
        .flat_map(|s| s.split(|c: char| !c.is_ascii_alphanumeric()))
        .any(|t| t.eq_ignore_ascii_case("cosmic"))
}

fn trusted_bin(name: &str) -> Option<String> {
    let mut candidates = Vec::new();
    for dir in ["/usr/bin", "/usr/sbin"] {
        candidates.push(PathBuf::from(dir).join(name));
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            candidates.push(dir.join(name));
        }
    }
    candidates.into_iter().find_map(|p| {
        if p.is_file() && path_is_trusted_executable(&p) {
            Some(p.to_string_lossy().into_owned())
        } else {
            None
        }
    })
}

fn path_is_trusted_executable(p: &Path) -> bool {
    let Ok(real) = std::fs::canonicalize(p) else {
        return false;
    };
    let s = real.to_string_lossy();
    s.starts_with("/usr/bin/")
        || s.starts_with("/usr/sbin/")
        || s.starts_with("/usr/lib/")
        || s.starts_with("/usr/libexec/")
}

/// Find a graphical askpass program so sudo can prompt for a password from the GUI (there is no
/// interactive terminal). Honors an existing `SUDO_ASKPASS`, then known helpers, then generates a
/// zenity/kdialog/yad/qarma/pinentry wrapper. Returns the askpass path, or `None` if nothing is available.
fn ensure_askpass_helper() -> Option<String> {
    if let Some(v) = std::env::var_os("SUDO_ASKPASS") {
        if !v.is_empty() {
            return Some(v.to_string_lossy().into_owned());
        }
    }

    const CANDIDATES: &[&str] = &[
        "/usr/bin/ksshaskpass",
        "/usr/bin/lxqt-openssh-askpass",
        "/usr/bin/x11-ssh-askpass",
        "/usr/lib/seahorse/ssh-askpass",
        "/usr/libexec/openssh/gnome-ssh-askpass",
        "/usr/lib/ssh/x11-ssh-askpass",
        "/usr/bin/ssh-askpass",
    ];
    for candidate in CANDIDATES {
        if Path::new(candidate).exists() {
            return Some((*candidate).to_string());
        }
    }

    // Generate a wrapper around zenity/kdialog/yad/qarma/pinentry as a last resort.
    // Embed absolute allowlisted paths so a later PATH prepend cannot hijack the password prompt.
    // Password on stdout; GTK/Qt warnings on stderr would otherwise land in the abs log.
    let tool = if let Some(bin) = trusted_bin("zenity") {
        format!(
            "exec {} --password --title='absgui: sudo password' 2>/dev/null",
            shell_quote(&bin)
        )
    } else if let Some(bin) = trusted_bin("kdialog") {
        format!(
            "exec {} --password 'absgui needs your sudo password to stop builds and unmount the ramdisk' 2>/dev/null",
            shell_quote(&bin)
        )
    } else if let Some(bin) = trusted_bin("yad") {
        format!(
            "exec {} --entry --hide-text --title='absgui: sudo password' --text='Password:' 2>/dev/null",
            shell_quote(&bin)
        )
    } else if let Some(bin) = trusted_bin("qarma") {
        format!(
            "exec {} --password --title='absgui: sudo password' 2>/dev/null",
            shell_quote(&bin)
        )
    } else {
        let pinentry = first_pinentry()?;
        format!(
            "printf '%s\\n' 'SETTITLE absgui' 'SETDESC absgui needs your sudo password' \
             'SETPROMPT Password:' 'GETPIN' 'BYE' | {} 2>/dev/null | sed -n 's/^D //p'",
            shell_quote(&pinentry)
        )
    };

    let dir = dirs::cache_dir()?.join("abs");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("askpass.sh");
    std::fs::write(&path, format!("#!/bin/sh\n{tool}\n")).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).ok()?;
    }
    Some(path.to_string_lossy().into_owned())
}

fn first_pinentry() -> Option<String> {
    [
        "pinentry-qt",
        "pinentry-gnome3",
        "pinentry-gtk-2",
        "pinentry",
    ]
    .into_iter()
    .find_map(trusted_bin)
}

/// Mark spawned `abs` as GUI-driven and wire up a graphical askpass when one is available.
/// Callers set stdin themselves: piped for live runs, null for one-shot JSON/abort.
fn apply_gui_sudo_env(cmd: &mut Command) {
    cmd.env("ABS_GUI", "1");
    if let Some(askpass) = ensure_askpass_helper() {
        cmd.env("SUDO_ASKPASS", askpass);
    }
}

/// Spawn abs for PGO with a pseudo-TTY or line-buffer wrapper when possible so makepkg output
/// streams to absgui instead of block-buffering for minutes.
/// Returns `(Command, optional wrapper line if different from inner)`.
fn spawn_pgo_command(inner: &str) -> (Command, Option<String>) {
    let (program, args, wrapper_display): (String, Vec<String>, String) =
        if command_exists("script") {
            // `-f` flushes script's pipe output after each write; without it the GUI sees silence
            // for minutes while kernel builds stream to the pseudo-TTY.
            (
                "script".into(),
                vec![
                    "-q".into(),
                    "-f".into(),
                    "-c".into(),
                    inner.to_string(),
                    "/dev/null".into(),
                ],
                format!("$ script -q -f -c {} /dev/null", shell_quote(inner)),
            )
        } else if command_exists("stdbuf") {
            let wrapped = format!("stdbuf -oL -eL {inner}");
            (
                "sh".into(),
                vec!["-c".into(), wrapped.clone()],
                format!("$ {wrapped}"),
            )
        } else {
            return (
                {
                    let mut cmd = Command::new("sh");
                    cmd.args(["-c", inner]);
                    cmd
                },
                None,
            );
        };

    let mut cmd = Command::new(program);
    cmd.args(args);
    let wrapper = if wrapper_display == format!("$ {inner}") {
        None
    } else {
        Some(wrapper_display)
    };
    (cmd, wrapper)
}

#[cfg(unix)]
fn list_process_tree(root: u32) -> Vec<u32> {
    use std::collections::HashSet;
    let mut order = Vec::new();
    let mut stack = vec![root];
    let mut seen = HashSet::new();
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        order.push(pid);
        let children_path = format!("/proc/{pid}/task/{pid}/children");
        if let Ok(data) = std::fs::read_to_string(&children_path) {
            for token in data.split_whitespace() {
                if let Ok(child) = token.parse::<u32>() {
                    stack.push(child);
                }
            }
        }
    }
    order
}

#[cfg(unix)]
fn terminate_process_group(pid: u32) {
    let pgid = format!("-{pid}");
    let _ = Command::new("kill").args(["-TERM", &pgid]).status();
    for child in list_process_tree(pid) {
        if child != pid {
            let _ = Command::new("kill")
                .args(["-TERM", &child.to_string()])
                .status();
        }
    }
    let _ = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status();
    thread::sleep(Duration::from_millis(2000));
    let _ = Command::new("kill").args(["-KILL", &pgid]).status();
    for child in list_process_tree(pid) {
        let _ = Command::new("kill")
            .args(["-KILL", &child.to_string()])
            .status();
    }
}

#[cfg(not(unix))]
fn terminate_process_group(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status();
}

const MAX_LOG_LINE_CHARS: usize = 4000;

/// Strip CSI/OSC color codes. Used for copy/save and selection metrics.
pub(crate) fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('[') => {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() || next == '~' {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                for next in chars.by_ref() {
                    if next == '\u{7}' || next == '\\' {
                        break;
                    }
                }
            }
            Some('(') | Some(')') => {
                chars.next();
                let _ = chars.next();
            }
            Some(_) => {
                let _ = chars.next();
            }
            None => break,
        }
    }
    out
}

/// Make a child-process line safe for the in-app log: keep SGR color codes, keep the last CR
/// progress frame, strip other control chars, and cap length so a huge blob cannot freeze iced.
pub fn sanitize_log_line(raw: &str) -> Option<String> {
    let last_cr = raw.rsplit('\r').next().unwrap_or(raw);
    let mut out = String::with_capacity(last_cr.len());
    let mut chars = last_cr.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.peek().copied() {
                Some('[') => {
                    out.push(c);
                    out.push(chars.next().unwrap());
                    for next in chars.by_ref() {
                        out.push(next);
                        if next.is_ascii_alphabetic() || next == '~' {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if next == '\u{7}' || next == '\\' {
                            break;
                        }
                    }
                }
                Some('(') | Some(')') => {
                    chars.next();
                    let _ = chars.next();
                }
                Some(_) => {
                    let _ = chars.next();
                }
                None => break,
            }
            continue;
        }
        if c == '\t' || !c.is_control() {
            out.push(c);
        }
    }
    let visible = strip_ansi(&out);
    if visible.trim().is_empty() || is_gui_toolkit_noise(&visible) {
        return None;
    }
    let count = visible.chars().count();
    if count > MAX_LOG_LINE_CHARS {
        let truncated: String = visible.chars().take(MAX_LOG_LINE_CHARS).collect();
        Some(format!("{truncated}… [truncated]"))
    } else {
        Some(out)
    }
}

/// GLib/GTK lines from zenity/yad askpass, e.g.
/// `(zenity:523014): Adwaita-WARNING **: 07:21:46.377: Using GtkSettings:...`
fn is_gui_toolkit_noise(visible: &str) -> bool {
    let t = visible.trim();
    let t = t.strip_prefix("[stderr] ").unwrap_or(t);
    let Some(rest) = t.strip_prefix('(') else {
        return false;
    };
    let Some((proc, msg)) = rest.split_once("): ") else {
        return false;
    };
    let Some(pid) = proc.rsplit(':').next() else {
        return false;
    };
    if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    msg.contains("-WARNING **:") || msg.contains("-CRITICAL **:") || msg.contains("-INFO **:")
}

const LOG_BATCH_INTERVAL: Duration = Duration::from_millis(80);
const LOG_BATCH_MAX: usize = 512;

fn send_log_batch(tx: &mut mpsc::Sender<PgoStreamEvent>, batch: &mut Vec<String>) {
    if batch.is_empty() {
        return;
    }
    let lines = std::mem::take(batch);
    iced::futures::executor::block_on(async {
        let _ = tx.send(PgoStreamEvent::Lines(lines)).await;
    });
}

/// Coalesce pipe lines so iced sees at most ~12 log messages per second.
fn spawn_log_batcher(
    tx: mpsc::Sender<PgoStreamEvent>,
) -> (std::sync::mpsc::SyncSender<String>, thread::JoinHandle<()>) {
    let (line_tx, line_rx) = std::sync::mpsc::sync_channel::<String>(4096);
    let handle = thread::spawn(move || {
        let mut batch = Vec::new();
        let mut tx = tx;
        loop {
            if batch.is_empty() {
                match line_rx.recv() {
                    Ok(line) => batch.push(line),
                    Err(_) => break,
                }
            }
            let deadline = std::time::Instant::now() + LOG_BATCH_INTERVAL;
            while batch.len() < LOG_BATCH_MAX {
                let wait = deadline.saturating_duration_since(std::time::Instant::now());
                if wait.is_zero() {
                    break;
                }
                match line_rx.recv_timeout(wait) {
                    Ok(line) => batch.push(line),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        send_log_batch(&mut tx, &mut batch);
                        return;
                    }
                }
            }
            send_log_batch(&mut tx, &mut batch);
        }
    });
    (line_tx, handle)
}

/// Read a child stream, emitting complete lines and flushing prompts that have no trailing newline.
fn stream_pipe_lines<R: Read + AsRawFd + Send>(
    mut reader: R,
    stderr_tag: Option<&'static str>,
    tx: std::sync::mpsc::SyncSender<String>,
) {
    let fd = reader.as_raw_fd();
    set_nonblocking(fd);
    let mut acc = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        let timeout_ms = if acc.is_empty() { -1 } else { 80 };
        if !poll_fd_readable(fd, timeout_ms) {
            if !emit_partial(&mut acc, stderr_tag, &tx) {
                break;
            }
            continue;
        }
        match reader.read(&mut tmp) {
            Ok(0) => {
                let _ = emit_partial(&mut acc, stderr_tag, &tx);
                break;
            }
            Ok(n) => {
                acc.extend_from_slice(&tmp[..n]);
                for line in drain_complete_lines(&mut acc) {
                    let Some(line) = sanitize_log_line(&line) else {
                        continue;
                    };
                    if tx.send(tagged_log_line(line, stderr_tag)).is_err() {
                        return;
                    }
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::Interrupted => {
                continue;
            }
            Err(_) => {
                let _ = emit_partial(&mut acc, stderr_tag, &tx);
                break;
            }
        }
    }
}

fn tagged_log_line(line: String, stderr_tag: Option<&str>) -> String {
    match stderr_tag {
        Some(tag) => format!("[{tag}] {line}"),
        None => line,
    }
}

fn emit_partial(
    acc: &mut Vec<u8>,
    stderr_tag: Option<&str>,
    tx: &std::sync::mpsc::SyncSender<String>,
) -> bool {
    if acc.is_empty() {
        return true;
    }
    let line = String::from_utf8_lossy(acc).into_owned();
    acc.clear();
    let Some(line) = sanitize_log_line(&line) else {
        return true;
    };
    tx.send(tagged_log_line(line, stderr_tag)).is_ok()
}

fn drain_complete_lines(acc: &mut Vec<u8>) -> Vec<String> {
    let mut lines = Vec::new();
    while let Some(pos) = acc.iter().position(|&b| b == b'\n') {
        let mut raw: Vec<u8> = acc.drain(..=pos).collect();
        if raw.last() == Some(&b'\n') {
            raw.pop();
        }
        if raw.last() == Some(&b'\r') {
            raw.pop();
        }
        lines.push(String::from_utf8_lossy(&raw).into_owned());
    }
    lines
}

fn set_nonblocking(fd: i32) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL, 0);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
}

fn poll_fd_readable(fd: i32, timeout_ms: i32) -> bool {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        let rc = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if rc < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == ErrorKind::Interrupted {
                continue;
            }
            return false;
        }
        return rc > 0;
    }
}

fn run_abs_command_streaming(
    inner: String,
    event_log: Option<PathBuf>,
    handle: PgoRunHandle,
    tx: mpsc::Sender<PgoStreamEvent>,
) {
    let send = |event: PgoStreamEvent| {
        let mut sender = tx.clone();
        iced::futures::executor::block_on(async move {
            let _ = sender.send(event).await;
        });
    };

    let (mut cmd, _wrapper_line) = spawn_pgo_command(&inner);

    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_gui_sudo_env(&mut cmd);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        // Detach from the shell's controlling TTY so nested bare `sudo` cannot steal it.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            send(PgoStreamEvent::Done(Err(abs_i18n::tf(
                "gui.msg.abs_start_failed",
                &[("e", &e.to_string())],
            ))));
            return;
        }
    };

    handle.set_pid(child.id());
    if let Some(stdin) = child.stdin.take() {
        handle.set_stdin(stdin);
    }

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let (line_tx, batcher) = spawn_log_batcher(tx.clone());
    let stdout_handle = stdout.map(|out| {
        let tx_out = line_tx.clone();
        thread::spawn(move || stream_pipe_lines(out, None, tx_out))
    });
    let stderr_handle = stderr.map(|err| {
        let tx_err = line_tx.clone();
        thread::spawn(move || stream_pipe_lines(err, Some("stderr"), tx_err))
    });
    drop(line_tx);

    if let Some(h) = stdout_handle {
        let _ = h.join();
    }
    if let Some(h) = stderr_handle {
        let _ = h.join();
    }
    let _ = batcher.join();

    let user_aborted = handle.user_aborted();
    handle.clear_pid();
    handle.clear_stdin();

    let status = match child.wait() {
        Ok(s) => s,
        Err(e) => {
            send(PgoStreamEvent::Done(Err(abs_i18n::tf(
                "gui.msg.abs_wait_failed",
                &[("e", &e.to_string())],
            ))));
            return;
        }
    };

    send(PgoStreamEvent::Done(Ok(AbsRunOutput {
        success: status.success(),
        exit_code: status.code(),
        event_log,
        user_aborted,
    })));
}

pub fn stream_abs_command(
    inner: String,
    handle: PgoRunHandle,
    event_log: Option<PathBuf>,
) -> impl Stream<Item = AbsPgoStreamItem> {
    stream::channel(128, async move |mut output| {
        let (tx, rx) = mpsc::channel(2048);
        thread::spawn(move || {
            run_abs_command_streaming(inner, event_log, handle, tx);
        });

        let mut rx = rx;
        while let Some(event) = rx.next().await {
            match event {
                PgoStreamEvent::Lines(mut batch) => {
                    let mut done = None;
                    while batch.len() < LOG_BATCH_MAX {
                        match rx.next().now_or_never() {
                            Some(Some(PgoStreamEvent::Lines(more))) => batch.extend(more),
                            Some(Some(PgoStreamEvent::Done(result))) => {
                                done = Some(result);
                                break;
                            }
                            Some(None) | None => break,
                        }
                    }
                    if output.send(AbsPgoStreamItem::Lines(batch)).await.is_err() {
                        return;
                    }
                    if let Some(result) = done {
                        let _ = output.send(AbsPgoStreamItem::Finished(result)).await;
                        return;
                    }
                }
                PgoStreamEvent::Done(result) => {
                    let _ = output.send(AbsPgoStreamItem::Finished(result)).await;
                    return;
                }
            }
        }
    })
}

pub fn stream_abs_pgo(
    action: PgoAction,
    package: String,
    event_log: Option<PathBuf>,
    pgo_stage: Option<String>,
    pgo_once: bool,
    pgo_auto: bool,
    handle: PgoRunHandle,
) -> impl Stream<Item = AbsPgoStreamItem> {
    let inner = format_abs_pgo_command(
        action,
        &package,
        event_log.as_deref(),
        pgo_stage.as_deref(),
        pgo_once,
        pgo_auto,
    );
    stream_abs_command(inner, handle, event_log)
}

pub fn run_abs_abort(package: &str) -> Result<String, String> {
    let mut cmd = Command::new(abs_binary());
    cmd.args(["--pgo-abort", package, "--pgo-keep-stage"]);
    apply_gui_sudo_env(&mut cmd);
    cmd.stdin(Stdio::null());
    let output = cmd.output().map_err(|e| format!("spawn: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

pub fn run_ramdisk_shutdown() -> Result<String, String> {
    let mut cmd = Command::new(abs_binary());
    cmd.arg("--ramdisk-shutdown");
    apply_gui_sudo_env(&mut cmd);
    cmd.stdin(Stdio::null());
    let output = cmd
        .output()
        .map_err(|e| format!("spawn abs --ramdisk-shutdown: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

pub fn default_event_log_path(package: &str) -> PathBuf {
    dirs::state_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("abs")
        .join("pgo")
        .join(format!("{package}.events.jsonl"))
}

/// PID file written by [`launch_in_terminal`] for external builds (sibling of the event log).
pub fn external_run_pid_path(package: &str) -> PathBuf {
    default_event_log_path(package)
        .parent()
        .map(|dir| dir.join(format!("{package}.term.pid")))
        .unwrap_or_else(|| PathBuf::from(format!("/tmp/{package}.term.pid")))
}

/// True when `path` holds a PID that still exists in `/proc` (or cannot be ruled out on non-Unix).
pub fn pid_file_process_alive(path: Option<&Path>) -> bool {
    let Some(path) = path else {
        return false;
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(pid) = raw.trim().parse::<u32>() else {
        return false;
    };
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

fn kill_pid_from_file(path: Option<&Path>) {
    let Some(path) = path else {
        return;
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(pid) = raw.trim().parse::<u32>() else {
        return;
    };
    terminate_process_group(pid);
    let _ = std::fs::remove_file(path);
}

pub fn ensure_event_log_path(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create event log directory {}: {e}", parent.display()))?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("create event log file {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::shell_quote;

    #[test]
    fn shell_quote_always_quotes_shell_metacharacters() {
        assert_eq!(
            shell_quote("kernel; touch /tmp/pwned"),
            "'kernel; touch /tmp/pwned'"
        );
        assert_eq!(shell_quote("a'b"), "'a'\"'\"'b'");
    }

    #[test]
    fn aur_pkg_name_rejects_injection() {
        assert!(super::valid_aur_pkg_name("linux-cachyos"));
        assert!(super::valid_aur_pkg_name("libstdc++"));
        assert!(super::valid_aur_pkg_name("foo@bar"));
        assert!(!super::valid_aur_pkg_name(""));
        assert!(!super::valid_aur_pkg_name(".."));
        assert!(!super::valid_aur_pkg_name("foo/bar"));
        assert!(!super::valid_aur_pkg_name("foo;bar"));
        assert!(!super::valid_aur_pkg_name("foo bar"));
        assert!(!super::valid_aur_pkg_name("h=evil"));
    }

    #[test]
    fn system_update_command_is_abs_ru() {
        let cmd = super::format_abs_system_update_command();
        assert!(cmd.contains("-RU"), "{cmd}");
    }

    #[test]
    fn install_repo_command_passes_package_names() {
        let cmd = super::format_install_repo_updates(&["firefox".into(), "linux".into()]);
        assert!(cmd.contains("--install-repo-updates"), "{cmd}");
        assert!(cmd.contains("firefox"), "{cmd}");
        assert!(cmd.contains("linux"), "{cmd}");
    }

    #[test]
    fn install_aur_command_uses_flag() {
        let cmd = super::format_install_aur("yay-bin");
        assert!(cmd.contains("--install-aur"), "{cmd}");
        assert!(cmd.contains("yay-bin"), "{cmd}");
    }

    #[test]
    fn sanitize_strips_ansi_and_cr_progress() {
        let line = "\u{1b}[34m==>\u{1b}[0m curl: \u{1b}[1;32mUp-to-date\u{1b}[0m";
        let kept = super::sanitize_log_line(line).expect("line");
        assert_eq!(super::strip_ansi(&kept), "==> curl: Up-to-date");
        assert!(kept.contains('\u{1b}'), "{kept}");
        assert_eq!(
            super::sanitize_log_line("Receiving objects: 1%\rReceiving objects: 100%").as_deref(),
            Some("Receiving objects: 100%")
        );
        assert_eq!(
            super::sanitize_log_line("hello\u{0}world").as_deref(),
            Some("helloworld")
        );
        assert!(
            super::sanitize_log_line(
                "(zenity:523014): Adwaita-WARNING **: 07:21:46.377: Using GtkSettings:gtk-application-prefer-dark-theme with libadwaita is unsupported. Please use AdwStyleManager:color-scheme instead."
            )
            .is_none()
        );
        assert!(super::sanitize_log_line(
            "[stderr] (yad:1): Gtk-WARNING **: 00:00:00.000: cannot open display"
        )
        .is_none());
        assert_eq!(
            super::sanitize_log_line("==> WARNING: Configured vmlinux lacks debug info").as_deref(),
            Some("==> WARNING: Configured vmlinux lacks debug info")
        );
    }

    #[test]
    fn sanitize_truncates_huge_json_blob() {
        let blob = format!(
            "{{\"url\":\"https://api.github.com/{}\"}}",
            "x".repeat(8000)
        );
        let out = super::sanitize_log_line(&blob).expect("line");
        assert!(out.ends_with("… [truncated]"), "{out}");
        assert!(out.chars().count() < blob.chars().count());
    }

    #[test]
    fn drain_complete_lines_keeps_partial_prompt() {
        let mut acc = b"Install it? [Y/n] ".to_vec();
        assert!(super::drain_complete_lines(&mut acc).is_empty());
        assert_eq!(acc, b"Install it? [Y/n] ");
        acc.extend_from_slice(b"n\nnext\npartial");
        let lines = super::drain_complete_lines(&mut acc);
        assert_eq!(lines, vec!["Install it? [Y/n] n", "next"]);
        assert_eq!(acc, b"partial");
    }

    #[test]
    fn desktop_env_detects_cosmic_tokens() {
        assert!(super::desktop_env_is_cosmic("COSMIC", "", ""));
        assert!(super::desktop_env_is_cosmic("pop:COSMIC", "", ""));
        assert!(super::desktop_env_is_cosmic("", "cosmic", ""));
        assert!(super::desktop_env_is_cosmic("", "", "cosmic"));
        assert!(super::desktop_env_is_cosmic(
            "GNOME:COSMIC",
            "gnome",
            "gnome"
        ));
        assert!(!super::desktop_env_is_cosmic("KDE", "", ""));
        assert!(!super::desktop_env_is_cosmic("GNOME", "plasma", "plasma"));
        assert!(!super::desktop_env_is_cosmic("XFCE", "xfce", "xfce"));
    }

    #[test]
    fn cosmic_term_uses_dash_e() {
        assert_eq!(super::terminal_exec_args("cosmic-term"), &["-e"] as &[&str]);
        let cosmic = super::terminal_candidates_from(None, true);
        let cosmic_term = cosmic
            .iter()
            .find(|(bin, _)| bin == "cosmic-term")
            .expect("cosmic-term candidate");
        assert_eq!(cosmic_term.1, vec!["-e".to_string()]);
    }

    #[test]
    fn path_is_trusted_executable_rejects_tmp() {
        let tmp = std::env::temp_dir().join(format!(
            "abs-untrusted-bin-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&tmp, b"#!/bin/sh\n").unwrap();
        assert!(
            !super::path_is_trusted_executable(&tmp),
            "{}",
            tmp.display()
        );
        let _ = std::fs::remove_file(&tmp);
        if std::path::Path::new("/usr/bin/true").is_file() {
            assert!(super::path_is_trusted_executable(std::path::Path::new(
                "/usr/bin/true"
            )));
        }
    }

    #[test]
    fn cosmic_session_prefers_cosmic_term_before_known_list() {
        let names: Vec<String> = super::terminal_candidates_from(None, true)
            .into_iter()
            .map(|(bin, _)| bin)
            .collect();
        let xdg = names
            .iter()
            .position(|n| n == "xdg-terminal-exec")
            .expect("xdg-terminal-exec");
        let cosmic_term = names
            .iter()
            .position(|n| n == "cosmic-term")
            .expect("cosmic-term");
        let kitty = names.iter().position(|n| n == "kitty").expect("kitty");
        assert!(xdg < cosmic_term, "{names:?}");
        assert!(cosmic_term < kitty, "{names:?}");
        assert_eq!(names.iter().filter(|n| *n == "cosmic-term").count(), 1);
    }

    #[test]
    fn non_cosmic_session_still_lists_cosmic_term() {
        let names: Vec<String> = super::terminal_candidates_from(None, false)
            .into_iter()
            .map(|(bin, _)| bin)
            .collect();
        assert_eq!(names.first().map(String::as_str), Some("xdg-terminal-exec"));
        assert_eq!(names.get(1).map(String::as_str), Some("kitty"));
        assert!(names.contains(&"cosmic-term".to_string()), "{names:?}");
    }

    #[test]
    fn finds_local_pkgbuild_under_aur() {
        let dir = std::env::temp_dir().join(format!(
            "absgui-pkgbuild-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let pkg = dir.join("aur").join("foo-bar");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("PKGBUILD"), "pkgver=1\n").unwrap();
        let got = super::find_local_pkgbuild(dir.to_str().unwrap(), "foo-bar");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(got.as_deref(), Some("pkgver=1\n"));
    }

    #[test]
    fn prefers_emerge_backup() {
        let dir = std::env::temp_dir().join(format!(
            "absgui-pkgbuild-bak-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let pkg = dir.join("foo-bar");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("PKGBUILD"), "pkgrel=2\n").unwrap();
        std::fs::write(pkg.join(".PKGBUILD.emerge_backup"), "pkgrel=1\n").unwrap();
        let got = super::find_local_pkgbuild(dir.to_str().unwrap(), "foo-bar");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(got.as_deref(), Some("pkgrel=1\n"));
    }
}
