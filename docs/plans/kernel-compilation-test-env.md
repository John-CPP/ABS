# Plan: dedicated environment for kernel compilation workflow tests

**Status:** ideas only — do not implement from this file until we pick an option.  
**Goal:** let an agent exercise ABS kernel build / PGO flows without rebooting the developer’s host.

The host (CachyOS + QEMU) stays up. Reboots, kernel installs, ramdisk mounts, and bootloader experiments happen only inside a throwaway guest (or cheaper stand-ins that never boot a new kernel).

---

## 1. What “the workflow” actually is

ABS has two kernel paths. They share config (`[packages.PKG.kernel]`, ramdisk letters, chroot) and diverge at install/boot.

### One-shot kernel (`abs --kernel-build PKG`)

1. Fetch/sync the kernel PKGBUILD tree.
2. Apply `[packages.PKG.kernel]` overrides (`_use_llvm_lto`, scheduler, etc.).
3. `makepkg` or `makechrootpkg` (`-h`).
4. Optional ramdisk (`w` = `src/`/`pkg/` on tmpfs; `c` = chroot on tmpfs; `p` = git tree on tmpfs — PGO warns that `p` dies across reboot).
5. `pacman -U` the resulting packages (headers, modules, vmlinuz, initramfs).
6. Stop. No reboot required to *test the compile*, but you cannot prove the kernel *boots* without one.

### Kernel PGO (`abs --pgo linux-cachyos`, CachyOS preset)

Persisted in `~/.config/abs/pgo/<package>.json`. Stages:

| Stage | What happens | Host impact if run on the laptop |
| --- | --- | --- |
| `stage1_build` | Debug AutoFDO kernel, install | Long compile + `pacman -U` |
| `wait_reboot1` | Hint / `--pgo-auto` → `sudo reboot` in 5s | **Reboots the machine** |
| `stage2_profile` | Boot check (`uname` + `/usr/lib/modules/<uname>/pkgbase`), `perf` + `pgo-benchmark.sh` | Must be **on** the stage-1 kernel |
| `stage2_build` | Thin-LTO `-lto` kernel with AFDO profile, install | Another long compile |
| `wait_reboot2` | Same as wait 1 | **Reboots again** |
| `stage3_profile` | Must be on the stage-2 *pkgbase* (version string alone is not enough) | Wrong kernel = ABS dies |
| `stage3_build` | Final kernel, install | Third compile |
| `done` | Removes user unit `abs-pgo@PKG.service` | |

`--pgo-auto` / `auto_restart = true` installs a **user** systemd oneshot (`ExecStart=abs --pgo-resume %i --pgo-auto`, `WantedBy=default.target`). That needs linger **or** a graphical login after boot. Profiling defaults to `ABS_PGO_BENCHMARK=fast` (sysbench + optional stress-ng, minutes). `cachyos` mode is a 30–60+ min download-heavy opt-in.

`--pgo-once`, `--pgo-goto`, `--pgo-stage`, `--pgo-status --json`, `--event-log=PATH` already exist and should be the agent’s control surface.

**Implication:** a container or chroot can compile. Only a VM (or the host) can reboot into the just-installed kernel and satisfy `verify_boot_kernel`. That is why the host must not be the PGO machine.

---

## 2. Test in layers (do not always compile linux-cachyos)

Use the cheapest layer that can fail the change under test. Full CachyOS PGO is hours and three kernel builds.

| Layer | Environment | Proves | Does not prove | Typical time |
| --- | --- | --- | --- | --- |
| **L0** | `cargo test` on the host | Stage IDs, `boot_matches` / pkgbase, CLI parsing, systemd unit text | Any real makepkg, boot, perf | seconds |
| **L1** | Host `abs --dry-run --pgo …` | Command graph, sudo prompts skipped, ramdisk “would mount” | Build, install, reboot | seconds |
| **L2** | Guest or throwaway overlay: stub `makepkg` / fake `.pkg.tar.zst` that installs a dummy `pkgbase` | State file, `--pgo-resume`, event JSON, holds during PGO, install prompt path | Real compiler, real boot | minutes |
| **L3** | QEMU guest, **tinyconfig** (or `linux` with a tiny `.config`) one-shot | Real `makepkg`, `pacman -U`, mkinitcpio, bootloader entry, **guest reboot**, `uname` | CachyOS PKGBUILD knobs, AutoFDO/Propeller, ramdisk size | ~5–20 min compile + reboot |
| **L4** | Same guest, `abs --kernel-build linux-cachyos` | Real CachyOS oneshot + install | PGO profiles / second kernel | ~30–90+ min |
| **L5** | Same guest, full `--pgo` with `auto_restart`, fast benchmark | The product pipeline, including two guest reboots and pkgbase disambiguation of `-lto` | Host hardware quirks, GPU, your exact laptop bootloader | hours |

**Recommendation:** implement L0–L2 first when we start (still not this document). Keep a QEMU snapshot ready for L3+. Run L5 only when PGO/reboot/boot-verify code changed, or as a rare soak.

Do **not** try to fake PGO by skipping boot verification. Stage 2 vs 3 *must* boot different pkgbases (`linux-cachyos` vs `linux-cachyos-lto`). That is a real bug class (`boot_matches` comments in `src/pgo.rs`).

---

## 3. Preferred environment: QEMU/KVM guest (you already use QEMU)

A dedicated x86_64 **KVM** VM named something like `abs-kernel-lab`. The agent talks to it over SSH (and optionally the QEMU monitor). The guest may `reboot` as often as it wants.

### Why QEMU rather than “just another VM product”

- You already run it; KVM is the fast path on this machine.
- Snapshots (`qemu-img snapshot` or libvirt) undo a botched `pacman -U`.
- Serial console (`-serial mon:stdio` or a socket) lets the agent see bootloader + kernel panic without a GUI.
- virtiofs/9p can share the ABS git tree and ccache from the host without copying tens of GB.
- Nested virtualization is **not** required (the guest compiles kernels; it does not need to run QEMU itself).

libvirt (`virsh`) on top of the same QEMU is optional sugar: `virsh snapshot-revert`, `virsh reboot`, DHCP leases. Fine if you already use virt-manager; not required.

### Guest OS

**CachyOS** (same family as the host) is the faithful target: `linux-cachyos` PKGBUILD, repos, `pkgbase`, systemd-boot or Limine, `cachyos-perf-sysctl`.

Arch with CachyOS repos is a fallback if a CachyOS cloud/QEMU image is annoying to seed. Do not use Debian/Fedora guests — ABS is pacman/`makepkg`.

### Resources (starting point, tune after first compile)

| Resource | L3 tiny kernel | L4/L5 linux-cachyos |
| --- | --- | --- |
| vCPU | 4 | All host cores minus 2 (`-smp`, `--cpu host`) |
| RAM | 8G | **32G** if ramdisk `w`; **16G** disk-only |
| Disk | 40G qcow2 | **80–128G** qcow2 (objects, packages, headers, initramfs) |
| Firmware | UEFI (OVMF) | UEFI, matching CachyOS (systemd-boot or Limine) |
| Network | virtio-net, user or bridge | Same; needs pacman + git |

Ramdisk `w` during a CachyOS kernel build is a tmpfs of unpacked `src/` + `pkg/` — that is the RAM spike. For the lab, prefer **disk-backed compile** (`ramdisk` off or `w` only with a high `size=`) so the guest does not OOM. Keep `p` off for PGO (lost on reboot).

### Disk / share layout

Keep **guest-local**:

- `/boot`, ESP, bootloader — never bind-mount the host’s
- Guest `/usr/lib/modules`
- Guest `abs.toml`, PGO state, user systemd units

Share from host (virtiofs or a second virtio-blk):

- ABS git checkout (agent builds `abs` on the host, installs the binary into the guest, **or** `cargo` inside the guest)
- Optional: `packages_path`, `ready_made_packages_path`, `profiles_archive_dir`, `ccache` — so L5 does not re-download the kernel tarball every snapshot revert

`profiles_archive_dir` **must survive guest reboot** (normal disk or virtiofs). tmpfs for that path would break PGO.

### Unattended boot (the hard part)

After `pacman -U`, ABS does **not** set the bootloader default. It prints “choose `/boot/vmlinuz-<pkgbase>`”. On a laptop that is you at the firmware menu. In a lab the agent cannot sit there.

Ideas (pick one when implementing):

1. **Boot timeout 0 + default = last installed kernel** (systemd-boot `default`, Limine `default_entry`). Usually enough for oneshot and for stage-1. Risky at wait_reboot2 if both kernels exist and default still points at stage-1.
2. **Oneshot next-boot entry** (`bootctl set-oneshot …` or Limine equivalent) from a small helper after install, using `expected_package_base` from PGO state. Closest to “human picked the right line”.
3. **QEMU `-kernel` / `-initrd` / `-append`** for L3 only: skip the guest bootloader. **Does not** test mkinitcpio/ESP. Do not use this as the L5 success criterion.
4. Serial + EFI menu automation: brittle; last resort.

Also: `loginctl enable-linger` for the abs user so `--pgo-auto` resumes without a GUI login. Headless guests will otherwise sit in `wait_reboot*` after reboot with the unit never starting.

SSH must come back after reboot (`PermitRootLogin` or a key for the abs user, `UseDNS no`). The agent loop is: run command → SSH drop → wait for port 22 → check `uname -r` and pkgbase → continue or collect logs.

### QEMU sketch (not a final script)

Direction only:

- `-enable-kvm -cpu host -smp N -m 32G`
- OVMF pflash
- virtio-scsi qcow2 + virtiofs for the repo
- `-netdev user,hostfwd=tcp::2222-:22` **or** a bridge
- serial to a unix socket or file (agent reads boot)
- **do not** pass `-no-reboot` (guest reboot must work)
- snapshot `clean` after first successful CachyOS install + rustup/base-devel/devtools/sysbench

### How the agent would drive it (later)

1. Refuse to run PGO/reboot if `systemd-detect-virt` is `none` **and** hostname is the laptop — safety rail.
2. `qemu-img snapshot -a clean` (or libvirt revert).
3. Start VM, wait for SSH.
4. Sync `abs` binary + `abs.toml` (lab config: no `auto_restart` until linger is proven).
5. Layer L3/L4/L5 command with `--yes --no-wait --event-log=/tmp/pgo.jsonl`.
6. On SSH death during `--pgo-auto`, wait, reconnect, `abs --pgo-status PKG --json`.
7. Save logs, revert snapshot.

Host `sudo reboot` must never appear in that playbook.

---

## 4. Other environments (when QEMU is not the best tool)

| Option | Use when | Avoid when |
| --- | --- | --- |
| **libvirt + QEMU** | You want snapshots/UI/`virsh console` | You do not want another daemon |
| **Incus / LXD VM** | You already manage VMs that way | Need a CachyOS image that Incus may not ship; still QEMU underneath |
| **systemd-nspawn / docker / podman** | L1–L2, chroot `makechrootpkg`, unit tests | Reboot into a new kernel (cannot) |
| **kexec inside the QEMU guest** | Faster “reboot” for L3 iteration (`kexec -l` + `-e` still changes `uname`) | Measuring firmware/ESP; `--pgo-auto` today calls `sudo reboot`, so kexec is a **divergent** path unless ABS is taught to use it |
| **Nested QEMU in the guest** | Never needed for this | Extra loss of CPU |
| **Cloud VM (Arch)** | Host is a laptop without disk/RAM for L5 | Cost, upload of sources, still need SSH + snapshots |
| **VirtualBox / VMware** | Only if QEMU is blocked | Extra stack; you already have QEMU |
| **Firecracker / microVM** | Not an Arch installer + bootloader lab | No pacman workflow |

**nspawn + makechrootpkg** is still useful: it tests `-h` / ramdisk `c` without a VM. It is **not** a substitute for PGO wait stages.

---

## 5. Fast kernel stand-in (L3), so we reboot without a 45-minute compile

Ideas, from more to less faithful:

1. **Upstream `linux` PKGBUILD with `tinyconfig` / `localmodconfig`** in the guest. Real vmlinuz, real modules dir, real `pkgbase`, real reboot. Weak: not CachyOS options.
2. **A lab-only PKGBUILD** (`linux-abs-lab`) that packages a prebuilt bzImage + a `pkgbase` file + a dummy `/boot/vmlinuz-linux-abs-lab`. Compile is seconds. Weak: does not exercise CachyOS `PKGBUILD` env (`_autofdo`, `_propeller`). Good for PGO **state machine + boot verify** if we add a non-cachyos preset later (`pgo_preset` is already reserved).
3. **Reuse a previously built `linux-cachyos` package** from `ready_made_packages_path` (skip compile, still `pacman -U` + reboot). Good soak for install/boot; misses compile regressions.

For L5, there is no honest shortcut around three CachyOS builds. Cache sources and ccache; snapshot after stage1 to retry stage2 without rebuilding debug.

---

## 6. What to assert (so a “test” is not just “it compiled”)

**One-shot**

- Non-zero `.pkg.tar.zst` in `ready_made_packages_path`
- `pacman -Q` kernel + headers
- `/boot/vmlinuz-<pkgbase>` and initramfs exist
- After guest reboot (L3+): `uname -r` and `/usr/lib/modules/$(uname -r)/pkgbase`

**PGO**

- State file stage transitions match `--event-log` (`stage_start` / `stage_done` / `reboot_required`)
- After wait_reboot1: pkgbase equals stage-1 package, not `-lto`
- After wait_reboot2: pkgbase equals stage-2 `-lto` package
- `profiles_archive_dir` has AFDO/propeller artifacts
- User unit enabled before auto reboot, gone on `done` / `--pgo-abort`
- `abs -U` refuses or warns while a pipeline is active (already unit-tested; worth one guest check)

**Ramdisk (optional extra)**

- `w` compile with `packages_path` still on disk after reboot
- `abs --ramdisk-shutdown` unmounts; snapshot revert as cleanup

**AbsGui**

- Not required for the agent’s kernel lab. If needed later: Spice/VNC + a display, or keep testing GUI on the host while kernel PGO stays CLI-in-guest (`ABS_BINARY` pointing at the guest is the wrong direction — run absgui *in* the guest).

---

## 7. Safety rails (host must not reboot)

Write these into any future automation before the first L4 run:

1. Lab `abs.toml` lives only in the guest (or `ABS_CONFIG` / XDG pointing at a guest path). Never point a guest at the host’s `~/.config/abs`.
2. Agent wrapper: if `systemd-detect-virt` is `none`, abort `--pgo-auto` / `sudo reboot`.
3. Distinct hostname (`abs-kernel-lab`) and a motd.
4. No virtiofs of host `/boot` or `/usr/lib/modules`.
5. QEMU process runs as the user; guest root is not host root.
6. Snapshots before PGO; default cleanup is revert, not “leave a half-installed kernel”.

---

## 8. Suggested implementation order (future, not now)

1. Document the SSH + snapshot commands you already use for QEMU (one `abs-kernel-lab` VM).
2. Seed CachyOS guest: `base-devel`, `git`, `rustup` or a copied `abs` binary, `devtools`, `sysbench`, `stress-ng`, linger, SSH keys, boot timeout 0.
3. L3: tiny kernel oneshot + reboot + pkgbase assert — proves the agent can survive a guest reboot.
4. Helper to set next-boot bootloader entry from PGO state (or confirm CachyOS already defaults to the new kernel).
5. L4 oneshot `linux-cachyos` when compile-path changes.
6. L5 full PGO with `--pgo-auto` and fast benchmark when `src/pgo.rs` / install / boot-verify changes.
7. Optional L2 stub PKGBUILD in-tree under `tests/fixtures/` for CI that has no KVM.

---

## 9. Open choices (decide before any implementation)

1. **libvirt or raw QEMU** for snapshots and console?
2. **virtiofs vs copy** of the ABS tree into the guest?
3. **CachyOS vs Arch** guest image?
4. Is **L3 tiny kernel** enough for day-to-day agent work, with L5 only on request?
5. Should `--pgo-auto` in the lab use real `reboot` (faithful) or guest **kexec** (faster, slightly different)?
6. RAM budget: is 32G for the guest acceptable on this machine, or disk-only compiles only?

---

## 10. Out of scope here

- No scripts, no VM image, no ABS code changes.
- No running `sudo reboot` on the host.
- No nested hypervisor inside the lab guest.
