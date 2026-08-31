//! Parse `benchie_*.log` comparison files and write charts.
//!
//! kbench writes this format. Old CachyOS-benchmarker metric names are ignored.

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
    CATEGORY_2.iter().any(|c| c.name == name) || EXTRA_METRICS.contains(&name)
}

fn ordered_metrics(series: &[KernelSeries]) -> Vec<String> {
    let present: std::collections::HashSet<&str> = series
        .iter()
        .flat_map(|s| s.averages.keys().map(String::as_str))
        .collect();
    let mut out = Vec::new();
    for name in CATEGORY_2
        .iter()
        .map(|c| c.name)
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
            if let Some(v) = s.averages.get(name) {
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
                metrics.insert(k.clone(), serde_json::Value::from(*v));
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
    let wins = wins_vs_stock_line(series);
    let table = winners_table_html(series);
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Test Performance</title>
    <style>
      :root {{ color-scheme: dark light; }}
      body {{ font-family: ui-sans-serif, system-ui, sans-serif; max-width: 960px; margin: 2rem auto; padding: 0 1.25rem 3rem; line-height: 1.5; }}
      table.winners {{ width: 100%; border-collapse: collapse; margin: 0.75rem 0 0.25rem; }}
      table.winners th, table.winners td {{ text-align: left; padding: 0.35rem 0.6rem; border-bottom: 1px solid #3a4550; }}
      table.winners th {{ font-size: 0.85rem; color: #8b98a5; }}
      .winner-note {{ color: #8b98a5; font-size: 0.9rem; }}
    </style>
</head>
<body>
    <h1>Test Performance</h1>

    <h2>Winner vs stock</h2>
    {table}

    <h2>Performance Comparison Between Different Kernel Versions</h2>
    <p>{wins}. Bar length is performance relative to stock (1_current);
       longer is better. Captions are the raw score and % vs stock.</p>
    <img src="kernel_version_comparison_All.svg" alt="Kernel Version Comparison - All Kernels"
         style="max-width: 100%; height: auto;">

    <h2>Categorized Results</h2>
    <p>kbench kernel-path metrics (↓ lower is better, ↑ higher is better).</p>
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
        "kbench — Kernel path metrics",
    ));
    for s in series.iter().rev() {
        let cat2: Vec<(&Cat2, f64)> = CATEGORY_2
            .iter()
            .filter_map(|c| s.averages.get(c.name).map(|v| (c, *v)))
            .collect();
        y += 28.0;
        let _ = write!(
            body,
            "{}",
            svg_text(pad, y, 13, true, "", &format!("{} — Kernel path", s.label))
        );
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
    let height = y + pad;
    let width = pad + label_w + chart_w + 120.0;
    format!("{}{body}</svg>", svg_open(width, height))
}

fn is_stock_kernel(s: &KernelSeries) -> bool {
    s.kernel == "1_current" || s.label == "1_current" || s.kernel.starts_with("1_current")
}

fn metric_higher_better(name: &str) -> bool {
    if name == "Total time (s)" {
        return false;
    }
    if let Some(c) = CATEGORY_2.iter().find(|c| c.name == name) {
        return c.higher_better;
    }
    name == "Total score"
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
}

fn compare_bars(series: &[KernelSeries], metric: &str) -> Vec<CompareBar> {
    let higher = metric_higher_better(metric);
    let baseline = series
        .iter()
        .find(|s| is_stock_kernel(s))
        .and_then(|s| s.averages.get(metric).copied())
        .filter(|v| *v > 0.0 && v.is_finite());

    let scores: Vec<Option<f64>> = series
        .iter()
        .map(|s| {
            let val = s.averages.get(metric).copied().unwrap_or(0.0);
            if let Some(b) = baseline {
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

    series
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let skipped = false;
            let val = s.averages.get(metric).copied().unwrap_or(0.0);
            let is_baseline = is_stock_kernel(s);
            let score = scores[i];
            let is_best =
                !skipped && score.is_some_and(|sc| (sc - best).abs() <= best.abs() * 1e-9 + 1e-6);
            let bar_frac = score
                .map(|sc| (sc / max_score).clamp(0.0, 1.0))
                .unwrap_or(0.0);
            let caption = if skipped {
                "SKIPPED*".into()
            } else {
                let mut c = format!("{val:.2}");
                if is_baseline {
                    c.push_str("  stock");
                } else if baseline.is_some()
                    && let Some(rel) = score
                {
                    c.push_str(&format!("  {:+.1}%", rel - 100.0));
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
            }
        })
        .collect()
}

fn wins_vs_stock_line(series: &[KernelSeries]) -> String {
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
    format!("Wins vs stock: {}", parts.join(" · "))
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

fn vs_stock_cell(series: &[KernelSeries], metric: &str, winners: &[&KernelSeries]) -> String {
    if winners.iter().any(|s| is_stock_kernel(s)) {
        return "0%".into();
    }
    let Some(baseline) = series
        .iter()
        .find(|s| is_stock_kernel(s))
        .and_then(|s| s.averages.get(metric).copied())
    else {
        return "—".into();
    };
    let Some(val) = winners
        .first()
        .and_then(|s| s.averages.get(metric).copied())
    else {
        return "—".into();
    };
    relative_performance(val, baseline, metric_higher_better(metric))
        .map(|rel| format!("{:+.1}%", rel - 100.0))
        .unwrap_or_else(|| "—".into())
}

fn winners_table_html(series: &[KernelSeries]) -> String {
    let mut rows = String::new();
    for metric in ordered_metrics(series) {
        let bars = compare_bars(series, &metric);
        let winners: Vec<&KernelSeries> = series
            .iter()
            .zip(&bars)
            .filter(|(_, bar)| bar.is_best && !bar.skipped)
            .map(|(s, _)| s)
            .collect();
        if winners.is_empty() {
            continue;
        }
        let names = winners
            .iter()
            .map(|s| winner_short_name(s))
            .collect::<Vec<_>>()
            .join(", ");
        let vs = vs_stock_cell(series, &metric, &winners);
        let _ = writeln!(
            rows,
            "      <tr><td>{}</td><td>{}</td><td>{}</td></tr>",
            xml_escape(&metric),
            xml_escape(&names),
            xml_escape(&vs)
        );
    }
    format!(
        r#"<table class="winners">
    <thead>
      <tr><th>Test</th><th>Winner</th><th>vs stock</th></tr>
    </thead>
    <tbody>
{rows}    </tbody>
  </table>
  <p class="winner-note">vs stock is how much better the winner is than <code>1_current</code>.
     Positive is faster / higher throughput. Stock winning a test is 0%.</p>"#
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
    let y = 72.0;
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
        "Bar length is performance vs stock (1_current). Captions: raw score and % vs stock.",
    ));
    body.push_str(&svg_text(
        pad,
        60.0,
        11,
        false,
        "",
        &wins_vs_stock_line(series),
    ));
    for (mi, name) in metrics.iter().enumerate() {
        let bars = compare_bars(series, name);
        let base_y = y + mi as f64 * group_h;
        body.push_str(&svg_text(pad, base_y + group_h / 2.0, 11, false, "", name));
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
    let height = legend_y + 36.0;
    let width = pad + label_w + chart_w + 200.0;
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
    fn old_cachyos_metrics_are_ignored() {
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
        assert!(!series[0].averages.contains_key("blender render"));
        assert!(!series[0].averages.contains_key("y-cruncher pi 1b"));
        assert_eq!(series[0].averages["schbench avg rps"], 800.0);
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
    /// (~0.4px). Each test must scale on its own, and captions must say % vs stock.
    #[test]
    fn kernel_comparison_scales_per_metric_and_shows_delta_vs_stock() {
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

        assert!(svg.contains("stock"), "baseline caption missing:\n{svg}");
        assert!(
            svg.contains("+10.0%"),
            "syscall 33 vs 30 is +10% vs stock:\n{svg}"
        );
        assert!(
            svg.contains("-10.0%"),
            "pipe 36000 vs 40000 is −10% vs stock:\n{svg}"
        );
        assert!(
            svg.contains("+25.0%"),
            "hackbench 1.20 vs 1.50 is +25% (lower-is-better):\n{svg}"
        );
        assert!(svg.contains("best"), "winner mark missing:\n{svg}");

        let html = fs::read_to_string(dir.join("test_performance.html")).unwrap();
        assert!(
            html.contains("Wins vs stock"),
            "per-test page should summarise winners:\n{html}"
        );
        assert!(
            html.contains("<th>Test</th>"),
            "winner table needs a Test column:\n{html}"
        );
        assert!(
            html.contains("<th>Winner</th>"),
            "winner table needs a Winner column:\n{html}"
        );
        assert!(
            html.contains("<th>vs stock</th>"),
            "winner table needs a vs stock column:\n{html}"
        );
        assert!(
            html.contains("<td>syscall getppid (Mops/s)</td>"),
            "table must name the test:\n{html}"
        );
        assert!(
            html.contains("<td>Propeller</td>"),
            "4_final is the Propeller kernel:\n{html}"
        );
        assert!(
            html.contains("<td>+10.0%</td>"),
            "syscall winner is +10% vs stock:\n{html}"
        );
        assert!(
            html.contains("<td>hackbench pipes (s)</td>"),
            "hackbench row missing:\n{html}"
        );
        assert!(
            html.contains("<td>+25.0%</td>"),
            "hackbench winner is +25% vs stock:\n{html}"
        );
        assert!(
            html.contains("<td>stock</td>"),
            "pipe winner is stock:\n{html}"
        );
        assert!(
            html.contains("<td>0%</td>"),
            "stock winning a test is 0% vs stock:\n{html}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
