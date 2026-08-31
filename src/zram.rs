//! Temporary ABS-owned zram when a PGO/ramdisk step looks too big for RAM.

pub const ABS_ZRAM_LABEL: &str = "abs-pgo";
pub const MEM_LIMIT_FLOOR: u64 = 256 * 1024 * 1024;
/// Extra zram capacity so a small MemAvailable dip after mkswap/swapon
/// does not leave the gate short (and print `short: 0.0 GiB`).
pub const HEADROOM_SLACK: u64 = MEM_LIMIT_FLOOR;
/// Conservative pages-per-RAM for zstd. 18:46 Propeller protobuf was ~4.7:1.
pub const ASSUMED_COMPRESSION: u64 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZramAction {
    NoneNeeded,
    SkipUnknownMem,
    SkipCapTooSmall { mem_limit: u64 },
    Setup { disksize: u64, mem_limit: u64 },
}

/// How ABS sizes the temporary `abs-pgo` zram device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZramMode {
    /// Do not bring up ABS zram.
    Off,
    /// Always set up the largest device `MemAvailable` allows (`mem_limit` ≈ remaining RAM,
    /// `disksize` = `mem_limit` × [`ASSUMED_COMPRESSION`]). Unused zram is a cap, not prealloc.
    Full,
}

pub fn parse_zram_mode(s: &str) -> Result<ZramMode, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "full" => Ok(ZramMode::Full),
        "off" => Ok(ZramMode::Off),
        other => Err(format!(
            "Invalid zram {other:?} (expected \"off\" or \"full\")"
        )),
    }
}

/// Package override (non-empty) wins; empty/omitted inherits `global`.
pub fn resolved_zram_mode(global: &str, package: Option<&str>) -> Result<ZramMode, String> {
    if let Some(pkg) = package.map(str::trim).filter(|s| !s.is_empty()) {
        return parse_zram_mode(pkg).map_err(|e| format!("package zram: {e}"));
    }
    parse_zram_mode(global)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OomPrompt {
    Recheck,
    Continue,
    Stop,
    Extend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OomGate {
    Proceed,
    Stop,
}

pub fn parse_meminfo_kb(meminfo: &str, key: &str) -> Option<u64> {
    let prefix = format!("{key}:");
    for line in meminfo.lines() {
        let Some(rest) = line.strip_prefix(&prefix) else {
            continue;
        };
        let kb = rest
            .trim()
            .strip_suffix(" kB")
            .unwrap_or(rest.trim())
            .trim()
            .parse::<u64>()
            .ok()?;
        return Some(kb);
    }
    None
}

/// Uncompressed swap pages zram can hold at `ASSUMED_COMPRESSION`.
/// `mem_limit == 0` is unlimited (sysfs default); that must count `disksize`, not 0.
pub fn abs_zram_extra(disksize: u64, mem_limit: u64) -> u64 {
    if mem_limit == 0 {
        return disksize;
    }
    disksize.min(mem_limit.saturating_mul(ASSUMED_COMPRESSION))
}

/// `SwapFree` for ABS zram is the uncompressed disksize. Extra is 2:1-backed net, not `mem_limit`.
pub fn usable_have(
    mem_available: Option<u64>,
    swap_free: u64,
    abs_zram_swap: u64,
    abs_zram_mem_limit: u64,
) -> Option<u64> {
    Some(
        mem_available?
            + swap_free
                .saturating_sub(abs_zram_swap)
                .saturating_add(abs_zram_extra(abs_zram_swap, abs_zram_mem_limit)),
    )
}

pub fn mem_limit_for_generation(shortfall: u64, avail: u64, generation: u32) -> u64 {
    let cap = match generation {
        0 => avail / 4,
        1 => avail / 2,
        _ => avail.saturating_sub(MEM_LIMIT_FLOOR),
    };
    shortfall.min(cap)
}

pub fn should_grow_abs_zram(
    existing_disksize: u64,
    existing_mem_limit: u64,
    want_disksize: u64,
    want_mem_limit: u64,
) -> bool {
    existing_disksize < want_disksize || existing_mem_limit < want_mem_limit
}

pub fn grow_plan(
    existing_disksize: u64,
    existing_mem_limit: u64,
    want_disksize: u64,
    want_mem_limit: u64,
) -> (u64, u64) {
    (
        existing_disksize.max(want_disksize),
        existing_mem_limit.max(want_mem_limit),
    )
}

pub fn headroom_covered(need: u64, have: u64) -> bool {
    have >= need || need.saturating_sub(have) <= HEADROOM_SLACK
}

pub fn parse_proc_swaps_size_bytes(text: &str, dev: &str) -> Option<u64> {
    for line in text.lines().skip(1) {
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else {
            continue;
        };
        if name != dev {
            continue;
        }
        let _ty = parts.next()?;
        let kb: u64 = parts.next()?.parse().ok()?;
        return Some(kb.saturating_mul(1024));
    }
    None
}

#[cfg(test)]
pub fn plan_zram(
    need: u64,
    mem_available: Option<u64>,
    swap_free: u64,
    generation: u32,
) -> ZramAction {
    plan_zram_mode(need, mem_available, swap_free, generation, ZramMode::Full)
}

pub fn plan_zram_mode(
    need: u64,
    mem_available: Option<u64>,
    swap_free: u64,
    generation: u32,
    mode: ZramMode,
) -> ZramAction {
    let _ = (need, swap_free);
    if matches!(mode, ZramMode::Off) {
        return ZramAction::NoneNeeded;
    }
    let Some(avail) = mem_available else {
        return ZramAction::SkipUnknownMem;
    };
    // generation 2 = MemAvailable − floor; ignore shortfall.
    let mem_limit = mem_limit_for_generation(u64::MAX, avail, 2.max(generation));
    if mem_limit < MEM_LIMIT_FLOOR {
        return ZramAction::SkipCapTooSmall { mem_limit };
    }
    ZramAction::Setup {
        disksize: mem_limit.saturating_mul(ASSUMED_COMPRESSION),
        mem_limit,
    }
}

pub fn is_zram_dev(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/dev/zram") else {
        return false;
    };
    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
}

pub fn is_abs_pgo_label(label: &str) -> bool {
    label.trim() == ABS_ZRAM_LABEL
}

pub fn parse_oom_prompt(input: &str) -> OomPrompt {
    match input.trim().to_ascii_lowercase().as_str() {
        "c" | "continue" | "y" | "yes" => OomPrompt::Continue,
        "s" | "stop" | "n" | "no" | "q" => OomPrompt::Stop,
        "e" | "extend" | "zram" => OomPrompt::Extend,
        _ => OomPrompt::Recheck,
    }
}

pub fn is_zram_mem_limit_sysfs(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/sys/block/zram") else {
        return false;
    };
    let Some(rest) = rest.strip_suffix("/mem_limit") else {
        return false;
    };
    let num = rest.strip_prefix('/').unwrap_or(rest);
    !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit())
}

pub fn format_zramctl_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * 1024 * 1024;
    if bytes >= GIB && bytes % GIB == 0 {
        format!("{}G", bytes / GIB)
    } else if bytes >= MIB && bytes % MIB == 0 {
        format!("{}M", bytes / MIB)
    } else if bytes >= KIB && bytes % KIB == 0 {
        format!("{}K", bytes / KIB)
    } else {
        bytes.to_string()
    }
}

/// `lsblk -no NAME,LABEL` lines (`zram0 abs-pgo` or `zram0`).
pub fn abs_pgo_zram_from_lsblk(table: &str) -> Option<String> {
    for line in table.lines() {
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else {
            continue;
        };
        let label = parts.next().unwrap_or("");
        let dev = if name.starts_with("/dev/") {
            name.to_string()
        } else {
            format!("/dev/{name}")
        };
        if is_zram_dev(&dev) && is_abs_pgo_label(label) {
            return Some(dev);
        }
    }
    None
}

fn read_proc_meminfo() -> String {
    std::fs::read_to_string("/proc/meminfo").unwrap_or_default()
}

fn mem_available_bytes() -> Option<u64> {
    parse_meminfo_kb(&read_proc_meminfo(), "MemAvailable").map(|kb| kb.saturating_mul(1024))
}

fn swap_free_bytes() -> u64 {
    parse_meminfo_kb(&read_proc_meminfo(), "SwapFree")
        .unwrap_or(0)
        .saturating_mul(1024)
}

fn fmt_gib(bytes: u64) -> String {
    const GIB: f64 = (1u64 << 30) as f64;
    const MIB: f64 = (1u64 << 20) as f64;
    let gib = bytes as f64 / GIB;
    if bytes > 0 && gib < 0.1 {
        format!("{:.0} MiB", bytes as f64 / MIB)
    } else {
        format!("{:.1} GiB", gib)
    }
}

fn lsblk_table() -> String {
    crate::utils::run_command_with_output("lsblk", &["-no", "NAME,LABEL"], None::<&str>)
        .unwrap_or_default()
}

fn find_abs_pgo_zram() -> Option<String> {
    abs_pgo_zram_from_lsblk(&lsblk_table())
}

fn zram_sysfs_disksize(dev: &str) -> Option<u64> {
    let n = dev.strip_prefix("/dev/zram")?;
    std::fs::read_to_string(format!("/sys/block/zram{n}/disksize"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn zram_sysfs_mem_limit(dev: &str) -> Option<u64> {
    let n = dev.strip_prefix("/dev/zram")?;
    std::fs::read_to_string(format!("/sys/block/zram{n}/mem_limit"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn abs_zram_accounting() -> (u64, u64) {
    let Some(dev) = find_abs_pgo_zram() else {
        return (0, 0);
    };
    let swaps = std::fs::read_to_string("/proc/swaps").unwrap_or_default();
    let abs_swap = parse_proc_swaps_size_bytes(&swaps, &dev).unwrap_or(0);
    let ml = zram_sysfs_mem_limit(&dev).unwrap_or(0);
    (abs_swap, ml)
}

fn zram_mem_limit_path(dev: &str) -> Option<String> {
    let n = dev.strip_prefix("/dev/zram")?;
    Some(format!("/sys/block/zram{n}/mem_limit"))
}

fn sudo(args: &[&str]) -> Result<(), String> {
    crate::utils::run_command("sudo", args, None::<&str>)
}

fn sudo_out(args: &[&str]) -> Result<String, String> {
    crate::utils::run_command_with_output("sudo", args, None::<&str>)
}

fn reset_zram(dev: &str) {
    let _ = sudo(&["swapoff", dev]);
    let _ = sudo(&["zramctl", "--reset", dev]);
}

/// Remove the ABS-owned zram device only. Other swap is left alone.
pub fn teardown_abs_zram() {
    let Some(dev) = find_abs_pgo_zram() else {
        return;
    };
    crate::blog!("Removing ABS zram {dev} (label {ABS_ZRAM_LABEL})");
    reset_zram(&dev);
}

fn set_mem_limit(dev: &str, mem_limit: u64) -> Result<(), String> {
    let sysfs = zram_mem_limit_path(dev).ok_or_else(|| format!("not a zram device: {dev}"))?;
    crate::utils::run_sudo_stdin(
        &["tee", sysfs.as_str()],
        format!("{mem_limit}\n").as_bytes(),
    )
}

fn bring_up_zram(disksize: u64, mem_limit: u64) -> Result<String, String> {
    if find_abs_pgo_zram().is_none() {
        let _ = sudo(&["modprobe", "zram"]);
    }
    if let Some(dev) = find_abs_pgo_zram() {
        let existing_disk = zram_sysfs_disksize(&dev).unwrap_or(0);
        let existing_ml = match zram_sysfs_mem_limit(&dev) {
            Some(0) | None => existing_disk,
            Some(n) => n,
        };
        if !should_grow_abs_zram(existing_disk, existing_ml, disksize, mem_limit) {
            crate::blog!(
                "Reusing ABS zram {dev} (disksize {} / mem_limit {} already covers {} / {})",
                fmt_gib(existing_disk),
                fmt_gib(existing_ml),
                fmt_gib(disksize),
                fmt_gib(mem_limit)
            );
            return Ok(dev);
        }
        crate::blog!(
            "Growing ABS zram {dev} to disksize={} mem_limit={}",
            fmt_gib(disksize),
            fmt_gib(mem_limit)
        );
        reset_zram(&dev);
    }
    let size = format_zramctl_size(disksize);
    let out = sudo_out(&["zramctl", "--find", "--size", &size, "--algorithm", "zstd"])?;
    let dev = out.trim();
    if !is_zram_dev(dev) {
        return Err(format!("zramctl --find returned {dev:?}"));
    }
    set_mem_limit(dev, mem_limit)?;
    sudo(&["mkswap", "-L", ABS_ZRAM_LABEL, dev])?;
    sudo(&["swapon", dev])?;
    crate::blog!(
        "Added ABS zram {dev} swap capacity {} backed by {} compressed RAM",
        fmt_gib(disksize),
        fmt_gib(mem_limit)
    );
    Ok(dev.to_string())
}

fn describe_action(action: ZramAction) -> String {
    match action {
        ZramAction::NoneNeeded => "not needed".into(),
        ZramAction::SkipUnknownMem => "skipped (MemAvailable unknown)".into(),
        ZramAction::SkipCapTooSmall { mem_limit } => format!(
            "skipped (mem_limit {} < {})",
            fmt_gib(mem_limit),
            fmt_gib(MEM_LIMIT_FLOOR)
        ),
        ZramAction::Setup {
            disksize,
            mem_limit,
        } => format!(
            "setup swap {} backed by {} RAM",
            fmt_gib(disksize),
            fmt_gib(mem_limit)
        ),
    }
}

fn read_oom_choice() -> OomPrompt {
    use std::io::{self, BufRead, IsTerminal, Write};
    if !io::stdin().is_terminal() {
        crate::ewarn!("No TTY: stopping instead of continuing with an OOM risk");
        return OomPrompt::Stop;
    }
    eprint!("  [r] Re-check now  [e] Extend ABS zram  [c] Continue anyway (OOM risk)  [s] Stop: ");
    let _ = io::stderr().flush();
    let mut line = String::new();
    if io::stdin().lock().read_line(&mut line).is_err() {
        return OomPrompt::Stop;
    }
    parse_oom_prompt(&line)
}

fn try_cover(need: u64, generation: u32, mode: ZramMode) -> (u64, ZramAction, String) {
    let avail = mem_available_bytes();
    let swap = swap_free_bytes();
    let (abs_swap, abs_ml) = abs_zram_accounting();
    let non_abs_swap = swap.saturating_sub(abs_swap);
    let have = usable_have(avail, swap, abs_swap, abs_ml).unwrap_or(non_abs_swap);
    let action = plan_zram_mode(need, avail, non_abs_swap, generation, mode);
    let mut tried = describe_action(action);
    if let ZramAction::Setup {
        disksize,
        mem_limit,
    } = action
        && !crate::is_dry_run_mode()
    {
        let (disksize, mem_limit) = grow_plan(abs_swap, abs_ml, disksize, mem_limit);
        match bring_up_zram(disksize, mem_limit) {
            Ok(dev) => tried = format!("added {dev} ({tried})"),
            Err(e) => tried = format!("failed ({e})"),
        }
    }
    let avail = mem_available_bytes();
    let swap = swap_free_bytes();
    let (abs_swap, abs_ml) = abs_zram_accounting();
    let have = usable_have(avail, swap, abs_swap, abs_ml).unwrap_or(have);
    (have, action, tried)
}

/// Before convert / compile / ramdisk mount / system update. `off` skips setup.
/// `full` adds the largest ABS zram remaining RAM allows; loops until the user
/// continues or stops.
pub fn ensure_headroom(step: &str, need_bytes: u64, mode: ZramMode) -> Result<OomGate, String> {
    if matches!(mode, ZramMode::Off) {
        return Ok(OomGate::Proceed);
    }
    if crate::is_dry_run_mode() {
        let avail = mem_available_bytes();
        let action = plan_zram_mode(need_bytes, avail, swap_free_bytes(), 0, mode);
        println!(
            "[DRY RUN] OOM gate {step}: need {} zram={mode:?} → {action:?}",
            fmt_gib(need_bytes)
        );
        return Ok(OomGate::Proceed);
    }
    let mut generation = 0u32;
    loop {
        let (have, action, tried) = try_cover(need_bytes, generation, mode);
        let (abs_swap, abs_ml) = abs_zram_accounting();
        let covered = usable_have(mem_available_bytes(), swap_free_bytes(), abs_swap, abs_ml)
            .is_some_and(|h| headroom_covered(need_bytes, h));
        if matches!(action, ZramAction::NoneNeeded) || covered {
            return Ok(OomGate::Proceed);
        }
        let short = need_bytes.saturating_sub(have);
        crate::ewarn!("RAM too tight for {step}");
        eprintln!("    need:    {}", fmt_gib(need_bytes));
        eprintln!(
            "    have:    {} (RAM + usable swap; ABS zram counts uncompressed pages, 2:1-capped)",
            fmt_gib(have)
        );
        eprintln!("    short:   {}", fmt_gib(short));
        eprintln!("    zram:    {tried}");
        match read_oom_choice() {
            OomPrompt::Recheck => continue,
            OomPrompt::Extend => {
                generation = generation.saturating_add(1);
                crate::blog!("Extending ABS zram RAM budget (round {generation})");
            }
            OomPrompt::Continue => return Ok(OomGate::Proceed),
            OomPrompt::Stop => return Ok(OomGate::Stop),
        }
    }
}

pub fn require_headroom(step: &str, need_bytes: u64, mode: ZramMode) {
    match ensure_headroom(step, need_bytes, mode) {
        Ok(OomGate::Proceed) => {}
        Ok(OomGate::Stop) => {
            crate::die!(
                "Stopped: not enough RAM for {step} (re-run when RAM is free, or continue)"
            );
        }
        Err(e) => crate::die!("{e}"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn plan_zram_full_when_have_covers_need() {
        const GIB: u64 = 1 << 30;
        match super::plan_zram(10 * GIB, Some(20 * GIB), 0, 0) {
            super::ZramAction::Setup {
                disksize,
                mem_limit,
            } => {
                assert_eq!(mem_limit, 20 * GIB - super::MEM_LIMIT_FLOOR);
                assert_eq!(disksize, mem_limit * super::ASSUMED_COMPRESSION);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_zram_mode_off_or_full() {
        assert_eq!(
            super::parse_zram_mode("full").unwrap(),
            super::ZramMode::Full
        );
        assert_eq!(
            super::parse_zram_mode("FULL").unwrap(),
            super::ZramMode::Full
        );
        assert_eq!(super::parse_zram_mode("off").unwrap(), super::ZramMode::Off);
        assert_eq!(super::parse_zram_mode("OFF").unwrap(), super::ZramMode::Off);
        assert!(super::parse_zram_mode("auto").is_err());
        assert!(super::parse_zram_mode("").is_err());
        assert!(super::parse_zram_mode("unknown").is_err());
        let err = super::parse_zram_mode("cachyos").unwrap_err();
        assert!(err.contains("off"), "{err}");
        assert!(err.contains("full"), "{err}");
    }

    #[test]
    fn resolved_zram_mode_package_overrides_global() {
        assert_eq!(
            super::resolved_zram_mode("full", None).unwrap(),
            super::ZramMode::Full
        );
        assert_eq!(
            super::resolved_zram_mode("full", Some("")).unwrap(),
            super::ZramMode::Full
        );
        assert_eq!(
            super::resolved_zram_mode("full", Some("off")).unwrap(),
            super::ZramMode::Off
        );
        assert_eq!(
            super::resolved_zram_mode("off", Some("full")).unwrap(),
            super::ZramMode::Full
        );
        assert!(super::resolved_zram_mode("auto", None).is_err());
        assert!(super::resolved_zram_mode("full", Some("auto")).is_err());
    }

    #[test]
    fn plan_zram_off_never_sets_up() {
        const GIB: u64 = 1 << 30;
        assert_eq!(
            super::plan_zram_mode(200 * GIB, Some(10 * GIB), 0, 0, super::ZramMode::Off),
            super::ZramAction::NoneNeeded
        );
    }

    #[test]
    fn plan_zram_full_maxes_even_when_ram_covers_need() {
        const GIB: u64 = 1 << 30;
        match super::plan_zram_mode(10 * GIB, Some(80 * GIB), 0, 0, super::ZramMode::Full) {
            super::ZramAction::Setup {
                disksize,
                mem_limit,
            } => {
                assert_eq!(mem_limit, 80 * GIB - super::MEM_LIMIT_FLOOR);
                assert_eq!(disksize, mem_limit * super::ASSUMED_COMPRESSION);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn plan_zram_full_ignores_generation_cap() {
        const GIB: u64 = 1 << 30;
        match super::plan_zram_mode(200 * GIB, Some(80 * GIB), 0, 0, super::ZramMode::Full) {
            super::ZramAction::Setup {
                disksize,
                mem_limit,
            } => {
                assert_eq!(mem_limit, 80 * GIB - super::MEM_LIMIT_FLOOR);
                assert_eq!(disksize, mem_limit * super::ASSUMED_COMPRESSION);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn plan_zram_skips_when_mem_limit_below_floor() {
        const MIB: u64 = 1 << 20;
        match super::plan_zram(200 * MIB, Some(100 * MIB), 0, 0) {
            super::ZramAction::SkipCapTooSmall { mem_limit } => {
                assert!(mem_limit < super::MEM_LIMIT_FLOOR);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn grow_plan_takes_max_so_leftover_is_replaced_not_added() {
        const GIB: u64 = 1 << 30;
        const MIB: u64 = 1 << 20;
        let remain = 100 * MIB + super::HEADROOM_SLACK;
        let (disk, ml) = super::grow_plan(5 * GIB, 5 * GIB, remain, remain);
        assert_eq!(disk, 5 * GIB);
        assert_eq!(ml, 5 * GIB);
        assert!(!super::should_grow_abs_zram(
            5 * GIB,
            5 * GIB,
            remain,
            remain
        ));
        let (disk, ml) = super::grow_plan(5 * GIB, 5 * GIB, 26 * GIB, 13 * GIB);
        assert_eq!(disk, 26 * GIB);
        assert_eq!(ml, 13 * GIB);
        assert!(super::should_grow_abs_zram(
            5 * GIB,
            5 * GIB,
            26 * GIB,
            13 * GIB
        ));
    }

    #[test]
    fn headroom_covered_when_shortfall_is_measurement_noise() {
        const GIB: u64 = 1 << 30;
        const MIB: u64 = 1 << 20;
        assert!(super::headroom_covered(95 * GIB + 100 * MIB, 95 * GIB));
        assert!(super::headroom_covered(10 * GIB, 10 * GIB));
        assert!(!super::headroom_covered(95 * GIB, 90 * GIB));
    }

    #[test]
    fn plan_zram_unknown_mem_skips() {
        assert_eq!(
            super::plan_zram(1 << 30, None, 0, 0),
            super::ZramAction::SkipUnknownMem
        );
    }

    #[test]
    fn parse_oom_prompt_empty_is_recheck() {
        assert_eq!(super::parse_oom_prompt(""), super::OomPrompt::Recheck);
        assert_eq!(super::parse_oom_prompt("  \n"), super::OomPrompt::Recheck);
        assert_eq!(super::parse_oom_prompt("r"), super::OomPrompt::Recheck);
        assert_eq!(super::parse_oom_prompt("c"), super::OomPrompt::Continue);
        assert_eq!(
            super::parse_oom_prompt("continue"),
            super::OomPrompt::Continue
        );
        assert_eq!(super::parse_oom_prompt("s"), super::OomPrompt::Stop);
        assert_eq!(super::parse_oom_prompt("no"), super::OomPrompt::Stop);
        assert_eq!(super::parse_oom_prompt("xyz"), super::OomPrompt::Recheck);
        assert_eq!(super::parse_oom_prompt("e"), super::OomPrompt::Extend);
        assert_eq!(super::parse_oom_prompt("extend"), super::OomPrompt::Extend);
    }

    #[test]
    fn usable_have_counts_abs_zram_mem_limit_not_disksize() {
        const GIB: u64 = 1 << 30;
        // 20 GiB RAM + 76 GiB zram disksize backed by only 5 GiB mem_limit → 10 GiB at 2:1.
        let have = super::usable_have(Some(20 * GIB), 76 * GIB, 76 * GIB, 5 * GIB).unwrap();
        assert_eq!(have, 40 * GIB);
        assert!(have < 92 * GIB + 4 * GIB);
    }

    #[test]
    fn mem_limit_for_generation_raises_the_cap() {
        const GIB: u64 = 1 << 30;
        let shortfall = 76 * GIB;
        let avail = 20 * GIB;
        assert_eq!(
            super::mem_limit_for_generation(shortfall, avail, 0),
            5 * GIB
        );
        assert_eq!(
            super::mem_limit_for_generation(shortfall, avail, 1),
            10 * GIB
        );
        assert_eq!(
            super::mem_limit_for_generation(shortfall, avail, 2),
            avail - super::MEM_LIMIT_FLOOR
        );
    }

    #[test]
    fn should_grow_when_mem_limit_is_below_planned() {
        const GIB: u64 = 1 << 30;
        assert!(super::should_grow_abs_zram(
            76 * GIB,
            5 * GIB,
            76 * GIB,
            10 * GIB
        ));
        assert!(!super::should_grow_abs_zram(
            76 * GIB,
            10 * GIB,
            76 * GIB,
            10 * GIB
        ));
        assert!(super::should_grow_abs_zram(
            8 * GIB,
            8 * GIB,
            22 * GIB,
            8 * GIB
        ));
    }

    #[test]
    fn parse_proc_swaps_size_for_zram_dev() {
        let text = "Filename\tType\tSize\tUsed\tPriority\n\
                    /dev/zram0\tpartition\t5242880\t0\t100\n";
        assert_eq!(
            super::parse_proc_swaps_size_bytes(text, "/dev/zram0"),
            Some(5242880 * 1024)
        );
        assert_eq!(super::parse_proc_swaps_size_bytes(text, "/dev/zram1"), None);
    }

    #[test]
    fn zram_dev_and_label() {
        assert!(super::is_zram_dev("/dev/zram0"));
        assert!(super::is_zram_dev("/dev/zram12"));
        assert!(!super::is_zram_dev("/dev/zram"));
        assert!(!super::is_zram_dev("/var/swapfile"));
        assert!(!super::is_zram_dev("/dev/sda1"));
        assert!(super::is_abs_pgo_label("abs-pgo"));
        assert!(!super::is_abs_pgo_label("zram-swap"));
        assert!(!super::is_abs_pgo_label(""));
        assert!(super::is_zram_mem_limit_sysfs("/sys/block/zram0/mem_limit"));
        assert!(!super::is_zram_mem_limit_sysfs("/sys/block/sda/mem_limit"));
    }

    #[test]
    fn parse_meminfo_kb_reads_keys() {
        let t = "MemTotal:       100 kB\nMemAvailable:    50 kB\nSwapFree:        10 kB\n";
        assert_eq!(super::parse_meminfo_kb(t, "MemAvailable"), Some(50));
        assert_eq!(super::parse_meminfo_kb(t, "SwapFree"), Some(10));
        assert_eq!(super::parse_meminfo_kb(t, "Nope"), None);
    }

    #[test]
    fn format_zramctl_size_prefers_g_and_m() {
        assert_eq!(super::format_zramctl_size(22 << 30), "22G");
        assert_eq!(super::format_zramctl_size(256 << 20), "256M");
    }

    #[test]
    fn usable_have_unlimited_mem_limit_counts_disksize() {
        const GIB: u64 = 1 << 30;
        // cp to sysfs leaves mem_limit 0. That must not net 0 extra (15:01 prompt).
        let have = super::usable_have(Some(87 * GIB + GIB / 10), 32 * GIB, 32 * GIB, 0).unwrap();
        assert!(
            have >= 95 * GIB + GIB / 10,
            "have {have} should cover 95.1 GiB"
        );
    }

    #[test]
    fn usable_have_2to1_device_on_87g_covers_propeller_need() {
        const GIB: u64 = 1 << 30;
        let avail = 87 * GIB + GIB / 10;
        let disk = 319 * GIB / 10;
        let have = super::usable_have(Some(avail), disk, disk, 16 * GIB).unwrap();
        assert!(have >= 95 * GIB + GIB / 10);
    }

    #[test]
    fn abs_pgo_zram_from_lsblk_picks_labeled_device() {
        let table = "zram0 zram-swap\nzram1 abs-pgo\n";
        assert_eq!(
            super::abs_pgo_zram_from_lsblk(table).as_deref(),
            Some("/dev/zram1")
        );
        assert_eq!(super::abs_pgo_zram_from_lsblk("zram0 zram-swap\n"), None);
    }

    #[test]
    fn usable_have_1to1_zram_counts_swap_pages() {
        const GIB: u64 = 1 << 30;
        // 4.9 GiB of swap pages is real extra; it was not enough for 95.1 GiB need.
        let have = super::usable_have(Some(90 * GIB), 5 * GIB, 5 * GIB, 5 * GIB).unwrap();
        assert_eq!(have, 95 * GIB);
        assert!(have < 95 * GIB + GIB / 10);
    }

    #[test]
    fn usable_have_2to1_backed_zram_counts_disksize() {
        const GIB: u64 = 1 << 30;
        let have = super::usable_have(Some(90 * GIB), 26 * GIB, 26 * GIB, 13 * GIB).unwrap();
        assert_eq!(have, 116 * GIB);
    }

    #[test]
    fn plan_zram_full_uses_remaining_ram_not_shortfall() {
        const GIB: u64 = 1 << 30;
        // Same gate as 14:37: need 95.1 GiB, MemAvailable 90 GiB — still full remaining RAM.
        let need = 95 * GIB + GIB / 10;
        let avail = 90 * GIB;
        match super::plan_zram(need, Some(avail), 0, 0) {
            super::ZramAction::Setup {
                disksize,
                mem_limit,
            } => {
                assert_eq!(mem_limit, avail - super::MEM_LIMIT_FLOOR);
                assert_eq!(disksize, mem_limit * super::ASSUMED_COMPRESSION);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn usable_have_36g_swap_does_not_cover_6x_propeller_need() {
        const GIB: u64 = 1 << 30;
        // 18:46: 36 GiB zram filled; convert was 5.0× file. 6× + 4 GiB min_free is ~142 GiB.
        let need = 6 * 23 * GIB + 4 * GIB;
        let have = super::usable_have(Some(87 * GIB), 36 * GIB, 36 * GIB, 18 * GIB).unwrap();
        assert!(have < need, "have {} must be below need {need}", have);
    }

    #[test]
    fn plan_zram_18_46_full_is_remaining_ram_times_compression() {
        const GIB: u64 = 1 << 30;
        let need = 6 * 23 * GIB + 4 * GIB;
        let avail = 87 * GIB;
        match super::plan_zram(need, Some(avail), 0, 0) {
            super::ZramAction::Setup {
                disksize,
                mem_limit,
            } => {
                assert_eq!(mem_limit, avail - super::MEM_LIMIT_FLOOR);
                assert_eq!(disksize, mem_limit * super::ASSUMED_COMPRESSION);
                assert!(disksize > 36 * GIB);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn grow_plan_replaces_undersized_leftover_instead_of_adding() {
        const GIB: u64 = 1 << 30;
        let (disk, ml) = super::grow_plan(5 * GIB, 5 * GIB, 26 * GIB, 13 * GIB);
        assert_eq!(disk, 26 * GIB);
        assert_eq!(ml, 13 * GIB);
    }
}
