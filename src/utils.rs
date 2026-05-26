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

/// Like [`run_command`], but captures stdout and stderr instead of inheriting them.
/// Use this when you want to suppress output in non-verbose mode.
pub fn run_command_quiet<P: AsRef<Path>>(cmd: &str, args: &[&str], cwd: Option<P>) -> Result<(), String> {
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

    let output = command
        .output()
        .map_err(|e| format!("Failed to execute '{}': {}", cmd, e))?;

    if output.status.success() {
        Ok(())
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

/// Remove prior `*.pkg.tar.zst` for this package base name from PKGDEST so old builds (e.g.
/// `-1.2`) are not offered alongside the new one (`-1.3`) after a recompile.
pub fn remove_stale_pkgs_in_pkgdest(pkgdest: &str, base_name: &str) {
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
            crate::vlog!("Removed stale package file: {}", name);
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

/// Run **`sudo -v`** so the password is cached before later **`sudo rm`** / **`sudo pacman`** calls.
/// Long runs also use [`spawn_sudo_keepalive`] to refresh the timestamp before it expires (~15 minutes by default).
pub fn prime_sudo_for_session() -> Result<(), String> {
    if crate::is_dry_run_mode() {
        return Ok(());
    }
    run_command("sudo", &["-v"], None::<&str>)
}

/// Background thread: **`sudo -v`** every few minutes while the process is alive (stops when the program exits).
pub fn spawn_sudo_keepalive() {
    if crate::is_dry_run_mode() {
        return;
    }
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_secs(3 * 60));
        let _ = prime_sudo_for_session();
    });
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
    match run_command_with_output("pacman", &["-Q", "--noconfirm", pkg], None::<&str>) {
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
pub fn vercmp(a: &str, b: &str) -> Result<i32, String> {
    if crate::is_dry_run_mode() {
        return Err("dry-run".into());
    }
    let out = run_command_with_output("vercmp", &[a, b], None::<&str>)?;
    match out.trim() {
        "-1" => Ok(-1),
        "0" => Ok(0),
        "1" => Ok(1),
        other => Err(format!("unexpected vercmp output: {:?}", other)),
    }
}

#[cfg(test)]
mod version_tests {
    use super::parse_srcinfo_full_version;

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
}
