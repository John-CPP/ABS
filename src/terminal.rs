//! Open a visible terminal emulator (PGO auto-resume from systemd has no TTY).
//! Candidate order is kept in sync with `absgui/src/abs_runner.rs`.

use crate::utils::sh_single_quote;
use crate::{blog, ewarn};
use std::fs::{self, File};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const IN_TERMINAL_ENV: &str = "ABS_PGO_IN_TERMINAL";

/// True when this process was started inside the terminal we opened.
pub fn already_in_visible_terminal() -> bool {
    std::env::var_os(IN_TERMINAL_ENV).is_some()
}

pub fn stdin_is_tty() -> bool {
    std::io::stdin().is_terminal()
}

pub fn graphical_display_available() -> bool {
    if display_env_ready(
        std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
        std::env::var("DISPLAY").ok().as_deref(),
    ) {
        return true;
    }
    let Some(text) = systemd_user_environment_text() else {
        return false;
    };
    let vars = parse_systemd_display_env(&text);
    display_env_ready(
        vars.iter()
            .find(|(k, _)| k == "WAYLAND_DISPLAY")
            .map(|(_, v)| v.as_str()),
        vars.iter()
            .find(|(k, _)| k == "DISPLAY")
            .map(|(_, v)| v.as_str()),
    )
}

fn display_env_ready(wayland: Option<&str>, display: Option<&str>) -> bool {
    nonempty_str(wayland) || nonempty_str(display)
}

fn nonempty_str(v: Option<&str>) -> bool {
    v.is_some_and(|s| !s.is_empty())
}

fn systemd_user_environment_text() -> Option<String> {
    let output = Command::new("systemctl")
        .args(["--user", "show-environment"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn parse_systemd_display_env(text: &str) -> Vec<(String, String)> {
    const KEYS: &[&str] = &["DISPLAY", "WAYLAND_DISPLAY", "XDG_CURRENT_DESKTOP"];
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((key, raw)) = line.split_once('=') else {
            continue;
        };
        if !KEYS.contains(&key) {
            continue;
        }
        let value = unquote_systemd_env_value(raw);
        if !value.is_empty() {
            out.push((key.to_string(), value));
        }
    }
    out
}

fn unquote_systemd_env_value(raw: &str) -> String {
    let t = raw.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        t[1..t.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else {
        t.to_string()
    }
}

fn display_env_for_child() -> Vec<(String, String)> {
    const KEYS: &[&str] = &[
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XDG_CURRENT_DESKTOP",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
    ];
    let mut vars = Vec::new();
    for key in KEYS {
        if let Ok(value) = std::env::var(key)
            && !value.is_empty()
        {
            vars.push(((*key).to_string(), value));
        }
    }
    if let Some(text) = systemd_user_environment_text() {
        for (key, value) in parse_systemd_display_env(&text) {
            if !vars.iter().any(|(k, _)| k == &key) {
                vars.push((key, value));
            }
        }
    }
    vars
}

/// Re-run this abs invocation where the user can type a sudo password, then wait until that copy exits.
/// GUI session → terminal emulator. Console/SSH session → that TTY.
pub fn handoff_auto_resume() -> Result<i32, String> {
    let mut gui_tried = false;
    let mut last_wait_log = Instant::now() - Duration::from_secs(30);
    loop {
        if !gui_tried && graphical_display_available() {
            gui_tried = true;
            match spawn_self_in_gui_terminal() {
                Ok(code) => return Ok(code),
                Err(e) => {
                    ewarn!(
                        "Could not open a PGO GUI terminal ({e}); waiting for a console login instead"
                    );
                }
            }
            continue;
        }
        if let Some(tty) = first_openable_session_tty() {
            blog!(
                "PGO auto-resume: continuing on {} (no desktop session)",
                tty.display()
            );
            return relaunch_self_on_tty(&tty);
        }
        if last_wait_log.elapsed() >= Duration::from_secs(30) {
            blog!(
                "PGO auto-resume: waiting for a graphical session or a console login (local TTY or SSH)…"
            );
            last_wait_log = Instant::now();
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn spawn_self_in_gui_terminal() -> Result<i32, String> {
    let exe = std::env::current_exe().map_err(|e| format!("resolve abs binary: {e}"))?;
    let mut inner = sh_single_quote(&exe.display().to_string());
    for arg in std::env::args().skip(1) {
        inner.push(' ');
        inner.push_str(&sh_single_quote(&arg));
    }

    let stamp = std::env::temp_dir().join(format!("abs-pgo-term-{}.running", std::process::id()));
    let status_path =
        std::env::temp_dir().join(format!("abs-pgo-term-{}.status", std::process::id()));
    let _ = fs::remove_file(&stamp);
    let _ = fs::remove_file(&status_path);

    let script = format!(
        "unset SUDO_ASKPASS ABS_GUI 2>/dev/null; \
         export {env}=1; \
         touch {stamp}; \
         trap 'rm -f {stamp}' EXIT INT TERM; \
         {inner}; code=$?; \
         printf '%s\\n' \"$code\" > {status}; \
         exit \"$code\"",
        env = IN_TERMINAL_ENV,
        stamp = sh_single_quote(&stamp.display().to_string()),
        inner = inner,
        status = sh_single_quote(&status_path.display().to_string()),
    );

    let mut last_term = String::new();
    for attempt in 1..=3 {
        let term = spawn_terminal(&script)?;
        last_term = term.clone();
        blog!("PGO auto-resume: opening {term} so the pipeline is visible");
        if stamp_appeared(&stamp, Duration::from_secs(45)) {
            return wait_for_stamp_finish(&stamp, &status_path);
        }
        ewarn!("PGO terminal {term} did not start the pipeline (attempt {attempt}/3)");
    }
    Err(format!(
        "terminal window did not start the PGO process (last tried {last_term})"
    ))
}

fn command_exists(name: &str) -> bool {
    let path = Path::new(name);
    if path.components().count() > 1 {
        return path.is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
}

fn spawn_terminal(script: &str) -> Result<String, String> {
    let candidates = terminal_candidates();
    let mut tried = Vec::new();
    for (bin, before) in candidates {
        if !command_exists(&bin) {
            continue;
        }
        tried.push(bin.clone());
        let mut cmd = Command::new(&bin);
        cmd.args(&before)
            .arg("bash")
            .arg("-lc")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        for (key, value) in display_env_for_child() {
            cmd.env(key, value);
        }
        match cmd.spawn() {
            Ok(child) => {
                // Reap in the background. Some emulators (gnome-terminal) exit immediately
                // after spawning a helper; the stamp file is what we wait on.
                std::thread::spawn(move || {
                    let mut child = child;
                    let _ = child.wait();
                });
                return Ok(bin);
            }
            Err(_) => continue,
        }
    }
    if tried.is_empty() {
        Err(
            "no terminal emulator found (install kitty, foot, konsole, gnome-terminal, … \
             or set ABSGUI_TERMINAL)"
                .into(),
        )
    } else {
        Err(format!(
            "could not launch a terminal (tried {})",
            tried.join(", ")
        ))
    }
}

fn stamp_appeared(stamp: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if stamp.is_file() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    stamp.is_file()
}

fn wait_for_stamp_finish(stamp: &Path, status_path: &Path) -> Result<i32, String> {
    while stamp.is_file() {
        std::thread::sleep(Duration::from_millis(500));
    }
    let code = fs::read_to_string(status_path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(1);
    let _ = fs::remove_file(status_path);
    Ok(code)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionTty {
    uid: u32,
    tty: String,
    remote: bool,
}

fn parse_list_session_ids(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.to_ascii_uppercase().starts_with("SESSION") {
            continue;
        }
        if let Some(id) = line.split_whitespace().next()
            && id.chars().all(|c| c.is_ascii_digit())
        {
            ids.push(id.to_string());
        }
    }
    ids
}

fn parse_show_session(text: &str) -> Option<SessionTty> {
    let mut uid = None;
    let mut tty = String::new();
    let mut remote = false;
    let mut session_type = String::new();
    for line in text.lines() {
        let Some((key, raw)) = line.split_once('=') else {
            continue;
        };
        let value = raw.trim();
        match key {
            "User" | "UID" | "Uid" => uid = value.parse().ok(),
            "TTY" => tty = value.to_string(),
            "Remote" => remote = value.eq_ignore_ascii_case("yes"),
            "Type" => session_type = value.to_ascii_lowercase(),
            _ => {}
        }
    }
    if matches!(
        session_type.as_str(),
        "wayland" | "x11" | "mir" | "web" | "unspecified"
    ) && tty.is_empty()
    {
        return None;
    }
    if matches!(session_type.as_str(), "wayland" | "x11" | "mir" | "web") {
        return None;
    }
    if tty.is_empty() {
        return None;
    }
    Some(SessionTty {
        uid: uid?,
        tty,
        remote,
    })
}

fn tty_device_path(tty: &str) -> Option<PathBuf> {
    let t = tty.trim();
    if t.is_empty() {
        return None;
    }
    if t.starts_with('/') {
        Some(PathBuf::from(t))
    } else {
        Some(PathBuf::from(format!("/dev/{t}")))
    }
}

fn is_kernel_vt(tty: &str) -> bool {
    let t = tty.trim().trim_start_matches("/dev/");
    t.starts_with("tty")
}

fn console_tty_candidates(sessions: &[SessionTty], uid: u32) -> Vec<PathBuf> {
    let mut vts = Vec::new();
    let mut local_pts = Vec::new();
    let mut remote_pts = Vec::new();
    for session in sessions {
        if session.uid != uid {
            continue;
        }
        let Some(path) = tty_device_path(&session.tty) else {
            continue;
        };
        if session.remote {
            remote_pts.push(path);
        } else if is_kernel_vt(&session.tty) {
            vts.push(path);
        } else {
            local_pts.push(path);
        }
    }
    vts.extend(local_pts);
    vts.extend(remote_pts);
    vts
}

fn first_openable_session_tty() -> Option<PathBuf> {
    let uid = current_uid();
    let sessions = load_loginctl_sessions();
    for path in console_tty_candidates(&sessions, uid) {
        if File::options().read(true).write(true).open(&path).is_ok() {
            return Some(path);
        }
    }
    None
}

fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        unsafe { libc::getuid() }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

fn load_loginctl_sessions() -> Vec<SessionTty> {
    let output = Command::new("loginctl")
        .args(["list-sessions", "--no-legend", "--no-pager"])
        .output()
        .ok();
    let Some(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let ids = parse_list_session_ids(&String::from_utf8_lossy(&output.stdout));
    let mut sessions = Vec::new();
    for id in ids {
        let shown = Command::new("loginctl")
            .args(["show-session", &id])
            .output()
            .ok();
        let Some(shown) = shown else {
            continue;
        };
        if !shown.status.success() {
            continue;
        }
        if let Some(session) = parse_show_session(&String::from_utf8_lossy(&shown.stdout)) {
            sessions.push(session);
        }
    }
    sessions
}

fn relaunch_self_on_tty(tty: &Path) -> Result<i32, String> {
    switch_to_vt(tty);
    let exe = std::env::current_exe().map_err(|e| format!("resolve abs binary: {e}"))?;
    let file = File::options()
        .read(true)
        .write(true)
        .open(tty)
        .map_err(|e| format!("open {}: {e}", tty.display()))?;
    let stdin = file
        .try_clone()
        .map_err(|e| format!("clone {}: {e}", tty.display()))?;
    let stdout = file
        .try_clone()
        .map_err(|e| format!("clone {}: {e}", tty.display()))?;
    let mut cmd = Command::new(&exe);
    cmd.args(std::env::args().skip(1))
        .env(IN_TERMINAL_ENV, "1")
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(file));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                let _ = libc::ioctl(0, libc::TIOCSCTTY, 0);
                Ok(())
            });
        }
    }
    let status = cmd
        .status()
        .map_err(|e| format!("spawn abs on {}: {e}", tty.display()))?;
    Ok(status.code().unwrap_or(1))
}

fn switch_to_vt(tty: &Path) {
    let Some(name) = tty.file_name().and_then(|s| s.to_str()) else {
        return;
    };
    let Some(n) = name.strip_prefix("tty").and_then(|s| s.parse::<u32>().ok()) else {
        return;
    };
    if n == 0 {
        return;
    }
    let _ = Command::new("chvt").arg(n.to_string()).status();
}

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

fn desktop_env_is_cosmic(current: &str, session: &str, desktop_session: &str) -> bool {
    [current, session, desktop_session]
        .into_iter()
        .flat_map(|s| s.split(|c: char| !c.is_ascii_alphanumeric()))
        .any(|t| t.eq_ignore_ascii_case("cosmic"))
}

fn terminal_exec_args(bin: &str) -> &'static [&'static str] {
    let name = bin.rsplit('/').next().unwrap_or(bin);
    match name {
        "xdg-terminal-exec" => &["--"],
        "cosmic-term"
        | "alacritty"
        | "ghostty"
        | "konsole"
        | "tilix"
        | "mate-terminal"
        | "lxterminal"
        | "st"
        | "urxvt"
        | "xterm"
        | "x-terminal-emulator" => &["-e"],
        "wezterm" => &["start", "--"],
        "gnome-terminal" => &["--"],
        "xfce4-terminal" => &["-x"],
        _ => &["-e"],
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosmic_term_is_early_on_cosmic() {
        let names: Vec<String> = terminal_candidates_from(None, true)
            .into_iter()
            .map(|(b, _)| b)
            .collect();
        let xdg = names.iter().position(|n| n == "xdg-terminal-exec").unwrap();
        let cosmic = names.iter().position(|n| n == "cosmic-term").unwrap();
        let kitty = names.iter().position(|n| n == "kitty").unwrap();
        assert!(xdg < cosmic && cosmic < kitty, "{names:?}");
    }

    #[test]
    fn absgui_terminal_comes_first() {
        let names: Vec<String> = terminal_candidates_from(Some("foot".into()), false)
            .into_iter()
            .map(|(b, _)| b)
            .collect();
        assert_eq!(names.first().map(String::as_str), Some("foot"));
    }

    #[test]
    fn handoff_gate_skips_tty_and_nested() {
        assert!(!should_handoff(true, false, true, false));
        assert!(!should_handoff(true, false, false, true));
        assert!(!should_handoff(false, false, false, false));
        assert!(!should_handoff(true, true, false, false));
        assert!(should_handoff(true, false, false, false));
    }

    #[test]
    fn graphical_display_requires_env_not_wayland_socket() {
        assert!(!display_env_ready(None, None));
        assert!(!display_env_ready(Some(""), Some("")));
        assert!(display_env_ready(Some("wayland-0"), None));
        assert!(display_env_ready(None, Some(":0")));
    }

    #[test]
    fn systemd_show_environment_extracts_display_vars() {
        let text = "\
HOME=/home/builder
WAYLAND_DISPLAY=wayland-0
DISPLAY=:0
XDG_CURRENT_DESKTOP=KDE
PATH=/usr/bin
";
        let vars = parse_systemd_display_env(text);
        assert_eq!(
            vars.iter()
                .find(|(k, _)| k == "WAYLAND_DISPLAY")
                .map(|(_, v)| v.as_str()),
            Some("wayland-0")
        );
        assert_eq!(
            vars.iter()
                .find(|(k, _)| k == "DISPLAY")
                .map(|(_, v)| v.as_str()),
            Some(":0")
        );
        assert_eq!(
            vars.iter()
                .find(|(k, _)| k == "XDG_CURRENT_DESKTOP")
                .map(|(_, v)| v.as_str()),
            Some("KDE")
        );
        assert!(vars.iter().all(|(k, _)| k != "PATH" && k != "HOME"));
    }

    #[test]
    fn tty_device_path_maps_loginctl_names() {
        assert_eq!(
            tty_device_path("tty1").as_deref(),
            Some(Path::new("/dev/tty1"))
        );
        assert_eq!(
            tty_device_path("pts/3").as_deref(),
            Some(Path::new("/dev/pts/3"))
        );
        assert_eq!(
            tty_device_path("/dev/ttyS0").as_deref(),
            Some(Path::new("/dev/ttyS0"))
        );
        assert_eq!(tty_device_path("").as_deref(), None);
        assert_eq!(tty_device_path("   ").as_deref(), None);
    }

    #[test]
    fn parse_show_session_skips_graphical_and_keeps_console() {
        assert!(
            parse_show_session(
                "\
User=1000
Name=builder
TTY=
Type=wayland
Remote=no
"
            )
            .is_none()
        );
        let tty = parse_show_session(
            "\
User=1000
Name=builder
TTY=tty1
Type=tty
Remote=no
",
        )
        .expect("local vt");
        assert_eq!(tty.uid, 1000);
        assert_eq!(tty.tty, "tty1");
        assert!(!tty.remote);
        let ssh = parse_show_session(
            "\
UID=1000
TTY=pts/2
Type=tty
Remote=yes
",
        )
        .expect("ssh");
        assert!(ssh.remote);
        assert_eq!(ssh.tty, "pts/2");
    }

    #[test]
    fn parse_list_session_ids_skips_header() {
        let text = "\
SESSION  UID USER SEAT TTY
     1 1000 builder seat0 tty1
     7 1000 builder - pts/0
";
        assert_eq!(parse_list_session_ids(text), ["1", "7"]);
    }

    #[test]
    fn console_tty_candidates_prefer_local_vt_over_ssh() {
        let sessions = vec![
            parse_show_session("User=1000\nTTY=pts/0\nType=tty\nRemote=yes\n").unwrap(),
            parse_show_session("User=1000\nTTY=tty1\nType=tty\nRemote=no\n").unwrap(),
            parse_show_session("User=1001\nTTY=tty2\nType=tty\nRemote=no\n").unwrap(),
        ];
        let paths = console_tty_candidates(&sessions, 1000);
        assert_eq!(
            paths,
            vec![PathBuf::from("/dev/tty1"), PathBuf::from("/dev/pts/0"),]
        );
    }
}

pub fn should_handoff(auto: bool, json: bool, is_tty: bool, already_in_terminal: bool) -> bool {
    auto && !json && !is_tty && !already_in_terminal
}
