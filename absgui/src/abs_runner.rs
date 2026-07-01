use iced::futures::SinkExt;
use iced::futures::Stream;
use iced::futures::channel::mpsc;
use iced::futures::StreamExt;
use iced::stream;
use serde::Deserialize;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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

#[derive(Debug, Clone)]
pub struct AbsRunOutput {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub event_log: Option<PathBuf>,
    pub user_aborted: bool,
}

#[derive(Debug)]
pub enum AbsPgoStreamItem {
    Line(String),
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
    Line(String),
    Done(Result<AbsRunOutput, String>),
}

/// Shared handle for the abs child spawned by a PGO run; used to stop compilation on Abort.
#[derive(Clone, Default)]
pub struct PgoRunHandle {
    pid: Arc<Mutex<Option<u32>>>,
    user_aborted: Arc<AtomicBool>,
}

impl PgoRunHandle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&self) {
        self.user_aborted.store(false, Ordering::SeqCst);
        *self.pid.lock().unwrap() = None;
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

    /// Send stop signals to the tracked in-app abs child and/or an external-terminal abs PID file.
    pub fn stop_running_build(&self, external_pid_file: Option<&Path>) {
        self.user_aborted.store(true, Ordering::SeqCst);
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
    let output = Command::new(&bin)
        .arg("--version")
        .output()
        .map_err(|e| {
            format!(
                "Cannot run `{bin}`: {e}. Install the abs package or set ABS_BINARY to the binary path."
            )
        })?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("`{bin} --version` failed: {err}"));
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

fn shell_quote(s: &str) -> String {
    if s.is_empty()
        || s.chars()
            .any(|c| c.is_whitespace() || c == '\'' || c == '\\' || "\"$`!".contains(c))
    {
        format!("'{}'", s.replace('\'', "'\"'\"'"))
    } else {
        s.to_string()
    }
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
    let mut candidates: Vec<(String, Vec<String>)> = Vec::new();
    if let Ok(t) = std::env::var("ABSGUI_TERMINAL") {
        let t = t.trim().to_string();
        if !t.is_empty() {
            candidates.push((t, vec!["-e".into()]));
        }
    }
    const KNOWN: &[(&str, &[&str])] = &[
        ("kitty", &[]),
        ("alacritty", &["-e"]),
        ("wezterm", &["start", "--"]),
        ("foot", &[]),
        ("ghostty", &["-e"]),
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
    for (bin, before) in KNOWN {
        candidates.push((
            (*bin).to_string(),
            before.iter().map(|s| (*s).to_string()).collect(),
        ));
    }

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
        Err("No terminal emulator found. Install one (kitty, alacritty, konsole, gnome-terminal, \
             xterm, …) or set ABSGUI_TERMINAL to your terminal command."
            .into())
    } else {
        Err(format!(
            "Failed to launch a terminal emulator (tried: {}).",
            tried.join(", ")
        ))
    }
}

fn command_exists(name: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {name} >/dev/null 2>&1"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Find a graphical askpass program so sudo can prompt for a password from the GUI (there is no
/// interactive terminal). Honors an existing `SUDO_ASKPASS`, then known helpers, then generates a
/// small zenity/kdialog wrapper. Returns the askpass path, or `None` if nothing is available.
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

    // Generate a wrapper around zenity/kdialog as a last resort.
    let tool = if command_exists("zenity") {
        "exec zenity --password --title='absgui: sudo password'"
    } else if command_exists("kdialog") {
        "exec kdialog --password 'absgui needs your sudo password to stop builds and unmount the ramdisk'"
    } else {
        return None;
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

/// Mark spawned `abs` as GUI-driven and wire up a graphical askpass when one is available.
fn apply_gui_sudo_env(cmd: &mut Command) {
    cmd.env("ABS_GUI", "1");
    cmd.stdin(Stdio::null());
    if let Some(askpass) = ensure_askpass_helper() {
        cmd.env("SUDO_ASKPASS", askpass);
    }
}

/// Spawn abs for PGO with a pseudo-TTY or line-buffer wrapper when possible so makepkg output
/// streams to absgui instead of block-buffering for minutes.
/// Returns `(Command, optional wrapper line if different from inner)`.
fn spawn_pgo_command(inner: &str) -> (Command, Option<String>) {
    let (program, args, wrapper_display): (String, Vec<String>, String) = if command_exists("script") {
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
    let _ = Command::new("kill")
        .args(["-TERM", &pgid])
        .status();
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
    let _ = Command::new("kill")
        .args(["-KILL", &pgid])
        .status();
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

/// Read a piped child stream line-by-line, including partial lines without a trailing `\n`.
fn stream_pipe_lines<R: Read + Send>(
    reader: R,
    stderr_tag: Option<&'static str>,
    tx: mpsc::Sender<PgoStreamEvent>,
) {
    let send = |event: PgoStreamEvent| {
        let mut sender = tx.clone();
        iced::futures::executor::block_on(async move {
            let _ = sender.send(event).await;
        });
    };
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(_) => {
                let mut line = String::from_utf8_lossy(&buf).into_owned();
                while line.ends_with('\n') || line.ends_with('\r') {
                    line.pop();
                }
                if line.is_empty() {
                    continue;
                }
                let msg = match stderr_tag {
                    Some(tag) => format!("[{tag}] {line}"),
                    None => line,
                };
                send(PgoStreamEvent::Line(msg));
            }
            Err(_) => break,
        }
    }
}

#[derive(Clone)]
struct PgoStreamFlags {
    stage: Option<String>,
    once: bool,
    auto_restart: bool,
}

fn run_abs_pgo_streaming(
    action: PgoAction,
    package: &str,
    event_log: Option<PathBuf>,
    flags: PgoStreamFlags,
    handle: PgoRunHandle,
    tx: mpsc::Sender<PgoStreamEvent>,
) {
    let send = |event: PgoStreamEvent| {
        let mut sender = tx.clone();
        iced::futures::executor::block_on(async move {
            let _ = sender.send(event).await;
        });
    };

    let inner = format_abs_pgo_command(
        action,
        package,
        event_log.as_deref(),
        flags.stage.as_deref(),
        flags.once,
        flags.auto_restart,
    );
    let (mut cmd, wrapper_line) = spawn_pgo_command(&inner);
    // The exact `$ abs …` line is already logged by the caller; only show the wrapper here.
    if let Some(wrapper) = wrapper_line {
        send(PgoStreamEvent::Line(wrapper));
    }

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
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
            send(PgoStreamEvent::Done(Err(format!("Failed to start abs: {e}"))));
            return;
        }
    };

    handle.set_pid(child.id());

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let tx_out = tx.clone();
    let stdout_handle = stdout.map(|out| {
        thread::spawn(move || stream_pipe_lines(out, None, tx_out))
    });

    let tx_err = tx.clone();
    let stderr_handle = stderr.map(|err| {
        thread::spawn(move || stream_pipe_lines(err, Some("stderr"), tx_err))
    });

    if let Some(h) = stdout_handle {
        let _ = h.join();
    }
    if let Some(h) = stderr_handle {
        let _ = h.join();
    }

    let user_aborted = handle.user_aborted();
    handle.clear_pid();

    let status = match child.wait() {
        Ok(s) => s,
        Err(e) => {
            send(PgoStreamEvent::Done(Err(format!("Failed waiting for abs: {e}"))));
            return;
        }
    };

    let exit_line = if let Some(code) = status.code() {
        format!("--- exited {code} ---")
    } else {
        "--- exited by signal ---".into()
    };
    send(PgoStreamEvent::Line(exit_line));

    send(PgoStreamEvent::Done(Ok(AbsRunOutput {
        success: status.success(),
        exit_code: status.code(),
        event_log,
        user_aborted,
    })));
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
    stream::channel(512, move |mut output| async move {
        let (tx, rx) = mpsc::channel(512);
        let package_copy = package.clone();
        let event_log_copy = event_log.clone();
        let handle_copy = handle.clone();
        let flags = PgoStreamFlags {
            stage: pgo_stage.clone(),
            once: pgo_once,
            auto_restart: pgo_auto,
        };
        thread::spawn(move || {
            run_abs_pgo_streaming(
                action,
                &package_copy,
                event_log_copy,
                flags,
                handle_copy,
                tx,
            );
        });

        let mut rx = rx;
        while let Some(event) = rx.next().await {
            match event {
                PgoStreamEvent::Line(line) => {
                    if output.send(AbsPgoStreamItem::Line(line)).await.is_err() {
                        break;
                    }
                }
                PgoStreamEvent::Done(result) => {
                    let _ = output.send(AbsPgoStreamItem::Finished(result)).await;
                    break;
                }
            }
        }
    })
}

pub fn run_abs_abort(package: &str) -> Result<String, String> {
    let mut cmd = Command::new(abs_binary());
    cmd.args(["--pgo-abort", package]);
    apply_gui_sudo_env(&mut cmd);
    let output = cmd
        .output()
        .map_err(|e| format!("spawn: {e}"))?;
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
