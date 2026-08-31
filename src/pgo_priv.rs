//! Passwordless sudo helper for PGO auto-resume after reboot.
//!
//! Auto-restart installs `/etc/sudoers.d/abs-pgo-<uid>` so this user may run
//! `abs --pgo-priv -- CMD…` without a password. The helper (this module) is the
//! only NOPASSWD target; it rejects anything outside a PGO-oriented allowlist.
//! The drop-in is removed when no auto-resume unit remains (pipeline done or abort),
//! after ramdisk/zram teardown so passwordless sudo still works for that cleanup, on
//! poweroff after `shutdown_after_finish`, or via `--purge`. Unrelated abs commands
//! such as `-R` must not touch it.

use crate::blog;
use crate::utils::{run_command, sh_single_quote, write_file_mode};
use std::ffi::CString;
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

static CLIENT_ENABLED: AtomicBool = AtomicBool::new(false);

/// Page cache + dentries + inodes. Scored comparisons and profiling must start cold.
pub(crate) const DROP_CACHES_SH: &str = "sync; echo 3 > /proc/sys/vm/drop_caches";

pub fn take_cli_args<I, S>(args: I) -> Option<Vec<String>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args: Vec<String> = args.into_iter().map(|s| s.as_ref().to_string()).collect();
    let i = args.iter().position(|a| a == "--pgo-priv")?;
    let mut rest = args[i + 1..].to_vec();
    if rest.first().map(String::as_str) == Some("--") {
        rest.remove(0);
    }
    Some(rest)
}

/// Entry point for `abs --pgo-priv -- …`. Returns a process exit code.
pub fn main_as_root(args: &[String]) -> i32 {
    if unsafe { libc::geteuid() } != 0 {
        eprintln!(
            "==> ERROR: abs --pgo-priv must be run via sudo (it is the PGO auto-resume helper)"
        );
        return 1;
    }
    match run_validated(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("==> ERROR: {e}");
            1
        }
    }
}

pub fn client_enabled() -> bool {
    CLIENT_ENABLED.load(Ordering::Relaxed)
}

/// If the sudoers drop-in is already installed, route later `sudo` through this helper.
pub fn try_enable_client() -> bool {
    if client_enabled() {
        return true;
    }
    if crate::is_dry_run_mode() {
        return false;
    }
    let Some(abs_bin) = abs_bin_path() else {
        return false;
    };
    let Some(real_sudo) = real_sudo_path() else {
        return false;
    };
    if !probe_nopasswd(real_sudo, &abs_bin) {
        return false;
    }
    if install_path_wrapper(real_sudo, &abs_bin).is_err() {
        return false;
    }
    CLIENT_ENABLED.store(true, Ordering::Relaxed);
    blog!("PGO auto-resume: using passwordless sudo helper");
    true
}

pub fn sudoers_dropin_path(uid: u32) -> PathBuf {
    PathBuf::from(format!("/etc/sudoers.d/{}", sudoers_dropin_filename(uid)))
}

pub fn sudoers_dropin_filename(uid: u32) -> String {
    format!("abs-pgo-{uid}")
}

pub fn is_sudoers_dropin_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    path.parent() == Some(Path::new("/etc/sudoers.d"))
        && name.starts_with("abs-pgo-")
        && name
            .strip_prefix("abs-pgo-")
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
}

pub fn sudoers_dropin_text(uid: u32, abs_bin: &str) -> Result<String, String> {
    if !abs_bin.starts_with('/') {
        return Err(format!(
            "abs binary path for sudoers must be absolute (got {abs_bin})"
        ));
    }
    if abs_bin.chars().any(|c| matches!(c, '\n' | '\r' | '#')) {
        return Err("abs binary path contains characters that cannot go in sudoers".into());
    }
    let quoted = sudoers_quote_path(abs_bin);
    Ok(format!(
        "# ABS PGO auto-resume — passwordless helper. Removed when the pipeline ends.\n\
         Defaults:#{uid} !requiretty\n\
         #{uid} ALL=(root) NOPASSWD: {quoted} --pgo-priv *\n"
    ))
}

pub fn sudoers_quote_path(path: &str) -> String {
    let mut out = String::new();
    for c in path.chars() {
        if matches!(c, '\\' | ' ' | ',' | ':' | '=' | '"') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

pub fn probe_argv(abs_bin: &str) -> Vec<String> {
    vec![
        "-n".into(),
        abs_bin.into(),
        "--pgo-priv".into(),
        "--".into(),
        "true".into(),
    ]
}

pub fn sudo_wrapper_script(real_sudo: &str, abs_bin: &str) -> String {
    format!(
        "#!/bin/sh\nexec {} -n {} --pgo-priv -- \"$@\"\n",
        sh_single_quote(real_sudo),
        sh_single_quote(abs_bin)
    )
}

/// Write `/etc/sudoers.d/abs-pgo-<uid>` (one password if the timestamp is cold).
pub fn install_dropin() -> Result<(), String> {
    let uid = invoking_uid();
    let abs_bin = abs_bin_path().ok_or_else(|| "could not resolve abs binary path".to_string())?;
    let text = sudoers_dropin_text(uid, &abs_bin)?;
    let dest = sudoers_dropin_path(uid);
    if crate::is_dry_run_mode() {
        println!("[DRY RUN] install {} ({})", dest.display(), text.trim());
        return Ok(());
    }
    let tmp_dir = std::env::temp_dir();
    let tmp = tmp_dir.join(format!("{}-{uid}.tmp", sudoers_dropin_filename(uid)));
    std::fs::write(&tmp, &text).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    let tmp_s = tmp.to_string_lossy().into_owned();
    let dest_s = dest.to_string_lossy().into_owned();
    let visudo = run_command("visudo", &["-c", "-f", tmp_s.as_str()], None::<&str>);
    if let Err(e) = visudo {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("visudo rejected PGO sudoers drop-in: {e}"));
    }
    let install = run_command(
        "sudo",
        &[
            "install",
            "-m0440",
            "-o",
            "root",
            "-g",
            "root",
            tmp_s.as_str(),
            dest_s.as_str(),
        ],
        None::<&str>,
    );
    let _ = std::fs::remove_file(&tmp);
    install?;
    blog!(
        "Installed {} so PGO auto-resume can run after reboot without a sudo password",
        dest.display()
    );
    let _ = try_enable_client();
    Ok(())
}

/// Remove the drop-in when no `abs-pgo@` auto-resume unit is still enabled.
/// Call after ramdisk/zram teardown on the PGO CLI path — not from generic process
/// exit, and not while the completing resume still needs passwordless sudo.
pub fn maybe_remove_dropin() {
    if resume_unit_enabled() {
        return;
    }
    let _ = remove_dropin();
}

pub fn remove_dropin() -> Result<(), String> {
    let dest = sudoers_dropin_path(invoking_uid());
    if crate::is_dry_run_mode() {
        println!("[DRY RUN] rm -f {}", dest.display());
        return Ok(());
    }
    if unsafe { libc::geteuid() } == 0 {
        match std::fs::remove_file(&dest) {
            Ok(()) => blog!("Removed {}", dest.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("remove {}: {e}", dest.display())),
        }
        return Ok(());
    }
    let Some(abs_bin) = abs_bin_path() else {
        return Ok(());
    };
    let Some(real_sudo) = real_sudo_path() else {
        return Ok(());
    };
    let dest_s = dest.to_string_lossy().into_owned();
    let ok = Command::new(real_sudo)
        .args([
            "-n",
            abs_bin.as_str(),
            "--pgo-priv",
            "--",
            "rm",
            "-f",
            dest_s.as_str(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        blog!("Removed {}", dest.display());
    }
    Ok(())
}

/// Parse and check helper argv (after `abs --pgo-priv --`). Does not execute.
pub fn validate_priv_argv(args: &[String]) -> Result<ValidatedPriv, String> {
    let parsed = parse_sudo_style(args)?;
    if parsed.argv.is_empty() {
        if parsed.user.is_some() {
            return Err("abs --pgo-priv -u requires a command".into());
        }
        return Ok(parsed);
    }
    if parsed.user.as_deref() == Some("root") {
        return Err("abs --pgo-priv refuses -u root".into());
    }
    if parsed.user.is_some() {
        // Dropping to the invoking user (or nobody): they could already run this command.
        return Ok(parsed);
    }
    validate_command(&parsed.argv)?;
    Ok(parsed)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedPriv {
    pub set_home: bool,
    pub user: Option<String>,
    pub argv: Vec<String>,
}

fn run_validated(args: &[String]) -> Result<i32, String> {
    let parsed = validate_priv_argv(args)?;
    if parsed.argv.is_empty() {
        return Ok(0);
    }
    if parsed.user.is_none() && is_poweroff_argv(&parsed.argv) {
        return run_privileged_poweroff();
    }
    let mut command = Command::new(&parsed.argv[0]);
    command.args(&parsed.argv[1..]);
    command.stdin(Stdio::inherit());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());
    if let Some(ref user) = parsed.user {
        let (uid, gid, home) = lookup_user(user)?;
        if uid == 0 {
            return Err("abs --pgo-priv refuses -u root".into());
        }
        if !drop_user_allowed(user, uid) {
            return Err(format!(
                "abs --pgo-priv -u {user} is not the invoking user (SUDO_USER)"
            ));
        }
        if parsed.set_home {
            command.env("HOME", home);
        }
        command.uid(uid);
        command.gid(gid);
    }
    let status = command
        .status()
        .map_err(|e| format!("failed to execute {}: {e}", parsed.argv[0]))?;
    Ok(status.code().unwrap_or(1))
}

fn parse_sudo_style(args: &[String]) -> Result<ValidatedPriv, String> {
    let mut set_home = false;
    let mut user = None;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "--" => {
                i += 1;
                break;
            }
            "-n" | "--non-interactive" | "-A" | "--askpass" | "-E" | "--preserve-env" | "-v"
            | "--validate" | "-k" | "--reset-timestamp" => {
                i += 1;
            }
            "-H" | "--set-home" => {
                set_home = true;
                i += 1;
            }
            "-u" | "--user" => {
                let name = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{a} requires a user name"))?;
                user = Some(name.clone());
                i += 2;
            }
            s if s.starts_with("--user=") => {
                user = Some(s["--user=".len()..].to_string());
                i += 1;
            }
            s if s.starts_with('-') => {
                return Err(format!("abs --pgo-priv rejected sudo flag {s}"));
            }
            _ => break,
        }
    }
    let argv: Vec<String> = args[i..].to_vec();
    if argv
        .iter()
        .any(|a| a.contains('\0') || a.contains('\n') || a.contains('\r'))
    {
        return Err("abs --pgo-priv rejected an argument with a newline".into());
    }
    if argv.first().map(String::as_str).is_some_and(is_true_bin) {
        return Ok(ValidatedPriv {
            set_home,
            user,
            argv: Vec::new(),
        });
    }
    Ok(ValidatedPriv {
        set_home,
        user,
        argv,
    })
}

fn is_true_bin(cmd: &str) -> bool {
    matches!(command_basename(cmd).as_str(), "true" | "gtrue")
}

fn command_basename(cmd: &str) -> String {
    Path::new(cmd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(cmd)
        .to_string()
}

fn validate_command(argv: &[String]) -> Result<(), String> {
    let cmd = argv.first().ok_or("abs --pgo-priv: missing command")?;
    let base = command_basename(cmd);
    match base.as_str() {
        "mkdir" => validate_mkdir_args(&argv[1..]),
        "chown" => validate_chown(&argv[1..]),
        "chmod" => validate_chmod(&argv[1..]),
        "mount" => validate_mount(&argv[1..]),
        "umount" => validate_umount(&argv[1..]),
        "pacman" => validate_pacman(&argv[1..]),
        "rm" => validate_rm(&argv[1..]),
        "cp" | "mv" | "ln" | "touch" | "rsync" | "install" | "tee" | "mkarchroot"
        | "makechrootpkg" | "arch-nspawn" | "systemd-nspawn" => validate_generic_paths(argv),
        "cat" => validate_cat(&argv[1..]),
        "perf" => validate_perf(&argv[1..]),
        "sysctl" => validate_sysctl(&argv[1..]),
        "sh" | "bash" => validate_shell(argv),
        "bootctl" => validate_bootctl(&argv[1..]),
        "systemctl" => validate_systemctl(&argv[1..]),
        "reboot" => validate_reboot(&argv[1..]),
        "poweroff" => validate_reboot(&argv[1..]),
        "grub-reboot" | "grub2-reboot" => validate_grub_reboot(&argv[1..]),
        "modprobe" => validate_modprobe(&argv[1..]),
        "zramctl" => validate_zramctl(&argv[1..]),
        "mkswap" => validate_mkswap(&argv[1..]),
        "swapon" | "swapoff" => validate_swap_device(cmd, &argv[1..]),
        _ if looks_like_sysctl_script(cmd) => validate_sysctl_script(cmd, &argv[1..]),
        other => Err(format!("abs --pgo-priv does not allow command {other:?}")),
    }
}

fn validate_mkdir_args(args: &[String]) -> Result<(), String> {
    let paths: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| !matches!(*a, "-p" | "-v"))
        .collect();
    if paths.is_empty() {
        return Err("mkdir requires a path".into());
    }
    for p in paths {
        path_writable_for_pgo(p)?;
    }
    Ok(())
}

fn validate_chown(args: &[String]) -> Result<(), String> {
    let mut i = 0;
    while i < args.len() && is_chown_option(&args[i]) {
        i += 1;
    }
    if args.get(i).map(String::as_str) == Some("--") {
        i += 1;
    }
    let rest = &args[i..];
    if rest.len() < 2 {
        return Err("chown requires owner and path".into());
    }
    let owner = rest[0].as_str();
    if owner.starts_with('-')
        || !owner
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-' | '.'))
    {
        return Err(format!("abs --pgo-priv rejected chown owner {owner}"));
    }
    for p in &rest[1..] {
        path_writable_for_pgo(p)?;
    }
    Ok(())
}

/// GNU `chown` short options used by ABS (`-hR` after `perf record`) and ramdisk setup (`-R`).
fn is_chown_option(arg: &str) -> bool {
    matches!(
        arg,
        "-h" | "-R"
            | "-c"
            | "-f"
            | "-v"
            | "-H"
            | "-L"
            | "-P"
            | "--no-dereference"
            | "--recursive"
            | "--changes"
            | "--silent"
            | "--quiet"
            | "--verbose"
    ) || (arg.starts_with('-')
        && !arg.starts_with("--")
        && arg.len() > 1
        && arg
            .bytes()
            .skip(1)
            .all(|b| matches!(b, b'h' | b'R' | b'c' | b'f' | b'v' | b'H' | b'L' | b'P')))
}

fn validate_chmod(args: &[String]) -> Result<(), String> {
    let mut i = 0;
    while i < args.len() && is_chmod_option(&args[i]) {
        i += 1;
    }
    if args.get(i).map(String::as_str) == Some("--") {
        i += 1;
    }
    let rest = &args[i..];
    if rest.len() < 2 {
        return Err("chmod requires mode and path".into());
    }
    let mode = rest[0].as_str();
    if !mode.chars().all(|c| c.is_ascii_digit()) || !(3..=4).contains(&mode.len()) {
        return Err(format!("abs --pgo-priv rejected chmod mode {mode}"));
    }
    for p in &rest[1..] {
        path_writable_for_pgo(p)?;
    }
    Ok(())
}

fn is_chmod_option(arg: &str) -> bool {
    matches!(
        arg,
        "-R" | "-c"
            | "-f"
            | "-v"
            | "--recursive"
            | "--changes"
            | "--silent"
            | "--quiet"
            | "--verbose"
    ) || (arg.starts_with('-')
        && !arg.starts_with("--")
        && arg.len() > 1
        && arg
            .bytes()
            .skip(1)
            .all(|b| matches!(b, b'R' | b'c' | b'f' | b'v')))
}

fn validate_mount(args: &[String]) -> Result<(), String> {
    let joined: Vec<&str> = args.iter().map(String::as_str).collect();
    if !joined.contains(&"-t") || !joined.iter().any(|a| *a == "tmpfs") {
        return Err("abs --pgo-priv only allows mounting tmpfs".into());
    }
    let Some(target) = joined.last() else {
        return Err("mount requires a target".into());
    };
    if *target == "tmpfs" || target.starts_with('-') {
        return Err("mount requires an abs* target directory".into());
    }
    abs_named_dir(target)
}

fn validate_umount(args: &[String]) -> Result<(), String> {
    let rest: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "-l" && *a != "-f" && *a != "-v")
        .collect();
    let Some(target) = rest.last() else {
        return Err("umount requires a path".into());
    };
    abs_named_dir(target)
}

fn validate_pacman(args: &[String]) -> Result<(), String> {
    for a in args {
        let s = a.as_str();
        if matches!(
            s,
            "-Syu"
                | "-Syyu"
                | "-Su"
                | "-Syy"
                | "--sysupgrade"
                | "-R"
                | "--remove"
                | "-D"
                | "--database"
        ) || s.starts_with("--config")
        {
            return Err(format!("abs --pgo-priv rejected pacman flag {s}"));
        }
        if (s == "-r" || s == "--root")
            && let Some(idx) = args.iter().position(|x| x == a)
            && let Some(root) = args.get(idx + 1)
            && path_has_abs_component(Path::new(root)).is_err()
        {
            return Err("pacman --root must be an ABS path".into());
        }
    }
    let mut saw_op = false;
    for a in args {
        if a == "-U" || a == "--upgrade" {
            saw_op = true;
        }
        if a == "-S" || a == "--sync" || a == "-Sy" {
            saw_op = true;
        }
        if a.starts_with('-') {
            continue;
        }
        if a.ends_with(".pkg.tar.zst")
            || a.ends_with(".pkg.tar.xz")
            || a.ends_with(".pkg.tar.gz")
            || a.ends_with(".pkg.tar.bz2")
        {
            path_writable_for_pgo(a)?;
            saw_op = true;
            continue;
        }
        if !valid_pacman_pkg_name(a) {
            return Err(format!("abs --pgo-priv rejected pacman argument {a}"));
        }
    }
    if !saw_op {
        return Err("abs --pgo-priv pacman requires -S or -U".into());
    }
    Ok(())
}

fn validate_rm(args: &[String]) -> Result<(), String> {
    let mut paths = Vec::new();
    for a in args {
        if a.starts_with('-') {
            if !matches!(a.as_str(), "-r" | "-f" | "-rf" | "-fr" | "-v") {
                return Err(format!("abs --pgo-priv rejected rm flag {a}"));
            }
            continue;
        }
        paths.push(a.as_str());
    }
    if paths.is_empty() {
        return Err("rm requires a path".into());
    }
    for p in paths {
        if is_sudoers_dropin_path(Path::new(p)) {
            continue;
        }
        path_writable_for_pgo(p)?;
        if is_blocked_root(Path::new(p)) {
            return Err(format!("abs --pgo-priv refused to rm {p}"));
        }
    }
    Ok(())
}

fn validate_generic_paths(argv: &[String]) -> Result<(), String> {
    for a in argv.iter().skip(1) {
        if a.starts_with('-') {
            continue;
        }
        if a.contains("..") {
            return Err("abs --pgo-priv rejected a path containing ..".into());
        }
        if a.starts_with('/') {
            let trimmed = a.trim_end_matches('/');
            if is_sudoers_dropin_path(Path::new(trimmed)) {
                continue;
            }
            path_writable_for_pgo(trimmed)?;
        }
    }
    Ok(())
}

fn validate_cat(args: &[String]) -> Result<(), String> {
    if args.len() != 1 {
        return Err("cat requires exactly one path".into());
    }
    let p = Path::new(&args[0]);
    let ok = args[0].ends_with("grub.cfg")
        || args[0].ends_with("limine.conf")
        || path_under(p, Path::new("/boot"))
        || path_under(p, Path::new("/efi"))
        || path_under(p, Path::new("/boot/efi"));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "abs --pgo-priv cat is limited to bootloader configs (got {})",
            args[0]
        ))
    }
}

fn validate_perf(args: &[String]) -> Result<(), String> {
    if args.first().map(String::as_str) != Some("record") {
        return Err("abs --pgo-priv only allows `perf record`".into());
    }
    Ok(())
}

fn validate_sysctl(args: &[String]) -> Result<(), String> {
    let mut i = 0;
    if args.first().map(String::as_str) == Some("-w") {
        i = 1;
    }
    let Some(assign) = args.get(i) else {
        return Err("sysctl -w requires an assignment".into());
    };
    let Some((key, val)) = assign.split_once('=') else {
        return Err("sysctl assignment must be key=value".into());
    };
    if !matches!(key, "kernel.kptr_restrict" | "kernel.perf_event_paranoid") {
        return Err(format!("abs --pgo-priv rejected sysctl key {key}"));
    }
    if val.parse::<i32>().is_err() {
        return Err(format!("abs --pgo-priv rejected sysctl value {val}"));
    }
    if args.len() > i + 1 {
        return Err("abs --pgo-priv sysctl takes one assignment".into());
    }
    Ok(())
}

fn validate_shell(argv: &[String]) -> Result<(), String> {
    if argv.get(1).map(String::as_str) != Some("-c") {
        return Err("abs --pgo-priv only allows `sh -c` for dropping caches".into());
    }
    let Some(script) = argv.get(2) else {
        return Err("sh -c requires a script".into());
    };
    if script.trim() != DROP_CACHES_SH {
        return Err("abs --pgo-priv rejected this shell snippet".into());
    }
    if argv.len() > 3 {
        return Err("abs --pgo-priv sh -c takes a single script".into());
    }
    Ok(())
}

fn validate_bootctl(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("status") if args.len() == 1 => Ok(()),
        Some("list") => {
            if args.len() == 1 || args == ["list", "--json=short"] {
                Ok(())
            } else {
                Err("abs --pgo-priv rejected bootctl list flags".into())
            }
        }
        Some("set-oneshot") if args.len() == 2 && boot_id_ok(&args[1]) => Ok(()),
        _ => Err("abs --pgo-priv only allows bootctl status|list|set-oneshot".into()),
    }
}

fn validate_systemctl(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("reboot") => {
            for a in &args[1..] {
                if let Some(id) = a.strip_prefix("--boot-loader-entry=") {
                    if !boot_id_ok(id) {
                        return Err(format!("abs --pgo-priv rejected boot entry {id}"));
                    }
                    continue;
                }
                return Err(format!("abs --pgo-priv rejected systemctl flag {a}"));
            }
            Ok(())
        }
        Some("poweroff") if args.len() == 1 => Ok(()),
        _ => Err("abs --pgo-priv only allows `systemctl reboot` or `systemctl poweroff`".into()),
    }
}

fn is_poweroff_argv(argv: &[String]) -> bool {
    match argv {
        [cmd, action] if cmd == "systemctl" && action == "poweroff" => true,
        [cmd] if cmd == "poweroff" => true,
        _ => false,
    }
}

/// Remove the auto-resume sudoers drop-in, then power off. One root invocation so
/// unattended PGO can halt after the helper is gone.
fn run_privileged_poweroff() -> Result<i32, String> {
    let dest = sudoers_dropin_path(invoking_uid());
    match std::fs::remove_file(&dest) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            eprintln!("==> WARNING: could not remove {}: {e}", dest.display());
        }
    }
    let status = Command::new("systemctl")
        .arg("poweroff")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| format!("failed to execute systemctl poweroff: {e}"))?;
    if status.success() {
        return Ok(0);
    }
    let fallback = Command::new("poweroff")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| format!("failed to execute poweroff: {e}"))?;
    Ok(fallback.code().unwrap_or(1))
}

fn validate_reboot(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        Ok(())
    } else {
        Err("abs --pgo-priv reboot takes no arguments".into())
    }
}

fn validate_grub_reboot(args: &[String]) -> Result<(), String> {
    if args.len() == 1 && boot_id_ok(&args[0]) {
        Ok(())
    } else {
        Err("abs --pgo-priv grub-reboot requires a single boot id".into())
    }
}

fn validate_modprobe(args: &[String]) -> Result<(), String> {
    if args == ["zram"] {
        Ok(())
    } else {
        Err("abs --pgo-priv only allows `modprobe zram`".into())
    }
}

fn zramctl_size_ok(s: &str) -> bool {
    let b = s.as_bytes();
    if b.is_empty() {
        return false;
    }
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 {
        return false;
    }
    matches!(
        &s[i..],
        "" | "K" | "k" | "M" | "m" | "G" | "g" | "T" | "t" | "KiB" | "MiB" | "GiB"
    )
}

fn validate_zramctl(args: &[String]) -> Result<(), String> {
    let mut i = 0;
    let mut saw_find = false;
    let mut reset_dev: Option<&str> = None;
    while i < args.len() {
        match args[i].as_str() {
            "--find" | "-f" => {
                saw_find = true;
                i += 1;
            }
            "--size" | "-s" => {
                let Some(sz) = args.get(i + 1) else {
                    return Err("zramctl --size requires a value".into());
                };
                if !zramctl_size_ok(sz) {
                    return Err(format!("abs --pgo-priv rejected zramctl size {sz}"));
                }
                i += 2;
            }
            "--algorithm" | "-a" => {
                let Some(alg) = args.get(i + 1).map(String::as_str) else {
                    return Err("zramctl --algorithm requires a value".into());
                };
                if alg != "zstd" {
                    return Err(format!("abs --pgo-priv rejected zramctl algorithm {alg}"));
                }
                i += 2;
            }
            "--reset" | "-r" => {
                let Some(dev) = args.get(i + 1).map(String::as_str) else {
                    return Err("zramctl --reset requires a device".into());
                };
                if !crate::zram::is_zram_dev(dev) {
                    return Err(format!("abs --pgo-priv rejected zramctl device {dev}"));
                }
                reset_dev = Some(dev);
                i += 2;
            }
            other if crate::zram::is_zram_dev(other) => {
                i += 1;
            }
            other => {
                return Err(format!("abs --pgo-priv rejected zramctl argument {other}"));
            }
        }
    }
    if saw_find || reset_dev.is_some() {
        Ok(())
    } else {
        Err("abs --pgo-priv zramctl requires --find or --reset".into())
    }
}

fn validate_mkswap(args: &[String]) -> Result<(), String> {
    if args.len() == 3 && args[0] == "-L" && crate::zram::is_abs_pgo_label(&args[1]) {
        if crate::zram::is_zram_dev(&args[2]) {
            return Ok(());
        }
        return Err(format!(
            "abs --pgo-priv mkswap device must be /dev/zramN (got {})",
            args[2]
        ));
    }
    Err("abs --pgo-priv only allows `mkswap -L abs-pgo /dev/zramN`".into())
}

fn validate_swap_device(cmd: &str, args: &[String]) -> Result<(), String> {
    let paths: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| !a.starts_with('-'))
        .collect();
    if paths.len() != 1 {
        return Err(format!("abs --pgo-priv {cmd} requires one /dev/zramN"));
    }
    if crate::zram::is_zram_dev(paths[0]) {
        Ok(())
    } else {
        Err(format!(
            "abs --pgo-priv {cmd} is limited to /dev/zramN (got {})",
            paths[0]
        ))
    }
}

fn looks_like_sysctl_script(cmd: &str) -> bool {
    Path::new(cmd).is_absolute()
}

fn validate_sysctl_script(cmd: &str, args: &[String]) -> Result<(), String> {
    let path = Path::new(cmd);
    if path.components().any(|c| c == Component::ParentDir) {
        return Err("sysctl script path must not contain ..".into());
    }
    let home_ok = std::env::var("SUDO_USER")
        .ok()
        .filter(|u| !u.is_empty())
        .map(|u| PathBuf::from("/home").join(u));
    let allowed = home_ok.is_some_and(|h| path_under(path, &h))
        || path_under(path, Path::new("/usr/share/abs"));
    if !allowed {
        return Err(format!(
            "sysctl script must be under $HOME or /usr/share/abs (got {cmd})"
        ));
    }
    if args == ["enable"] || args == ["disable"] {
        Ok(())
    } else {
        Err("sysctl script only accepts enable|disable".into())
    }
}

fn boot_id_ok(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+' | '/'))
        && !id.contains("..")
}

fn valid_pacman_pkg_name(name: &str) -> bool {
    let b = name.as_bytes();
    if b.is_empty() || b.len() > 128 || b.iter().all(|&c| c == b'.') {
        return false;
    }
    b.iter().all(|c| {
        matches!(
            c,
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'@' | b'.' | b'_' | b'+' | b'-'
        )
    })
}

fn abs_named_dir(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.starts_with("abs") {
        Ok(())
    } else {
        Err(format!(
            "abs --pgo-priv mount/umount target must be named abs* (got {path})"
        ))
    }
}

fn path_writable_for_pgo(path: &str) -> Result<(), String> {
    if crate::zram::is_zram_mem_limit_sysfs(path) {
        return Ok(());
    }
    let p = Path::new(path);
    if is_blocked_root(p) {
        return Err(format!("abs --pgo-priv refused system path {path}"));
    }
    if path_under(p, Path::new("/etc")) && !is_sudoers_dropin_path(p) {
        return Err(format!("abs --pgo-priv refused path under /etc: {path}"));
    }
    if path_under(p, Path::new("/usr"))
        || path_under(p, Path::new("/boot"))
        || path_under(p, Path::new("/bin"))
        || path_under(p, Path::new("/sbin"))
        || path_under(p, Path::new("/lib"))
        || path_under(p, Path::new("/root"))
    {
        return Err(format!("abs --pgo-priv refused system path {path}"));
    }
    if path_under(p, Path::new("/home"))
        || path_under(p, Path::new("/tmp"))
        || path_under(p, Path::new("/run"))
        || path_under(p, Path::new("/var/tmp"))
        || path_under(p, Path::new("/opt"))
        || path_has_abs_component(p).is_ok()
        || path_has_pgo_convert_component(p)
    {
        return Ok(());
    }
    Err(format!(
        "abs --pgo-priv path must be under home/tmp/run, named abs*, or under pgo-convert: {path}"
    ))
}

fn path_has_pgo_convert_component(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == "pgo-convert")
}

fn path_has_abs_component(path: &Path) -> Result<(), String> {
    if path
        .components()
        .any(|c| c.as_os_str().to_str().is_some_and(|s| s.starts_with("abs")))
    {
        Ok(())
    } else {
        Err(format!("{} has no abs* path component", path.display()))
    }
}

fn path_under(path: &Path, prefix: &Path) -> bool {
    let a = path.components();
    let b = prefix.components();
    let mut ai = a;
    for bc in b {
        match ai.next() {
            Some(ac) if ac == bc => {}
            _ => return false,
        }
    }
    true
}

fn is_blocked_root(path: &Path) -> bool {
    matches!(
        path.to_str(),
        Some("/")
            | Some("/usr")
            | Some("/etc")
            | Some("/bin")
            | Some("/sbin")
            | Some("/home")
            | Some("/root")
            | Some("/var")
            | Some("/boot")
            | Some("/proc")
            | Some("/sys")
            | Some("/dev")
            | Some("/run")
            | Some("/tmp")
    )
}

fn drop_user_allowed(name: &str, uid: u32) -> bool {
    if let Ok(sudo_uid) = std::env::var("SUDO_UID")
        && sudo_uid.parse::<u32>() == Ok(uid)
    {
        return true;
    }
    if let Ok(sudo_user) = std::env::var("SUDO_USER")
        && sudo_user == name
    {
        return true;
    }
    name == "nobody"
}

fn lookup_user(name: &str) -> Result<(u32, u32, PathBuf), String> {
    let cname = CString::new(name.as_bytes()).map_err(|_| "invalid user name".to_string())?;
    let pw = unsafe { libc::getpwnam(cname.as_ptr()) };
    if pw.is_null() {
        return Err(format!("unknown user {name}"));
    }
    let pw = unsafe { &*pw };
    let home = unsafe { std::ffi::CStr::from_ptr(pw.pw_dir) }
        .to_string_lossy()
        .into_owned();
    Ok((pw.pw_uid, pw.pw_gid, PathBuf::from(home)))
}

fn invoking_uid() -> u32 {
    if let Ok(s) = std::env::var("SUDO_UID")
        && let Ok(u) = s.parse::<u32>()
    {
        return u;
    }
    unsafe { libc::getuid() }
}

fn abs_bin_path() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let resolved = std::fs::canonicalize(&exe).unwrap_or(exe);
    let s = resolved.to_string_lossy().into_owned();
    if s.starts_with('/') { Some(s) } else { None }
}

fn real_sudo_path() -> Option<&'static str> {
    ["/usr/bin/sudo", "/bin/sudo"]
        .into_iter()
        .find(|p| Path::new(p).is_file())
}

fn probe_nopasswd(real_sudo: &str, abs_bin: &str) -> bool {
    Command::new(real_sudo)
        .args(probe_argv(abs_bin))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn install_path_wrapper(real_sudo: &str, abs_bin: &str) -> Result<(), String> {
    let cache = dirs::cache_dir().ok_or_else(|| "no cache dir".to_string())?;
    let dir = cache.join("abs").join("sudo-pgo");
    let wrapper = dir.join("sudo");
    write_file_mode(&wrapper, &sudo_wrapper_script(real_sudo, abs_bin), 0o700)?;
    let dir_s = dir.to_string_lossy().into_owned();
    let path = std::env::var("PATH").unwrap_or_default();
    if path.split(':').next() == Some(dir_s.as_str()) {
        return Ok(());
    }
    let new_path = if path.is_empty() {
        dir_s
    } else {
        format!("{dir_s}:{path}")
    };
    unsafe {
        std::env::set_var("PATH", new_path);
    }
    Ok(())
}

fn resume_unit_enabled() -> bool {
    let Some(config) = dirs::config_dir() else {
        return false;
    };
    let unit_dir = config.join("systemd").join("user");
    for wants in ["default.target.wants", "graphical-session.target.wants"] {
        let dir = unit_dir.join(wants);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        if entries
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("abs-pgo@"))
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// Example argv only — production matches path *rules* (home, /run, abs*-named dirs),
    /// not a specific user or ramdisk mount.

    fn v(args: &[&str]) -> Result<ValidatedPriv, String> {
        validate_priv_argv(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn take_cli_args_reads_after_double_dash() {
        let args = take_cli_args([
            "--pgo-resume",
            "pkg",
            "--pgo-priv",
            "--",
            "pacman",
            "-U",
            "a",
        ]);
        assert_eq!(args.unwrap(), vec!["pacman", "-U", "a"]);
    }

    #[test]
    fn take_cli_args_none_without_flag() {
        assert!(take_cli_args(["--pgo-resume", "pkg"]).is_none());
    }

    #[test]
    fn visudo_accepts_generated_dropin() {
        let text = sudoers_dropin_text(1000, "/usr/bin/abs").unwrap();
        let tmp = std::env::temp_dir().join(format!(
            "abs-pgo-visudo-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&tmp, &text).unwrap();
        let status = Command::new("visudo")
            .args(["-c", "-f"])
            .arg(&tmp)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = std::fs::remove_file(&tmp);
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => panic!("visudo -c rejected generated sudoers (exit {s}): {text}"),
            Err(_) => {}
        }
    }

    #[test]
    fn sudoers_dropin_filename_has_no_dot() {
        let name = sudoers_dropin_filename(1000);
        assert_eq!(name, "abs-pgo-1000");
        assert!(!name.contains('.'));
        assert!(is_sudoers_dropin_path(&PathBuf::from(
            "/etc/sudoers.d/abs-pgo-1000"
        )));
        assert!(!is_sudoers_dropin_path(Path::new(
            "/etc/sudoers.d/abs-pgo-1000.bak"
        )));
        assert!(!is_sudoers_dropin_path(Path::new("/tmp/abs-pgo-1000")));
    }

    #[test]
    fn sudoers_text_pins_uid_and_helper_glob() {
        let text = sudoers_dropin_text(1000, "/usr/bin/abs").unwrap();
        assert!(text.contains("Defaults:#1000 !requiretty"), "{text}");
        assert!(
            text.contains("#1000 ALL=(root) NOPASSWD: /usr/bin/abs --pgo-priv *"),
            "{text}"
        );
        assert!(!text.contains("NOPASSWD: ALL"), "{text}");
    }

    #[test]
    fn sudoers_rejects_relative_abs_path() {
        assert!(sudoers_dropin_text(1, "abs").is_err());
    }

    #[test]
    fn sudoers_quotes_spaces_in_abs_path() {
        let q = sudoers_quote_path("/opt/my abs/abs");
        assert_eq!(q, "/opt/my\\ abs/abs");
    }

    #[test]
    fn wrapper_script_uses_noninteractive_helper() {
        let s = sudo_wrapper_script("/usr/bin/sudo", "/usr/bin/abs");
        assert!(s.contains("-n"), "{s}");
        assert!(s.contains("--pgo-priv"), "{s}");
        assert!(s.contains("\"$@\""), "{s}");
    }

    #[test]
    fn probe_argv_is_noninteractive_true() {
        assert_eq!(
            probe_argv("/usr/bin/abs"),
            vec!["-n", "/usr/bin/abs", "--pgo-priv", "--", "true"]
        );
    }

    #[test]
    fn validate_strips_sudo_flags_and_allows_ping() {
        assert!(v(&["-n", "-v"]).unwrap().argv.is_empty());
        assert!(v(&["-n", "--", "true"]).unwrap().argv.is_empty());
        assert!(v(&["/usr/bin/true"]).unwrap().argv.is_empty());
    }

    #[test]
    fn validate_allows_tmpfs_mount_on_any_abs_named_dir() {
        v(&[
            "mount",
            "-t",
            "tmpfs",
            "-o",
            "size=16G,mode=0755",
            "tmpfs",
            "/run/abs-ram",
        ])
        .unwrap();
        v(&[
            "mount",
            "-t",
            "tmpfs",
            "-o",
            "size=8G",
            "tmpfs",
            "/mnt/abs-scratch",
        ])
        .unwrap();
        assert!(v(&["mount", "-t", "tmpfs", "tmpfs", "/run/tmpfs"]).is_err());
    }

    #[test]
    fn validate_rejects_mount_on_root() {
        assert!(v(&["mount", "-t", "tmpfs", "tmpfs", "/"]).is_err());
        assert!(v(&["mount", "-t", "ext4", "/dev/sda1", "/mnt/abs-scratch"]).is_err());
    }

    #[test]
    fn validate_allows_umount_abs_named_dirs() {
        v(&["umount", "/run/abs-ram"]).unwrap();
        v(&["umount", "-l", "/mnt/abs-scratch"]).unwrap();
    }

    #[test]
    fn validate_allows_pacman_u_of_pkg_file() {
        v(&[
            "pacman",
            "-U",
            "--noconfirm",
            "/home/builder/ready/linux-cachyos-1-1-x86_64.pkg.tar.zst",
        ])
        .unwrap();
    }

    #[test]
    fn validate_allows_pacman_s_needed() {
        v(&["pacman", "-S", "--needed", "--noconfirm", "cmake"]).unwrap();
    }

    #[test]
    fn validate_rejects_pacman_syu() {
        assert!(v(&["pacman", "-Syu", "--noconfirm"]).is_err());
        assert!(v(&["pacman", "-R", "sudo"]).is_err());
        assert!(v(&["pacman", "--config", "/tmp/x", "-U", "a.pkg.tar.zst"]).is_err());
    }

    #[test]
    fn validate_allows_perf_record() {
        v(&["perf", "record", "-b", "-o", "/tmp/perf.data", "--", "true"]).unwrap();
    }

    #[test]
    fn validate_rejects_perf_script() {
        assert!(v(&["perf", "script"]).is_err());
    }

    #[test]
    fn validate_allows_bootctl_and_reboot() {
        v(&["bootctl", "status"]).unwrap();
        v(&["bootctl", "list", "--json=short"]).unwrap();
        v(&["bootctl", "set-oneshot", "linux-cachyos"]).unwrap();
        v(&["systemctl", "reboot", "--boot-loader-entry=linux-cachyos"]).unwrap();
        v(&["reboot"]).unwrap();
        v(&["grub-reboot", "1"]).unwrap();
        v(&["systemctl", "poweroff"]).unwrap();
        v(&["poweroff"]).unwrap();
        assert!(v(&["systemctl", "halt"]).is_err());
        assert!(v(&["systemctl", "poweroff", "--firmware-setup"]).is_err());
    }

    #[test]
    fn poweroff_argv_matches_systemctl_and_bare_poweroff() {
        assert!(is_poweroff_argv(&["systemctl".into(), "poweroff".into()]));
        assert!(is_poweroff_argv(&["poweroff".into()]));
        assert!(!is_poweroff_argv(&["systemctl".into(), "reboot".into()]));
    }

    #[test]
    fn validate_allows_drop_caches_sh() {
        v(&["sh", "-c", DROP_CACHES_SH]).unwrap();
    }

    #[test]
    fn validate_rejects_arbitrary_shell() {
        assert!(v(&["sh", "-c", "id"]).is_err());
        assert!(v(&["bash", "-c", "chmod 777 /etc/shadow"]).is_err());
    }

    #[test]
    fn validate_allows_sysctl_pgo_keys() {
        v(&["sysctl", "-w", "kernel.kptr_restrict=0"]).unwrap();
        v(&["sysctl", "-w", "kernel.perf_event_paranoid=-1"]).unwrap();
        assert!(v(&["sysctl", "-w", "kernel.sysrq=1"]).is_err());
    }

    #[test]
    fn validate_allows_rm_abs_ram_and_sudoers() {
        v(&["rm", "-rf", "/mnt/abs-scratch/work"]).unwrap();
        v(&["rm", "-f", "/etc/sudoers.d/abs-pgo-1000"]).unwrap();
    }

    #[test]
    fn validate_rejects_rm_root() {
        assert!(v(&["rm", "-rf", "/"]).is_err());
        assert!(v(&["rm", "-rf", "/etc"]).is_err());
        assert!(v(&["rm", "-f", "/etc/passwd"]).is_err());
    }

    #[test]
    fn validate_rejects_unknown_command() {
        assert!(v(&["vim", "/etc/passwd"]).is_err());
        assert!(v(&["-s"]).is_err());
    }

    #[test]
    fn validate_rejects_newline_injection() {
        assert!(v(&["mkdir", "-p", "/run/abs-ram\nchmod 777 /"]).is_err());
    }

    #[test]
    fn validate_rejects_cp_to_etc() {
        assert!(v(&["cp", "/tmp/x", "/etc/passwd"]).is_err());
        v(&[
            "cp",
            "/home/builder/makepkg.conf",
            "/mnt/abs-scratch/chroot/base/root/etc/makepkg.conf",
        ])
        .unwrap();
        v(&[
            "install",
            "-m0440",
            "/tmp/abs-pgo-1000.tmp",
            "/etc/sudoers.d/abs-pgo-1000",
        ])
        .unwrap();
    }

    #[test]
    fn validate_mkdir_abs_ram() {
        v(&["mkdir", "-p", "/run/abs-ram"]).unwrap();
        v(&["mkdir", "-p", "/var/tmp/abs-build"]).unwrap();
        assert!(v(&["mkdir", "-p", "/etc/evil"]).is_err());
    }

    #[test]
    fn validate_allows_chown_hr_cluster_used_after_perf_record() {
        v(&[
            "chown",
            "-hR",
            "john:john",
            "/run/abs-ram/pgo-scratch/linux-cachyos/kernel.data",
        ])
        .unwrap();
        v(&[
            "chown",
            "-hR",
            "john:john",
            "/run/abs-ram/pgo-scratch/linux-cachyos/propeller.data",
        ])
        .unwrap();
        v(&[
            "chown",
            "-Rh",
            "john:john",
            "/run/abs-ram/pgo-scratch/linux-cachyos/kernel.data",
        ])
        .unwrap();
        v(&[
            "chown",
            "-h",
            "-R",
            "john:john",
            "/run/abs-ram/pgo-scratch/linux-cachyos/kernel.data",
        ])
        .unwrap();
        v(&["chown", "-R", "1000:1000", "/run/abs-ram"]).unwrap();
        v(&["chown", "1000:1000", "/tmp/abs-pgo-benchmark.sh"]).unwrap();
        v(&["chmod", "-R", "755", "/run/abs-ram/pgo-scratch"]).unwrap();
        let err = v(&["chown", "-hR", "john:john", "/etc/passwd"]).unwrap_err();
        assert!(
            err.contains("/etc/passwd") || err.contains("refused"),
            "{err}"
        );
        assert!(
            v(&["chown", "-evil", "john:john", "/run/abs-ram/x"]).is_err(),
            "unknown clustered flags must not steal the owner token"
        );
    }

    #[test]
    fn validate_allows_convert_spill_outside_home_tmp_run() {
        // `{profiles_archive_dir}/pgo-convert/<pkg>/` is convert scratch, not an
        // abs*-named dir, and the archive may live on a data mount.
        v(&[
            "chown",
            "-hR",
            "john:john",
            "/media/storage/tmp/pgo-convert/linux-cachyos/propeller.data",
        ])
        .unwrap();
        v(&[
            "rm",
            "-f",
            "/media/storage/tmp/pgo-convert/linux-cachyos/propeller.data",
        ])
        .unwrap();
        v(&[
            "mkdir",
            "-p",
            "/media/storage/tmp/pgo-convert/linux-cachyos",
        ])
        .unwrap();
        let err = v(&[
            "chown",
            "-hR",
            "john:john",
            "/media/storage/tmp/linux-cachyos/propeller.data",
        ])
        .unwrap_err();
        assert!(
            err.contains("home/tmp/run") || err.contains("named abs"),
            "{err}"
        );
    }

    #[test]
    fn validate_allows_abs_pgo_zram_commands() {
        v(&["modprobe", "zram"]).unwrap();
        v(&["zramctl", "--find", "--size", "16G", "--algorithm", "zstd"]).unwrap();
        v(&["zramctl", "--reset", "/dev/zram0"]).unwrap();
        v(&["mkswap", "-L", "abs-pgo", "/dev/zram0"]).unwrap();
        v(&["swapon", "/dev/zram0"]).unwrap();
        v(&["swapoff", "/dev/zram0"]).unwrap();
        v(&["tee", "/sys/block/zram0/mem_limit"]).unwrap();
    }

    #[test]
    fn validate_rejects_non_abs_swap() {
        assert!(v(&["swapon", "/var/swapfile"]).is_err());
        assert!(v(&["swapon", "/dev/sda1"]).is_err());
        assert!(v(&["mkswap", "/dev/zram0"]).is_err());
        assert!(v(&["mkswap", "-L", "other", "/dev/zram0"]).is_err());
        assert!(v(&["modprobe", "zram", "num_devices=8"]).is_err());
        assert!(v(&["zramctl", "--reset", "/dev/sda"]).is_err());
    }

    #[test]
    fn validate_allows_privilege_drop_for_benchmark() {
        let got = v(&[
            "-H",
            "-u",
            "builder",
            "env",
            "ABS_PGO_BENCHMARK=fast",
            "bash",
            "/home/builder/.local/share/abs/pgo-benchmark.sh",
        ])
        .unwrap();
        assert_eq!(got.user.as_deref(), Some("builder"));
        assert!(got.set_home);
        assert_eq!(got.argv[0], "env");
        assert!(v(&["env", "FOO=1", "true"]).is_err());
        assert!(v(&["-u", "root", "id"]).is_err());
        assert!(v(&["-u", "builder"]).is_err());
    }

    #[test]
    fn sudoers_dropin_is_not_gc_from_ramdisk_shutdown() {
        // RamdiskShutdown Drop runs for every abs command (including `abs -R`).
        // The passwordless helper belongs to the PGO auto-resume lifecycle, not ramdisk.
        let ramdisk = include_str!("ramdisk.rs");
        assert!(
            !ramdisk.contains("maybe_remove_dropin"),
            "ramdisk::shutdown must not remove /etc/sudoers.d/abs-pgo-* (non-PGO commands share that path)"
        );
        let pgo = include_str!("pgo.rs");
        let start = pgo
            .find("pub fn remove_pgo_auto_resume_service")
            .expect("remove_pgo_auto_resume_service");
        let rest = &pgo[start..];
        let body_end = rest[1..]
            .find("\nfn ")
            .or_else(|| rest[1..].find("\npub fn "))
            .map(|i| i + 1)
            .unwrap_or(rest.len());
        let body = &rest[..body_end];
        assert!(
            !body.contains("maybe_remove_dropin") && !body.contains("remove_dropin"),
            "dropping sudoers while the resume process is still exiting breaks zram teardown: {body}"
        );
        let main = include_str!("main.rs");
        assert!(
            main.contains("pgo_priv::maybe_remove_dropin"),
            "PGO CLI must drop /etc/sudoers.d/abs-pgo-* after ramdisk/zram shutdown, when no resume unit remains"
        );
    }
}
