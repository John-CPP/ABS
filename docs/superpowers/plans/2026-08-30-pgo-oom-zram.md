# PGO OOM zram Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When convert, PGO kernel compile, or ramdisk mount looks too big for `MemAvailable + SwapFree`, ABS adds a labeled `abs-pgo` zram device for the shortfall, then removes only that device on exit; if still tight, loop until the user continues or stops.

**Architecture:** Pure decision helpers in `src/zram.rs` (no live `swapon` in tests). `--pgo-priv` allowlists `modprobe zram`, `zramctl`, `mkswap -L abs-pgo`, `swapon`/`swapoff` of `/dev/zramN`, and `tee` to zram `mem_limit`. Call `ensure_headroom` from convert, `run_pgo_build`, and `ramdisk::mount_session`. Teardown from `ramdisk::shutdown` / interrupt.

**Tech Stack:** Rust (`abs` binary unit tests), util-linux `zramctl`, existing sudo / `abs --pgo-priv`.

## Global Constraints

- No new config key. Always on.
- Never modify or remove swap/zram that is not labeled `abs-pgo`.
- No second ABS zram device; recreate the `abs-pgo` one larger if needed.
- `disksize` = `shortfall + 8 GiB` (swap pages); `mem_limit = min(that, MemAvailable/4)`.
- `have` counts uncompressed swap pages (`min(disksize, 4×mem_limit)`; `mem_limit` 0 = `disksize`).
- Unknown `MemAvailable` → no zram, treat as short, go to the loop.
- Empty prompt = re-check, never continue. No TTY → stop, do not hang, do not continue.
- Convert relocate is unchanged; zram runs after relocate.
- Do not commit unless the user asks.

---

### Task 1: Pure helpers in `src/zram.rs`

**Files:**
- Create: `src/zram.rs`
- Modify: `src/main.rs` (add `mod zram;`)

**Produces:**
- `ABS_ZRAM_LABEL = "abs-pgo"`
- `MEM_LIMIT_FLOOR: u64 = 256 * 1024 * 1024`
- `parse_meminfo_kb(meminfo: &str, key: &str) -> Option<u64>`
- `have_bytes(mem_available: Option<u64>, swap_free: u64) -> Option<u64>` — `None` if MemAvailable unknown
- `enum ZramAction { NoneNeeded, SkipUnknownMem, SkipCapTooSmall { mem_limit: u64 }, Setup { disksize: u64, mem_limit: u64 } }`
- `plan_zram(need: u64, mem_available: Option<u64>, swap_free: u64) -> ZramAction`
- `is_zram_dev(path: &str) -> bool` — `/dev/zram` + digits only
- `is_abs_pgo_label(label: &str) -> bool`
- `enum OomPrompt { Recheck, Continue, Stop }`
- `parse_oom_prompt(input: &str) -> OomPrompt`

- [ ] **Step 1: Write failing tests** in `src/zram.rs` `#[cfg(test)]`

```rust
#[test]
fn plan_zram_none_when_have_covers_need() {
    const GIB: u64 = 1 << 30;
    assert_eq!(
        super::plan_zram(10 * GIB, Some(20 * GIB), 0),
        super::ZramAction::NoneNeeded
    );
}

#[test]
fn plan_zram_shortfall_and_cap() {
    const GIB: u64 = 1 << 30;
    // need 92G, avail 70G, swap 0 → shortfall 22G; mem_limit = min(22G, 70G/4=17.5G) = 17.5G
    match super::plan_zram(92 * GIB, Some(70 * GIB), 0) {
        super::ZramAction::Setup { disksize, mem_limit } => {
            assert_eq!(disksize, 22 * GIB);
            assert_eq!(mem_limit, 70 * GIB / 4);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn plan_zram_swap_free_counts_as_have() {
    const GIB: u64 = 1 << 30;
    assert_eq!(
        super::plan_zram(10 * GIB, Some(4 * GIB), 8 * GIB),
        super::ZramAction::NoneNeeded
    );
}

#[test]
fn plan_zram_skips_when_mem_limit_below_floor() {
    const MIB: u64 = 1 << 20;
    match super::plan_zram(200 * MIB, Some(100 * MIB), 0) {
        super::ZramAction::SkipCapTooSmall { mem_limit } => {
            assert!(mem_limit < super::MEM_LIMIT_FLOOR);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn plan_zram_unknown_mem_skips() {
    assert_eq!(
        super::plan_zram(1 << 30, None, 0),
        super::ZramAction::SkipUnknownMem
    );
}

#[test]
fn parse_oom_prompt_empty_is_recheck() {
    assert_eq!(super::parse_oom_prompt(""), super::OomPrompt::Recheck);
    assert_eq!(super::parse_oom_prompt("  \n"), super::OomPrompt::Recheck);
    assert_eq!(super::parse_oom_prompt("r"), super::OomPrompt::Recheck);
    assert_eq!(super::parse_oom_prompt("c"), super::OomPrompt::Continue);
    assert_eq!(super::parse_oom_prompt("continue"), super::OomPrompt::Continue);
    assert_eq!(super::parse_oom_prompt("s"), super::OomPrompt::Stop);
    assert_eq!(super::parse_oom_prompt("no"), super::OomPrompt::Stop);
    assert_eq!(super::parse_oom_prompt("xyz"), super::OomPrompt::Recheck);
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
}

#[test]
fn parse_meminfo_kb_reads_keys() {
    let t = "MemTotal:       100 kB\nMemAvailable:    50 kB\nSwapFree:        10 kB\n";
    assert_eq!(super::parse_meminfo_kb(t, "MemAvailable"), Some(50));
    assert_eq!(super::parse_meminfo_kb(t, "SwapFree"), Some(10));
    assert_eq!(super::parse_meminfo_kb(t, "Nope"), None);
}
```

- [ ] **Step 2: Run** `cargo test --bin abs zram:: -- --exact` style filter `zram::tests::` — expect FAIL (module missing).

- [ ] **Step 3: Implement helpers** (no `swapon`). `plan_zram`: if `mem_available` is `None` → `SkipUnknownMem`; `have = avail + swap_free`; if `have >= need` → `NoneNeeded`; `shortfall = need - have`; `mem_limit = min(shortfall, avail/4)`; if `mem_limit < MEM_LIMIT_FLOOR` → `SkipCapTooSmall`; else `Setup { disksize: shortfall, mem_limit }`. `parse_oom_prompt`: trim/lowercase; `c`/`continue`/`y`/`yes` → Continue; `s`/`stop`/`n`/`no`/`q` → Stop; else Recheck.

- [ ] **Step 4: Run** `cargo test --bin abs zram::` — expect PASS.

---

### Task 2: `--pgo-priv` allowlist

**Files:**
- Modify: `src/pgo_priv.rs` (`validate_command` + new validators + tests next to existing chown tests)

**Produces:** allow `modprobe zram`; `zramctl --find/--size/--algorithm zstd/--reset` on `/dev/zramN`; `mkswap -L abs-pgo /dev/zramN`; `swapon`/`swapoff` `/dev/zramN`; `tee` to `/sys/block/zramN/mem_limit`. Reject swap files and unlabeled mkswap.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn validate_allows_abs_pgo_zram_commands() {
    v(&["modprobe", "zram"]).unwrap();
    v(&["zramctl", "--find", "--size", "16G", "--algorithm", "zstd"]).unwrap();
    v(&["zramctl", "--reset", "/dev/zram0"]).unwrap();
    v(&["mkswap", "-L", "abs-pgo", "/dev/zram0"]).unwrap();
    v(&["swapon", "/dev/zram0"]).unwrap();
    v(&["swapoff", "/dev/zram0"]).unwrap();
    v(&["tee", "/sys/block/zram0/mem_limit"]).unwrap();
}

#[test]
fn validate_rejects_non_abs_swap() {
    assert!(v(&["swapon", "/var/swapfile"]).is_err());
    assert!(v(&["swapon", "/dev/sda1"]).is_err());
    assert!(v(&["mkswap", "/dev/zram0"]).is_err());
    assert!(v(&["mkswap", "-L", "other", "/dev/zram0"]).is_err());
    assert!(v(&["modprobe", "zram", "num_devices=8"]).is_err());
    assert!(v(&["zramctl", "--reset", "/dev/sda"]).is_err());
}
```

- [ ] **Step 2: Run** those tests — expect FAIL (`does not allow command "modprobe"`).

- [ ] **Step 3: Implement validators.** `is_zram_dev` can live in `zram.rs` and be `pub(crate)` so pgo_priv reuses it. `tee` to mem_limit: in `path_writable_for_pgo` or `validate_generic_paths`, allow `/sys/block/zram{N}/mem_limit` matching `^/sys/block/zram[0-9]+/mem_limit$`.

- [ ] **Step 4: Run** `cargo test --bin abs pgo_priv::` — expect PASS.

---

### Task 3: Bring-up / teardown I/O

**Files:**
- Modify: `src/zram.rs` (sudo via `utils::run_command`)

**Produces:**
- `enum OomGate { Proceed, Stop }`
- `ensure_headroom(step: &str, need_bytes: u64) -> Result<OomGate, String>`
- `teardown_abs_zram()` — `swapoff` + `zramctl --reset` only for devices whose swap label is `abs-pgo`
- Find `abs-pgo` via `/dev/zram*` + `lsblk -no LABEL` (or `blkid -s LABEL -o value`); ignore other labels
- Setup: `modprobe zram` if needed; if existing `abs-pgo` disksize `< shortfall`, swapoff/reset then create; `zramctl --find --size <disksize> --algorithm zstd`; `echo mem_limit | sudo tee ...`; `mkswap -L abs-pgo`; `swapon`
- Loop: print need/have/shortfall/what was tried; if stdin is a TTY, read line and `parse_oom_prompt`; Recheck retries plan+setup; Continue → `Proceed`; Stop → `Stop`; no TTY → print and `Stop`

- [ ] **Step 1: Unit-test command/size formatting** (`format_zramctl_size(bytes) -> String` like `22G` / `256M`) and leftover-label selection from a fake `lsblk` table. No live swapon.

- [ ] **Step 2: Implement I/O.** Dry-run mode: log and skip real swapon.

- [ ] **Step 3: Run** `cargo test --bin abs zram::` — expect PASS.

---

### Task 4: Wire gates + exit teardown

**Files:**
- Modify: `src/pgo.rs` (`run_stage2_profile` / `run_stage3_profile` after `maybe_relocate_perf_for_convert`; `run_pgo_build` before `process_package_pgo`)
- Modify: `src/ramdisk.rs` (`mount_session` replace `die!` on low `MemAvailable`; `shutdown` and `shutdown_on_interrupt` call `zram::teardown_abs_zram`)

**Need:**
- Convert: `convert_anon_estimate_bytes(file_len, kind) + min_free_ram_mb * 1MiB`
- Compile / ramdisk: `min_free_ram_mb * 1MiB`
- `OomGate::Stop` → `die!` with a short message that the user stopped because RAM was too tight (state already saved by the pipeline where applicable)

- [ ] **Step 1: Call sites only; no new estimate formula.**

- [ ] **Step 2: Run** `cargo test --bin abs pgo:: zram:: pgo_priv:: ramdisk::` — expect PASS.

---

## Spec coverage

| Spec | Task |
| --- | --- |
| Convert / compile / ramdisk gates | 4 |
| Shortfall + cap + 256 MiB floor | 1 |
| ABS-owned `abs-pgo` device, no user zram | 3 |
| Interactive loop / no TTY stop | 3 |
| `--pgo-priv` allowlist | 2 |
| Teardown on exit | 4 |
| Relocate unchanged | 4 (after relocate) |
