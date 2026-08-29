//! Parse `cachyos-benchmarker` `benchie_*.log` files and write comparison charts.
//!
//! Replaces CachyOS `benchmark_scraper.py` so ABS comparison reports do not need Python.

use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const SVG_HATCH: &str = concat!(
    r#"<defs><pattern id="hatch" patternUnits="userSpaceOnUse" width="6" height="6">"#,
    r##"<path d="M0,6 L6,0" stroke="#999" stroke-width="1"/></pattern></defs>"##,
);

const Y_CRUNCHER_SKIP: f64 = 0.01;
const Y_CRUNCHER: &str = "y-cruncher pi 1b";

const CATEGORY_1: &[&str] = &[
    "stress-ng cpu-cache-mem",
    "perf sched msg fork thread",
    "perf memcpy",
    "calculating prime numbers",
    "namd 92K atoms",
    "argon2 hashing",
    "ffmpeg compilation",
    "xz compression",
    "kernel defconfig",
    "blender render",
    "x265 encoding",
    "y-cruncher pi 1b",
];

struct Cat2 {
    name: &'static str,
    higher_better: bool,
    unit: &'static str,
}

const CATEGORY_2: &[Cat2] = &[
    Cat2 {
        name: "schbench p99 latency (us)",
        higher_better: false,
        unit: "us",
    },
    Cat2 {
        name: "schbench avg rps",
        higher_better: true,
        unit: "rps",
    },
    Cat2 {
        name: "cyclictest max latency (us)",
        higher_better: false,
        unit: "us",
    },
];

const EXTRA_METRICS: &[&str] = &["Total time (s)", "Total score"];

const PALETTE: &[&str] = &[
    "#4e79a7", "#f28e2b", "#e15759", "#76b7b2", "#59a14f", "#edc948", "#b07aa1", "#ff9da7",
    "#9c755f", "#bab0ab",
];

#[derive(Clone, Debug)]
struct KernelSeries {
    label: String,
    kernel: String,
    scx_scheduler: String,
    scx_version: String,
    yc_skipped: bool,
    averages: BTreeMap<String, f64>,
}

#[derive(Serialize)]
struct JsonEntry {
    kernel: String,
    scx_scheduler: String,
    scx_version: String,
    metrics: BTreeMap<String, serde_json::Value>,
}

/// Parse `benchie_*.log` in `dir` and write SVG charts, CSV/JSON, and `test_performance.html`.
/// Returns `Ok(false)` when no usable logs were found.
pub fn scrape_benchie_dir(dir: &Path) -> Result<bool, String> {
    let series = parse_logs(dir)?;
    if series.is_empty() {
        return Ok(false);
    }
    write_artifacts(dir, &series)?;
    Ok(true)
}

fn parse_logs(dir: &Path) -> Result<Vec<KernelSeries>, String> {
    let mut files: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("read {}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("benchie_") && n.ends_with(".log"))
        })
        .collect();
    files.sort();

    let mut samples: BTreeMap<String, (KernelSeries, HashMap<String, Vec<f64>>)> = BTreeMap::new();
    for path in files {
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let Some(parsed) = parse_one_log(&text) else {
            continue;
        };
        let entry = samples.entry(parsed.label.clone()).or_insert_with(|| {
            (
                KernelSeries {
                    label: parsed.label.clone(),
                    kernel: parsed.kernel.clone(),
                    scx_scheduler: parsed.scx_scheduler.clone(),
                    scx_version: parsed.scx_version.clone(),
                    yc_skipped: false,
                    averages: BTreeMap::new(),
                },
                HashMap::new(),
            )
        });
        for (name, value) in parsed.samples {
            entry.1.entry(name).or_default().push(value);
        }
    }

    let mut out = Vec::new();
    for (_, (mut series, values)) in samples {
        let yc = values.get(Y_CRUNCHER).cloned().unwrap_or_default();
        series.yc_skipped = !yc.is_empty()
            && yc.iter().all(|v| *v <= Y_CRUNCHER_SKIP)
            && series.kernel.to_ascii_lowercase().contains("infinity");
        series.averages = values
            .into_iter()
            .map(|(k, v)| (k, v.iter().sum::<f64>() / v.len() as f64))
            .collect();
        out.push(series);
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(out)
}

struct ParsedLog {
    label: String,
    kernel: String,
    scx_scheduler: String,
    scx_version: String,
    samples: Vec<(String, f64)>,
}

fn parse_one_log(text: &str) -> Option<ParsedLog> {
    let kernel = capture_token(text, "Kernel:")?;
    let scx_scheduler = capture_token(text, "SCX Scheduler:").unwrap_or_else(|| "none".into());
    let scx_version = capture_token(text, "SCX Version:").unwrap_or_else(|| "none".into());
    let label = if scx_scheduler != "none" && scx_version != "none" {
        format!("{kernel}_{scx_scheduler}_{scx_version}")
    } else {
        kernel.clone()
    };
    let mut samples = Vec::new();
    for line in text.lines() {
        let Some((name, rest)) = line.rsplit_once(':') else {
            continue;
        };
        let name = name.trim();
        if !is_known_metric(name) {
            continue;
        }
        let Some(value) = rest.split_whitespace().next().and_then(|s| s.parse().ok()) else {
            continue;
        };
        samples.push((name.to_string(), value));
    }
    if samples.is_empty() {
        return None;
    }
    Some(ParsedLog {
        label,
        kernel,
        scx_scheduler,
        scx_version,
        samples,
    })
}

fn capture_token(text: &str, prefix: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim();
        line.strip_prefix(prefix)
            .map(str::trim)
            .and_then(|s| s.split_whitespace().next())
            .map(str::to_string)
    })
}

fn is_known_metric(name: &str) -> bool {
    CATEGORY_1.contains(&name)
        || CATEGORY_2.iter().any(|c| c.name == name)
        || EXTRA_METRICS.contains(&name)
}

fn ordered_metrics(series: &[KernelSeries]) -> Vec<String> {
    let present: std::collections::HashSet<&str> = series
        .iter()
        .flat_map(|s| s.averages.keys().map(String::as_str))
        .collect();
    let mut out = Vec::new();
    for name in CATEGORY_1
        .iter()
        .copied()
        .chain(CATEGORY_2.iter().map(|c| c.name))
        .chain(EXTRA_METRICS.iter().copied())
    {
        if present.contains(name) {
            out.push(name.to_string());
        }
    }
    out
}

fn write_artifacts(dir: &Path, series: &[KernelSeries]) -> Result<(), String> {
    let cat = categorized_svg(series);
    fs::write(dir.join("categorized_comparison_All.svg"), cat)
        .map_err(|e| format!("write categorized SVG: {e}"))?;
    let cmp = kernel_comparison_svg(series);
    fs::write(dir.join("kernel_version_comparison_All.svg"), cmp)
        .map_err(|e| format!("write kernel comparison SVG: {e}"))?;

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Keep a stable-enough ISO-like name without extra crates.
    let ts = format_utc_stamp(stamp);
    let csv_name = format!("test_results_{ts}.csv");
    let json_name = format!("test_results_{ts}.json");
    fs::write(dir.join(&csv_name), csv_export(series)).map_err(|e| format!("write CSV: {e}"))?;
    fs::write(
        dir.join(&json_name),
        serde_json::to_string_pretty(&json_export(series)).map_err(|e| format!("JSON: {e}"))?,
    )
    .map_err(|e| format!("write JSON: {e}"))?;
    fs::write(
        dir.join("test_performance.html"),
        performance_html(series, &csv_name, &json_name),
    )
    .map_err(|e| format!("write HTML: {e}"))?;
    Ok(())
}

fn format_utc_stamp(unix: u64) -> String {
    // YYYY-MM-DDTHH-MM-SSZ without chrono.
    let secs = unix;
    let days = secs / 86400;
    let rem = secs % 86400;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}-{min:02}-{sec:02}Z")
}

/// Howard Hinnant civil-from-days (Unix epoch 1970-01-01 = day 0).
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i32 + era as i32 * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn csv_export(series: &[KernelSeries]) -> String {
    let metrics = ordered_metrics(series);
    let mut out = String::from("Kernel,SCX Scheduler,SCX Version");
    for name in &metrics {
        out.push(',');
        out.push_str(&csv_cell(name));
    }
    out.push('\n');
    for s in series {
        out.push_str(&csv_cell(&s.kernel));
        out.push(',');
        out.push_str(&csv_cell(&s.scx_scheduler));
        out.push(',');
        out.push_str(&csv_cell(&s.scx_version));
        for name in &metrics {
            out.push(',');
            if name == Y_CRUNCHER && s.yc_skipped {
                out.push_str("skipped");
            } else if let Some(v) = s.averages.get(name) {
                out.push_str(&format!("{v}"));
            }
        }
        out.push('\n');
    }
    out
}

fn csv_cell(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn json_export(series: &[KernelSeries]) -> Vec<JsonEntry> {
    series
        .iter()
        .map(|s| {
            let mut metrics = BTreeMap::new();
            for (k, v) in &s.averages {
                if k == Y_CRUNCHER && s.yc_skipped {
                    metrics.insert(k.clone(), serde_json::Value::String("skipped".into()));
                } else {
                    metrics.insert(k.clone(), serde_json::Value::from(*v));
                }
            }
            JsonEntry {
                kernel: s.kernel.clone(),
                scx_scheduler: s.scx_scheduler.clone(),
                scx_version: s.scx_version.clone(),
                metrics,
            }
        })
        .collect()
}

fn performance_html(series: &[KernelSeries], csv: &str, json: &str) -> String {
    let yc_note = if series.iter().any(|s| s.yc_skipped) {
        "\n       <em>y-cruncher pi 1b skipped on Infinity Scheduler — v3 design \
         trades synthetic throughput for real-world responsiveness.</em>"
    } else {
        ""
    };
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Test Performance</title>
</head>
<body>
    <h1>Test Performance</h1>

    <h2>Categorized Results</h2>
    <p>Category 1: Throughput &amp; Compilation (lower is better).
       Category 2: Scheduler Latency (↓ lower is better, ↑ higher is better).{yc_note}</p>
    <img src="categorized_comparison_All.svg" alt="Categorized Comparison - All Kernels"
         style="max-width: 100%; height: auto;">

    <h2>Performance Comparison Between Different Kernel Versions</h2>
    <img src="kernel_version_comparison_All.svg" alt="Kernel Version Comparison - All Kernels"
         style="max-width: 100%; height: auto;">

    <h2>Raw Data Exports</h2>
    <p>
        <a href="{csv}">Download Results (CSV)</a> |
        <a href="{json}">Download Results (JSON)</a>
    </p>
</body>
</html>"#
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn svg_open(width: f64, height: f64) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width:.0}" height="{height:.0}" viewBox="0 0 {width:.0} {height:.0}">{SVG_HATCH}"#
    )
}

fn svg_text(x: f64, y: f64, size: u32, bold: bool, fill: &str, text: &str) -> String {
    let weight = if bold { r#" font-weight="bold""# } else { "" };
    let fill_attr = if fill.is_empty() {
        String::new()
    } else {
        format!(r#" fill="{fill}""#)
    };
    format!(
        r#"<text x="{x:.1}" y="{y:.1}" font-size="{size}" font-family="sans-serif"{weight}{fill_attr}>{}</text>"#,
        xml_escape(text)
    )
}

fn svg_rect(x: f64, y: f64, w: f64, h: f64, fill: &str, extra: &str) -> String {
    format!(r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" fill="{fill}"{extra}/>"#)
}

fn categorized_svg(series: &[KernelSeries]) -> String {
    let row_h = 22.0;
    let label_w = 220.0;
    let chart_w = 640.0;
    let pad = 16.0;
    let mut y = 40.0;
    let mut body = String::new();
    body.push_str(&svg_text(
        pad,
        24.0,
        16,
        true,
        "",
        "CachyOS Benchmarker — Categorized Results (All mode)",
    ));
    for s in series.iter().rev() {
        let cat1: Vec<(&str, f64)> = CATEGORY_1
            .iter()
            .copied()
            .filter_map(|n| s.averages.get(n).map(|v| (n, *v)))
            .collect();
        let cat2: Vec<(&Cat2, f64)> = CATEGORY_2
            .iter()
            .filter_map(|c| s.averages.get(c.name).map(|v| (c, *v)))
            .collect();
        y += 28.0;
        let _ = write!(
            body,
            "{}",
            svg_text(
                pad,
                y,
                13,
                true,
                "",
                &format!("{} — Throughput & Compilation", s.label)
            )
        );
        y += 8.0;
        let max1 = cat1
            .iter()
            .map(|(_, v)| *v)
            .fold(0.0_f64, f64::max)
            .max(1.0);
        for (name, val) in cat1.iter().rev() {
            y += row_h;
            let w = (*val / max1) * chart_w;
            let skipped = *name == Y_CRUNCHER && s.yc_skipped;
            body.push_str(&svg_text(pad, y, 11, false, "", name));
            if skipped {
                body.push_str(&svg_rect(
                    pad + label_w,
                    y - 14.0,
                    w,
                    16.0,
                    "url(#hatch)",
                    r##" stroke="#999""##,
                ));
                body.push_str(&svg_text(
                    pad + label_w + w + 6.0,
                    y,
                    11,
                    false,
                    "#cc3333",
                    "SKIPPED*",
                ));
            } else {
                body.push_str(&svg_rect(
                    pad + label_w,
                    y - 14.0,
                    w,
                    16.0,
                    "skyblue",
                    r##" stroke="#999" stroke-width="0.6""##,
                ));
                body.push_str(&svg_text(
                    pad + label_w + w + 6.0,
                    y,
                    11,
                    false,
                    "",
                    &format!("{val:.2}"),
                ));
            }
        }
        y += 24.0;
        body.push_str(&svg_text(
            pad,
            y,
            13,
            true,
            "",
            &format!("{} — Scheduler Latency", s.label),
        ));
        y += 8.0;
        let max2 = cat2
            .iter()
            .map(|(_, v)| *v)
            .fold(0.0_f64, f64::max)
            .max(1.0);
        for (c, val) in cat2.iter().rev() {
            y += row_h;
            let w = (*val / max2) * chart_w;
            let fill = if c.higher_better {
                "#59a14f"
            } else {
                "#e15759"
            };
            let dir = if c.higher_better { "↑" } else { "↓" };
            body.push_str(&svg_text(pad, y, 11, false, "", c.name));
            body.push_str(&svg_rect(pad + label_w, y - 14.0, w, 16.0, fill, ""));
            body.push_str(&svg_text(
                pad + label_w + w + 6.0,
                y,
                11,
                false,
                "",
                &format!("{val:.2} {} {dir}", c.unit),
            ));
        }
        y += 16.0;
    }
    if series.iter().any(|s| s.yc_skipped) {
        y += 20.0;
        body.push_str(&svg_text(
            pad,
            y,
            10,
            false,
            "#666666",
            "* y-cruncher pi 1b skipped on Infinity Scheduler",
        ));
    }
    let height = y + pad;
    let width = pad + label_w + chart_w + 120.0;
    format!("{}{body}</svg>", svg_open(width, height))
}

fn kernel_comparison_svg(series: &[KernelSeries]) -> String {
    let metrics = ordered_metrics(series);
    if metrics.is_empty() {
        return format!(
            "{}{}</svg>",
            svg_open(400.0, 80.0),
            svg_text(16.0, 40.0, 14, false, "", "No metrics")
        );
    }
    let n_k = series.len().max(1);
    let bar_h = 14.0;
    let group_h = bar_h * n_k as f64 + 8.0;
    let label_w = 220.0;
    let chart_w = 520.0;
    let pad = 16.0;
    let y = 48.0;
    let mut body = String::new();
    body.push_str(&svg_text(
        pad,
        24.0,
        15,
        true,
        "",
        "Test Performance Comparison Between Different Kernel Versions (All mode)",
    ));
    let max_v = series
        .iter()
        .flat_map(|s| s.averages.values().copied())
        .fold(0.0_f64, f64::max)
        .max(1.0);
    for (mi, name) in metrics.iter().rev().enumerate() {
        let base_y = y + mi as f64 * group_h;
        body.push_str(&svg_text(pad, base_y + group_h / 2.0, 11, false, "", name));
        for (ki, s) in series.iter().enumerate() {
            let by = base_y + ki as f64 * bar_h;
            let val = s.averages.get(name).copied().unwrap_or(0.0);
            let skipped = name == Y_CRUNCHER && s.yc_skipped;
            let w = (val / max_v) * chart_w;
            let color = PALETTE[ki % PALETTE.len()];
            if skipped {
                body.push_str(&svg_rect(
                    pad + label_w,
                    by,
                    w.max(4.0),
                    bar_h - 2.0,
                    "url(#hatch)",
                    r##" stroke="#999""##,
                ));
                body.push_str(&svg_text(
                    pad + label_w + 4.0,
                    by + bar_h - 4.0,
                    10,
                    false,
                    "#cc3333",
                    "SKIPPED*",
                ));
            } else {
                body.push_str(&svg_rect(pad + label_w, by, w, bar_h - 2.0, color, ""));
                body.push_str(&svg_text(
                    pad + label_w + w + 4.0,
                    by + bar_h - 4.0,
                    10,
                    false,
                    "",
                    &format!("{val:.2}"),
                ));
            }
        }
    }
    let legend_y = y + metrics.len() as f64 * group_h + 24.0;
    for (ki, s) in series.iter().enumerate() {
        let x = pad + ki as f64 * 180.0;
        let color = PALETTE[ki % PALETTE.len()];
        body.push_str(&svg_rect(x, legend_y, 12.0, 12.0, color, ""));
        body.push_str(&svg_text(
            x + 16.0,
            legend_y + 11.0,
            11,
            false,
            "",
            &s.label,
        ));
    }
    let mut height = legend_y + 36.0;
    if series.iter().any(|s| s.yc_skipped) {
        height += 16.0;
        body.push_str(&svg_text(
            pad,
            height,
            10,
            false,
            "#666666",
            "* y-cruncher pi 1b skipped on Infinity Scheduler",
        ));
        height += 12.0;
    }
    let width = pad + label_w + chart_w + 80.0;
    format!("{}{body}</svg>", svg_open(width, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_dir() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "abs-scrape-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    const LOG_A: &str = "\
Kernel: 1_current
SCX Scheduler: none
SCX Version: none
System: test box
stress-ng cpu-cache-mem: 10.0
perf memcpy: 4.0
y-cruncher pi 1b: 12.5
schbench p99 latency (us): 20
schbench avg rps: 800
Total time (s): 100
";

    const LOG_B: &str = "\
Kernel: 4_final
SCX Scheduler: none
SCX Version: none
System: test box
stress-ng cpu-cache-mem: 8.0
perf memcpy: 3.0
y-cruncher pi 1b: 11.0
schbench p99 latency (us): 18
schbench avg rps: 900
Total time (s): 90
";

    const LOG_INF: &str = "\
Kernel: linux-infinity-test
SCX Scheduler: none
SCX Version: none
System: test box
stress-ng cpu-cache-mem: 9.0
y-cruncher pi 1b: 0.01
";

    #[test]
    fn parse_extracts_kernel_and_averages_repeat_samples() {
        let doubled = format!("{LOG_A}stress-ng cpu-cache-mem: 20.0\n");
        let parsed = parse_one_log(&doubled).unwrap();
        assert_eq!(parsed.kernel, "1_current");
        assert_eq!(parsed.label, "1_current");
        let samples: Vec<f64> = parsed
            .samples
            .iter()
            .filter(|(n, _)| n == "stress-ng cpu-cache-mem")
            .map(|(_, v)| *v)
            .collect();
        assert_eq!(samples, vec![10.0, 20.0]);
    }

    #[test]
    fn scx_label_appends_scheduler_and_version() {
        let log = "\
Kernel: 6.12.1
SCX Scheduler: bpfland
SCX Version: 1.0.0
stress-ng cpu-cache-mem: 1
";
        let parsed = parse_one_log(log).unwrap();
        assert_eq!(parsed.label, "6.12.1_bpfland_1.0.0");
    }

    #[test]
    fn scrape_writes_svg_html_and_exports() {
        let dir = unique_dir();
        fs::write(dir.join("benchie_abs-current_a.log"), LOG_A).unwrap();
        fs::write(dir.join("benchie_abs-final_b.log"), LOG_B).unwrap();
        assert!(scrape_benchie_dir(&dir).unwrap());
        let html = fs::read_to_string(dir.join("test_performance.html")).unwrap();
        assert!(html.contains("categorized_comparison_All.svg"));
        assert!(html.contains("kernel_version_comparison_All.svg"));
        assert!(dir.join("categorized_comparison_All.svg").is_file());
        assert!(dir.join("kernel_version_comparison_All.svg").is_file());
        let svg = fs::read_to_string(dir.join("categorized_comparison_All.svg")).unwrap();
        assert!(svg.contains("1_current"));
        assert!(svg.contains("4_final"));
        assert!(svg.contains("stress-ng cpu-cache-mem"));
        let json_path = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("test_results_") && n.ends_with(".json"))
            })
            .unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(json_path).unwrap()).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn infinity_placeholder_marks_y_cruncher_skipped() {
        let dir = unique_dir();
        fs::write(dir.join("benchie_abs-current_a.log"), LOG_INF).unwrap();
        assert!(scrape_benchie_dir(&dir).unwrap());
        let svg = fs::read_to_string(dir.join("categorized_comparison_All.svg")).unwrap();
        assert!(svg.contains("SKIPPED*"));
        let csv_path = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("test_results_") && n.ends_with(".csv"))
            })
            .unwrap();
        let csv = fs::read_to_string(csv_path).unwrap();
        assert!(csv.contains("skipped"), "{csv}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_dir_is_ok_false() {
        let dir = unique_dir();
        assert!(!scrape_benchie_dir(&dir).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn kernel_comparison_aligns_when_kernels_have_different_tests() {
        let dir = unique_dir();
        fs::write(dir.join("benchie_abs-current_a.log"), LOG_A).unwrap();
        fs::write(
            dir.join("benchie_abs-final_b.log"),
            "\
Kernel: 4_final
SCX Scheduler: none
SCX Version: none
System: test box
stress-ng cpu-cache-mem: 8.0
perf memcpy: 3.0
perf sched msg fork thread: 1.0
calculating prime numbers: 2.0
namd 92K atoms: 5.0
argon2 hashing: 6.0
ffmpeg compilation: 7.0
xz compression: 8.0
kernel defconfig: 9.0
blender render: 10.0
x265 encoding: 11.0
y-cruncher pi 1b: 11.0
schbench p99 latency (us): 18
schbench avg rps: 900
cyclictest max latency (us): 40
Total time (s): 90
Total score: 1.1
",
        )
        .unwrap();
        assert!(scrape_benchie_dir(&dir).unwrap());
        let svg = fs::read_to_string(dir.join("kernel_version_comparison_All.svg")).unwrap();
        assert!(svg.contains("1_current"), "{svg}");
        assert!(svg.contains("4_final"), "{svg}");
        assert!(svg.contains("cyclictest max latency (us)"), "{svg}");
        assert!(svg.contains("stress-ng cpu-cache-mem"), "{svg}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn averages_repeat_lines_in_one_file() {
        let dir = unique_dir();
        let log = format!("{LOG_A}stress-ng cpu-cache-mem: 20.0\n");
        fs::write(dir.join("benchie_abs-current_a.log"), log).unwrap();
        let series = parse_logs(&dir).unwrap();
        let v = series[0].averages["stress-ng cpu-cache-mem"];
        assert!((v - 15.0).abs() < 1e-9, "{v}");
        let _ = fs::remove_dir_all(&dir);
    }
}
