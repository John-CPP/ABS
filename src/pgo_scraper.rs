//! Parse `benchie_*.log` comparison files and write charts.
//!
//! kbench and CachyOS-benchmarker both write this format. ABS charts replace
//! CachyOS `benchmark_scraper.py`.

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

struct Cat2 {
    name: &'static str,
    higher_better: bool,
    unit: &'static str,
}

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
    // Kernel-path metrics from the `kbench` scoring workload. Userspace compute
    // barely moves with a faster kernel; these sit directly on syscall, scheduler,
    // VFS, network and block paths.
    Cat2 {
        name: "syscall getppid (Mops/s)",
        higher_better: true,
        unit: "Mops/s",
    },
    Cat2 {
        name: "sched pipe (Kops/s)",
        higher_better: true,
        unit: "Kops/s",
    },
    Cat2 {
        name: "sched messaging (s)",
        higher_better: false,
        unit: "s",
    },
    Cat2 {
        name: "futex hash (Kops/s)",
        higher_better: true,
        unit: "Kops/s",
    },
    Cat2 {
        name: "epoll wait (Kops/s)",
        higher_better: true,
        unit: "Kops/s",
    },
    Cat2 {
        name: "hackbench pipes (s)",
        higher_better: false,
        unit: "s",
    },
    Cat2 {
        name: "context switch (Kops/s)",
        higher_better: true,
        unit: "Kops/s",
    },
    Cat2 {
        name: "page fault (Kops/s)",
        higher_better: true,
        unit: "Kops/s",
    },
    Cat2 {
        name: "mmap/munmap (Kops/s)",
        higher_better: true,
        unit: "Kops/s",
    },
    Cat2 {
        name: "vfs open/close (Kops/s)",
        higher_better: true,
        unit: "Kops/s",
    },
    Cat2 {
        name: "dentry lookup (Kops/s)",
        higher_better: true,
        unit: "Kops/s",
    },
    Cat2 {
        name: "unix socket (Kops/s)",
        higher_better: true,
        unit: "Kops/s",
    },
    Cat2 {
        name: "udp loopback (Kops/s)",
        higher_better: true,
        unit: "Kops/s",
    },
    Cat2 {
        name: "pipe throughput (Kops/s)",
        higher_better: true,
        unit: "Kops/s",
    },
    Cat2 {
        name: "fork+exec (ops/s)",
        higher_better: true,
        unit: "ops/s",
    },
    Cat2 {
        name: "io_uring (Kops/s)",
        higher_better: true,
        unit: "Kops/s",
    },
    Cat2 {
        name: "buffered file io (Kops/s)",
        higher_better: true,
        unit: "Kops/s",
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

fn has_category1(series: &[KernelSeries]) -> bool {
    series
        .iter()
        .any(|s| CATEGORY_1.iter().any(|n| s.averages.contains_key(*n)))
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
    fs::write(dir.join("winners_table.html"), winners_table_html(series))
        .map_err(|e| format!("write winners table: {e}"))?;
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
    let wins = best_per_test_line(series);
    let table = winners_table_html(series);
    let cat_caption = if has_category1(series) {
        "CachyOS + kbench. Same encoding as above, grouped by workload. Longer is better in every group (including latency)."
    } else {
        "kbench kernel-path metrics. Longer bar is better, including latency."
    };
    let yc_note = if series.iter().any(|s| s.yc_skipped) {
        " y-cruncher pi 1b skipped on Infinity Scheduler."
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
    <style>
      :root {{ color-scheme: dark light; }}
      body {{ font-family: ui-sans-serif, system-ui, sans-serif; max-width: 1080px; margin: 2rem auto; padding: 0 1.25rem 3rem; line-height: 1.5; }}
      table.winners {{ width: 100%; border-collapse: collapse; margin: 0.75rem 0 0.25rem; }}
      table.winners th, table.winners td {{ text-align: left; padding: 0.35rem 0.6rem; border-bottom: 1px solid #3a4550; }}
      table.winners th {{ font-size: 0.85rem; color: #8b98a5; }}
      .winner-note {{ color: #8b98a5; font-size: 0.9rem; }}
      .callout {{ border: 1px solid #8a6d3b; background: rgba(138, 109, 59, 0.12); border-radius: 8px; padding: 0.8rem 1rem; margin: 1rem 0; }}
      .callout strong {{ display: block; margin-bottom: 0.25rem; }}
    </style>
</head>
<body>
    <h1>Test Performance</h1>

    <h2>Ranking vs stock</h2>
    {table}

    <h2>Performance Comparison Between Different Kernel Versions</h2>
    <p>{wins}. Longer bar is better. Captions are the raw measurement plus faster/slower
       (or lower/higher latency) vs stock. Last group is geomean vs stock.</p>
    <img src="kernel_version_comparison_All.svg" alt="Kernel Version Comparison - All Kernels"
         style="max-width: 100%; height: auto;">

    <h2>Categorized Results</h2>
    <p>{cat_caption}{yc_note}</p>
    <img src="categorized_comparison_All.svg" alt="Categorized Comparison - All Kernels"
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

fn paint_compare_bars(
    body: &mut String,
    series: &[KernelSeries],
    name: &str,
    base_y: f64,
    pad: f64,
    label_w: f64,
    chart_w: f64,
    bar_h: f64,
    group_h: f64,
) {
    let bars = compare_bars(series, name);
    body.push_str(&svg_text(
        pad,
        base_y + group_h / 2.0,
        11,
        false,
        "",
        &metric_axis_label(name),
    ));
    for (ki, bar) in bars.iter().enumerate() {
        let by = base_y + ki as f64 * bar_h;
        let w = bar.bar_frac * chart_w;
        let color = PALETTE[ki % PALETTE.len()];
        if bar.skipped {
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
            let extra = if bar.is_best {
                r##" stroke="#222" stroke-width="1.2""##
            } else {
                ""
            };
            body.push_str(&svg_rect(
                pad + label_w,
                by,
                w.max(0.0),
                bar_h - 2.0,
                color,
                extra,
            ));
            body.push_str(&svg_text(
                pad + label_w + w + 6.0,
                by + bar_h - 4.0,
                10,
                bar.is_best,
                "",
                &bar.caption,
            ));
        }
    }
}

fn categorized_svg(series: &[KernelSeries]) -> String {
    let metrics = ordered_metrics(series);
    let n_k = series.len().max(1);
    let bar_h = 16.0;
    let group_h = bar_h * n_k as f64 + 14.0;
    let label_w = 220.0;
    let chart_w = 520.0;
    let pad = 16.0;
    let sections: [(&str, Vec<String>); 3] = [
        (
            "Throughput & compile — raw is seconds; longer bar = faster",
            metrics
                .iter()
                .filter(|m| CATEGORY_1.contains(&m.as_str()))
                .cloned()
                .collect(),
        ),
        (
            "Scheduler & kernel path — longer bar = better (lower latency / higher rate)",
            metrics
                .iter()
                .filter(|m| CATEGORY_2.iter().any(|c| c.name == m.as_str()))
                .cloned()
                .collect(),
        ),
        (
            "Suite totals — Total score is golf (lower pts is better)",
            metrics
                .iter()
                .filter(|m| EXTRA_METRICS.contains(&m.as_str()))
                .cloned()
                .collect(),
        ),
    ];
    let mut height = 72.0;
    for (_, names) in &sections {
        if names.is_empty() {
            continue;
        }
        height += 32.0 + names.len() as f64 * group_h + 8.0;
    }
    if series.iter().any(|s| s.yc_skipped) {
        height += 20.0;
    }
    height += 48.0;
    let width = pad + label_w + chart_w + 280.0;
    let title = if has_category1(series) {
        "CachyOS + kbench — Categorized results"
    } else {
        "kbench — Kernel path metrics"
    };
    let mut body = String::new();
    body.push_str(&svg_text(pad, 22.0, 15, true, "", title));
    body.push_str(&svg_text(
        pad,
        40.0,
        11,
        false,
        "#666666",
        "Same encoding as the kernel chart. Long bar + small µs = lower latency (the win).",
    ));
    body.push_str(&svg_text(
        pad,
        56.0,
        11,
        false,
        "#666666",
        "Latency captions are lower/higher µs vs stock. Total score is golf (lower pts is better).",
    ));
    let mut y = 72.0;
    for (heading, names) in &sections {
        if names.is_empty() {
            continue;
        }
        y += 22.0;
        body.push_str(&svg_text(pad, y, 13, true, "", heading));
        y += 10.0;
        for name in names {
            paint_compare_bars(
                &mut body, series, name, y, pad, label_w, chart_w, bar_h, group_h,
            );
            y += group_h;
        }
        y += 8.0;
    }
    if series.iter().any(|s| s.yc_skipped) {
        y += 16.0;
        body.push_str(&svg_text(
            pad,
            y,
            10,
            false,
            "#666666",
            "* y-cruncher pi 1b skipped on Infinity Scheduler",
        ));
    }
    for (ki, s) in series.iter().enumerate() {
        let x = pad + ki as f64 * 180.0;
        body.push_str(&svg_rect(
            x,
            y + 16.0,
            12.0,
            12.0,
            PALETTE[ki % PALETTE.len()],
            "",
        ));
        body.push_str(&svg_text(x + 16.0, y + 27.0, 11, false, "", &s.label));
    }
    format!("{}{body}</svg>", svg_open(width, height.max(y + 48.0)))
}

fn is_stock_kernel(s: &KernelSeries) -> bool {
    s.kernel == "1_current" || s.label == "1_current" || s.kernel.starts_with("1_current")
}

fn metric_higher_better(name: &str) -> bool {
    if name == "Total time (s)" || name == "Total score" {
        return false;
    }
    if let Some(c) = CATEGORY_2.iter().find(|c| c.name == name) {
        return c.higher_better;
    }
    false
}

fn metric_unit(name: &str) -> &'static str {
    if let Some(c) = CATEGORY_2.iter().find(|c| c.name == name) {
        return c.unit;
    }
    if name == "Total score" { "pts" } else { "s" }
}

fn metric_is_latency(name: &str) -> bool {
    metric_unit(name) == "us" || name.contains("latency")
}

fn format_raw(metric: &str, val: f64) -> String {
    match metric_unit(metric) {
        "us" => format!("{val:.0} µs"),
        "rps" => format!("{val:.0} rps"),
        "pts" => format!("{val:.2} pts"),
        "s" => {
            if val >= 100.0 {
                format!("{val:.0} s")
            } else if val >= 10.0 {
                format!("{val:.2} s")
            } else {
                format!("{val:.3} s")
            }
        }
        unit => format!("{val:.2} {unit}"),
    }
}

fn metric_axis_label(name: &str) -> String {
    let dir = if metric_higher_better(name) {
        "↑"
    } else {
        "↓"
    };
    format!("{name}  {dir}")
}

fn stock_value(series: &[KernelSeries], metric: &str) -> Option<f64> {
    series
        .iter()
        .find(|s| is_stock_kernel(s))
        .and_then(|s| s.averages.get(metric).copied())
        .filter(|v| v.is_finite() && *v > 0.0)
}

/// Phrase vs stock for captions/tables. Latency uses the raw µs change.
fn vs_stock_phrase(metric: &str, value: f64, stock: f64) -> Option<String> {
    if !(stock > 0.0 && value.is_finite()) {
        return None;
    }
    if metric_is_latency(metric) {
        let pct = (value / stock - 1.0) * 100.0;
        if pct.abs() < 0.5 {
            return Some("same latency".into());
        }
        if pct < 0.0 {
            return Some(format!("{:.0}% lower latency", -pct));
        }
        return Some(format!("{:.0}% higher latency", pct));
    }
    let rel = relative_performance(value, stock, metric_higher_better(metric))?;
    let d = rel - 100.0;
    if d.abs() < 0.05 {
        return Some("even".into());
    }
    let (pos, neg) = if metric == "Total score" {
        ("better", "worse")
    } else if metric_higher_better(metric) {
        ("higher", "lower")
    } else {
        ("faster", "slower")
    };
    if d > 0.0 {
        Some(format!("{:.1}% {pos}", d))
    } else {
        Some(format!("{:.1}% {neg}", -d))
    }
}

/// Performance vs baseline as a percent (100 = equal). Higher is always better.
fn relative_performance(value: f64, baseline: f64, higher_better: bool) -> Option<f64> {
    if !value.is_finite() || !baseline.is_finite() || baseline <= 0.0 {
        return None;
    }
    if higher_better {
        Some(value / baseline * 100.0)
    } else if value > 0.0 {
        Some(baseline / value * 100.0)
    } else {
        None
    }
}

struct CompareBar {
    caption: String,
    bar_frac: f64,
    skipped: bool,
    is_best: bool,
    rel: Option<f64>,
}

fn format_delta(rel: f64) -> String {
    let d = rel - 100.0;
    if d.abs() < 0.05 {
        "0%".into()
    } else {
        format!("{:+.1}%", d)
    }
}

fn ordinal(n: usize) -> String {
    match n {
        1 => "1st".into(),
        2 => "2nd".into(),
        3 => "3rd".into(),
        n if (11..=13).contains(&(n % 100)) => format!("{n}th"),
        n if n % 10 == 1 => format!("{n}st"),
        n if n % 10 == 2 => format!("{n}nd"),
        n if n % 10 == 3 => format!("{n}rd"),
        n => format!("{n}th"),
    }
}

fn metric_worst_baseline(series: &[KernelSeries], metric: &str) -> Option<f64> {
    let higher = metric_higher_better(metric);
    let mut worst: Option<f64> = None;
    for s in series {
        if metric == Y_CRUNCHER && s.yc_skipped {
            continue;
        }
        let val = s.averages.get(metric).copied().unwrap_or(0.0);
        if !val.is_finite() || val <= 0.0 {
            continue;
        }
        worst = Some(match worst {
            None => val,
            Some(w) => {
                if higher {
                    w.min(val)
                } else {
                    w.max(val)
                }
            }
        });
    }
    worst
}

fn compare_bars(series: &[KernelSeries], metric: &str) -> Vec<CompareBar> {
    let higher = metric_higher_better(metric);
    let stock = stock_value(series, metric);
    let baseline = stock.or_else(|| metric_worst_baseline(series, metric));

    let scores: Vec<Option<f64>> = series
        .iter()
        .map(|s| {
            let skipped = metric == Y_CRUNCHER && s.yc_skipped;
            let val = s.averages.get(metric).copied().unwrap_or(0.0);
            if skipped {
                None
            } else if let Some(b) = baseline {
                relative_performance(val, b, higher)
            } else if higher && val.is_finite() && val > 0.0 {
                Some(val)
            } else if !higher && val > 0.0 {
                Some(1.0 / val)
            } else {
                None
            }
        })
        .collect();

    let max_score = scores
        .iter()
        .flatten()
        .copied()
        .fold(0.0_f64, f64::max)
        .max(1e-9);
    let best = scores
        .iter()
        .flatten()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let worst_score = scores
        .iter()
        .flatten()
        .copied()
        .fold(f64::INFINITY, f64::min);

    series
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let skipped = metric == Y_CRUNCHER && s.yc_skipped;
            let val = s.averages.get(metric).copied().unwrap_or(0.0);
            let score = scores[i];
            let is_best =
                !skipped && score.is_some_and(|sc| (sc - best).abs() <= best.abs() * 1e-9 + 1e-6);
            let is_worst = !skipped
                && score
                    .is_some_and(|sc| (sc - worst_score).abs() <= worst_score.abs() * 1e-9 + 1e-6);
            let bar_frac = score
                .map(|sc| (sc / max_score).clamp(0.0, 1.0))
                .unwrap_or(0.0);
            let caption = if skipped {
                "SKIPPED*".into()
            } else {
                let mut c = format_raw(metric, val);
                if is_stock_kernel(s) {
                    c.push_str("  stock");
                } else if let Some(st) = stock {
                    if let Some(phrase) = vs_stock_phrase(metric, val, st) {
                        c.push(' ');
                        c.push(' ');
                        c.push_str(&phrase);
                    }
                } else if let Some(rel) = score {
                    c.push_str(&format!("  {}", format_delta(rel)));
                }
                if is_worst {
                    c.push_str("  worst");
                }
                if is_best {
                    c.push_str("  best");
                }
                c
            };
            CompareBar {
                caption,
                bar_frac,
                skipped,
                is_best,
                rel: score,
            }
        })
        .collect()
}

fn best_per_test_line(series: &[KernelSeries]) -> String {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for metric in ordered_metrics(series) {
        for (s, bar) in series.iter().zip(compare_bars(series, &metric)) {
            if bar.is_best && !bar.skipped {
                *counts.entry(s.label.clone()).or_default() += 1;
            }
        }
    }
    let parts: Vec<String> = series
        .iter()
        .map(|s| format!("{} {}", s.label, counts.get(&s.label).copied().unwrap_or(0)))
        .collect();
    format!("Best per test: {}", parts.join(" · "))
}

fn kernel_geomean_vs_stock(series: &[KernelSeries]) -> Vec<Option<f64>> {
    let metrics = ordered_metrics(series);
    let stock = series.iter().find(|s| is_stock_kernel(s));
    series
        .iter()
        .map(|s| {
            if is_stock_kernel(s) {
                return Some(100.0);
            }
            let stock = stock?;
            let mut logs = Vec::new();
            for m in &metrics {
                if m == Y_CRUNCHER && (s.yc_skipped || stock.yc_skipped) {
                    continue;
                }
                let Some(bv) = stock
                    .averages
                    .get(m)
                    .copied()
                    .filter(|v| *v > 0.0 && v.is_finite())
                else {
                    continue;
                };
                let Some(v) = s
                    .averages
                    .get(m)
                    .copied()
                    .filter(|v| *v > 0.0 && v.is_finite())
                else {
                    continue;
                };
                if let Some(rel) = relative_performance(v, bv, metric_higher_better(m))
                    && rel > 0.0
                {
                    logs.push(rel.ln());
                }
            }
            if logs.is_empty() {
                None
            } else {
                Some((logs.iter().sum::<f64>() / logs.len() as f64).exp())
            }
        })
        .collect()
}

fn geomean_compare_bars(series: &[KernelSeries]) -> Vec<CompareBar> {
    let geos = kernel_geomean_vs_stock(series);
    let max_score = geos
        .iter()
        .flatten()
        .copied()
        .fold(0.0_f64, f64::max)
        .max(1e-9);
    let best = geos
        .iter()
        .flatten()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    series
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let rel = geos[i];
            let skipped = rel.is_none();
            let is_best = rel.is_some_and(|sc| (sc - best).abs() <= best.abs() * 1e-9 + 1e-6);
            let bar_frac = rel
                .map(|sc| (sc / max_score).clamp(0.0, 1.0))
                .unwrap_or(0.0);
            let caption = if skipped {
                "—".into()
            } else {
                let mut c = format_delta(rel.unwrap());
                if is_stock_kernel(s) {
                    c.push_str("  stock");
                }
                if is_best {
                    c.push_str("  best");
                }
                c
            };
            CompareBar {
                caption,
                bar_frac,
                skipped,
                is_best,
                rel,
            }
        })
        .collect()
}

fn winner_short_name(s: &KernelSeries) -> String {
    let t = s.label.as_str();
    if is_stock_kernel(s) {
        "stock".into()
    } else if t.contains("debug") {
        if t.contains("perf") {
            "debug (perf)".into()
        } else {
            "debug".into()
        }
    } else if t.contains("autofdo") {
        if t.contains("perf") {
            "AutoFDO (perf)".into()
        } else {
            "AutoFDO".into()
        }
    } else if t.contains("final") {
        "Propeller".into()
    } else {
        s.label.clone()
    }
}

fn podium_cell(metric: &str, s: &KernelSeries, stock: Option<f64>) -> String {
    let val = s.averages.get(metric).copied().unwrap_or(0.0);
    let raw = format_raw(metric, val);
    let name = winner_short_name(s);
    if is_stock_kernel(s) {
        format!("{name} {raw}")
    } else if let Some(st) = stock {
        match vs_stock_phrase(metric, val, st) {
            Some(p) => format!("{name} {raw} ({p})"),
            None => format!("{name} {raw}"),
        }
    } else {
        format!("{name} {raw}")
    }
}

fn latency_explainer_html(series: &[KernelSeries]) -> String {
    const COLS: &[(&str, &str)] = &[
        ("cyclictest max latency (us)", "cyclictest max ↓µs"),
        ("schbench p99 latency (us)", "schbench p99 ↓µs"),
        ("schbench avg rps", "schbench avg ↑rps"),
    ];
    let has_latency = COLS
        .iter()
        .any(|(n, _)| n.contains("latency") && series.iter().any(|s| s.averages.contains_key(*n)));
    if !has_latency {
        return String::new();
    }
    let mut head = String::from("<tr><th>Kernel</th>");
    let mut used: Vec<&str> = Vec::new();
    for (name, label) in COLS {
        if series.iter().any(|s| s.averages.contains_key(*name)) {
            let _ = write!(head, "<th>{}</th>", xml_escape(label));
            used.push(*name);
        }
    }
    head.push_str("</tr>");
    let mut body = String::new();
    for s in series {
        let _ = write!(body, "<tr><td>{}</td>", xml_escape(&winner_short_name(s)));
        for name in &used {
            let bars = compare_bars(series, name);
            let is_best = series
                .iter()
                .zip(&bars)
                .find(|(k, _)| k.label == s.label)
                .is_some_and(|(_, b)| b.is_best);
            let val = s.averages.get(*name).copied().unwrap_or(0.0);
            let mut cell = format_raw(name, val);
            if is_best {
                cell.push_str(" *");
            }
            let _ = write!(body, "<td>{}</td>", xml_escape(&cell));
        }
        body.push_str("</tr>");
    }
    format!(
        r#"<h3>How to read latency</h3>
  <div class="callout">
    <strong>Lower microseconds win. A long bar next to a small µs number is the good result.</strong>
    Captions say “lower latency” / “higher latency” vs stock, not an inverted percent.
  </div>
  <table class="winners">
    <thead>{head}</thead>
    <tbody>{body}</tbody>
  </table>
  <p class="winner-note">* best (lowest µs / highest rps). Single-run max latency is noisy.</p>
"#
    )
}

fn winners_table_html(series: &[KernelSeries]) -> String {
    let mut rows = String::new();
    for metric in ordered_metrics(series) {
        let bars = compare_bars(series, &metric);
        if bars.iter().all(|b| b.skipped) {
            continue;
        }
        let stock = stock_value(series, &metric);
        let mut ranked: Vec<(&KernelSeries, f64)> = series
            .iter()
            .zip(&bars)
            .filter(|(_, bar)| !bar.skipped)
            .filter_map(|(s, bar)| bar.rel.map(|rel| (s, rel)))
            .collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let cell = |i: usize| -> String {
            ranked
                .get(i)
                .map(|(s, _)| xml_escape(&podium_cell(&metric, s, stock)))
                .unwrap_or_else(|| "—".into())
        };
        let _ = writeln!(
            rows,
            "      <tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            xml_escape(&metric_axis_label(&metric)),
            cell(0),
            cell(1),
            cell(2),
        );
    }
    format!(
        r#"{latency}<table class="winners">
    <thead>
      <tr><th>Test</th><th>1st</th><th>2nd</th><th>3rd</th></tr>
    </thead>
    <tbody>
{rows}    </tbody>
  </table>
  <p class="winner-note">The number with a unit is the raw measurement. Phrases in parentheses
     are vs stock. For latency that is lower/higher microseconds, not an inverted “faster”
     percent. 4th place is omitted. ↓ in the test name means a smaller raw value is better.</p>
{geo}"#,
        latency = latency_explainer_html(series),
        geo = geomean_table_html(series)
    )
}

fn geomean_table_html(series: &[KernelSeries]) -> String {
    let geos = kernel_geomean_vs_stock(series);
    let mut ranked: Vec<(usize, &KernelSeries, f64)> = series
        .iter()
        .enumerate()
        .filter_map(|(i, s)| geos[i].map(|g| (i, s, g)))
        .collect();
    ranked.sort_by(|a, b| {
        b.2.partial_cmp(&a.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    if ranked.is_empty() {
        return String::new();
    }
    let mut rows = String::new();
    for (place, (_i, s, geo)) in ranked.iter().enumerate() {
        let _ = writeln!(
            rows,
            "      <tr><td>{}</td><td>{}</td><td>{}</td></tr>",
            xml_escape(&ordinal(place + 1)),
            xml_escape(&winner_short_name(s)),
            xml_escape(&format_delta(*geo)),
        );
    }
    format!(
        r#"  <h3>Geomean vs stock</h3>
  <table class="winners">
    <thead>
      <tr><th>Place</th><th>Kernel</th><th>vs stock</th></tr>
    </thead>
    <tbody>
{rows}    </tbody>
  </table>
  <p class="winner-note">Geometric mean of per-test performance vs <code>1_current</code>.
     All kernels are listed (including stock at 0%). Total score is golf (lower pts = better).</p>"#
    )
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
    let bar_h = 16.0;
    let group_h = bar_h * n_k as f64 + 10.0;
    let label_w = 220.0;
    let chart_w = 520.0;
    let pad = 16.0;
    let mut body = String::new();
    body.push_str(&svg_text(
        pad,
        24.0,
        15,
        true,
        "",
        "Kernel comparison — longer bar is better",
    ));
    body.push_str(&svg_text(
        pad,
        44.0,
        11,
        false,
        "#666666",
        "↓ tests: smaller raw number is better. Last group: geomean vs stock.",
    ));
    body.push_str(&svg_text(
        pad,
        58.0,
        11,
        false,
        "#666666",
        "First number is raw. Latency says lower/higher µs vs stock. Longest bar = best — not the largest µs.",
    ));
    body.push_str(&svg_text(
        pad,
        72.0,
        11,
        false,
        "",
        &best_per_test_line(series),
    ));
    let y = 86.0;
    for (mi, name) in metrics.iter().enumerate() {
        let base_y = y + mi as f64 * group_h;
        paint_compare_bars(
            &mut body, series, name, base_y, pad, label_w, chart_w, bar_h, group_h,
        );
    }
    let geo_bars = geomean_compare_bars(series);
    let has_geomean = geo_bars.iter().any(|b| b.rel.is_some());
    let extra_groups = if has_geomean { 1.0 } else { 0.0 };
    if has_geomean {
        let base_y = y + metrics.len() as f64 * group_h;
        body.push_str(&svg_text(
            pad,
            base_y + group_h / 2.0,
            11,
            true,
            "",
            "Geomean vs stock",
        ));
        for (ki, bar) in geo_bars.iter().enumerate() {
            let by = base_y + ki as f64 * bar_h;
            let w = bar.bar_frac * chart_w;
            let color = PALETTE[ki % PALETTE.len()];
            let extra = if bar.is_best {
                r##" stroke="#222" stroke-width="1.2""##
            } else {
                ""
            };
            body.push_str(&svg_rect(
                pad + label_w,
                by,
                w.max(0.0),
                bar_h - 2.0,
                color,
                extra,
            ));
            body.push_str(&svg_text(
                pad + label_w + w + 6.0,
                by + bar_h - 4.0,
                10,
                bar.is_best,
                "",
                &bar.caption,
            ));
        }
    }
    let legend_y = y + (metrics.len() as f64 + extra_groups) * group_h + 24.0;
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
    let height = legend_y + 36.0;
    let width = pad + label_w + chart_w + 280.0;
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
schbench p99 latency (us): 20
schbench avg rps: 800
syscall getppid (Mops/s): 20.0
Total time (s): 100
";

    const LOG_B: &str = "\
Kernel: 4_final
SCX Scheduler: none
SCX Version: none
System: test box
schbench p99 latency (us): 18
schbench avg rps: 900
syscall getppid (Mops/s): 28.0
Total time (s): 90
";

    #[test]
    fn parse_extracts_kernel_and_averages_repeat_samples() {
        let doubled = format!("{LOG_A}schbench avg rps: 900\n");
        let parsed = parse_one_log(&doubled).unwrap();
        assert_eq!(parsed.kernel, "1_current");
        assert_eq!(parsed.label, "1_current");
        let samples: Vec<f64> = parsed
            .samples
            .iter()
            .filter(|(n, _)| n == "schbench avg rps")
            .map(|(_, v)| *v)
            .collect();
        assert_eq!(samples, vec![800.0, 900.0]);
    }

    /// A `kbench` log (standalone or appended to a cachyos run) must chart. These
    /// metrics are the only ones in the suite sensitive enough to show a kernel gain.
    #[test]
    fn kernel_metrics_from_kbench_are_charted() {
        let log = "\
Kernel: 4_final
syscall getppid (Mops/s): 28.831
sched pipe (Kops/s): 1135.073
sched messaging (s): 0.040
futex hash (Kops/s): 12420.479
epoll wait (Kops/s): 372.572
hackbench pipes (s): 1.234
context switch (Kops/s): 2637.897
page fault (Kops/s): 512.5
mmap/munmap (Kops/s): 88.25
vfs open/close (Kops/s): 640.75
dentry lookup (Kops/s): 310.5
unix socket (Kops/s): 455.25
udp loopback (Kops/s): 210.75
pipe throughput (Kops/s): 980.5
fork+exec (ops/s): 4210.0
io_uring (Kops/s): 150.25
buffered file io (Kops/s): 75.5
";
        let parsed = parse_one_log(log).expect("kbench log must parse");
        assert_eq!(parsed.kernel, "4_final");
        assert_eq!(
            parsed.samples.len(),
            17,
            "every kernel metric must be known"
        );

        let dir = unique_dir();
        fs::write(dir.join("benchie_abs-final_kbench.log"), log).unwrap();
        assert!(scrape_benchie_dir(&dir).unwrap());
        let svg = fs::read_to_string(dir.join("categorized_comparison_All.svg")).unwrap();
        assert!(svg.contains("syscall getppid (Mops/s)"), "{svg}");
        assert!(svg.contains("context switch (Kops/s)"), "{svg}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Latency/time metrics must not be treated as higher-is-better.
    #[test]
    fn kernel_time_metrics_are_lower_better() {
        for name in ["sched messaging (s)", "hackbench pipes (s)"] {
            let cat = CATEGORY_2
                .iter()
                .find(|c| c.name == name)
                .unwrap_or_else(|| panic!("{name} missing from metric table"));
            assert!(!cat.higher_better, "{name} is a duration");
        }
        for name in ["syscall getppid (Mops/s)", "context switch (Kops/s)"] {
            let cat = CATEGORY_2.iter().find(|c| c.name == name).unwrap();
            assert!(cat.higher_better, "{name} is a rate");
        }
    }

    #[test]
    fn scx_label_appends_scheduler_and_version() {
        let log = "\
Kernel: 6.12.1
SCX Scheduler: bpfland
SCX Version: 1.0.0
schbench avg rps: 1
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
        assert!(svg.contains("schbench avg rps"));
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
    fn cachyos_metrics_are_charted() {
        let dir = unique_dir();
        fs::write(
            dir.join("benchie_abs-current_a.log"),
            "\
Kernel: 1_current
SCX Scheduler: none
SCX Version: none
blender render: 10.0
y-cruncher pi 1b: 12.5
schbench avg rps: 800
",
        )
        .unwrap();
        let series = parse_logs(&dir).unwrap();
        assert_eq!(series[0].averages["blender render"], 10.0);
        assert_eq!(series[0].averages["y-cruncher pi 1b"], 12.5);
        assert_eq!(series[0].averages["schbench avg rps"], 800.0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn combined_cachyos_and_kbench_log_charts_both() {
        let dir = unique_dir();
        fs::write(
            dir.join("benchie_abs-current.log"),
            "\
Kernel: 1_current
SCX Scheduler: none
SCX Version: none
blender render: 10.0
hackbench pipes (s): 1.50
",
        )
        .unwrap();
        fs::write(
            dir.join("benchie_abs-final.log"),
            "\
Kernel: 4_final
SCX Scheduler: none
SCX Version: none
blender render: 9.0
hackbench pipes (s): 1.20
",
        )
        .unwrap();
        assert!(scrape_benchie_dir(&dir).unwrap());
        let svg = fs::read_to_string(dir.join("categorized_comparison_All.svg")).unwrap();
        assert!(svg.contains("blender render"), "{svg}");
        assert!(svg.contains("hackbench pipes (s)"), "{svg}");
        assert!(svg.contains("CachyOS + kbench"), "{svg}");
        let html = fs::read_to_string(dir.join("test_performance.html")).unwrap();
        assert!(html.contains("CachyOS + kbench"), "{html}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn y_cruncher_skip_hatches_on_infinity() {
        let dir = unique_dir();
        fs::write(
            dir.join("benchie_abs-current.log"),
            "\
Kernel: 1_current
SCX Scheduler: none
SCX Version: none
y-cruncher pi 1b: 12.5
",
        )
        .unwrap();
        fs::write(
            dir.join("benchie_infinity.log"),
            "\
Kernel: 4_final_infinity
SCX Scheduler: none
SCX Version: none
y-cruncher pi 1b: 0.01
",
        )
        .unwrap();
        assert!(scrape_benchie_dir(&dir).unwrap());
        let svg = fs::read_to_string(dir.join("categorized_comparison_All.svg")).unwrap();
        assert!(svg.contains("SKIPPED*"), "{svg}");
        assert!(svg.contains("url(#hatch)"), "{svg}");
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
schbench p99 latency (us): 18
schbench avg rps: 900
cyclictest max latency (us): 40
syscall getppid (Mops/s): 28.0
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
        assert!(!svg.contains("stress-ng cpu-cache-mem"), "{svg}");
        assert!(!svg.contains("blender render"), "{svg}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn averages_repeat_lines_in_one_file() {
        let dir = unique_dir();
        let log = format!("{LOG_A}schbench avg rps: 1000\n");
        fs::write(dir.join("benchie_abs-current_a.log"), log).unwrap();
        let series = parse_logs(&dir).unwrap();
        let v = series[0].averages["schbench avg rps"];
        assert!((v - 900.0).abs() < 1e-9, "{v}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Bar widths between a metric label and the next left-column label (`x="16.0"`).
    fn svg_bar_widths(svg: &str, metric: &str) -> Vec<f64> {
        let start = svg
            .find(metric)
            .unwrap_or_else(|| panic!("{metric} missing"));
        let after_label = svg[start..].find("</text>").map(|i| start + i + 7).unwrap();
        let rest = &svg[after_label..];
        let next = rest.find(r#"<text x="16.0""#).unwrap_or(rest.len());
        rest[..next]
            .split(r#"<rect x="236.0""#)
            .skip(1)
            .filter_map(|s| {
                s.split(r#"width=""#)
                    .nth(1)?
                    .split('"')
                    .next()?
                    .parse()
                    .ok()
            })
            .collect()
    }

    /// Pipe throughput is ~1000× syscall rate. A global max makes syscall bars vanish
    /// (~0.4px). Each test scales on its own. Captions are vs stock; geomean is vs stock.
    #[test]
    fn kernel_comparison_scales_per_metric_and_ranks_vs_worst() {
        let dir = unique_dir();
        fs::write(
            dir.join("benchie_abs-current.log"),
            "\
Kernel: 1_current
SCX Scheduler: none
SCX Version: none
syscall getppid (Mops/s): 30.0
pipe throughput (Kops/s): 40000.0
hackbench pipes (s): 1.50
",
        )
        .unwrap();
        fs::write(
            dir.join("benchie_abs-final.log"),
            "\
Kernel: 4_final
SCX Scheduler: none
SCX Version: none
syscall getppid (Mops/s): 33.0
pipe throughput (Kops/s): 36000.0
hackbench pipes (s): 1.20
",
        )
        .unwrap();
        assert!(scrape_benchie_dir(&dir).unwrap());
        let svg = fs::read_to_string(dir.join("kernel_version_comparison_All.svg")).unwrap();

        let syscall_w = svg_bar_widths(&svg, "syscall getppid (Mops/s)");
        assert_eq!(syscall_w.len(), 2, "{syscall_w:?} in {svg}");
        let syscall_max = syscall_w.iter().copied().fold(0.0, f64::max);
        let syscall_min = syscall_w.iter().copied().fold(f64::MAX, f64::min);
        assert!(
            syscall_max > 400.0,
            "syscall winner must use per-metric full scale, got {syscall_w:?}"
        );
        assert!(
            syscall_min > 200.0,
            "stock syscall must stay visible, got {syscall_w:?}"
        );

        assert!(svg.contains("stock"), "stock caption missing:\n{svg}");
        assert!(svg.contains("worst"), "worst caption missing:\n{svg}");
        assert!(
            svg.contains("10.0% higher"),
            "syscall 33 vs stock 30 is 10% higher:\n{svg}"
        );
        assert!(
            svg.contains("10.0% lower"),
            "pipe 36000 vs stock 40000 is 10% lower:\n{svg}"
        );
        assert!(
            svg.contains("25.0% faster"),
            "hackbench 1.20 vs stock 1.50 is 25% faster:\n{svg}"
        );
        assert!(svg.contains("best"), "winner mark missing:\n{svg}");
        assert!(
            svg.contains("Geomean vs stock"),
            "geomean group missing:\n{svg}"
        );
        assert!(
            svg.contains("+7.4%"),
            "geomean of 110%, 90%, 125% vs stock is +7.4%:\n{svg}"
        );

        let html = fs::read_to_string(dir.join("test_performance.html")).unwrap();
        assert!(
            html.contains("Best per test"),
            "per-test page should summarise winners:\n{html}"
        );
        assert!(
            html.contains("<th>Test</th>"),
            "ranking table needs a Test column:\n{html}"
        );
        assert!(
            html.contains("<th>1st</th>"),
            "ranking table needs a 1st column:\n{html}"
        );
        assert!(
            html.contains("<th>2nd</th>"),
            "ranking table needs a 2nd column:\n{html}"
        );
        assert!(
            html.contains("<th>3rd</th>"),
            "ranking table needs a 3rd column:\n{html}"
        );
        assert!(
            html.contains("<td>syscall getppid (Mops/s)  ↑</td>"),
            "table must name the test:\n{html}"
        );
        assert!(
            html.contains("Propeller 33.00 Mops/s (10.0% higher)"),
            "syscall 1st is Propeller vs stock:\n{html}"
        );
        assert!(
            html.contains("hackbench pipes (s)"),
            "hackbench row missing:\n{html}"
        );
        assert!(
            html.contains("Propeller 1.200 s (25.0% faster)"),
            "hackbench 1st is Propeller vs stock:\n{html}"
        );
        assert!(
            html.contains("stock 40000.00 Kops/s"),
            "pipe 1st is stock:\n{html}"
        );
        assert!(
            html.contains("<h3>Geomean vs stock</h3>"),
            "geomean table missing:\n{html}"
        );
        assert!(
            html.contains("<td>1st</td>"),
            "geomean must rank 1st:\n{html}"
        );
        assert!(
            html.contains("<td>2nd</td>"),
            "geomean must rank 2nd:\n{html}"
        );
        assert!(
            html.contains("Propeller") && html.contains("+7.4%"),
            "geomean 1st is Propeller +7.4% vs stock:\n{html}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ranking_table_shows_podium_and_all_geomean_places() {
        let dir = unique_dir();
        fs::write(
            dir.join("benchie_abs-current.log"),
            "\
Kernel: 1_current
syscall getppid (Mops/s): 10.0
pipe throughput (Kops/s): 10.0
",
        )
        .unwrap();
        fs::write(
            dir.join("benchie_abs-debug_clean.log"),
            "\
Kernel: 2_debug_clean
syscall getppid (Mops/s): 11.0
pipe throughput (Kops/s): 9.0
",
        )
        .unwrap();
        fs::write(
            dir.join("benchie_abs-autofdo_clean.log"),
            "\
Kernel: 3_autofdo_clean
syscall getppid (Mops/s): 12.0
pipe throughput (Kops/s): 10.0
",
        )
        .unwrap();
        fs::write(
            dir.join("benchie_abs-final.log"),
            "\
Kernel: 4_final
syscall getppid (Mops/s): 13.0
pipe throughput (Kops/s): 11.0
",
        )
        .unwrap();
        assert!(scrape_benchie_dir(&dir).unwrap());
        let html = fs::read_to_string(dir.join("test_performance.html")).unwrap();
        assert!(
            html.contains("<th>1st</th>")
                && html.contains("<th>2nd</th>")
                && html.contains("<th>3rd</th>"),
            "podium columns missing:\n{html}"
        );
        assert!(
            html.contains("Propeller 13.00 Mops/s (30.0% higher)"),
            "syscall 1st vs stock is Propeller +30%:\n{html}"
        );
        assert!(
            html.contains("AutoFDO 12.00 Mops/s (20.0% higher)"),
            "syscall 2nd is AutoFDO +20%:\n{html}"
        );
        assert!(
            html.contains("debug 11.00 Mops/s (10.0% higher)"),
            "syscall 3rd is debug +10%:\n{html}"
        );
        assert!(
            html.contains("<td>1st</td>")
                && html.contains("<td>2nd</td>")
                && html.contains("<td>3rd</td>")
                && html.contains("<td>4th</td>"),
            "geomean must list all four places:\n{html}"
        );
        assert!(
            html.contains("+19.6%"),
            "Propeller geomean sqrt(1.3*1.1) is +19.6% vs stock:\n{html}"
        );
        assert!(
            html.contains("+9.5%"),
            "AutoFDO geomean sqrt(1.2*1.0) is +9.5% vs stock:\n{html}"
        );
        assert!(
            html.contains("-0.5%"),
            "debug geomean sqrt(1.1*0.9) is −0.5% vs stock:\n{html}"
        );
        let geo_idx = html
            .find("<h3>Geomean vs stock</h3>")
            .expect("geomean heading");
        let geo = &html[geo_idx..];
        assert!(
            geo.contains("<td>stock</td>") && geo.contains("<td>0%</td>"),
            "stock is geomean baseline at 0%:\n{geo}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn total_score_is_golf() {
        assert!(
            !metric_higher_better("Total score"),
            "Total score is golf (lower pts is better)"
        );
    }

    #[test]
    fn latency_captions_use_raw_us_and_lower_higher() {
        let dir = unique_dir();
        fs::write(
            dir.join("benchie_abs-current.log"),
            "\
Kernel: 1_current
cyclictest max latency (us): 911
schbench p99 latency (us): 494
",
        )
        .unwrap();
        fs::write(
            dir.join("benchie_abs-final.log"),
            "\
Kernel: 4_final
cyclictest max latency (us): 427
schbench p99 latency (us): 655
",
        )
        .unwrap();
        assert!(scrape_benchie_dir(&dir).unwrap());
        let svg = fs::read_to_string(dir.join("kernel_version_comparison_All.svg")).unwrap();
        assert!(svg.contains("427 µs"), "raw winner missing:\n{svg}");
        assert!(svg.contains("911 µs"), "raw stock missing:\n{svg}");
        assert!(
            svg.contains("53% lower latency"),
            "cyclictest must say lower latency, not an inverted +113%:\n{svg}"
        );
        assert!(
            !svg.contains("+113"),
            "inverted +113% caption is misleading:\n{svg}"
        );
        assert!(svg.contains("655 µs"), "{svg}");
        assert!(
            svg.contains("33% higher latency"),
            "schbench p99 Propeller is higher latency:\n{svg}"
        );
        let cat = fs::read_to_string(dir.join("categorized_comparison_All.svg")).unwrap();
        let kernel_w = svg_bar_widths(&svg, "cyclictest max latency (us)");
        let cat_w = svg_bar_widths(&cat, "cyclictest max latency (us)");
        assert!(
            kernel_w.len() >= 2,
            "{kernel_w:?}\n{svg}"
        );
        assert!(
            cat_w.len() >= 2,
            "{cat_w:?}\n{cat}"
        );
        assert!(
            kernel_w[1] > kernel_w[0],
            "Propeller (lower µs) must get the longer bar:\n{kernel_w:?}\n{svg}"
        );
        assert!(
            cat_w[1] > cat_w[0],
            "categorized chart must use the same encoding:\n{cat_w:?}\n{cat}"
        );
        let html = fs::read_to_string(dir.join("test_performance.html")).unwrap();
        assert!(
            html.contains("53% lower latency"),
            "winners table must not use inverted +113%:\n{html}"
        );
        assert!(
            html.contains("How to read latency") || html.contains("lower microseconds"),
            "report must explain latency bars:\n{html}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn total_score_lower_pts_is_best() {
        let dir = unique_dir();
        fs::write(
            dir.join("benchie_abs-current.log"),
            "\
Kernel: 1_current
Total score: 67.74
",
        )
        .unwrap();
        fs::write(
            dir.join("benchie_abs-final.log"),
            "\
Kernel: 4_final
Total score: 64.85
",
        )
        .unwrap();
        assert!(scrape_benchie_dir(&dir).unwrap());
        let svg = fs::read_to_string(dir.join("kernel_version_comparison_All.svg")).unwrap();
        assert!(svg.contains("64.85 pts"), "{svg}");
        assert!(svg.contains("4.5% better"), "golf win vs stock:\n{svg}");
        let widths = svg_bar_widths(&svg, "Total score");
        assert_eq!(widths.len(), 2, "{widths:?}\n{svg}");
        assert!(
            widths[1] > widths[0],
            "lower Total score must get the longer bar:\n{widths:?}\n{svg}"
        );
        let html = fs::read_to_string(dir.join("winners_table.html")).unwrap();
        assert!(
            html.contains("Propeller") && html.contains("64.85 pts"),
            "Propeller must win Total score:\n{html}"
        );
        assert!(
            !html.contains("stock") || !html.contains("1st") || html.contains("Propeller"),
            "{html}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
