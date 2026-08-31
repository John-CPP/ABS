# PGO ramdisk residency and RAM-reboot Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep only the current PGO step’s necessary files on tmpfs, drop raw captures after convert, and remount or reboot when leftover PGO RAM cannot be freed.

**Architecture:** Pure decision helpers and scratch cleanup in `src/pgo.rs`; blocking remount in `src/ramdisk.rs`. Collection reuses ramdisk `.data` only when conversion has not finished. Interactive stdin prompts sit in front of AutoFDO/Propeller `perf record`.

**Tech Stack:** Rust (`abs` binary unit tests), existing `sudo umount`/`mount` ramdisk helpers, user systemd `abs-pgo@` resume unit.

## Global Constraints

- Do not copy raw `kernel.data` / `propeller.data` to the package repo or archive.
- Do not use global `Meminfo Shmem`; only leftover scratch files and `app-abs-pgo.slice` `memory.stat` `shmem`.
- Reclaim threshold: 1 GiB PGO slice shmem.
- Auto reboot uses `cli.pgo_auto || pgo.auto_restart`. RAM reboot keeps `stage2_profile` / `stage3_profile` and does not change the bootloader oneshot.
- Default Yes for remount and for the optional reboot question (`[Y/n]`).
- Do not lazy-unmount on the free-space remount path.
- No new GUI overlay.

---

### Task 1: Decision helpers

**Files:**
- Modify: `src/pgo.rs` (helpers + tests next to existing perf tests)

**Produces:**
- `parse_cgroup_shmem_bytes(stat: &str) -> Option<u64>`
- `pgo_shmem_unreclaimable(shmem: Option<u64>) -> bool`
- `parse_confirm_default_yes(input: &str) -> bool`
- `should_reuse_raw_perf(usable: bool, converted_ready: bool) -> bool`
- `ram_reclaimed(leftovers_empty: bool, shmem: Option<u64>) -> bool`

- [ ] Tests then implementation for the helpers above.

### Task 2: Scratch leftover listing and unlink

**Files:**
- Modify: `src/pgo.rs` (`leftover_pgo_scratch_files`, `drop_pgo_scratch_captures`, `drop_raw_perf_after_convert`)

**Produces:** listing of `.data`, sidecars, probe, scratch `.afdo`, propeller texts; unlink helpers that leave missing paths alone.

- [ ] Tests then implementation.

### Task 3: Reuse policy and stop copying raw captures to repo

**Files:**
- Modify: `src/pgo.rs` `collect_or_reuse_perf_data`, `existing_perf_data`, `run_stage2_profile`, `run_stage3_profile`

**Produces:** reuse scratch `.data` only when `converted_ready` is false; never `sync_perf_data_to_repo` for raw captures; after successful convert, drop ramdisk `.data` (and scratch converted copies that now exist on disk). Before Propeller capture, drop leftover `kernel.data`.

- [ ] Update/add tests (`existing_perf_data` scratch-only reuse; drop-after-convert).

### Task 4: Blocking remount

**Files:**
- Modify: `src/ramdisk.rs`

**Produces:**
- `unmount_blocking(mount: &Path) -> Result<(), String>` (no `-l`)
- `remount_ramdisk_fresh(config: &Config) -> Result<(), String>` (kill holders, blocking umount, mount new tmpfs, skip chroot reuse)

- [ ] Unit-test any parse/path helpers; remount itself is integration-wired from PGO.

### Task 5: Prompt + reboot gate

**Files:**
- Modify: `src/pgo.rs` (`prepare_profile_ram` before collection)

**Produces:** interactive remount prompt when leftovers exist and we are not retrying convert; after remount, if `ram_reclaimed` is false, reboot gate (`trigger_pgo_auto_reboot` when auto, else ask; save state; no `perf record`).

- [ ] Tests for `parse_confirm_default_yes` and `ram_reclaimed`; gate uses those.

---
