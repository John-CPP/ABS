# Global zram + PGO follow-up Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make ABS zram a global max-size capability (off/full, per-package, compiles + system update), tear it down for kbench without unmounting ramdisk, unmount ramdisk before zram on exit, reject dead PGO presets, and write one timestamped compare-benchmarks folder per pipeline.

**Architecture:** `ZramMode` becomes `Off | Full`. Config validates global/per-package zram and PGO presets. Call `require_headroom` from compile and system update using resolved mode. Reorder `ramdisk::shutdown`. Kbench persists tiny profiles then `teardown_abs_zram`. PGO state stores `compare_run_dir`. Scraper drops CachyOS-benchmarker series. AbsGui/wizard/i18n follow.

**Tech Stack:** Rust (`abs` + `absgui` unit tests), existing `abs --pgo-priv` zram allowlist.

## Global Constraints

- Never modify or remove swap/zram that is not labeled `abs-pgo`.
- No rsync allowlist for kernel package files. Existing `.git/` and raw `.data` excludes stay.
- Do not unmount ramdisk for kbench.
- No live `swapon` in unit tests.
- Do not commit unless the user asks.

---

### Task 1: ZramMode Off | Full

**Files:** Modify `src/zram.rs`

**Produces:** `parse_zram_mode(s) -> Result<ZramMode, String>`, `resolved_zram_mode(global, package_override) -> Result<ZramMode, String>`, `ZramMode::{Off, Full}`

- [ ] Failing tests for off/full, reject auto/unknown, inherit
- [ ] Implement parse + resolve; `plan_zram_mode` treats Off as NoneNeeded; Full unchanged
- [ ] Remove Auto variant and tests that depend on it

### Task 2: Config validation + per-package field

**Files:** `src/config.rs`

**Produces:** `PackageConfig.zram: Option<String>`, default global zram `"full"`, `check()` rejects bad zram and presets

- [ ] Tests: default full; package zram; reject auto/cachyos/fast
- [ ] Fields + `check()` error strings from spec

### Task 3: Shutdown order + compile/update gates

**Files:** `src/ramdisk.rs`, `src/build.rs`, `src/pgo.rs`, `src/system.rs`

- [ ] Exit: unmount ramdisk then teardown zram
- [ ] `require_headroom` on non-PGO compile, PGO compile, convert, ramdisk mount, system update using resolved mode
- [ ] Off skips setup

### Task 4: Kbench zram down / restore

**Files:** `src/pgo.rs`

- [ ] Persist AFDO/Propeller texts from scratch to clone + archive before teardown
- [ ] Copy failure aborts kbench without teardown
- [ ] Tear down zram, run kbench, restore if mode is Full
- [ ] `copy_to_repo` refuses `.data` and `benchie_*.log`

### Task 5: Per-pipeline compare dirs

**Files:** `src/pgo.rs`, `src/config.rs`

- [ ] Stamp `YYYY-MM-DD-HHMMSS` (local)
- [ ] `PgoState.compare_run_dir`; start/restart creates; resume reuses; missing field creates once

### Task 6: Scraper + docs + wizard + AbsGui + i18n

**Files:** `src/pgo_scraper.rs`, `src/config_wizard/catalog.rs`, `absgui/*`, locales, `abs.toml.example`, `README.md`

- [ ] Drop y-cruncher/blender/cachyos-benchmarker chart series
- [ ] Wizard/GUI: zram off|full, package zram inherit/off/full, presets kernel/kbench/auto
- [ ] Wizard zram visible even when ramdisk disabled
