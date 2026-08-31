# PGO convert relocate (tmpfs → disk)

Date: 2026-08-30

## Problem

`perf record` writes raw `.data` onto the ramdisk tmpfs. Conversion (`generate_propeller_profiles`, `llvm-profgen`) then builds a large anonymous working set **while the tmpfs file still occupies RAM**. On 2026-08-30, a 23 GiB `propeller.data` plus ~64 GiB converter anon (~2.8× the file) OOM-killed `generate_propel` on a 96 GiB swapless machine (cgroup peak 87.5 GiB).

Finished work is still the converted profile. The raw file is only needed until convert succeeds. It does not need to stay on tmpfs during convert.

## Config

`[packages.<pkg>.pgo] convert_relocate`

| Value | Meaning |
| --- | --- |
| `force` (default) | If the capture is on tmpfs, copy it to disk convert-scratch, unlink the tmpfs copy, then convert. |
| `smart` | Keep on tmpfs only when remaining `MemAvailable` clearly covers a **pessimistic** convert working set plus `ramdisk.min_free_ram_mb`. Otherwise same as force. Unknown `MemAvailable` → relocate. |

No `never`. Scratch already on disk is a no-op.

## Smart estimate (fail closed)

Measured: `generate_propeller_profiles` anon RSS / file size ≈ 2.80 on a 23 GiB LBR capture. Tmpfs pages are extra (already excluded from `MemAvailable`).

Keep on tmpfs iff:

```
MemAvailable >= convert_anon_estimate + min_free_ram_bytes
```

| Tool | Estimate | Why |
| --- | --- | --- |
| Propeller (`generate_propeller_profiles`, `create_llvm_prof`) | `6 × file_size` | 5.0× RSS+swapents observed 2026-08-30 18:46; 6× is the bar. |
| `llvm-profgen` | `2 × file_size` | Streams more than Propeller; AutoFDO 8 GiB converted on this host. 2× is still pessimistic. |

`MemAvailable` already excludes the tmpfs file, so this is “will convert’s **new** anon fit.” If the inequality fails, relocate (free the file, then convert from reclaimable disk pages).

Disk mmap is reclaimable; convert may still need ~3× file as anon. Relocate does not refuse convert when the estimate is tight — it only decides residency. Force is the safe default.

## Spill location

`{profiles_archive_dir}/pgo-convert/<package>/<kernel.data|propeller.data>` (+ `.kernel.json` sidecar).

Not the package repo. Not a finished profile. Reuse this file to retry convert after a crash. After successful convert, delete spill and any leftover ramdisk copy.

Copy + unlink (cross-device). Then `drop_caches` so dest page cache does not sit beside convert.

## Pipeline

After `collect_or_reuse_perf_data`, before convert, in `stage2_profile` and `stage3_profile`.
