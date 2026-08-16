//! Parse tmpfs `size=` values and compare them to installed RAM.

#[derive(Debug, Clone, PartialEq)]
pub enum SizeVsRam {
    Invalid,
    Fits { bytes: u64, ratio: f32 },
    Exceeds { bytes: u64, ratio: f32 },
}

pub fn mem_total_bytes() -> Option<u64> {
    mem_total_and_used().map(|(total, _)| total)
}

/// `(MemTotal, MemTotal − MemAvailable)` in bytes.
pub fn mem_total_and_used() -> Option<(u64, u64)> {
    let content = std::fs::read_to_string("/proc/meminfo").ok()?;
    let mut total_kb = 0u64;
    let mut avail_kb = None;
    let mut free_kb = 0u64;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            if let Some(num) = rest.split_whitespace().next() {
                total_kb = num.parse().unwrap_or(0);
            }
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            if let Some(num) = rest.split_whitespace().next() {
                avail_kb = Some(num.parse().unwrap_or(0));
            }
        } else if let Some(rest) = line.strip_prefix("MemFree:") {
            if let Some(num) = rest.split_whitespace().next() {
                free_kb = num.parse().unwrap_or(0);
            }
        }
    }
    if total_kb == 0 {
        return None;
    }
    let avail_kb = avail_kb.unwrap_or(free_kb);
    let total = total_kb.saturating_mul(1024);
    let used = total.saturating_sub(avail_kb.saturating_mul(1024));
    Some((total, used))
}

/// Left-only, overlap, gap, right-only fractions of a dual-ended share bar.
pub fn share_bar_segments(left: f32, right: f32) -> (f32, f32, f32, f32) {
    let left = left.clamp(0.0, 1.0);
    let right = right.clamp(0.0, 1.0);
    let overlap = (left + right - 1.0).max(0.0);
    let left_only = left - overlap;
    let right_only = right - overlap;
    let gap = (1.0 - left_only - overlap - right_only).max(0.0);
    (left_only, overlap, gap, right_only)
}

pub fn deficiency_bytes(ramdisk: u64, used: u64, total: u64) -> u64 {
    ramdisk.saturating_add(used).saturating_sub(total)
}

pub fn check(size: &str, mem_total_bytes: u64) -> SizeVsRam {
    let Ok(bytes) = size_bytes(size, mem_total_bytes) else {
        return SizeVsRam::Invalid;
    };
    if mem_total_bytes == 0 {
        return SizeVsRam::Fits { bytes, ratio: 0.0 };
    }
    let ratio = bytes as f32 / mem_total_bytes as f32;
    if bytes > mem_total_bytes {
        SizeVsRam::Exceeds { bytes, ratio }
    } else {
        SizeVsRam::Fits { bytes, ratio }
    }
}

pub fn ensure_fits(size: &str, mem_total_bytes: u64) -> Result<(), String> {
    match check(size, mem_total_bytes) {
        SizeVsRam::Fits { .. } => Ok(()),
        SizeVsRam::Invalid => Err(abs_i18n::t("gui.msg.ramdisk_size_invalid").into()),
        SizeVsRam::Exceeds { .. } => Err(abs_i18n::tf(
            "gui.msg.ramdisk_size_exceeds_ram",
            &[("size", size.trim()), ("ram", &fmt_bytes(mem_total_bytes))],
        )),
    }
}

pub fn fmt_bytes(bytes: u64) -> String {
    let g = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    if g >= 10.0 {
        format!("{g:.0}G")
    } else {
        format!("{g:.1}G")
    }
}

fn size_bytes(size: &str, mem_total_bytes: u64) -> Result<u64, ()> {
    let s = size.trim();
    if s.is_empty() || s.contains(',') || s.contains('=') || s.contains(' ') {
        return Err(());
    }
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 {
        return Err(());
    }
    let n: u64 = s[..i].parse().map_err(|_| ())?;
    match &s[i..] {
        "" => Ok(n),
        "%" => {
            if n > 100 {
                return Err(());
            }
            Ok(mem_total_bytes.saturating_mul(n) / 100)
        }
        "k" | "K" | "ki" | "Ki" => n.checked_mul(1024).ok_or(()),
        "m" | "M" | "mi" | "Mi" => n.checked_mul(1024 * 1024).ok_or(()),
        "g" | "G" | "gi" | "Gi" => n.checked_mul(1024 * 1024 * 1024).ok_or(()),
        "t" | "T" | "ti" | "Ti" => n.checked_mul(1024u64.pow(4)).ok_or(()),
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAM_32G: u64 = 32 * 1024 * 1024 * 1024;

    #[test]
    fn sixteen_g_fits_32g() {
        match check("16G", RAM_32G) {
            SizeVsRam::Fits { bytes, ratio } => {
                assert_eq!(bytes, 16 * 1024 * 1024 * 1024);
                assert!((ratio - 0.5).abs() < 0.01);
            }
            other => panic!("{other:?}"),
        }
        assert!(ensure_fits("16G", RAM_32G).is_ok());
    }

    #[test]
    fn percent_of_ram() {
        match check("50%", RAM_32G) {
            SizeVsRam::Fits { bytes, ratio } => {
                assert_eq!(bytes, RAM_32G / 2);
                assert!((ratio - 0.5).abs() < 0.01);
            }
            other => panic!("{other:?}"),
        }
        assert!(ensure_fits("50%", RAM_32G).is_ok());
        assert!(ensure_fits("150%", RAM_32G).is_err());
    }

    #[test]
    fn one_six_nine_g_exceeds_32g() {
        match check("169G", RAM_32G) {
            SizeVsRam::Exceeds { .. } => {}
            other => panic!("{other:?}"),
        }
        assert!(ensure_fits("169G", RAM_32G).is_err());
    }

    #[test]
    fn rejects_junk() {
        assert!(matches!(check("nope", RAM_32G), SizeVsRam::Invalid));
        assert!(matches!(check("16G,uid=0", RAM_32G), SizeVsRam::Invalid));
    }

    #[test]
    fn share_bar_overlap_is_the_shortfall() {
        let disk = 10.0 / 24.0;
        let used = 15.0 / 24.0;
        let (left, overlap, gap, right) = super::share_bar_segments(disk, used);
        assert!((overlap - 1.0 / 24.0).abs() < 1e-5);
        assert!(gap.abs() < 1e-6);
        assert!((left - (10.0 / 24.0 - overlap)).abs() < 1e-5);
        assert!((right - (15.0 / 24.0 - overlap)).abs() < 1e-5);
        assert_eq!(
            super::deficiency_bytes(
                10 * 1024 * 1024 * 1024,
                15 * 1024 * 1024 * 1024,
                24 * 1024 * 1024 * 1024
            ),
            1024 * 1024 * 1024
        );
    }

    #[test]
    fn share_bar_gap_when_both_fit() {
        let (left, overlap, gap, right) = super::share_bar_segments(0.25, 0.25);
        assert!((left - 0.25).abs() < f32::EPSILON);
        assert!((right - 0.25).abs() < f32::EPSILON);
        assert!(overlap.abs() < f32::EPSILON);
        assert!((gap - 0.5).abs() < f32::EPSILON);
        assert_eq!(
            super::deficiency_bytes(
                8 * 1024 * 1024 * 1024,
                8 * 1024 * 1024 * 1024,
                24 * 1024 * 1024 * 1024
            ),
            0
        );
    }
}
