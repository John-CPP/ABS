use std::fs;
use std::path::Path;
use std::process::Command;

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

pub fn run_command<P: AsRef<Path>>(cmd: &str, args: &[&str], cwd: Option<P>) -> Result<(), String> {
    if crate::is_dry_run_mode() {
        let rendered_cmd = if args.is_empty() {
            cmd.to_string()
        } else {
            format!("{} {}", cmd, args.join(" "))
        };
        println!("[DRY RUN] {}", rendered_cmd);
        return Ok(());
    }

    let mut command = Command::new(cmd);
    command.args(args);

    if let Some(dir) = cwd {
        command.current_dir(dir);
    }

    // Use `.status()` so stdout/stderr are inherited and long builds (makepkg /
    // makechrootpkg) show live output like the Bash script does.
    let status = command
        .status()
        .map_err(|e| format!("Failed to execute '{}': {}", cmd, e))?;

    if status.success() {
        Ok(())
    } else {
        let rendered_cmd = if args.is_empty() {
            cmd.to_string()
        } else {
            format!("{} {}", cmd, args.join(" "))
        };
        Err(format!(
            "Command failed: `{}` (exit: {})\n(Output was printed above.)",
            rendered_cmd,
            status
                .code()
                .map_or_else(|| "signal".to_string(), |c| c.to_string())
        ))
    }
}

/// Run a multi-line shell snippet in `cwd`, streaming combined output to the terminal
/// (`tee`) while saving a copy for callers that need to parse logs (e.g. missing PGP keys).
pub fn run_shell_in_dir_with_tee<P: AsRef<Path>>(cwd: P, shell_body: &str) -> Result<(), String> {
    if crate::is_dry_run_mode() {
        println!(
            "[DRY RUN] bash (build in {}, tee to log)",
            cwd.as_ref().display()
        );
        return Ok(());
    }

    let tmp = std::env::temp_dir();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let inner_path = tmp.join(format!("arch_emerge_build_{}_inner.sh", stamp));
    let log_path = tmp.join(format!("arch_emerge_build_{}.log", stamp));

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
    // One script for `bash -c`: run inner helper, mirror output to terminal + log, propagate failure.
    let pipeline = format!(
        "bash {} 2>&1 | tee {}; exit ${{PIPESTATUS[0]}}",
        inner_arg, log_arg
    );

    let status = Command::new("bash")
        .arg("-o")
        .arg("pipefail")
        .arg("-c")
        .arg(&pipeline)
        .status()
        .map_err(|e| format!("failed to run build pipeline: {}", e))?;

    let log_text = std::fs::read_to_string(&log_path).unwrap_or_default();
    let _ = std::fs::remove_file(&inner_path);
    let _ = std::fs::remove_file(&log_path);

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

/// Remove prior `*.pkg.tar.zst` for this package base name from PKGDEST so old builds (e.g.
/// `-1.2`) are not offered alongside the new one (`-1.3`) after a recompile.
pub fn remove_stale_pkgs_in_pkgdest(pkgdest: &str, base_name: &str, verbose: bool) {
    if crate::is_dry_run_mode() {
        println!(
            "[DRY RUN] rm stale {}-*.pkg.tar.zst in {}",
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
        if !name.ends_with(".pkg.tar.zst") || !name.starts_with(&prefix) {
            continue;
        }
        if fs::remove_file(&path).is_err() {
            let _ = run_command(
                "sudo",
                &["rm", "-f", path.to_string_lossy().as_ref()],
                None::<&str>,
            );
        } else {
            crate::vlog!(verbose, "Removed stale package file: {}", name);
        }
    }
}

pub fn run_command_with_output<P: AsRef<Path>>(
    cmd: &str,
    args: &[&str],
    cwd: Option<P>,
) -> Result<String, String> {
    if crate::is_dry_run_mode() {
        let rendered_cmd = if args.is_empty() {
            cmd.to_string()
        } else {
            format!("{} {}", cmd, args.join(" "))
        };
        println!("[DRY RUN] {}", rendered_cmd);
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

/// Like [`run_command_with_output`], but sets extra environment variables on the child process.
pub fn run_command_with_output_env<P: AsRef<Path>>(
    cmd: &str,
    args: &[&str],
    cwd: Option<P>,
    env: &[(&str, &str)],
) -> Result<String, String> {
    if crate::is_dry_run_mode() {
        let rendered_cmd = if args.is_empty() {
            cmd.to_string()
        } else {
            format!("{} {}", cmd, args.join(" "))
        };
        println!("[DRY RUN] {} (env: {:?})", rendered_cmd, env);
        return Ok(String::new());
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

pub fn check_sudo_removal<P: AsRef<Path>>(path: P) -> Result<(), String> {
    let p = path.as_ref();
    if !p.exists() {
        return Ok(());
    }

    // Try standard remove first
    if std::fs::remove_dir_all(p).is_err() {
        // If it fails, likely due to root permissions, try sudo
        run_command(
            "sudo",
            &["rm", "-rf", p.to_string_lossy().as_ref()],
            None::<&str>,
        )?;
    }

    Ok(())
}
