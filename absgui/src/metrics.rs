use crate::app_settings::AppTheme;
use crate::style;
use iced::widget::{container, row, text};
use iced::{Alignment, Element, Font, Padding};

#[derive(Debug, Clone, Default)]
pub struct SystemMetrics {
    pub cpu_percent: f32,
    pub cpu_cores: usize,
    pub ram_used_gb: f32,
    pub ram_total_gb: f32,
    pub ram_percent: f32,
    pub cpu_temp_c: Option<f32>,
    pub cpu_freq_ghz: Option<f32>,
    pub boot_release: Option<String>,
    pub sched_ext: Option<String>,
}

#[derive(Debug, Default)]
pub struct MetricsSampler {
    last_cpu_total: u64,
    last_cpu_idle: u64,
    pub current: SystemMetrics,
}

impl MetricsSampler {
    pub fn new() -> Self {
        let mut s = Self::default();
        s.sample();
        s
    }

    pub fn sample(&mut self) -> SystemMetrics {
        if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
            let mut mem_total_kb = 0u64;
            let mut mem_avail_kb = 0u64;
            for line in content.lines() {
                if let Some(rest) = line.strip_prefix("MemTotal:") {
                    if let Some(num) = rest.split_whitespace().next() {
                        mem_total_kb = num.parse().unwrap_or(0);
                    }
                } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
                    if let Some(num) = rest.split_whitespace().next() {
                        mem_avail_kb = num.parse().unwrap_or(0);
                    }
                }
            }
            if mem_total_kb > 0 {
                let total_gb = mem_total_kb as f32 / (1024.0 * 1024.0);
                let avail_gb = mem_avail_kb as f32 / (1024.0 * 1024.0);
                let used_gb = (total_gb - avail_gb).max(0.0);
                self.current.ram_total_gb = total_gb;
                self.current.ram_used_gb = used_gb;
                self.current.ram_percent = ((used_gb / total_gb) * 100.0).clamp(0.0, 100.0);
            }
        }

        if let Ok(content) = std::fs::read_to_string("/proc/stat") {
            if let Some(line) = content.lines().next() {
                if line.starts_with("cpu ") {
                    let parts: Vec<u64> = line
                        .split_whitespace()
                        .skip(1)
                        .filter_map(|s| s.parse().ok())
                        .collect();
                    if parts.len() >= 4 {
                        let total: u64 = parts.iter().sum();
                        let idle = parts[3] + parts.get(4).copied().unwrap_or(0);
                        if self.last_cpu_total > 0 && total > self.last_cpu_total {
                            let delta_total = total - self.last_cpu_total;
                            let delta_idle = idle.saturating_sub(self.last_cpu_idle);
                            let delta_active = delta_total.saturating_sub(delta_idle);
                            self.current.cpu_percent = ((delta_active as f32 / delta_total as f32)
                                * 100.0)
                                .clamp(0.0, 100.0);
                        }
                        self.last_cpu_total = total;
                        self.last_cpu_idle = idle;
                    }
                }
            }
        }

        self.current.cpu_cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        self.current.cpu_temp_c = sample_cpu_temp_c();
        self.current.cpu_freq_ghz = sample_cpu_freq_ghz();
        if self.current.boot_release.is_none() {
            self.current.boot_release = sample_uname_r();
        }
        self.current.sched_ext = sample_sched_ext();
        self.current.clone()
    }
}

fn plausible_cpu_c(c: f32) -> bool {
    (10.0..110.0).contains(&c)
}

fn milli_celsius(raw: &str) -> Option<f32> {
    let milli: f32 = raw.trim().parse().ok()?;
    let c = milli / 1000.0;
    plausible_cpu_c(c).then_some(c)
}

fn sample_cpu_temp_c() -> Option<f32> {
    sample_hwmon_cpu_temp().or_else(sample_thermal_zone_temp)
}

fn sample_thermal_zone_temp() -> Option<f32> {
    let dir = std::fs::read_dir("/sys/class/thermal").ok()?;
    let mut fallback = None;
    for ent in dir.flatten() {
        let path = ent.path();
        let typ = std::fs::read_to_string(path.join("type")).unwrap_or_default();
        let typ = typ.trim().to_ascii_lowercase();
        let Ok(raw) = std::fs::read_to_string(path.join("temp")) else {
            continue;
        };
        let Some(c) = milli_celsius(&raw) else {
            continue;
        };
        let prefer = typ.contains("x86_pkg")
            || typ.contains("k10temp")
            || typ.contains("tctl")
            || typ.contains("cpu")
            || typ == "acpitz";
        if prefer {
            return Some(c);
        }
        if fallback.is_none() {
            fallback = Some(c);
        }
    }
    fallback
}

/// Package/die temp from hwmon (k10temp Tctl, coretemp, …). Skips GPU, NVMe, and 0 °C PCH stubs.
fn sample_hwmon_cpu_temp() -> Option<f32> {
    let dir = std::fs::read_dir("/sys/class/hwmon").ok()?;
    let mut best: Option<(u8, f32)> = None;
    for ent in dir.flatten() {
        let path = ent.path();
        let name = std::fs::read_to_string(path.join("name")).unwrap_or_default();
        let name = name.trim().to_ascii_lowercase();
        if name == "nvme"
            || name == "amdgpu"
            || name == "radeon"
            || name == "nouveau"
            || name == "i915"
            || name == "xe"
            || name.starts_with("iwl")
            || name.starts_with("ath")
            || name.starts_with("mt79")
            || name.starts_with("hidpp")
            || name.starts_with("r8169")
        {
            continue;
        }
        let chip_rank: u8 = match name.as_str() {
            "k10temp" | "zenpower" | "coretemp" | "k8temp" | "cpu_thermal" => 0,
            _ => 8,
        };
        let Ok(files) = std::fs::read_dir(&path) else {
            continue;
        };
        for file in files.flatten() {
            let fname = file.file_name();
            let fname = fname.to_string_lossy();
            let Some(stem) = fname.strip_suffix("_input") else {
                continue;
            };
            if !stem.starts_with("temp") {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(file.path()) else {
                continue;
            };
            let Some(c) = milli_celsius(&raw) else {
                continue;
            };
            let label = std::fs::read_to_string(path.join(format!("{stem}_label")))
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            let label_rank: u8 = if label.contains("tctl")
                || label.contains("tdie")
                || label.contains("package")
                || label.contains("physical")
            {
                0
            } else if chip_rank == 0 && (label.is_empty() || stem == "temp1") {
                1
            } else if label == "cputin" || label.contains("cpu") {
                4
            } else if chip_rank == 0 {
                3
            } else {
                continue;
            };
            let rank = chip_rank.saturating_mul(10).saturating_add(label_rank);
            let better = match best {
                Some((best_rank, _)) => rank < best_rank,
                None => true,
            };
            if better {
                best = Some((rank, c));
            }
        }
    }
    best.map(|(_, c)| c)
}

fn sample_cpu_freq_ghz() -> Option<f32> {
    let raw = std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq")
        .or_else(|_| {
            std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_cur_freq")
        })
        .ok()?;
    let khz: f32 = raw.trim().parse().ok()?;
    if khz <= 0.0 {
        return None;
    }
    Some(khz / 1_000_000.0)
}

fn sample_uname_r() -> Option<String> {
    let out = std::process::Command::new("uname")
        .arg("-r")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let s = s.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn sample_sched_ext() -> Option<String> {
    for path in [
        "/sys/kernel/sched_ext/root/ops",
        "/sys/kernel/sched_ext/root/enable",
    ] {
        if let Ok(s) = std::fs::read_to_string(path) {
            let s = s.trim().to_string();
            if !s.is_empty() && s != "0" && s != "none" {
                return Some(s);
            }
        }
    }
    None
}

fn cpu_value_text(metrics: &SystemMetrics) -> String {
    let pct = format!("{:.0}%", metrics.cpu_percent);
    match (metrics.cpu_temp_c, metrics.cpu_freq_ghz) {
        (Some(t), Some(f)) => format!("{pct}  {t:.0}°C  {f:.1}GHz"),
        (Some(t), None) => format!("{pct}  {t:.0}°C"),
        (None, Some(f)) => format!("{pct}  {f:.1}GHz"),
        (None, None) => pct,
    }
}

/// Compact hardware pill for the top navigation bar (no meter bars).
pub fn hardware_pill_widget<'a, Message: 'a>(
    metrics: &SystemMetrics,
    app_theme: AppTheme,
    compact: bool,
) -> Element<'a, Message> {
    let cyan = style::primary(app_theme);
    let emerald = match app_theme {
        AppTheme::Dark => iced::Color::from_rgb8(0x34, 0xd3, 0x99),
        AppTheme::Light => iced::Color::from_rgb8(0x05, 0x96, 0x69),
    };
    let mono = Font::MONOSPACE;

    let cpu_value = if compact {
        format!("{:.0}%", metrics.cpu_percent)
    } else {
        cpu_value_text(metrics)
    };
    let mut cpu = row![text("⚡").size(13).color(cyan)];
    if !compact {
        cpu = cpu.push(
            text("CPU:")
                .size(style::TEXT_CHIP)
                .color(style::muted(app_theme)),
        );
    }
    let cpu = cpu
        .push(
            text(cpu_value)
                .size(style::TEXT_CHIP)
                .font(Font {
                    weight: iced::font::Weight::Bold,
                    ..mono
                })
                .color(cyan),
        )
        .spacing(6)
        .align_y(Alignment::Center);

    let ram_value = if compact {
        format!("{:.0}%", metrics.ram_percent)
    } else {
        format!(
            "{:.1} / {:.0} GB ({:.0}%)",
            metrics.ram_used_gb, metrics.ram_total_gb, metrics.ram_percent
        )
    };
    let mut ram = row![text("▭").size(13).color(emerald)];
    if !compact {
        ram = ram.push(
            text("RAM:")
                .size(style::TEXT_CHIP)
                .color(style::muted(app_theme)),
        );
    }
    let ram = ram
        .push(
            text(ram_value)
                .size(style::TEXT_CHIP)
                .font(Font {
                    weight: iced::font::Weight::Bold,
                    ..mono
                })
                .color(emerald),
        )
        .spacing(6)
        .align_y(Alignment::Center);

    let gap = if compact { 10 } else { 16 };
    let pad = if compact {
        Padding::from([5.0, 10.0])
    } else {
        Padding::from([5.0, 14.0])
    };
    container(row![cpu, ram].spacing(gap).align_y(Alignment::Center))
        .padding(pad)
        .style(style::hardware_pill(app_theme))
        .into()
}

#[cfg(test)]
mod tests {
    use super::{cpu_value_text, milli_celsius, SystemMetrics};

    #[test]
    fn milli_celsius_skips_zero_and_gpu_hot() {
        assert_eq!(milli_celsius("0"), None);
        assert_eq!(milli_celsius("-62000"), None);
        assert_eq!(milli_celsius("112000"), None);
        assert_eq!(milli_celsius("53625").map(|c| c.round()), Some(54.0));
    }

    #[test]
    fn cpu_text_labels_units_without_at_sign() {
        let mut m = SystemMetrics::default();
        m.cpu_percent = 0.0;
        m.cpu_freq_ghz = Some(4.7);
        assert_eq!(cpu_value_text(&m), "0%  4.7GHz");
        m.cpu_temp_c = Some(54.0);
        assert_eq!(cpu_value_text(&m), "0%  54°C  4.7GHz");
    }
}
