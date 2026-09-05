# CachyOS no-perf compare preset

Date: 2026-08-31

Supersedes the compare-preset and “do not run CachyOS-benchmarker” clauses in
[`2026-08-30-global-zram-pgo-followup-design.md`](2026-08-30-global-zram-pgo-followup-design.md).
Training under `perf` stays `kernel`. Clone cleanliness still holds: CachyOS
payloads never land in the package git clone.

## Problem

No-perf comparison always runs `kbench`. `compare_preset` is validated as
`kbench`/`auto` and then ignored (`resolved_compare_preset` always returns
`kbench`). The bundled script no longer invokes `cachyos-benchmarker`, and the
Rust scraper drops userspace metric names (blender, y-cruncher, …).

That scoring suite is still useful: it is the CachyOS numbers people already
compare kernels with. It must never run under `perf record` (userspace-bound,
30–60+ min, AutoFDO would treat syscall/VFS/net as cold).

## Goals

- Honor `compare_preset`: `kbench` | `auto` | `cachyos` | `kbench+cachyos`.
- Default unchanged: empty/`auto` → `kbench`.
- CachyOS runs only on standalone no-perf compares (`compare_current`,
  `compare_debug_clean`, `compare_autofdo_clean`, `compare_final`).
- Combined mode: one `benchie_*.log` with CachyOS rows plus appended kbench
  rows. Charts show both.
- Restore CachyOS metric names in the Rust scraper. ABS still writes charts
  (skip CachyOS `benchmark_scraper.py`).
- Fail fast if CachyOS is selected and `cachyos-benchmarker` is missing. No
  silent fallback to kbench.

## Non-goals

- Running CachyOS (or any userspace suite) under `perf record`.
- Changing `benchmark_preset` (still `kernel` only).
- Putting Python on PATH or restoring the old `python` shim.
- Shortening the CachyOS suite via `profiling_quality` (that knob is kbench
  length only).
- Cross-pipeline chart scraping.
- Vendoring CachyOS-benchmarker or its blender/y-cruncher/ffmpeg payloads.
- Unmounting ramdisk for compare (zram teardown for no-perf stays).

## Config

`[packages.<name>.pgo] compare_preset`

| Value | Scoring workload |
| --- | --- |
| unset / empty / `auto` | `kbench` (default) |
| `kbench` | Kernel-path microbenchmarks (~3–10 min from `profiling_quality`) |
| `cachyos` | System `cachyos-benchmarker` (full suite, ~30–60 min; first run downloads) |
| `kbench+cachyos` | CachyOS first, then kbench appended to that run’s log |

Unknown values fail config load and PGO start:

```text
PGO preset does not exist: packages.linux-cachyos.pgo.compare_preset = "fast"
Allowed: kbench, auto, cachyos, kbench+cachyos
```

`benchmark_preset` stays `kernel` only. Leftover `cachyos` / `fast` there still
fail validation (training must not pick up the compare suite).

Script-only alias: `ABS_PGO_BENCHMARK=full` means `cachyos`. Do not accept
`full` in `abs.toml`.

## Pipeline

`resolved_compare_preset` returns the table above (stop hard-coding `kbench`).
`standalone_compare_command` already passes that string as
`ABS_PGO_BENCHMARK`. Profiling still uses `resolved_benchmark_preset` →
`kernel` only. `profile_compare_stage` stays `None` (never append scoring
under `perf`).

Order for a standalone compare is unchanged: persist tiny profiles → tear
down ABS zram → drop page cache → CPU warm-up → scored run → charts.

If any compare checkbox is on and the resolved preset contains `cachyos`,
PGO **start** requires `cachyos-benchmarker` on PATH. Missing binary → die
with an install hint. Do not start a multi-hour pipeline that will fail at
the first scored run.

Same-boot after convert: zram still comes down for every no-perf compare,
including CachyOS (scores otherwise sit on compressed swap).

## Bundled script

`assets/pgo-benchmark.sh` modes:

| `ABS_PGO_BENCHMARK` | Action |
| --- | --- |
| `kernel` / empty | Training (unchanged) |
| `kbench` | Kernel-path scoring (unchanged) |
| `warmup` | CPU turbo warm-up (unchanged) |
| `cachyos` / `full` | `run_cachyos_benchmarker` |
| `kbench+cachyos` | CachyOS, then append kbench lines to that run’s `benchie_*.log` |

Restore the proven wrappers (not a Python-on-PATH shim):

- `find_real_cachyos_benchmarker` — skip leftover `.abs-bin` shims in
  `WORKDIR`.
- `install_skip_scraper_wrapper` — temp copy of the real script with the
  `python … benchmark_scraper.py` line replaced by a skip message. ABS writes
  charts.
- `install_quiet_wget_wrapper` — progress lines without a wget bar.
- Asset-cache check (ffmpeg / linux / y-cruncher / namd / y4m / blend /
  firefox tarball) so the first-run “downloads, very slow” message is honest.
- Move existing `benchie_*.log` aside (`.abs-benchie-prev`) before each
  CachyOS invocation so a leaked scraper cannot mix test counts.
- 2-minute heartbeat (`cachyos-benchmarker still running`).
- `checksys()` stdin: empty (skip extra cache drop — ABS already dropped
  caches) then `ABS_PGO_COMPARE_LABEL`.

`WORKDIR` is `resolved_benchmark_workdir` (default
`{profiles_archive_dir}/benchmark-workdir`). Downloads and extracts stay
there. Never write blender / y-cruncher / ffmpeg / namd / firefox payloads
into the package clone.

### Combined mode

1. Run CachyOS as above (produces `benchie_<label>_*.log`).
2. Collect kbench metrics into a temp file (same `collect_kernel_metrics` as
   standalone kbench).
3. Append those `name: value` lines to the latest CachyOS `benchie_*.log` in
   `WORKDIR`.
4. Do **not** write a second `benchie_*_kbench-*.log` for that stage.

CachyOS failure → stage fails (no kbench-only log). Kbench collect failure
after a successful CachyOS run → stage fails (partial logs would make stages
incomparable). Missing `cachyos-benchmarker` → exit 127.

`ABS_PGO_DRY_RUN=1` prints the plan for `cachyos` / `kbench+cachyos` without
downloading.

## Charts

`src/pgo_scraper.rs` parses known metric names only. Restore userspace names
and keep kernel-path names.

**CATEGORY_1** (wall time, lower is better):

- stress-ng cpu-cache-mem
- perf sched msg fork thread
- perf memcpy
- calculating prime numbers
- namd 92K atoms
- argon2 hashing
- ffmpeg compilation
- xz compression
- kernel defconfig
- blender render
- x265 encoding
- y-cruncher pi 1b

**CATEGORY_2** (unchanged, mixed direction): schbench p99 / avg rps,
cyclictest max latency, plus the kbench syscall/sched/VFS/net metrics.

**EXTRA_METRICS:** Total time (s), Total score.

Restore y-cruncher skip: every sample `<= 0.01` → hatch that bar (Infinity
Scheduler). `ordered_metrics` / SVG / HTML skip names not present in the
logs, so a kbench-only run does not show blender and a CachyOS-only run does
not show hackbench.

HTML categorized caption: “CachyOS + kbench” when any CATEGORY_1 value is
present; otherwise keep the current kbench wording.

## AbsGui, docs, i18n

- Kernel PGO page: compare-preset picker next to the compare checkboxes:
  `kbench` | `cachyos` | `kbench+cachyos` (store `auto` as default in toml
  when unset; picker shows `kbench` for `auto`).
- Wire `KStr` + `PgoSection.compare_preset` (field exists on the model, no
  editor yet).
- Help text: scoring only, never under `perf`; CachyOS needs
  `cachyos-benchmarker` and is 30–60+ min; combined appends kbench to the
  same log.
- `abs.toml.example`, README comparison section, all `abs-i18n` locales.
- Start log line: print the resolved preset, not “always kbench”.

## Tests

Invert the current “CachyOS is gone” assertions. No live
`cachyos-benchmarker` download in CI.

- `validate_pgo_compare_preset`: accept the four values; reject `fast`,
  `full`, `kernel`.
- `resolved_compare_preset`: empty/`auto` → `kbench`; honor `cachyos` and
  `kbench+cachyos`.
- `resolved_benchmark_preset` still always `kernel`.
- Bundled script contains `cachyos)` / `kbench+cachyos)` /
  `install_skip_scraper_wrapper` / `append` of kernel metrics. Still must
  **not** contain `cat > "${bindir}/python"` or `exec /usr/bin/python`.
- `standalone_compare_command(..., "cachyos", ...)` sets
  `ABS_PGO_BENCHMARK='cachyos'`.
- Scraper: CachyOS names parse; kbench names parse; combined fixture charts
  both; blender-only log does not require kbench rows; y-cruncher `0.01`
  hatches.
- PGO start helper (binary presence passed in, no live PATH in CI): compare
  on + preset `cachyos` without binary → error string; compare off + preset
  `cachyos` → no binary required (preset unused). `kbench+cachyos` uses the
  same check.
