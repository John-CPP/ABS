use std::collections::HashMap;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

static TRACKED_CHILDREN: OnceLock<Mutex<HashMap<u32, ()>>> = OnceLock::new();
static EXIT_PAUSE_REQUESTED: AtomicBool = AtomicBool::new(false);
static SKIP_EXIT_PAUSE: AtomicBool = AtomicBool::new(false);

fn tracked_children() -> &'static Mutex<HashMap<u32, ()>> {
    TRACKED_CHILDREN.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Mark that abs ran build/update work; [`wait_before_exit_if_needed`] may prompt before teardown.
pub fn request_exit_pause() {
    EXIT_PAUSE_REQUESTED.store(true, Ordering::Relaxed);
}

/// Disable the interactive exit pause (`--no-wait` / `ABS_NO_EXIT_PAUSE`).
pub fn set_skip_exit_pause(skip: bool) {
    SKIP_EXIT_PAUSE.store(skip, Ordering::Relaxed);
}

/// When jobs finished on an interactive TTY, wait for Enter before ramdisk unmount / process exit.
pub fn wait_before_exit_if_needed() {
    if !EXIT_PAUSE_REQUESTED.load(Ordering::Relaxed) {
        return;
    }
    if SKIP_EXIT_PAUSE.load(Ordering::Relaxed) {
        return;
    }
    if std::env::var_os("ABS_GUI").is_some() || std::env::var_os("ABS_NO_EXIT_PAUSE").is_some() {
        return;
    }
    if crate::is_dry_run_mode() || !io::stdin().is_terminal() {
        return;
    }
    for path in crate::ramdisk::pending_workdir_paths() {
        let _ = writeln!(
            io::stdout(),
            "==> Ramdisk build tree at {} (inspect logs here before pressing Enter)",
            path.display()
        );
    }
    let _ = writeln!(io::stdout());
    let _ = writeln!(
        io::stdout(),
        "==> All jobs are finished. Press <Enter> to exit..."
    );
    let _ = io::stdout().flush();
    let mut line = String::new();
    let _ = io::stdin().read_line(&mut line);
}

fn track_child(pid: u32) {
    tracked_children().lock().unwrap().insert(pid, ());
}

fn untrack_child(id: u32) {
    tracked_children().lock().unwrap().remove(&id);
}

#[cfg(unix)]
fn list_process_tree(root: u32) -> Vec<u32> {
    let mut order = Vec::new();
    let mut stack = vec![root];
    let mut seen = std::collections::HashSet::new();
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        order.push(pid);
        let children_path = format!("/proc/{pid}/task/{pid}/children");
        if let Ok(data) = fs::read_to_string(&children_path) {
            for token in data.split_whitespace() {
                if let Ok(child) = token.parse::<u32>() {
                    stack.push(child);
                }
            }
        }
    }
    order
}

#[cfg(not(unix))]
fn list_process_tree(root: u32) -> Vec<u32> {
    vec![root]
}

fn signal_process_tree(root: u32, sig: i32) {
    let mut pids = list_process_tree(root);
    pids.reverse();
    for pid in pids {
        signal_pid(pid as i32, sig);
    }
}

pub fn sh_single_quote(s: &str) -> String {
    let mut out = String::from('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

fn format_command_error(
    cmd: &str,
    args: &[&str],
    stdout: &[u8],
    stderr: &[u8],
    code: Option<i32>,
) -> String {
    let rendered_cmd = if args.is_empty() {
        cmd.to_string()
    } else {
        format!("{} {}", cmd, args.join(" "))
    };
    let mut message = format!(
        "Command failed: `{}` (exit: {})",
        rendered_cmd,
        code.map_or_else(|| "signal".to_string(), |c| c.to_string())
    );
    let out = String::from_utf8_lossy(stdout).trim().to_string();
    let err = String::from_utf8_lossy(stderr).trim().to_string();
    if !out.is_empty() {
        message.push_str(&format!("\nstdout:\n{}", out));
    }
    if !err.is_empty() {
        message.push_str(&format!("\nstderr:\n{}", err));
    }
    message
}

fn is_readonly_command(cmd: &str, args: &[&str]) -> bool {
    if cmd == "pacman" {
        return args.first().is_some_and(|a| *a == "-Q" || *a == "--query");
    }
    if cmd == "vercmp" {
        return true;
    }
    if cmd == "makepkg" {
        return args.contains(&"--printsrcinfo");
    }
    if cmd == "bsdtar" {
        return args.contains(&"-xOf") || args.contains(&"-xO");
    }
    if cmd == "bash"
        && args.contains(&"-c")
            && let Some(script) = args.last()
                && script.contains("source PKGBUILD") && script.contains("pkgver") {
                    return true;
                }
    if cmd == "curl" {
        return true;
    }
    false
}

/// System paths that must never appear as a configured ABS root or sudo deletion target.
const BLOCKED_DELETION_ROOTS: &[&str] = &[
    "/", "/usr", "/etc", "/bin", "/sbin", "/home", "/root", "/var", "/lib", "/lib64", "/opt",
    "/srv", "/boot", "/proc", "/sys", "/dev", "/run",
];

static DELETABLE_ROOTS: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();

fn signal_pid(pid: i32, sig: i32) {
    unsafe {
        let _ = libc::kill(pid, sig);
    }
}

/// Send SIGINT/TERM/KILL to ABS-spawned subprocess trees (e.g. rsync, makepkg, sudo).
pub fn terminate_foreground_children() {
    let roots: Vec<u32> = tracked_children()
        .lock()
        .unwrap()
        .keys()
        .copied()
        .collect();
    if roots.is_empty() {
        return;
    }
    crate::vlog!(
        "Stopping {} foreground subprocess(es) before ramdisk cleanup...",
        roots.len()
    );
    for root in &roots {
        signal_process_tree(*root, libc::SIGINT);
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    for root in &roots {
        signal_process_tree(*root, libc::SIGTERM);
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
    for root in roots {
        signal_process_tree(root, libc::SIGKILL);
        untrack_child(root);
    }
}

/// Best-effort kill of processes whose cwd lives under `prefix` (orphaned makepkg/gcc after abort).
pub fn kill_processes_with_cwd_under(prefix: &Path, label: &str) {
    let Ok(resolved) = resolve_path_for_deletion(prefix) else {
        return;
    };
    let pids = find_pids_with_cwd_under(&resolved);
    if pids.is_empty() {
        return;
    }
    crate::vlog!(
        "Sending SIGTERM to {} process(es) under {} ({})...",
        pids.len(),
        resolved.display(),
        label
    );
    for pid in &pids {
        signal_pid(*pid as i32, libc::SIGTERM);
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
    for pid in find_pids_with_cwd_under(&resolved) {
        signal_pid(pid as i32, libc::SIGKILL);
    }
}

/// Kill other `abs` processes running PGO / kernel builds for `package` (e.g. a build started in
/// an external terminal that absgui does not track by PID).
#[cfg(unix)]
pub fn kill_abs_cli_processes(package: &str) {
    let self_pid = std::process::id();
    let mut targets = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        let cmdline_path = format!("/proc/{pid}/cmdline");
        let Ok(data) = fs::read(&cmdline_path) else {
            continue;
        };
        if data.is_empty() {
            continue;
        }
        let cmdline = data
            .split(|&b| b == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part))
            .collect::<Vec<_>>()
            .join(" ");
        if !cmdline.contains("abs") {
            continue;
        }
        if cmdline.contains("--pgo-abort") || cmdline.contains("--pgo-status") {
            continue;
        }
        let build_flag = cmdline.contains("--pgo-resume")
            || cmdline.contains("--kernel-build")
            || cmdline.split_whitespace().any(|w| w == "--pgo");
        if !build_flag {
            continue;
        }
        if !cmdline.split_whitespace().any(|w| w == package) {
            continue;
        }
        targets.push(pid);
    }
    if targets.is_empty() {
        return;
    }
    crate::vlog!(
        "Sending SIGTERM to {} abs build process(es) for {}...",
        targets.len(),
        package
    );
    for pid in &targets {
        signal_pid(*pid as i32, libc::SIGTERM);
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
    for pid in targets {
        let still_alive = Path::new(&format!("/proc/{pid}")).exists();
        if still_alive {
            signal_pid(pid as i32, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
pub fn kill_abs_cli_processes(_package: &str) {}

fn find_pids_with_cwd_under(prefix: &Path) -> Vec<u32> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut pids = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        let cwd_link = PathBuf::from(format!("/proc/{pid}/cwd"));
        let Ok(cwd) = fs::read_link(&cwd_link) else {
            continue;
        };
        let cwd_resolved = if cwd.exists() {
            fs::canonicalize(&cwd).unwrap_or(cwd)
        } else {
            cwd
        };
        if path_has_prefix(prefix, &cwd_resolved) {
            pids.push(pid);
        }
    }
    pids.sort_unstable();
    pids.dedup();
    pids
}

fn deletable_roots() -> &'static Mutex<Vec<PathBuf>> {
    DELETABLE_ROOTS.get_or_init(|| Mutex::new(Vec::new()))
}

fn path_components(path: &Path) -> Vec<Component<'_>> {
    path.components().collect()
}

/// True when `path` equals `prefix` or is a child of `prefix` (avoids `/foo` matching `/foobar`).
pub fn path_has_prefix(prefix: &Path, path: &Path) -> bool {
    let prefix = path_components(prefix);
    let path = path_components(path);
    if path.len() < prefix.len() {
        return false;
    }
    path[..prefix.len()] == prefix[..]
}

fn is_blocked_deletion_root(path: &Path) -> bool {
    BLOCKED_DELETION_ROOTS
        .iter()
        .any(|blocked| path == Path::new(blocked))
}

/// Resolve a path for containment checks: canonicalize when it exists, otherwise normalize to absolute.
pub fn resolve_path_for_deletion(path: &Path) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() {
        return Err("empty path".into());
    }
    if path.is_relative() {
        return Err(format!(
            "path must be absolute for deletion checks: {}",
            path.display()
        ));
    }
    if path.exists() {
        fs::canonicalize(path).map_err(|e| {
            format!("failed to canonicalize {} for deletion check: {}", path.display(), e)
        })
    } else {
        std::path::absolute(path).map_err(|e| {
            format!("failed to resolve absolute path {}: {}", path.display(), e)
        })
    }
}

/// Validate a configured `[paths]` entry before ABS uses it for destructive operations.
pub fn validate_config_path(key: &str, path: &str) -> Result<(), String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(format!("{key} cannot be empty"));
    }
    let resolved = resolve_path_for_deletion(Path::new(trimmed))?;
    if is_blocked_deletion_root(&resolved) {
        return Err(format!(
            "{key} must not point at a system directory: {}",
            resolved.display()
        ));
    }
    Ok(())
}

/// Register canonical ABS-managed roots; all sudo removals must stay inside one of them.
pub fn init_deletable_roots(
    packages_path: &str,
    chroot_base_path: &str,
    ready_made_packages_path: &str,
    extra_roots: &[PathBuf],
) -> Result<(), String> {
    let roots = [packages_path, chroot_base_path, ready_made_packages_path];
    let mut canonical = Vec::with_capacity(roots.len() + extra_roots.len());
    for path in roots {
        let resolved = resolve_path_for_deletion(Path::new(path))?;
        if is_blocked_deletion_root(&resolved) {
            return Err(format!(
                "refusing to register system directory as deletable root: {}",
                resolved.display()
            ));
        }
        canonical.push(resolved);
    }
    for path in extra_roots {
        let resolved = resolve_path_for_deletion(path)?;
        if is_blocked_deletion_root(&resolved) {
            return Err(format!(
                "refusing to register system directory as deletable root: {}",
                resolved.display()
            ));
        }
        if !canonical.iter().any(|existing| existing == &resolved) {
            canonical.push(resolved);
        }
    }
    *deletable_roots().lock().unwrap() = canonical;
    Ok(())
}

/// Append ramdisk (or other runtime) roots after the initial config validation pass.
pub fn append_deletable_roots(extra_roots: &[PathBuf]) -> Result<(), String> {
    let mut roots = deletable_roots().lock().unwrap();
    if roots.is_empty() {
        return Err("deletable path roots are not initialized (internal error)".into());
    }
    for path in extra_roots {
        let resolved = resolve_path_for_deletion(path)?;
        if is_blocked_deletion_root(&resolved) {
            return Err(format!(
                "refusing to register system directory as deletable root: {}",
                resolved.display()
            ));
        }
        if !roots.iter().any(|existing| existing == &resolved) {
            roots.push(resolved);
        }
    }
    Ok(())
}

/// Refuse sudo deletion unless `path` resolves inside a registered ABS root (follows symlinks).
pub fn validate_deletable_path(path: &Path) -> Result<(), String> {
    let resolved = resolve_path_for_deletion(path)?;
    if is_blocked_deletion_root(&resolved) {
        return Err(format!(
            "refusing to delete system path: {}",
            resolved.display()
        ));
    }

    let roots = deletable_roots().lock().unwrap();
    if roots.is_empty() {
        return Err(
            "deletable path roots are not initialized (internal error)".into(),
        );
    }
    if roots.iter().any(|root| path_has_prefix(root, &resolved)) {
        return Ok(());
    }

    Err(format!(
        "refusing to delete path outside ABS-managed directories: {} (resolves to {})",
        path.display(),
        resolved.display()
    ))
}

/// Shell-style command line for logging (`git pull --ff-only`).
pub fn render_command_line(cmd: &str, args: &[&str]) -> String {
    if args.is_empty() {
        cmd.to_string()
    } else {
        format!("{} {}", cmd, args.join(" "))
    }
}

/// True when abs should print `$ command` lines before spawning (absgui log / `-v` only).
fn should_echo_commands() -> bool {
    crate::is_verbose_mode() || std::env::var_os("ABS_GUI").is_some()
}

/// Print a command to stdout before execution (absgui build logs and `-v` only).
pub fn echo_command<P: AsRef<Path>>(cmd: &str, args: &[&str], cwd: Option<P>) {
    if !should_echo_commands() {
        return;
    }
    let line = render_command_line(cmd, args);
    if let Some(dir) = cwd {
        println!("$ (cd {} && {line})", dir.as_ref().display());
    } else {
        println!("$ {line}");
    }
}

/// Print a shell snippet to stdout before execution.
pub fn echo_shell_command<P: AsRef<Path>>(shell_body: &str, cwd: Option<P>) {
    if !should_echo_commands() {
        return;
    }
    if let Some(dir) = cwd {
        println!("$ (cd {} && {shell_body})", dir.as_ref().display());
    } else {
        println!("$ {shell_body}");
    }
}

fn echo_captured_output(stdout: &[u8], stderr: &[u8]) {
    if !should_echo_commands() {
        return;
    }
    if !stdout.is_empty() {
        print!("{}", String::from_utf8_lossy(stdout));
        if !stdout.ends_with(b"\n") {
            println!();
        }
    }
    if !stderr.is_empty() {
        for line in String::from_utf8_lossy(stderr).lines() {
            eprintln!("[stderr] {line}");
        }
    }
}

/// True when absgui (or another GUI parent) set `ABS_GUI` and a graphical askpass helper.
fn use_sudo_askpass() -> bool {
    std::env::var_os("ABS_GUI").is_some() && std::env::var_os("SUDO_ASKPASS").is_some()
}

/// `sudo` prefix for `sh -c` snippets. GUI children inherit the launching shell's controlling TTY,
/// so bare `sudo` prompts there instead of the askpass dialog; `-A` routes through `SUDO_ASKPASS`.
pub fn shell_sudo() -> &'static str {
    if use_sudo_askpass() {
        "sudo -A"
    } else {
        "sudo"
    }
}

/// Prepend `sudo -A` when a GUI askpass is configured so prompts never steal the parent terminal.
fn sudo_prefixed_args(args: &[&str]) -> Vec<String> {
    if use_sudo_askpass()
        && !args.contains(&"-A")
        && !args.contains(&"-n")
    {
        let mut v = vec!["-A".to_string()];
        v.extend(args.iter().map(|s| (*s).to_string()));
        v
    } else {
        args.iter().map(|s| (*s).to_string()).collect()
    }
}

fn configure_sudo_command(command: &mut Command, args: &[&str]) {
    let owned = sudo_prefixed_args(args);
    let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
    command.args(&refs);
    if use_sudo_askpass() {
        command.stdin(Stdio::null());
    } else {
        command.stdin(Stdio::inherit());
    }
}

/// UID/GID of the user driving the build (not root when invoked via sudo).
pub fn build_uid_gid() -> (u32, u32) {
    if let (Ok(uid), Ok(gid)) = (std::env::var("SUDO_UID"), std::env::var("SUDO_GID"))
        && let (Ok(u), Ok(g)) = (uid.parse::<u32>(), gid.parse::<u32>())
    {
        return (u, g);
    }
    unsafe { (libc::getuid(), libc::getgid()) }
}

/// `chown` a root-owned artifact back to the build user when stage-2 profiling left it behind.
pub fn ensure_build_user_can_read(path: &Path) -> Result<(), String> {
    if fs::OpenOptions::new().read(true).open(path).is_ok() {
        return Ok(());
    }
    let (uid, gid) = build_uid_gid();
    let owner = format!("{uid}:{gid}");
    let path_s = path.to_string_lossy();
    run_command("sudo", &["chown", &owner, path_s.as_ref()], None::<&str>)
}

pub fn run_command<P: AsRef<Path>>(cmd: &str, args: &[&str], cwd: Option<P>) -> Result<(), String> {
    if crate::is_dry_run_mode() && !is_readonly_command(cmd, args) {
        let rendered_cmd = render_command_line(cmd, args);
        println!("[DRY RUN] {}", rendered_cmd);
        return Ok(());
    }

    let owned_sudo_args;
    let exec_args: Vec<&str> = if cmd == "sudo" {
        owned_sudo_args = sudo_prefixed_args(args);
        owned_sudo_args.iter().map(String::as_str).collect()
    } else {
        args.to_vec()
    };

    echo_command(cmd, &exec_args, cwd.as_ref().map(|p| p.as_ref()));

    let mut command = Command::new(cmd);

    if let Some(dir) = cwd {
        command.current_dir(dir);
    }

    if cmd == "sudo" {
        configure_sudo_command(&mut command, args);
    } else {
        command.args(&exec_args);
    }

    let mut child = command
        .spawn()
        .map_err(|e| format!("Failed to execute '{}': {}", cmd, e))?;
    track_child(child.id());
    let status = child
        .wait()
        .map_err(|e| format!("Failed to wait on '{}': {}", cmd, e))?;
    untrack_child(child.id());

    if status.success() {
        Ok(())
    } else {
        let rendered_cmd = render_command_line(cmd, args);
        Err(format!(
            "Command failed: `{}` (exit: {})\n(Output was printed above.)",
            rendered_cmd,
            status
                .code()
                .map_or_else(|| "signal".to_string(), |c| c.to_string())
        ))
    }
}

/// Like [`run_command`], but captures stdout/stderr instead of inheriting them when silent.
/// At normal or verbose log level, behaves like [`run_command`] (echo + live output).
pub fn run_command_quiet<P: AsRef<Path>>(cmd: &str, args: &[&str], cwd: Option<P>) -> Result<(), String> {
    if crate::verbosity() >= crate::Verbosity::Normal {
        return run_command(cmd, args, cwd);
    }

    if crate::is_dry_run_mode() && !is_readonly_command(cmd, args) {
        println!("[DRY RUN] {}", render_command_line(cmd, args));
        return Ok(());
    }

    let owned_sudo_args;
    let exec_args: Vec<&str> = if cmd == "sudo" {
        owned_sudo_args = sudo_prefixed_args(args);
        owned_sudo_args.iter().map(String::as_str).collect()
    } else {
        args.to_vec()
    };

    let mut command = Command::new(cmd);

    if let Some(dir) = cwd {
        command.current_dir(dir);
    }

    if cmd == "sudo" {
        configure_sudo_command(&mut command, args);
    } else {
        command.args(&exec_args);
    }

    let output = command
        .output()
        .map_err(|e| format!("Failed to execute '{}': {}", cmd, e))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(format_command_error(
            cmd,
            &exec_args,
            &output.stdout,
            &output.stderr,
            output.status.code(),
        ))
    }
}

/// How [`run_shell_in_dir_with_tee`] runs the build pipeline.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShellRunOpts {
    /// Stream output directly to the terminal instead of `script(1)` (needed for makechrootpkg).
    pub live_output: bool,
    /// When set, print a heartbeat every 45s while the command runs.
    pub heartbeat_label: Option<&'static str>,
}

/// Always-visible phase line on stderr (flushed), even when abs runs with `-s`.
pub fn phase_banner(msg: impl AsRef<str>) {
    eprintln!("==> {}", msg.as_ref());
    let _ = io::stderr().flush();
}

/// Reset terminal attributes after `script(1)`, devtools ANSI, or Ctrl+C mid-line.
pub fn restore_terminal() {
    if !io::stdin().is_terminal() {
        return;
    }
    let _ = Command::new("sh")
        .arg("-c")
        .arg("printf '\\033[0m\\033[?25h\\n' 2>/dev/null; stty sane 2>/dev/null || true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Restore the terminal when dropped (normal exit). Interrupt handlers must call [`restore_terminal`] too.
pub struct TerminalRestoreGuard(bool);

impl TerminalRestoreGuard {
    pub fn new() -> Self {
        Self(io::stdin().is_terminal())
    }
}

impl Drop for TerminalRestoreGuard {
    fn drop(&mut self) {
        if self.0 {
            restore_terminal();
        }
    }
}

/// Run a multi-line shell snippet in `cwd`, streaming combined output to the terminal
/// while saving a copy for callers that need to parse logs (e.g. missing PGP keys).
///
/// Uses `script` when available so subprocesses (makepkg → `pacman -U` → `sudo`) get a real TTY.
/// A plain `cmd | tee` pipeline breaks sudo password prompts.
pub fn run_shell_in_dir_with_tee<P: AsRef<Path>>(
    cwd: P,
    shell_body: &str,
    opts: ShellRunOpts,
) -> Result<(), String> {
    if crate::is_dry_run_mode() {
        echo_shell_command(shell_body, Some(cwd.as_ref()));
        println!("[DRY RUN] (output streamed via tee)");
        return Ok(());
    }

    let _terminal_guard = TerminalRestoreGuard::new();

    echo_shell_command(shell_body, Some(cwd.as_ref()));

    let tmp = std::env::temp_dir();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let inner_path = tmp.join(format!("abs_build_{}_inner.sh", stamp));
    let log_path = tmp.join(format!("abs_build_{}.log", stamp));

    let cwd_q = sh_single_quote(&cwd.as_ref().to_string_lossy());
    let inner_contents = format!("#!/bin/bash\nset -e\ncd {}\n{}\n", cwd_q, shell_body);

    std::fs::write(&inner_path, inner_contents)
        .map_err(|e| format!("failed to write build helper script: {}", e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&inner_path, std::fs::Permissions::from_mode(0o700));
    }

    let inner_arg = sh_single_quote(inner_path.to_string_lossy().as_ref());
    let log_arg = sh_single_quote(log_path.to_string_lossy().as_ref());

    let pipeline = if opts.live_output {
        let stdbuf = if command_exists("stdbuf") {
            "stdbuf -oL -eL "
        } else {
            ""
        };
        format!(
            "{stdbuf}bash {inner_arg} 2>&1 | {stdbuf}tee {log_arg}; exit ${{PIPESTATUS[0]}}",
        )
    } else if command_exists("script") && io::stdin().is_terminal() {
        let stdbuf = if command_exists("stdbuf") {
            "stdbuf -oL -eL "
        } else {
            ""
        };
        format!(
            "{stdbuf}script -q -f -c {cmd} {log}",
            cmd = sh_single_quote(&format!("{stdbuf}bash {inner_arg}")),
            log = log_arg,
        )
    } else {
        let stdbuf = if command_exists("stdbuf") {
            "stdbuf -oL -eL "
        } else {
            ""
        };
        format!(
            "{stdbuf}bash {inner_arg} 2>&1 | {stdbuf}tee {log_arg}; exit ${{PIPESTATUS[0]}}",
        )
    };

    let mut command = Command::new("bash");
    command
        .arg("-o")
        .arg("pipefail")
        .arg("-c")
        .arg(&pipeline);

    let mut child = command
        .spawn()
        .map_err(|e| format!("failed to run build pipeline: {}", e))?;
    track_child(child.id());

    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let heartbeat = opts.heartbeat_label.map(|label| {
        let done_flag = std::sync::Arc::clone(&done);
        let label = label.to_string();
        std::thread::spawn(move || {
            let mut elapsed_secs = 0u64;
            while !done_flag.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_secs(45));
                if done_flag.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                elapsed_secs += 45;
                let mins = elapsed_secs / 60;
                let secs = elapsed_secs % 60;
                eprintln!(
                    "==> Still running: {label} ({mins}m {secs:02}s) — chroot sync and dependency install can take a long time for large packages"
                );
            }
        })
    });

    let status = child
        .wait()
        .map_err(|e| format!("failed to wait on build pipeline: {}", e))?;
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Some(handle) = heartbeat {
        let _ = handle.join();
    }
    untrack_child(child.id());

    let log_text = std::fs::read_to_string(&log_path).unwrap_or_default();
    let _ = std::fs::remove_file(&inner_path);
    let _ = std::fs::remove_file(&log_path);
    restore_terminal();

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Command failed (exit: {})\n{}",
            status
                .code()
                .map_or_else(|| "signal".to_string(), |c| c.to_string()),
            log_text
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

/// Remove **`src/`** and **`pkg/`** under the package directory (makepkg workdirs) before a fresh build.
pub fn remove_src_pkg_workdirs(repo_dir: &Path) -> Result<(), String> {
    if crate::is_dry_run_mode() {
        println!(
            "[DRY RUN] rm -rf {}/src {}/pkg",
            repo_dir.display(),
            repo_dir.display()
        );
        return Ok(());
    }

    for name in ["src", "pkg"] {
        let p = repo_dir.join(name);
        if p.exists() {
            check_sudo_removal(&p)?;
        }
    }
    Ok(())
}

/// True for a built package artifact (`pkgname-ver-rel-arch.pkg.tar.<ext>`) regardless of the
/// configured `PKGEXT` (zst/xz/gz/lz4/uncompressed). Excludes detached signatures (`.sig`).
pub fn is_package_artifact(name: &str) -> bool {
    name.contains(".pkg.tar") && !name.ends_with(".sig")
}

/// Remove prior package artifacts for this package base name from PKGDEST so old builds (e.g.
/// `-1.2`) are not offered alongside the new one (`-1.3`) after a recompile.
pub fn remove_stale_pkgs_in_pkgdest(pkgdest: &str, base_name: &str) {
    if crate::is_dry_run_mode() {
        println!(
            "[DRY RUN] rm stale {}-*.pkg.tar.* in {}",
            base_name, pkgdest
        );
        return;
    }

    let dir = Path::new(pkgdest);
    let prefix = format!("{}-", base_name);
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !is_package_artifact(name) || !name.starts_with(&prefix) {
            continue;
        }
        if let Err(e) = validate_deletable_path(&path) {
            eprintln!(
                "==> WARNING: Skipping stale package removal for {}: {}",
                path.display(),
                e
            );
            continue;
        }
        let removed = if crate::force_sudo_clean() {
            false
        } else {
            fs::remove_file(&path).is_ok()
        };
        if !removed {
            let _ = run_command(
                "sudo",
                &["rm", "-f", path.to_string_lossy().as_ref()],
                None::<&str>,
            );
        } else {
            crate::vlog!("Removed stale package file: {}", name);
        }
    }
}

/// Like [`run_command_with_output`], but never logs the command or prints captured output.
/// Used for background update checks that must stay silent and must not interleave with sudo prompts.
pub fn run_command_with_output_silent<P: AsRef<Path>>(
    cmd: &str,
    args: &[&str],
    cwd: Option<P>,
) -> Result<String, String> {
    if crate::is_dry_run_mode() && !is_readonly_command(cmd, args) {
        return Ok(String::new());
    }

    let mut command = Command::new(cmd);
    command.args(args);

    if let Some(dir) = cwd {
        command.current_dir(dir);
    }

    let output = command
        .output()
        .map_err(|e| format!("Failed to execute '{}': {}", cmd, e))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(format_command_error(
            cmd,
            args,
            &output.stdout,
            &output.stderr,
            output.status.code(),
        ))
    }
}

pub fn run_command_with_output<P: AsRef<Path>>(
    cmd: &str,
    args: &[&str],
    cwd: Option<P>,
) -> Result<String, String> {
    if crate::is_dry_run_mode() && !is_readonly_command(cmd, args) {
        println!("[DRY RUN] {}", render_command_line(cmd, args));
        return Ok(String::new());
    }

    echo_command(cmd, args, cwd.as_ref().map(|p| p.as_ref()));

    let mut command = Command::new(cmd);
    command.args(args);

    if let Some(dir) = cwd {
        command.current_dir(dir);
    }

    let output = command
        .output()
        .map_err(|e| format!("Failed to execute '{}': {}", cmd, e))?;

    echo_captured_output(&output.stdout, &output.stderr);

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(format_command_error(
            cmd,
            args,
            &output.stdout,
            &output.stderr,
            output.status.code(),
        ))
    }
}

/// Like [`run_command_with_output`], but sets extra environment variables on the child process.
pub fn run_command_with_output_env<P: AsRef<Path>>(
    cmd: &str,
    args: &[&str],
    cwd: Option<P>,
    env: &[(&str, &str)],
) -> Result<String, String> {
    if crate::is_dry_run_mode() && !is_readonly_command(cmd, args) {
        println!(
            "[DRY RUN] {} (env: {:?})",
            render_command_line(cmd, args),
            env
        );
        return Ok(String::new());
    }

    if should_echo_commands() {
        let mut line = render_command_line(cmd, args);
        if !env.is_empty() {
            let env_bits: Vec<String> = env
                .iter()
                .map(|(k, v)| format!("{k}={}", sh_single_quote(v)))
                .collect();
            line = format!("{} {}", env_bits.join(" "), line);
        }
        if let Some(dir) = cwd.as_ref() {
            println!("$ (cd {} && {line})", dir.as_ref().display());
        } else {
            println!("$ {line}");
        }
    }

    let mut command = Command::new(cmd);
    command.args(args);
    for (k, v) in env {
        command.env(k, v);
    }
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }

    let output = command
        .output()
        .map_err(|e| format!("Failed to execute '{}': {}", cmd, e))?;

    echo_captured_output(&output.stdout, &output.stderr);

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(format_command_error(
            cmd,
            args,
            &output.stdout,
            &output.stderr,
            output.status.code(),
        ))
    }
}

/// True when sudo already has a valid cached timestamp (checked non-interactively, never blocks).
fn sudo_timestamp_valid() -> bool {
    Command::new("sudo")
        .args(["-n", "-v"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run **`sudo -v`** so the password is cached before later **`sudo rm`** / **`sudo pacman`** calls.
/// Long runs also use [`spawn_sudo_keepalive`] to refresh the timestamp before it expires (~15 minutes by default).
///
/// When launched from absgui (`ABS_GUI=1`) there is no interactive terminal, so a bare `sudo -v`
/// would block forever on an invisible password prompt. In that case we require a graphical askpass
/// (`SUDO_ASKPASS`) and use `sudo -A`, or fail with a clear message instead of hanging.
pub fn prime_sudo_for_session() -> Result<(), String> {
    if crate::is_dry_run_mode() {
        return Ok(());
    }
    // Fast path: timestamp already valid (also refreshes it). Never blocks.
    if sudo_timestamp_valid() {
        return Ok(());
    }
    // absgui children inherit the launching terminal as stdin; never use it for sudo there.
    if std::env::var_os("ABS_GUI").is_some() {
        if std::env::var_os("SUDO_ASKPASS").is_some() {
            return run_command("sudo", &["-v"], None::<&str>);
        }
        return Err(
            "sudo needs a password but no graphical askpass program was found. Install one of \
             ksshaskpass / lxqt-openssh-askpass / x11-ssh-askpass / zenity / kdialog, or enable \
             passwordless sudo for the build commands."
                .into(),
        );
    }
    if !io::stdin().is_terminal() && std::env::var_os("SUDO_ASKPASS").is_some() {
        return run_command("sudo", &["-v"], None::<&str>);
    }
    run_command("sudo", &["-v"], None::<&str>)
}

/// Background thread: refresh **`sudo`** timestamp every few minutes (non-interactive only).
pub fn spawn_sudo_keepalive() {
    if crate::is_dry_run_mode() {
        return;
    }
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_secs(3 * 60));
        // Never prompt from a background thread (`sudo -v` would steal/break the TTY).
        let _ = Command::new("sudo")
            .args(["-n", "-v"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    });
}

pub fn check_sudo_removal<P: AsRef<Path>>(path: P) -> Result<(), String> {
    let p = path.as_ref();
    if !p.exists() {
        return Ok(());
    }

    validate_deletable_path(p)?;

    if !crate::force_sudo_clean() && std::fs::remove_dir_all(p).is_ok() {
        return Ok(());
    }

    run_command(
        "sudo",
        &["rm", "-rf", p.to_string_lossy().as_ref()],
        None::<&str>,
    )?;

    Ok(())
}

/// Parse `makepkg --printsrcinfo` output into one Arch-style version string (`epoch:pkgver-pkgrel`).
pub fn parse_srcinfo_full_version(text: &str) -> Result<String, String> {
    let mut epoch: Option<String> = None;
    let mut pkgver: Option<String> = None;
    let mut pkgrel: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "epoch" => {
                let v = value.trim();
                if !v.is_empty() {
                    epoch = Some(v.to_string());
                }
            }
            "pkgver" => pkgver = Some(value.trim().to_string()),
            "pkgrel" => pkgrel = Some(value.trim().to_string()),
            _ => {}
        }
    }
    let pkgver = pkgver.ok_or_else(|| "pkgver missing in --printsrcinfo".to_string())?;
    let pkgrel = pkgrel.ok_or_else(|| "pkgrel missing in --printsrcinfo".to_string())?;
    if let Some(e) = epoch
        && e != "0"
        && !e.is_empty()
    {
        return Ok(format!("{e}:{pkgver}-{pkgrel}"));
    }
    Ok(format!("{pkgver}-{pkgrel}"))
}

pub fn makepkg_printsrcinfo_full_version(repo_dir: &Path) -> Result<String, String> {
    if crate::is_dry_run_mode() {
        return Err("dry-run".into());
    }
    let text = run_command_with_output("makepkg", &["--printsrcinfo"], Some(repo_dir))?;
    parse_srcinfo_full_version(&text)
}

/// Fast version read for comparisons: `.SRCINFO` if present, else `source PKGBUILD` in bash,
/// else **`makepkg --printsrcinfo`** (slowest).
pub fn read_pkg_full_version_from_dir(pkg_dir: &Path) -> Result<String, String> {
    if crate::is_dry_run_mode() {
        return Err("dry-run".into());
    }
    let srcinfo_path = pkg_dir.join(".SRCINFO");
    let pkgbuild_path = pkg_dir.join("PKGBUILD");

    let use_srcinfo = if srcinfo_path.is_file() {
        if let (Ok(src_meta), Ok(pkg_meta)) = (fs::metadata(&srcinfo_path), fs::metadata(&pkgbuild_path)) {
            if let (Ok(src_time), Ok(pkg_time)) = (src_meta.modified(), pkg_meta.modified()) {
                pkg_time <= src_time
            } else {
                true
            }
        } else {
            true
        }
    } else {
        false
    };

    if use_srcinfo {
        let text = fs::read_to_string(&srcinfo_path)
            .map_err(|e| format!("read {}: {}", srcinfo_path.display(), e))?;
        return parse_srcinfo_full_version(&text);
    }

    let script = r#"set -e; [[ -f PKGBUILD ]] || exit 1; source PKGBUILD 2>/dev/null || exit 1; [[ -n "${pkgver:-}" && -n "${pkgrel:-}" ]] || exit 1; e="${epoch:-0}"; if [[ -n "$e" && "$e" != "0" ]]; then printf '%s:%s-%s' "$e" "$pkgver" "$pkgrel"; else printf '%s-%s' "$pkgver" "$pkgrel"; fi"#;

    let output = Command::new("bash")
        .current_dir(pkg_dir)
        .args(["-c", script])
        .output()
        .map_err(|e| format!("bash PKGBUILD probe: {}", e))?;

    if output.status.success() {
        let v = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !v.is_empty() {
            return Ok(v);
        }
    }

    makepkg_printsrcinfo_full_version(pkg_dir)
}

/// Installed version from `pacman -Q` (without package name), or `None` if not installed.
pub fn pacman_query_version(pkg: &str) -> Result<Option<String>, String> {
    if crate::is_dry_run_mode() {
        return Err("dry-run".into());
    }
    match run_command_with_output("pacman", &["-Q", pkg], None::<&str>) {
        Ok(out) => {
            let line = out.lines().next().unwrap_or("").trim();
            let version = line
                .split_once(char::is_whitespace)
                .map(|(_, v)| v.trim().to_string());
            Ok(version.filter(|s| !s.is_empty()))
        }
        Err(_) => Ok(None),
    }
}

/// `vercmp a b` stdout: `-1`, `0`, or `1` (see `vercmp(8)`).
fn parse_vercmp_output(out: &str) -> Result<i32, String> {
    match out.trim() {
        "-1" => Ok(-1),
        "0" => Ok(0),
        "1" => Ok(1),
        other => Err(format!("unexpected vercmp output: {:?}", other)),
    }
}

pub fn vercmp(a: &str, b: &str) -> Result<i32, String> {
    if crate::is_dry_run_mode() {
        return Err("dry-run".into());
    }
    let out = run_command_with_output("vercmp", &[a, b], None::<&str>)?;
    parse_vercmp_output(&out)
}

/// Like [`vercmp`], but never logs the command or its output.
pub(crate) fn vercmp_silent(a: &str, b: &str) -> Result<i32, String> {
    if crate::is_dry_run_mode() {
        return Err("dry-run".into());
    }
    let out = run_command_with_output_silent("vercmp", &[a, b], None::<&str>)?;
    parse_vercmp_output(&out)
}

fn parse_pacman_si_output(out: &str) -> Option<String> {
    #[derive(Debug, Default)]
    struct PacmanSiEntry {
        repository: String,
        version: String,
    }

    let mut entries = Vec::new();
    let mut current_entry = PacmanSiEntry::default();

    for line in out.lines() {
        let line = line.trim();
        if let Some((key, val)) = line.split_once(':') {
            let key = key.trim();
            let val = val.trim();
            if key == "Repository" {
                if !current_entry.repository.is_empty() && !current_entry.version.is_empty() {
                    entries.push(current_entry);
                }
                current_entry = PacmanSiEntry {
                    repository: val.to_string(),
                    version: String::new(),
                };
            } else if key == "Version" {
                current_entry.version = val.to_string();
            }
        }
    }
    if !current_entry.repository.is_empty() && !current_entry.version.is_empty() {
        entries.push(current_entry);
    }

    // Filter out -testing, -staging, and unstable repositories
    let is_stable_repo = |repo: &str| {
        let r = repo.to_lowercase();
        !r.contains("testing") && !r.contains("staging") && !r.contains("unstable")
    };

    entries
        .into_iter()
        .find(|entry| is_stable_repo(&entry.repository))
        .map(|entry| entry.version)
}

/// Sync database version from `pacman -Si` (without package name) for stable repos, or `None` if not found.
pub fn pacman_sync_version(pkg: &str) -> Result<Option<String>, String> {
    if crate::is_dry_run_mode() {
        return Err("dry-run".into());
    }
    match run_command_with_output("pacman", &["-Si", pkg], None::<&str>) {
        Ok(out) => Ok(parse_pacman_si_output(&out)),
        Err(_) => Ok(None),
    }
}

const PACKAGER_KEYRING_PATHS: &[&str] = &[
    "/usr/share/pacman/keyrings/archlinux.gpg",
    "/usr/share/pacman/keyrings/cachyos.gpg",
];

fn launch_dirmngr_if_needed() {
    let _ = Command::new("gpgconf")
        .args(["--launch", "dirmngr"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn run_gpg_silent(args: &[&str]) -> Result<(), String> {
    let output = Command::new("gpg")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("failed to run gpg: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn gpg_key_listed(key: &str) -> bool {
    Command::new("gpg")
        .args(["--batch", "--list-keys", key])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn gpg_has_public_key(key: &str) -> bool {
    if gpg_key_listed(key) {
        return true;
    }
    if key.len() > 16 {
        return gpg_key_listed(&key[key.len() - 16..]);
    }
    false
}

pub fn gpg_key_short_id(key: &str) -> &str {
    if key.len() > 16 {
        &key[key.len() - 16..]
    } else {
        key
    }
}

/// Import Arch/CachyOS packager keys into the user keyring (`~/.gnupg`) for makepkg PGP checks.
pub fn seed_user_gpg_from_packager_keyrings() {
    if crate::is_dry_run_mode() {
        return;
    }
    for path in PACKAGER_KEYRING_PATHS {
        let path = Path::new(path);
        if !path.is_file() {
            continue;
        }
        if path.metadata().map(|m| m.len() == 0).unwrap_or(true) {
            continue;
        }
        if let Err(e) = run_gpg_silent(&["--batch", "--import", path.to_string_lossy().as_ref()]) {
            crate::vlog!("gpg --import {}: {}", path.display(), e);
        }
    }
}

/// Import a missing packager key for makepkg source verification.
pub fn import_gpg_key_for_build(key: &str) -> Result<(), String> {
    if crate::is_dry_run_mode() {
        return Ok(());
    }
    if gpg_has_public_key(key) {
        return Ok(());
    }

    launch_dirmngr_if_needed();

    let _ = run_gpg_silent(&["--locate-keys", key]);
    if gpg_has_public_key(key) {
        return Ok(());
    }

    const KEYSERVERS: &[&str] = &[
        "hkps://keyserver.archlinux.org",
        "hkps://keyserver.ubuntu.com",
        "hkps://keys.openpgp.org",
    ];
    for server in KEYSERVERS {
        if run_gpg_silent(&["--keyserver", server, "--recv-keys", key]).is_ok()
            && gpg_has_public_key(key)
        {
            return Ok(());
        }
    }

    if import_gpg_key_via_http(key).is_ok() && gpg_has_public_key(key) {
        return Ok(());
    }

    seed_user_gpg_from_packager_keyrings();
    if gpg_has_public_key(key) {
        return Ok(());
    }

    Err(format!(
        "could not import PGP key {} (tried locate-keys, keyservers, HTTP lookup, and packager keyrings)",
        gpg_key_short_id(key)
    ))
}

fn import_gpg_key_via_http(key: &str) -> Result<(), String> {
    let short = gpg_key_short_id(key);
    let url = format!(
        "https://keyserver.ubuntu.com/pks/lookup?op=get&search=0x{short}"
    );
    let output = Command::new("curl")
        .args(["-fsSL", &url])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("curl failed: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    if output.stdout.is_empty() {
        return Err("empty keyserver response".into());
    }
    let mut child = Command::new("gpg")
        .args(["--batch", "--import"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("gpg --import failed to start: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(&output.stdout)
            .map_err(|e| format!("gpg --import stdin: {e}"))?;
    }
    let import_out = child
        .wait_with_output()
        .map_err(|e| format!("gpg --import wait: {e}"))?;
    if import_out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&import_out.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod artifact_tests {
    use super::is_package_artifact;

    #[test]
    fn matches_any_pkgext_and_excludes_signatures() {
        assert!(is_package_artifact("mesa-26.1.0-1-x86_64.pkg.tar.zst"));
        assert!(is_package_artifact("mesa-26.1.0-1-x86_64.pkg.tar.xz"));
        assert!(is_package_artifact("mesa-26.1.0-1-x86_64.pkg.tar.gz"));
        assert!(is_package_artifact("mesa-26.1.0-1-x86_64.pkg.tar.lz4"));
        assert!(is_package_artifact("mesa-26.1.0-1-x86_64.pkg.tar"));
        assert!(!is_package_artifact("mesa-26.1.0-1-x86_64.pkg.tar.zst.sig"));
        assert!(!is_package_artifact("PKGBUILD"));
        assert!(!is_package_artifact("mesa-26.1.0.tar.gz"));
    }
}

#[cfg(test)]
mod version_tests {
    use super::{parse_pacman_si_output, parse_srcinfo_full_version};

    #[test]
    fn parse_srcinfo_with_epoch() {
        let s = "pkgbase = mesa\npkgver = 26.1.0\npkgrel = 1\nepoch = 2\n";
        assert_eq!(parse_srcinfo_full_version(s).unwrap(), "2:26.1.0-1");
    }

    #[test]
    fn parse_srcinfo_no_epoch() {
        let s = "pkgver=1.0\npkgrel=3\n";
        assert_eq!(parse_srcinfo_full_version(s).unwrap(), "1.0-3");
    }

    #[test]
    fn test_parse_pacman_si_output() {
        let sample = "\
Repository      : core
Name            : systemd
Version         : 260.1-2
Description     : system and service manager

Repository      : core-testing
Name            : systemd
Version         : 261-1
Description     : system and service manager
";
        assert_eq!(parse_pacman_si_output(sample), Some("260.1-2".to_string()));

        let sample_only_testing = "\
Repository      : core-testing
Name            : systemd
Version         : 261-1
";
        assert_eq!(parse_pacman_si_output(sample_only_testing), None);

        let sample_with_staging = "\
Repository      : extra-staging
Name            : systemd
Version         : 261-2

Repository      : extra
Name            : systemd
Version         : 260.1-3
";
        assert_eq!(parse_pacman_si_output(sample_with_staging), Some("260.1-3".to_string()));
    }
}

#[cfg(test)]
mod terminal_tests {
    use super::restore_terminal;

    #[test]
    fn restore_terminal_is_noop_off_tty() {
        restore_terminal();
    }
}

#[cfg(test)]
mod path_safety_tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    /// `init_deletable_roots` mutates a process-global, so tests that call it must not run
    /// concurrently with one another.
    static ROOTS_GUARD: Mutex<()> = Mutex::new(());

    #[test]
    fn path_has_prefix_does_not_match_partial_directory_name() {
        assert!(!path_has_prefix(
            Path::new("/foo"),
            Path::new("/foobar/baz")
        ));
        assert!(path_has_prefix(
            Path::new("/foo"),
            Path::new("/foo/bar")
        ));
        assert!(path_has_prefix(
            Path::new("/foo/bar"),
            Path::new("/foo/bar")
        ));
    }

    #[test]
    fn validate_deletable_path_rejects_outside_registered_roots() {
        let _guard = ROOTS_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!(
            "abs_path_safety_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let packages = base.join("packages");
        let chroot = base.join("chroot");
        let ready = base.join("ready");
        fs::create_dir_all(&packages).unwrap();
        fs::create_dir_all(&chroot).unwrap();
        fs::create_dir_all(&ready).unwrap();

        init_deletable_roots(
            packages.to_str().unwrap(),
            chroot.to_str().unwrap(),
            ready.to_str().unwrap(),
            &[],
        )
        .unwrap();

        let inside = packages.join("aur").join("foo");
        fs::create_dir_all(&inside).unwrap();
        validate_deletable_path(&inside).unwrap();

        let outside = std::env::temp_dir().join(format!(
            "abs_outside_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&outside).unwrap();
        assert!(validate_deletable_path(&outside).is_err());

        let _ = fs::remove_dir_all(&base);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn validate_deletable_path_rejects_symlink_escape() {
        let _guard = ROOTS_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!(
            "abs_symlink_safety_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let packages = base.join("packages");
        let chroot = base.join("chroot");
        let ready = base.join("ready");
        let outside = base.join("outside");
        fs::create_dir_all(&packages).unwrap();
        fs::create_dir_all(&chroot).unwrap();
        fs::create_dir_all(&ready).unwrap();
        fs::create_dir_all(&outside).unwrap();

        init_deletable_roots(
            packages.to_str().unwrap(),
            chroot.to_str().unwrap(),
            ready.to_str().unwrap(),
            &[],
        )
        .unwrap();

        let trap = packages.join("trap");
        symlink(&outside, &trap).unwrap();
        assert!(validate_deletable_path(&trap).is_err());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn validate_config_path_rejects_system_root() {
        assert!(validate_config_path("paths.packages_path", "/").is_err());
        assert!(validate_config_path("paths.packages_path", "/usr").is_err());
    }

    #[test]
    fn exit_pause_skipped_when_not_requested() {
        EXIT_PAUSE_REQUESTED.store(false, Ordering::Relaxed);
        SKIP_EXIT_PAUSE.store(false, Ordering::Relaxed);
        wait_before_exit_if_needed();
    }

    #[test]
    fn exit_pause_skipped_when_disabled() {
        EXIT_PAUSE_REQUESTED.store(true, Ordering::Relaxed);
        SKIP_EXIT_PAUSE.store(true, Ordering::Relaxed);
        wait_before_exit_if_needed();
        SKIP_EXIT_PAUSE.store(false, Ordering::Relaxed);
        EXIT_PAUSE_REQUESTED.store(false, Ordering::Relaxed);
    }
}
