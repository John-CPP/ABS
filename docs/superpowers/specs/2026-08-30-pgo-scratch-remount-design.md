# PGO ramdisk free / remount / RAM-reboot

Date: 2026-08-30

## Problem

`perf record` writes raw captures onto the ABS ramdisk tmpfs (`/run/abs-ram/pgo-scratch/<package>/`). That mount **is RAM**. Bytes in `kernel.data` / `propeller.data` count as used memory until the files are deleted or the tmpfs is unmounted.

**Finished work is the converted profiles, not the raw `.data` file.** AutoFDO is finished when `kernel-compilation.afdo` is on disk (repo/archive). Propeller is finished when `propeller_cc_profile.txt` / `propeller_ld_profile.txt` exist for the same-boot build (on disk, or on ramdisk only if `propeller_profiles_on_ram` is on). Today ABS also copies huge `kernel.data` to the repo and leaves the ramdisk copy after conversion. Neither copy is required once conversion succeeded.

A raw `.data` that was **never** converted (OOM, abort, crash) was never finished. ABS today may still treat a large leftover on `/run` as reusable and skip a new capture. That is the wrong signal when the user is starting collection again.

Restarting AutoFDO/Propeller collection therefore needs a clean ramdisk because:

1. Leftover raw `.data` is either an unfinished capture or a copy that should not keep occupying RAM.
2. Those bytes compete with the **next** capture. A 20 GiB AutoFDO `kernel.data` still on tmpfs is 20 GiB the Propeller run cannot use for `propeller.data`. `--mmap-pages` is a separate ring-buffer allocation in the same RAM pool; leftover files do not “break mmap” by themselves, they just leave less memory free.
3. After `rm` or a lazy `umount -l`, detached tmpfs pages can stay charged to `app-abs-pgo.slice` with no mount and no process mapping. A new mount does not reclaim that superblock. A reboot does.

## Goals

- At each PGO step, ramdisk holds only what that step still needs (see residency below).
- Interactive AutoFDO/Propeller collection starts on a clean scratch when leftovers exist; the user is asked before remounting the whole ramdisk.
- If remount cannot drop leftover PGO tmpfs accounting, collection does not start.
- Reboot policy matches existing PGO wait stages: `auto_restart` / `--pgo-auto` reboots and resumes; otherwise ask.
- After a successful reboot, resume the **current profile stage** on the same kernel (not `wait_reboot1/2`).

## Non-goals

- Changing LLVM `-c` periods or mmap-page defaults (already handled separately).
- A second GUI overlay; `abs` already runs in a terminal from absgui.
- Wiping converted profiles in the package repo or archive.
- Using global `Meminfo Shmem` (browsers / psd). Only PGO ramdisk usage and `app-abs-pgo.slice` shmem.
- Copying raw `kernel.data` / `propeller.data` to HDD “just in case.” Crash before conversion means recapture unless the ramdisk copy is still there.

## Ramdisk residency (what may occupy RAM)

Rule: **one necessary PGO scratch artifact on tmpfs at a time.** Build chroot/`work` follow existing ramdisk flags and are unchanged.

| Step | On ramdisk (necessary) | On disk | Drop from ramdisk |
| --- | --- | --- | --- |
| AutoFDO `perf record` | `kernel.data` growing | existing `.afdo` if any (unused until this conversion finishes) | any prior `propeller.data`, converted copies, probe files |
| AutoFDO convert | none after `convert_relocate` (default `force` copies `.data` to `{archive}/pgo-convert/<pkg>/`) | write `.afdo` to repo+archive | after success: delete ramdisk and convert-scratch `.data` |
| AutoFDO kernel build (same boot) | chroot/`work` only | `.afdo` in repo | no PGO `.data` |
| Propeller `perf record` | `propeller.data` growing | `.afdo` in repo (needed later for stage-3 **build**, not for this capture) | `kernel.data` must already be gone |
| Propeller convert | none after relocate, unless `convert_relocate = "smart"` kept it on tmpfs | if persist-to-disk: write cc/ld texts to repo+archive | after success: delete ramdisk and convert-scratch `.data`. If persist-to-disk, also delete scratch cc/ld. If `propeller_profiles_on_ram`: keep **only** the small cc/ld texts on scratch for the same-boot build |
| Propeller kernel build (same boot) | chroot/`work`; plus cc/ld texts **only** when `propeller_profiles_on_ram` | `.afdo`; cc/ld if they were persisted | no `propeller.data` |

Raw `.data` is never copied to the package repo. Convert staging lives under `{profiles_archive_dir}/pgo-convert/<package>/` only while convert has not finished. Converted profiles are never kept on ramdisk once a disk copy exists, except the `propeller_profiles_on_ram` exception (small texts, same boot, no HDD archive).

If conversion fails, keep the convert-scratch (or ramdisk) `.data` so convert can retry without recapturing. That file is still necessary.

## When the prompt runs

Before `perf record` in `stage2_profile` and `stage3_profile`, if **either**:

- Leftover files exist on the package scratch: `kernel.data`, `propeller.data`, `abs-pgo-branch-stack-probe.data`, or their `.kernel.json` sidecars; or
- PGO cgroup shmem is still large (`app-abs-pgo.slice` `memory.stat` `shmem` ≥ 1 GiB) even when the mount is gone.

If neither is true, do not prompt.

`--pgo-auto` (systemd after reboot, or `pgo.auto_restart`) skips the optional wipe prompt when there is nothing unreclaimable. If unreclaimable PGO shmem is still present, follow the reboot gate below even in auto mode (collection cannot succeed).

## Prompt (interactive)

Default **Yes**:

```
PGO scratch on the ramdisk still holds leftover captures / tmpfs pages
(<path>, <size>; PGO cgroup shmem <N> GiB).
Remount the ramdisk for a fresh AutoFDO/Propeller capture? [Y/n]
```

- **Yes:** remount path (below).
- **No:** keep the current ramdisk (cached chroot stays). Do **not** treat leftover ramdisk `.data` as a finished profile; delete the package scratch captures when that is possible without remounting. If PGO shmem is still unreclaimable, take the reboot gate. Collection only proceeds when RAM is actually free.

## Remount (Yes)

1. Kill processes with cwd or open files under `ramdisk.mount_point`.
2. Blocking `umount` of the mount point. Do **not** count lazy `umount -l` as success for this path.
3. `mount` a new tmpfs with the configured size/mode and recreate the usual tree (`work`, `chroot`, `packages`, `pgo-scratch/<package>`).
4. Cached ramdisk chroot is discarded. That is intended.

`reclaim_mount_on_startup` must not reuse a dirty tmpfs when the user asked to free space.

## Reclaim check

After remount (or after a failed blocking umount):

**Reclaimed** if the package scratch has no leftover captures **and** PGO slice `shmem` is below 1 GiB (or the slice is gone).

**Unreclaimable** if blocking umount failed, lazy umount was required, or PGO slice `shmem` stays ≥ 1 GiB.

Unreclaimable → reboot gate. Do not start `perf record`.

## Reboot gate

Keep `current_stage` as `stage2_profile` or `stage3_profile`. Do not switch to `wait_reboot*`. Do not change the oneshot bootloader entry; reboot into the kernel already running.

Auto is `cli.pgo_auto || pgo.auto_restart` (same as wait stages).

**Auto on:** install `abs-pgo@<package>.service`, log that detached tmpfs is still holding RAM, 5 s countdown, reboot. After boot the unit runs `--pgo-resume <package> --pgo-auto`, opens a visible terminal as today, and runs the same profile stage on a clean machine (empty scratch, no prompt).

**Auto off:** do not reboot on our own. Print that collection cannot continue until RAM is released, and ask:

```
Reboot now to free leftover PGO ramdisk memory?
After reboot run: abs --pgo-resume <package>
Reboot now? [Y/n]
```

- **Yes:** save state, reboot (no auto-resume unit). User resumes by hand.
- **No:** save state, stop. User can reboot later and `--pgo-resume`. Still no `perf record`.

## Error / logging

Log measured scratch bytes, PGO slice shmem before/after remount, and whether umount was blocking or lazy. On the reboot gate emit the existing `RebootRequired` event so the GUI status line can show a reboot is needed.

## Testing

- Leftover scratch files → interactive path would prompt; stdin `y` remounts; stdin `n` deletes package scratch captures without unmounting the whole ramdisk.
- Starting collection does not skip `perf record` just because a large ramdisk `.data` exists (converted `.afdo` / propeller texts on disk are the finished artifacts).
- Blocking umount success + slice shmem below threshold → collection may proceed (do not actually call `perf` in the unit test).
- Slice shmem ≥ 1 GiB after remount → reboot gate; auto flag true → would call the same helper as wait-stage auto reboot (mock reboot).
- Auto flag false → reboot gate asks; `n` saves state and does not reboot.
- State file stays on `stage2_profile` / `stage3_profile` across the RAM reboot path.
- Global Shmem / browser cgroups are ignored.

## Implementation sketch

- After successful AutoFDO/Propeller conversion, unlink ramdisk `.data` (and scratch copies of profiles that now exist on disk). Stop copying raw captures to the package repo.
- Before Propeller collection, drop leftover AutoFDO `kernel.data` from scratch (conversion already produced `.afdo` on disk).
- Helpers in `src/ramdisk.rs`: blocking unmount, remount, PGO slice shmem read, leftover scratch listing.
- Gate in `src/pgo.rs` immediately before `probe_branch_stack_sampling` / `collect_or_reuse_perf_data` for stages 2 and 3.
- Stdin confirm via the existing yes/no helper style (`src/purge.rs` `confirm_yes_no`), default yes for remount, default yes for optional reboot when auto is off.
- Reuse `install_pgo_auto_resume_service` + `trigger_pgo_auto_reboot` for auto; `boot_entry::reboot(None)` when the user confirms reboot without auto.
