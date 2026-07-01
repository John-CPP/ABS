use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Default profiling workload shipped with ABS (from the user's kernel/benchmark.sh workflow).
pub const BENCHMARK_SCRIPT: &str = include_str!("../assets/pgo-benchmark.sh");

/// Per-user materialized copy when ABS is run from `cargo build` without a system install.
pub fn bundled_benchmark_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("abs")
        .join("pgo-benchmark.sh")
}

/// Write the embedded script to `~/.local/share/abs/pgo-benchmark.sh` (mode 755).
pub fn materialize_bundled_benchmark() -> Result<PathBuf, String> {
    let path = bundled_benchmark_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("create benchmark dir {}: {e}", parent.display()))?;
    }
    match write_executable(&path, BENCHMARK_SCRIPT.as_bytes()) {
        Ok(()) => Ok(path),
        Err(first) => {
            reclaim_script_for_build_user(&path)?;
            write_executable(&path, BENCHMARK_SCRIPT.as_bytes())
                .map_err(|e| format!("{first}; after reclaim: {e}"))?;
            Ok(path)
        }
    }
}

/// Config override, else materialize embedded script (always refreshed; never a stale pacman copy).
pub fn resolve_benchmark_command(configured: &Option<String>) -> Result<PathBuf, String> {
    if let Some(raw) = configured.as_ref().filter(|s| !s.trim().is_empty()) {
        let path = crate::config::expand_user_path(raw.trim());
        if !path.is_file() {
            return Err(format!(
                "benchmark_command is not a file: {}",
                path.display()
            ));
        }
        ensure_executable(&path)?;
        return Ok(path);
    }

    materialize_bundled_benchmark()
        .and_then(|path| ensure_usable_script(&path).map(|()| path))
        .or_else(|e| {
        let tmp = std::env::temp_dir().join(format!("abs-pgo-benchmark-{}.sh", std::process::id()));
        write_executable(&tmp, BENCHMARK_SCRIPT.as_bytes())
            .map_err(|write_err| format!("{e}; fallback write {}: {write_err}", tmp.display()))?;
        Ok(tmp)
    })
}

/// Shell word(s) to run a benchmark script under `sudo -H -u` (bash avoids lost +x / root-owned scripts).
pub fn shell_benchmark_runner(path: &Path) -> String {
    format!("bash {}", crate::utils::sh_single_quote(&path.to_string_lossy()))
}

#[cfg(unix)]
fn write_executable(path: &Path, contents: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o755)
        .open(path)
        .map_err(|e| format!("write benchmark script {}: {e}", path.display()))?;
    file.write_all(contents)
        .map_err(|e| format!("write benchmark script {}: {e}", path.display()))?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("chmod benchmark script {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn write_executable(path: &Path, contents: &[u8]) -> Result<(), String> {
    fs::write(path, contents).map_err(|e| format!("write benchmark script {}: {e}", path.display()))
}

#[cfg(unix)]
fn reclaim_script_for_build_user(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let (uid, gid) = crate::utils::build_uid_gid();
    let owner = format!("{uid}:{gid}");
    let path_s = path.to_string_lossy();
    crate::utils::run_command(
        "sudo",
        &["chown", &owner, path_s.as_ref()],
        None::<&str>,
    )?;
    crate::utils::run_command("sudo", &["chmod", "755", path_s.as_ref()], None::<&str>)?;
    Ok(())
}

#[cfg(not(unix))]
fn reclaim_script_for_build_user(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn ensure_usable_script(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    if fs::set_permissions(path, fs::Permissions::from_mode(0o755)).is_err() {
        reclaim_script_for_build_user(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod benchmark script {}: {e}", path.display()))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_usable_script(path: &Path) -> Result<(), String> {
    ensure_executable(path)
}

#[cfg(unix)]
fn ensure_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::metadata(path)
        .map_err(|e| format!("benchmark script {}: {e}", path.display()))?;
    if meta.permissions().mode() & 0o111 == 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod benchmark script {}: {e}", path.display()))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_benchmark_has_shebang() {
        assert!(BENCHMARK_SCRIPT.starts_with("#!/"));
        assert!(BENCHMARK_SCRIPT.contains("ABS_PGO_PROFILE_DIR"));
    }

    #[test]
    fn embedded_benchmark_defaults_to_fast_mode() {
        assert!(BENCHMARK_SCRIPT.contains("ABS_PGO_BENCHMARK"));
        assert!(BENCHMARK_SCRIPT.contains("run_fast_benchmark"));
        assert!(BENCHMARK_SCRIPT.contains("fast|\"\") run_fast_benchmark"));
    }

    #[test]
    fn embedded_benchmark_cachyos_is_opt_in() {
        assert!(BENCHMARK_SCRIPT.contains("cachyos|full) run_cachyos_benchmarker"));
    }

    #[test]
    fn resolve_uses_embedded_fast_benchmark() {
        let path = resolve_benchmark_command(&None).unwrap();
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("run_fast_benchmark"));
        assert!(body.contains("ABS_PGO_BENCHMARK"));
    }

    #[test]
    fn shell_benchmark_runner_uses_bash() {
        let runner = shell_benchmark_runner(Path::new("/tmp/foo.sh"));
        assert!(runner.starts_with("bash "));
        assert!(runner.contains("/tmp/foo.sh"));
    }

    #[test]
    fn resolve_uses_configured_path_when_set() {
        let path = resolve_benchmark_command(&Some("/bin/true".into())).unwrap();
        assert_eq!(path, PathBuf::from("/bin/true"));
    }
}
