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
        if !path.is_absolute() {
            return Err(format!(
                "benchmark_command must be an absolute path: {}",
                path.display()
            ));
        }
        let allowed = dirs::home_dir().is_some_and(|h| crate::utils::path_has_prefix(&h, &path))
            || crate::utils::path_has_prefix(Path::new("/usr/share/abs"), &path)
            || crate::utils::path_has_prefix(
                &bundled_benchmark_path().parent().unwrap_or(Path::new("/")),
                &path,
            );
        if !allowed {
            return Err(format!(
                "benchmark_command must be under $HOME or /usr/share/abs: {}",
                path.display()
            ));
        }
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
            let tmp =
                std::env::temp_dir().join(format!("abs-pgo-benchmark-{}.sh", std::process::id()));
            write_executable(&tmp, BENCHMARK_SCRIPT.as_bytes()).map_err(|write_err| {
                format!("{e}; fallback write {}: {write_err}", tmp.display())
            })?;
            Ok(tmp)
        })
}

/// Shell word(s) to run a benchmark script under `sudo -H -u` (bash avoids lost +x / root-owned scripts).
pub fn shell_benchmark_runner(path: &Path) -> String {
    format!(
        "bash {}",
        crate::utils::sh_single_quote(&path.to_string_lossy())
    )
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
    crate::utils::run_command("sudo", &["chown", &owner, path_s.as_ref()], None::<&str>)?;
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
    let meta =
        fs::metadata(path).map_err(|e| format!("benchmark script {}: {e}", path.display()))?;
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

/// Rewrite `Kernel:` so comparison charts treat each PGO stage as a distinct series
/// (`Kernel: (\S+)`). Debug/AutoFDO comparison logs come from the profiling run; current/final
/// are clean (no perf) copies.
pub fn relabel_compare_log(text: &str, stage: &str, uname: &str) -> String {
    let safe_uname = uname
        .split_whitespace()
        .next()
        .unwrap_or(uname)
        .replace('/', "-");
    let label = format!("{stage}_{safe_uname}");
    let mut out = String::with_capacity(text.len() + label.len() + 16);
    let mut replaced = false;
    for line in text.lines() {
        if !replaced && line.starts_with("Kernel:") {
            out.push_str("Kernel: ");
            out.push_str(&label);
            out.push('\n');
            replaced = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !replaced {
        out.insert_str(0, &format!("Kernel: {label}\n"));
    }
    out
}

/// File name prefix used by cachyos-benchmarker: `benchie_<label>_<date>.log`.
pub fn compare_run_label(stage: &str) -> String {
    format!("abs-{stage}")
}

/// Fast sysbench/stress-ng warm-up before a standalone (no-perf) comparison.
pub fn warmup_compare_command(workdir: &Path, script: &Path) -> String {
    let dir = crate::utils::sh_single_quote(&workdir.to_string_lossy());
    format!(
        "env ABS_PGO_PROFILE_DIR={dir} ABS_PGO_BENCHMARK_DIR={dir} ABS_PGO_BENCHMARK=fast {runner}",
        runner = shell_benchmark_runner(script),
    )
}

/// Clean comparison run: same bundled script as profiling so PATH wrappers apply.
pub fn standalone_compare_command(workdir: &Path, label: &str, script: &Path) -> String {
    let dir = crate::utils::sh_single_quote(&workdir.to_string_lossy());
    format!(
        "env ABS_PGO_PROFILE_DIR={dir} ABS_PGO_BENCHMARK_DIR={dir} ABS_PGO_BENCHMARK=cachyos ABS_PGO_COMPARE_LABEL={label} {runner}",
        label = crate::utils::sh_single_quote(label),
        runner = shell_benchmark_runner(script),
    )
}

pub fn compare_stage_is_overhead(slug: &str) -> bool {
    matches!(slug, "debug" | "autofdo")
}

pub fn include_stage_in_chart_set(slug: &str, with_overhead: bool) -> bool {
    with_overhead || !compare_stage_is_overhead(slug)
}

pub fn chart_kernel_token(slug: &str) -> &'static str {
    match slug {
        "current" => "1_current",
        "debug" => "2_debug_perf",
        "debug_clean" => "2_debug_clean",
        "autofdo" => "3_autofdo_perf",
        "autofdo_clean" => "3_autofdo_clean",
        "final" => "4_final",
        _ => "0_other",
    }
}

pub fn chart_set_dir_name(with_overhead: bool) -> &'static str {
    if with_overhead {
        "with-overhead"
    } else {
        "without-overhead"
    }
}

const BENCHIE_SLUGS: &[&str] = &[
    "autofdo_clean",
    "debug_clean",
    "autofdo",
    "debug",
    "current",
    "final",
];

pub fn slug_from_benchie_name(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("benchie_abs-")?.strip_suffix(".log")?;
    BENCHIE_SLUGS
        .iter()
        .copied()
        .find(|slug| rest == *slug || rest.starts_with(&format!("{slug}_")))
}

pub fn relabel_kernel_token(text: &str, token: &str) -> String {
    let mut out = String::with_capacity(text.len() + token.len() + 16);
    let mut replaced = false;
    for line in text.lines() {
        if !replaced && line.starts_with("Kernel:") {
            out.push_str("Kernel: ");
            out.push_str(token);
            out.push('\n');
            replaced = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !replaced {
        out.insert_str(0, &format!("Kernel: {token}\n"));
    }
    out
}

pub fn compare_index_html(
    has_overhead: bool,
    without_tokens: &[&str],
    with_tokens: &[&str],
) -> String {
    fn series_list(tokens: &[&str]) -> String {
        if tokens.is_empty() {
            return "<li><em>No logs in this set yet.</em></li>\n".into();
        }
        tokens
            .iter()
            .map(|t| {
                let note = match *t {
                    "2_debug_perf" | "3_autofdo_perf" => {
                        " — includes <code>perf record</code> overhead"
                    }
                    "2_debug_clean" | "3_autofdo_clean" => " — clean run, no perf",
                    "1_current" => " — stock kernel, no perf",
                    "4_final" => " — Propeller kernel, no perf",
                    _ => "",
                };
                format!("<li><code>{t}</code>{note}</li>\n")
            })
            .collect()
    }

    let overhead_section = if has_overhead {
        format!(
            r#"<section>
  <h2>With overhead</h2>
  <p>Includes the AutoFDO and Propeller collection passes (scores under
  <code>perf record</code>) plus any extra clean runs. Use this set to see the
  profiling-pass cost and the full pipeline.</p>
  <ul>
{series}  </ul>
  <p class="charts">
    <a href="with-overhead/categorized_comparison_All.svg">Categorized comparison</a>
    · <a href="with-overhead/kernel_version_comparison_All.svg">Kernel comparison</a>
    · <a href="with-overhead/test_performance.html">Per-test table</a>
  </p>
  <p><img src="with-overhead/categorized_comparison_All.svg" alt="With-overhead categorized comparison"></p>
</section>
"#,
            series = series_list(with_tokens)
        )
    } else {
        r#"<section>
  <h2>With overhead</h2>
  <p>No AutoFDO/Propeller collection logs yet. Enable
  <strong>Benchmark debug kernel (with perf)</strong> and
  <strong>Benchmark AutoFDO kernel (with perf)</strong> to add those series.</p>
</section>
"#
        .into()
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>ABS PGO kernel comparison</title>
<style>
  :root {{ color-scheme: dark light; }}
  body {{
    font-family: ui-sans-serif, system-ui, sans-serif;
    max-width: 960px;
    margin: 2rem auto;
    padding: 0 1.25rem 3rem;
    line-height: 1.5;
  }}
  h1 {{ font-size: 1.6rem; margin-bottom: 0.4rem; }}
  .lead {{ color: #8b98a5; margin-top: 0; }}
  section {{
    margin-top: 2rem;
    padding: 1rem 1.2rem;
    border: 1px solid #3a4550;
    border-radius: 10px;
  }}
  h2 {{ font-size: 1.15rem; margin-top: 0; }}
  code {{ font-size: 0.92em; }}
  img {{ max-width: 100%; height: auto; border-radius: 6px; margin-top: 0.75rem; }}
  .charts a {{ margin-right: 0.4rem; }}
</style>
</head>
<body>
<h1>ABS PGO kernel comparison</h1>
<p class="lead">Two chart sets from the same pipeline. Debug and AutoFDO
collection runs sample under <code>perf record</code>; extra clean checkboxes
add matching scores without that overhead. Stock and final runs are always
clean.</p>
<section>
  <h2>Without overhead</h2>
  <p>Fair kernel-to-kernel comparison: only runs that did <em>not</em> record
  with perf. Missing debug/AutoFDO clean logs mean those stages were not
  requested.</p>
  <ul>
{without}  </ul>
  <p class="charts">
    <a href="without-overhead/categorized_comparison_All.svg">Categorized comparison</a>
    · <a href="without-overhead/kernel_version_comparison_All.svg">Kernel comparison</a>
    · <a href="without-overhead/test_performance.html">Per-test table</a>
  </p>
  <p><img src="without-overhead/categorized_comparison_All.svg" alt="Without-overhead categorized comparison"></p>
</section>
{overhead}
</body>
</html>
"#,
        without = series_list(without_tokens),
        overhead = overhead_section
    )
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
    fn resolve_rejects_configured_path_outside_allowlist() {
        let err = resolve_benchmark_command(&Some("/bin/true".into())).unwrap_err();
        assert!(err.contains("must be under $HOME or /usr/share/abs"));
    }

    #[test]
    fn relabel_compare_log_rewrites_kernel_token() {
        let log = "Kernel: 7.2.2-1-cachyos\nTotal score: 1.2\n";
        let out = relabel_compare_log(log, "current", "7.2.2-1-cachyos");
        assert!(out.contains("Kernel: current_7.2.2-1-cachyos\n"), "{out}");
        assert!(out.contains("Total score: 1.2"));
    }

    #[test]
    fn relabel_compare_log_inserts_kernel_when_missing() {
        let out = relabel_compare_log("Total score: 1\n", "final", "6.1.0-1-cachyos");
        assert!(out.starts_with("Kernel: final_6.1.0-1-cachyos\n"));
    }

    #[test]
    fn warmup_compare_command_uses_fast_preset() {
        let cmd =
            warmup_compare_command(Path::new("/tmp/bench"), Path::new("/tmp/pgo-benchmark.sh"));
        assert!(cmd.contains("ABS_PGO_BENCHMARK=fast"), "{cmd}");
        assert!(!cmd.contains("ABS_PGO_BENCHMARK=cachyos"), "{cmd}");
        assert!(!cmd.contains("ABS_PGO_COMPARE_LABEL"), "{cmd}");
    }

    #[test]
    fn compare_run_label_is_benchie_safe() {
        assert_eq!(compare_run_label("current"), "abs-current");
        assert_eq!(compare_run_label("final"), "abs-final");
        assert_eq!(compare_run_label("debug"), "abs-debug");
        assert_eq!(compare_run_label("autofdo"), "abs-autofdo");
        assert_eq!(compare_run_label("debug_clean"), "abs-debug_clean");
        assert_eq!(compare_run_label("autofdo_clean"), "abs-autofdo_clean");
    }

    #[test]
    fn overhead_stages_are_debug_and_autofdo_perf_runs() {
        assert!(compare_stage_is_overhead("debug"));
        assert!(compare_stage_is_overhead("autofdo"));
        assert!(!compare_stage_is_overhead("current"));
        assert!(!compare_stage_is_overhead("final"));
        assert!(!compare_stage_is_overhead("debug_clean"));
        assert!(!compare_stage_is_overhead("autofdo_clean"));
    }

    #[test]
    fn without_overhead_chart_set_drops_perf_runs() {
        assert!(include_stage_in_chart_set("current", false));
        assert!(!include_stage_in_chart_set("debug", false));
        assert!(include_stage_in_chart_set("debug_clean", false));
        assert!(!include_stage_in_chart_set("autofdo", false));
        assert!(include_stage_in_chart_set("autofdo_clean", false));
        assert!(include_stage_in_chart_set("final", false));
    }

    #[test]
    fn with_overhead_chart_set_keeps_every_stage() {
        for slug in [
            "current",
            "debug",
            "debug_clean",
            "autofdo",
            "autofdo_clean",
            "final",
        ] {
            assert!(include_stage_in_chart_set(slug, true), "{slug}");
        }
    }

    #[test]
    fn chart_kernel_tokens_sort_pipeline_order() {
        assert_eq!(chart_kernel_token("current"), "1_current");
        assert_eq!(chart_kernel_token("debug"), "2_debug_perf");
        assert_eq!(chart_kernel_token("debug_clean"), "2_debug_clean");
        assert_eq!(chart_kernel_token("autofdo"), "3_autofdo_perf");
        assert_eq!(chart_kernel_token("autofdo_clean"), "3_autofdo_clean");
        assert_eq!(chart_kernel_token("final"), "4_final");
    }

    #[test]
    fn slug_from_benchie_name_prefers_clean_suffix() {
        assert_eq!(
            slug_from_benchie_name("benchie_abs-autofdo_clean_7.2.2-1-cachyos.log"),
            Some("autofdo_clean")
        );
        assert_eq!(
            slug_from_benchie_name("benchie_abs-autofdo_7.2.2-1-cachyos.log"),
            Some("autofdo")
        );
        assert_eq!(
            slug_from_benchie_name("benchie_abs-debug_clean_7.2.log"),
            Some("debug_clean")
        );
        assert_eq!(slug_from_benchie_name("notes.txt"), None);
    }

    #[test]
    fn compare_index_html_explains_both_sets() {
        let html = compare_index_html(
            true,
            &["1_current", "4_final"],
            &["1_current", "2_debug_perf", "4_final"],
        );
        assert!(html.contains("without-overhead"));
        assert!(html.contains("with-overhead"));
        assert!(html.contains("2_debug_perf"));
        assert!(html.contains("perf record"));
        assert!(html.contains("categorized_comparison_All.svg"));
        assert!(!html.contains("categorized_comparison_All.png"));
    }

    #[cfg(unix)]
    fn chmod_755(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Persistent PGO workdirs keep `.abs-bin/cachyos-benchmarker` from the last run.
    /// The wget wrapper prepends that directory to PATH, so lookup must skip the leftover
    /// shim instead of treating it as the real CachyOS script.
    #[cfg(unix)]
    #[test]
    fn leftover_abs_shim_does_not_block_cachyos_mode() {
        let dir = unique_temp_dir("abs-cb-leftover");
        let workdir = dir.join("wd");
        let bindir = workdir.join(".abs-bin");
        fs::create_dir_all(&bindir).unwrap();
        let leftover = bindir.join("cachyos-benchmarker");
        fs::write(
            &leftover,
            "#!/usr/bin/env bash\necho leftover-shim\nexit 0\n",
        )
        .unwrap();
        chmod_755(&leftover);

        let realdir = dir.join("realbin");
        fs::create_dir_all(&realdir).unwrap();
        let real = realdir.join("cachyos-benchmarker");
        fs::write(&real, "#!/usr/bin/env bash\necho fake-real-ran\nexit 0\n").unwrap();
        chmod_755(&real);

        let script = dir.join("pgo-benchmark.sh");
        fs::write(&script, BENCHMARK_SCRIPT).unwrap();
        chmod_755(&script);

        let path = format!("{}:/usr/bin:/bin", realdir.display());
        let out = std::process::Command::new("bash")
            .arg(&script)
            .env("ABS_PGO_BENCHMARK", "cachyos")
            .env("ABS_PGO_BENCHMARK_DIR", &workdir)
            .env("ABS_PGO_PROFILE_DIR", &workdir)
            .env("PATH", path)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "status={:?} stdout={stdout} stderr={stderr}",
            out.status
        );
        assert!(
            !stderr.contains("refusing to wrap the ABS cachyos-benchmarker shim"),
            "stderr={stderr}"
        );
        assert!(stdout.contains("fake-real-ran"), "{stdout}");
        assert!(!stdout.contains("leftover-shim"), "{stdout}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn skip_scraper_wrapper_patches_cachyos_benchmarker_not_python() {
        let start = BENCHMARK_SCRIPT
            .find("cat > \"${bindir}/cachyos-benchmarker\" << 'EOF'\n")
            .expect("cachyos-benchmarker wrapper heredoc");
        let body = BENCHMARK_SCRIPT[start..]
            .split_once("<< 'EOF'\n")
            .unwrap()
            .1
            .split_once("\nEOF\n")
            .unwrap()
            .0;
        let dir = std::env::temp_dir().join(format!(
            "abs-cbwrap-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("cachyos-benchmarker-real");
        fs::write(
            &fake,
            r#"#!/usr/bin/env bash
set -euo pipefail
SCRIPTDIR=$(dirname "$(readlink -f "$0")")
echo scored
if [[ -f "$SCRIPTDIR/benchmark_scraper.py" ]]; then
	python "$SCRIPTDIR/benchmark_scraper.py"
else
	python /usr/bin/benchmark_scraper.py
fi
echo after-scraper
"#,
        )
        .unwrap();
        let wrap = dir.join("cachyos-benchmarker");
        fs::write(&wrap, format!("{body}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
            fs::set_permissions(&wrap, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let out = std::process::Command::new(&wrap)
            .env("ABS_CACHYOS_BENCHMARKER_REAL", &fake)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "status={:?} stdout={stdout} stderr={stderr}",
            out.status
        );
        assert!(stdout.contains("scored"), "{stdout}");
        assert!(
            stdout.contains("skipping CachyOS benchmark_scraper.py"),
            "{stdout}"
        );
        assert!(stdout.contains("after-scraper"), "{stdout}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn embedded_benchmark_feeds_compare_label_to_cachyos_benchmarker() {
        assert!(BENCHMARK_SCRIPT.contains("ABS_PGO_COMPARE_LABEL"));
        assert!(BENCHMARK_SCRIPT.contains("printf '%s\\n' '' \"${label}\""));
    }

    #[test]
    fn embedded_benchmark_does_not_wrap_python() {
        assert!(
            !BENCHMARK_SCRIPT.contains("cat > \"${bindir}/python\""),
            "ABS must not install a python PATH shim"
        );
        assert!(
            !BENCHMARK_SCRIPT.contains("exec /usr/bin/python"),
            "ABS must not exec python"
        );
    }

    #[test]
    fn embedded_benchmark_skips_cachyos_python_scraper() {
        assert!(
            BENCHMARK_SCRIPT.contains("cat > \"${bindir}/cachyos-benchmarker\""),
            "wrapper must patch CachyOS's matplotlib scraper out of cachyos-benchmarker"
        );
        assert!(
            BENCHMARK_SCRIPT.contains("skipping CachyOS benchmark_scraper.py"),
            "wrapper should log that ABS charts replace the Python scraper"
        );
        assert!(
            BENCHMARK_SCRIPT.contains("find_real_cachyos_benchmarker"),
            "PATH lookup must skip a leftover ABS shim in the persistent workdir"
        );
    }

    #[test]
    fn standalone_compare_runs_through_bundled_script() {
        let cmd = standalone_compare_command(
            Path::new("/tmp/wd"),
            "abs-current",
            Path::new("/tmp/pgo-benchmark.sh"),
        );
        assert!(cmd.contains("ABS_PGO_BENCHMARK=cachyos"), "{cmd}");
        assert!(cmd.contains("ABS_PGO_COMPARE_LABEL="), "{cmd}");
        assert!(cmd.contains("pgo-benchmark.sh"), "{cmd}");
        assert!(
            !cmd.contains("| cachyos-benchmarker"),
            "direct cachyos-benchmarker skips the no-scraper wrapper: {cmd}"
        );
    }
}
