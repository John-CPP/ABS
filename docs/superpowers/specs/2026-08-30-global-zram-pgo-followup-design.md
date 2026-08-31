# Global zram, PGO clone cleanup, per-pipeline compare dirs

Date: 2026-08-30

Supersedes the PGO-only, always-on, `auto`/`full` gate in
[`2026-08-30-pgo-oom-zram-design.md`](2026-08-30-pgo-oom-zram-design.md)
for **when** zram is used and **how it is sized**. Ownership (`abs-pgo` label),
`--pgo-priv` allowlist, and “never touch the user’s zram” stay.

## Problem

A finished PGO pipeline showed:

1. Propeller convert used **210 GiB** of zram swap. `auto` (shortfall + 8 GiB,
   first cap `MemAvailable/4`) is too small. The device must always be the
   largest remaining RAM allows (`full`).
2. Zram exists only at convert / PGO compile / ramdisk-mount gates. Ordinary
   package compiles and system update have none. The knob lives under
   `[ramdisk]` and is easy to miss as a global capability.
3. No-perf kbench must not run with ABS zram active (scores are junk).
4. Ramdisk `w` rsync of
   `/media/storage/packages/abs/packages/aur/linux-cachyos` copied leftover
   CachyOS-benchmarker payloads (blender, y-cruncher, firefox tarball, ffmpeg,
   namd), an extracted `linux-6.14.7` tree, and charts onto tmpfs. Kernel
   tarballs already stay on disk via `SRCDEST=.makepkg-src`. Those leftovers
   were **not** shipped in the ABS git bundle; an older run dumped them into
   the clone. The clone was cleaned on 2026-08-30 (518 MiB kernel tree left).
   `/media/storage/tmp/benchmark-workdir` was removed; it is recreated empty.
5. `benchmark_preset = cachyos` / `fast` and `compare_preset` leftovers are
   silently ignored. That hides a broken config.
6. All pipelines write
   `{profiles_archive_dir}/compare-benchmarks/` and replace per-stage logs, so
   runs cannot be compared later.

## Goals

- Global zram: `off` | `full` (default `full`). Per-package inherit / `off` /
  `full`. Always max remaining RAM when on.
- Available for ramdisk compiles, disk compiles, and system update; AbsGui RAM
  Disk tab plus package settings.
- Session-scoped device. Tear down for no-perf kbench (ramdisk stays mounted).
  On process exit / `--purge`: unmount ramdisk **then** zram.
- Stop writing CachyOS-benchmarker junk, raw `.data`, and compare charts into
  the package clone. Keep rsync as-is (no component allowlist).
- Reject unknown PGO presets instead of ignoring them.
- One timestamped compare-benchmarks folder per pipeline start.

## Non-goals

- Permanent zram / `systemd-zram-generator` / disk swap files.
- Changing or removing the user’s zram.
- Rsync allowlists or extra excludes for kernel package files (PKGBUILD can
  grow/shrink). Existing `.git/` and raw `.data` excludes stay.
- Unmounting ramdisk for kbench.
- Cross-pipeline chart scraper (separate folders are enough to compare runs).
- Migrating old flat files in `compare-benchmarks/`.
- Live `swapon` in unit tests.

## Config

### Global

`[ramdisk] zram` — still in that section and on the AbsGui RAM Disk tab, even
when ramdisk is disabled (zram is independent of tmpfs).

| Value | Meaning |
| --- | --- |
| `full` (default) | Bring up ABS zram at max remaining RAM. |
| `off` | Do not bring up ABS zram. |

Unknown values (including leftover `auto`) fail config load.

Sizing when `full` (unchanged from today’s `full` mode):

- `mem_limit = MemAvailable − 256 MiB` (floor 256 MiB; skip if below floor).
- `disksize = mem_limit × 4` (zstd assumed compression).
- Unused zram is a cap, not preallocated.

### Per-package

`[packages.<name>] zram` and the same field on AbsGui package settings (next
to ramdisk targets) and kernel defaults.

| Value | Meaning |
| --- | --- |
| unset / omitted | Inherit global. |
| `full` | On, max size. |
| `off` | Off for this package’s compile. |

Unknown values fail config load. System update has no package: use global
only.

### PGO presets

`[packages.<name>.pgo]`

| Key | Allowed |
| --- | --- |
| `benchmark_preset` | `kernel` only (empty uses default `kernel`) |
| `compare_preset` | `kbench` or `auto` (`auto` = `kbench`) |

Anything else (`cachyos`, `fast`, `kbench+cachyos`, …) fails config load /
PGO start:

```text
PGO preset does not exist: packages.linux-cachyos.pgo.benchmark_preset = "cachyos"
Allowed: kernel
```

AbsGui and the wizard only offer those values.

## Lifecycle

Label, `--pgo-priv`, and “one ABS device” stay as in the OOM zram spec.

**Bring-up** on the first of: package compile (PGO or not), AutoFDO/Propeller
convert, ramdisk mount, system update — if that step wants zram (`full` after
inherit). Keep the device until abs exits, except the teardown cases below.

**Want zram** = resolved mode is `full` (global, or per-package override).

**Per-package `off` while global is `full`:** tear down ABS zram for that
package’s compile, restore afterward if a later step still wants zram. Same
idea as kbench.

### Kbench (no-perf compare)

Zram distorts kernel microbenchmarks. Ramdisk does **not** need to come down:

- After reboot, tmpfs and zram are already gone.
- Same boot after convert: large `.data` is already relocated and unlinked.
  Scratch holds only tiny AFDO/Propeller texts (~2–3 MiB). Converter anon that
  filled 210 GiB of swap has exited.

Order:

1. If Propeller/AFDO profiles exist only on ramdisk scratch
   (`propeller_profiles_on_ram`), copy them to the package clone. If
   `profiles_archive_dir` is set, copy them there too so later
   `restore_profiles_to_repo` still works. Copy failure → **do not** tear down
   zram; fail kbench with that error.
2. Leave ramdisk mounted.
3. Tear down ABS zram.
4. Run kbench (existing drop_caches + CPU warm-up).
5. If the next step wants zram (usually the next compile), bring it back
   `full`.

### Process exit, `--purge`, `--ramdisk-shutdown`

Today `ramdisk::shutdown` tears down zram **before** unmounting tmpfs. Reverse
that:

1. Persist chroot seed if configured (existing).
2. Unmount ABS ramdisk (workdir can be tens of GiB and swapped).
3. Tear down ABS zram.

Ctrl+C still leaves ramdisk mounted for retry, but still copies tiny scratch
profiles to disk **before** zram teardown so retry is not sitting on swapped
tmpfs for those files.

Zram teardown failure is logged; do not hang.

## Where zram runs

| Step | Source of mode |
| --- | --- |
| AutoFDO / Propeller convert | Per-package then global (same package as the pipeline) |
| PGO kernel compile | Per-package then global |
| Non-PGO package compile | Per-package then global |
| Ramdisk mount | Global |
| System update (`-U` / `-RU` pacman/yay) | Global |
| No-perf kbench | Always tear down first |

Convert still uses the existing `need` estimates (2×/6× file + `min_free_ram`)
for the OOM prompt. Bring-up itself is always max (`full`), not shortfall.
If setup fails or the floor is not met: same four-choice loop (re-check /
extend / continue / stop). No TTY → stop.

## Clone cleanliness

No rsync allowlist. After the 2026-08-30 cleanup, a clone rsync is PKGBUILD,
config, `.SRCINFO`, git, AFDO/Propeller profiles, `.makepkg-src`.

ABS must not put junk back:

- Training/compare stay kernel + kbench. Do not download or extract blender,
  y-cruncher, ffmpeg, firefox, namd, or wrap `cachyos-benchmarker`.
- `perf` output stays on scratch or convert-spill. After convert, delete raw
  `.data` there. Never write `.data` into the clone.
- Comparison logs and charts go only under the pipeline compare dir (below).
  Not the clone.
- Kernel tarball stays in `.makepkg-src` (`SRCDEST`). Do not leave an
  extracted `linux-*` tree in the clone.
- Remove dead CachyOS-benchmarker series from `pgo_scraper` (y-cruncher /
  blender). Ignore old log metric names; keep kbench metrics.

## Per-pipeline compare directories

On **`--pgo` start** and **`--pgo-restart`**, create:

```text
{profiles_archive_dir}/compare-benchmarks/YYYY-MM-DD-HHMMSS/
```

Local time, filesystem-safe (example:
`/media/storage/tmp/compare-benchmarks/2026-08-30-211600/`).

- All kbench logs and charts for that pipeline go only in that folder.
- Store the path in PGO state. Resume after reboot writes to the same folder.
- Old state without the field: create a folder on first compare of that
  resume, save it, reuse it for the rest of the pipeline.
- `--pgo-restart` / a new `--pgo` → new timestamp.
- Leave existing files that sit directly in `compare-benchmarks/` (no migrate).
- Charts still compare stages inside one pipeline. No cross-run scraper.

## AbsGui and wizard

- RAM Disk tab: zram picker `off` | `full` (drop `auto`). Help text: max
  remaining RAM; used for compiles and system update, not only PGO convert.
- Package editor and kernel defaults: zram inherit / off / full next to
  ramdisk targets.
- PGO preset dropdowns: only `kernel` and `kbench`/`auto`.
- Wizard catalog, `abs.toml.example`, README, all `abs-i18n` locales.

## Tests

Pure helpers, no live `swapon`:

- Parse `off`/`full`; reject `auto` and unknown.
- Per-package inherit / off / full resolution.
- Invalid `benchmark_preset` / `compare_preset` fail; `auto` → kbench;
  `kernel` accepted.
- Compare dir name matches `YYYY-MM-DD-HHMMSS`; resume reuses state path.
- Kbench order helper: persist profiles → zram down, ramdisk not unmounted.
- Shutdown order helper: ramdisk unmount before zram teardown.
- Clone write paths: converted profiles may land in the clone; `.data` and
  `benchie_*.log` must not.

## Pipeline vs previous zram spec

Relocate is unchanged. Zram is no longer “add only the shortfall at PGO
gates.” It is a session-scoped max-size ABS swap for any compile/update that
opts in, with a mandatory down-period for kbench and a ramdisk-then-zram
teardown on exit.
