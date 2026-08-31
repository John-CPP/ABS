# PGO OOM zram (temporary ABS-owned swap)

Date: 2026-08-30

## Problem

Convert relocate already moves raw `.data` off tmpfs. Convert can still OOM. On 2026-08-30, `generate_propeller_profiles` hit **88.3 GiB anon RSS** on a **96 GiB, swapless** machine (`shmem-rss: 0`). A second run the same day added **4.9 GiB** of 1:1 zram (`disksize == mem_limit`); convert reached **87.7 GiB RSS + 4.25 GiB swapped** (~4× the 23 GiB file), filled swap, and was killed again. The user does not want a permanent zram setup. ABS must add swap only when a step looks too big, size it so the task actually fits, then remove it if it was not already there.

## Goals

- Before AutoFDO/Propeller convert, PGO kernel compile, and ramdisk mount: if the step’s estimate does not fit in `MemAvailable + SwapFree`, ABS adds an ABS-owned zram device for the shortfall.
- Existing swap/zram is never modified or removed.
- If the machine is still too tight after that, explain the situation, offer choices, and re-check until the user continues or stops.
- Tear down only the ABS device on abs exit, pipeline end, abort, `--purge`, and leftover cleanup on the next abs start.
- No new config key. Always on.

## Non-goals

- Permanent zram, `systemd-zram-generator`, or disk swap files.
- Growing or replacing the user’s zram.
- Inventing a kernel-compile peak (no per-`-j` model).
- Changing convert_relocate. Relocate still decides tmpfs vs disk; zram covers anonymous working set after that.
- Live `swapon` in unit tests.

## When the gate runs

| Step | Call site | `need` |
| --- | --- | --- |
| AutoFDO convert | `stage2_profile`, after relocate, before the converter | `2 × file_size + min_free_ram` when the tool is `llvm-profgen`; otherwise same as Propeller (`6 ×`) |
| Propeller convert | `stage3_profile`, after relocate, before the converter | `6 × file_size + min_free_ram` |
| PGO kernel compile | PGO stage 1/2/3 `makepkg` / build, before compile starts | `min_free_ram` only |
| Ramdisk mount | `ramdisk::mount_session` (replaces today’s `die!` when `MemAvailable < min_free_ram_mb`) | `min_free_ram` only |

`min_free_ram` is `ramdisk.min_free_ram_mb` in bytes. Unknown `MemAvailable` fails closed (treat as short).

`have` = `MemAvailable` + non-ABS `SwapFree` + ABS zram uncompressed pages (`min(disksize, 2 × mem_limit)`; `mem_limit` 0 = unlimited = `disksize`). A 1:1 leftover still counts its swap pages; it was too *small* (4.9 GiB), not worth 0. Shortfall = `max(0, need − have)`.

If `have >= need`, start the step. Do not prompt.

## Bring-up

Only when shortfall > 0:

1. If a swap device labeled `abs-pgo` already exists, reuse it unless planned `disksize` or `mem_limit` is larger — then `swapoff` + reset + recreate. Never create a second ABS device. Never change any other zram device.
2. Otherwise: `modprobe zram` if `/dev/zram*` is missing; `zramctl --find`; algorithm `zstd`.
3. `mem_limit` = `min(shortfall + 8 GiB, cap)`. Cap starts at `MemAvailable / 4`. Each **Extend** raises it to ½, then to `MemAvailable − 256 MiB`. If `mem_limit < 256 MiB`, skip zram and go to the loop.
4. `disksize` = `shortfall + 8 GiB` (uncompressed swap **pages**; 18:46 filled a 36 GiB device that was only `2 × mem_limit`). `mem_limit` only caps compressed RAM.
5. `mkswap -L abs-pgo` and `swapon` on that `/dev/zramN` only.
6. Re-read usable `have` (2:1-backed extra, not disksize). If still short, go to the loop.

If an undersized `abs-pgo` leftover exists, recreate it at this target (max of existing vs planned). Do not add a second device. Do not grow by summing the leftover with the plan (that left a 4.9 GiB 1:1 device in place).

## Interactive loop

If still short (zram skipped, setup failed, cap too small, or mem_limit does not cover `need`):

Print what is next, `need`, `have`, shortfall, and what ABS already tried.

Choices:

1. **Re-check now** — user may have closed apps or freed RAM; retry zram at the current cap.
2. **Extend ABS zram** — recreate the ABS device with a higher `mem_limit` (½ of RAM, then almost all remaining RAM).
3. **Continue anyway** — start the step; OOM risk is accepted.
4. **Stop** — do not start the step; leave pipeline state so resume can retry. Tear down ABS zram on the way out.

Empty input / unknown text = re-check, never continue. AbsGui PTY uses the same four choices. No TTY (including `auto_restart` with no terminal): print the same explanation and **stop**. Do not hang. Do not continue.

The loop repeats until continue or stop.

## Ownership and teardown

Ownership is swap label `abs-pgo` on `/dev/zramN`. Any other label is the user’s.

Teardown is `swapoff` + `zramctl --reset` on the `abs-pgo` device only:

- abs process exit (same ramdisk-style exit handlers)
- pipeline end, abort, `--purge`
- next abs start: leftover `abs-pgo` is adopted as ours (reuse at later gates in this process; always tear down on this process exit). A leftover with any other label is ignored.

Keep the device across convert → compile → ramdisk in the same process. Reboot drops zram; resume runs the gate again.

## `--pgo-priv` allowlist

Allow only:

- `modprobe zram` (no extra module parameters)
- `zramctl` targeting `/dev/zramN` (`--find`, `--size`, `--algorithm`, `--reset`)
- `mkswap -L abs-pgo /dev/zramN`
- `swapon` / `swapoff` of `/dev/zramN`

Reject `swapon` of anything else (swap files, `/dev/sd*`, unlabeled policy is still `/dev/zramN` only). `mkswap` without label `abs-pgo` is rejected.

## Tests

Pure helpers, no real `swapon`:

- Shortfall: `need`, `MemAvailable`, `SwapFree` → expected extra (`shortfall + 8 GiB`) and `disksize = 2 × mem_limit`
- Cap: `mem_limit = min(shortfall + 8 GiB, MemAvailable/4)`; skip zram when that is `< 256 MiB`
- 1:1 leftover (`disksize == mem_limit`) nets 0 extra; 2:1 device nets `mem_limit`
- Label parse: `abs-pgo` vs other labels vs no label
- Helper: allow `swapon /dev/zram0`; reject `swapon /var/swapfile` and `mkswap /dev/zram0` without `-L abs-pgo`
- Prompt parse: recheck / continue / stop / empty → recheck

## Pipeline vs relocate

Relocate still runs first for convert. Zram is the next defense for converter anon (and the ramdisk/compile floor). A 23 GiB Propeller file on a 96 GiB box: `need ≈ 92 GiB + min_free`; after relocate, `MemAvailable` is high but not 92 GiB + OS. ABS adds ~`(shortfall + 8 GiB) × 2` of zstd swap (about 26 GiB disksize / 13 GiB mem_limit on the 14:37 numbers) instead of an OOM kill.
