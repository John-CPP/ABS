# ABS

ABS is a package builder for Arch Linux and CachyOS (and likely other Arch-based distros). The idea is Gentoo-emerge-like: you choose packages, ABS fetches the PKGBUILD, compiles, and can install.

The git `HEAD` of this repository is the working version. There are no GitHub Releases.

---

## Minimum to start

You need [rustup](https://rustup.rs/) (stable Rust), plus `base-devel`, `git`, `sudo`, and `pacman`. Install `devtools` only if you want chroot builds (`-h`).

```bash
git clone https://github.com/John-CPP/ABS.git
cd ABS/aur && makepkg -si
abs --config-wizard          # first-time setup (also asked automatically if no config exists)
abs --repo=aur some-package  # or: abs mesa
```

That `makepkg -si` installs the CLI, the GUI, and the `abs-full` metapackage.

- Guided config: `abs --config-wizard` (Enter keeps the suggested/current value). Reconfigure copies the previous `abs.toml` to `abs.toml.bak` beside it.
- Edit the file in an editor instead: `abs --configure` or `abs --configure=nano` (`$EDITOR`, then `$VISUAL`, then `vi`; tested: vim, nano, kate).
- Everyday: `abs -U` upgrades the system; `abs -RU` also refreshes watched packages and compiles what is newer ([Everyday use](#everyday-use)).
- Later: `abs --self-update` to pull a newer git `HEAD`. `abs --purge` or `pacman -Rns abs-full` to remove ABS itself ([Update ABS](#update-abs), [Uninstall](#uninstall)).

Config file (first match wins): `$XDG_CONFIG_HOME/abs/abs.toml` (usually `~/.config/abs/abs.toml`), then `/etc/abs/abs.toml`. Full key list: [abs.toml.example](abs.toml.example).

---

## Install options

`cd aur && makepkg -si` builds and installs **all three** split packages:

| Package    | Role                                     |
| ---------- | ---------------------------------------- |
| `abs`      | CLI, PGO benchmark script, documentation |
| `absgui`   | GUI (depends on `abs`)                   |
| `abs-full` | Metapackage that depends on both         |

CLI only (do **not** use `makepkg -si` for this — `-i` would install everything):

```bash
cd aur
makepkg -s
sudo pacman -U abs-[0-9]*.pkg.tar.zst   # CLI only; not absgui-*.pkg.tar.zst or abs-full-*.pkg.tar.zst
```

From a git checkout without pacman, **`make install` already builds the release profile**, then installs binaries, the PGO script, and the desktop icon:

```bash
make install                 # = release build + install to /usr (PREFIX/DESTDIR work as usual)
make install-fast            # quicker optimized build (no LTO); binaries only, no desktop/PGO assets
make aur                     # same as: cd aur && makepkg -si
```

Install to your home prefix: `make install-fast PREFIX=$HOME/.local`.

Cargo aliases: `cargo fast` → `target/fast/`, `cargo rel` → `target/release/`. There is no `make uninstall`.

Equivalent without Make (release layout):

```bash
cargo build --release
sudo install -Dm755 ./target/release/abs /usr/bin/abs
sudo install -Dm755 ./target/release/absgui /usr/bin/absgui
sudo install -Dm755 ./assets/pgo-benchmark.sh /usr/share/abs/pgo-benchmark.sh
sudo install -Dm644 ./absgui/assets/icon.png /usr/share/icons/hicolor/256x256/apps/absgui.png
sudo install -Dm644 ./absgui/absgui.desktop /usr/share/applications/absgui.desktop
```

---

## Update ABS

ABS does not use GitHub Releases. `abs --self-update` compares `Cargo.toml` versions at git **HEAD**. With `self_update_use_pacman = true` (the default), it looks in `ready_made_packages_path` for already-built `abs` / `absgui` / `abs-full` packages of that version and installs them. If they are missing, it clones this repo, builds `aur/` with `makepkg` (`PKGDEST` = `ready_made_packages_path`), then upgrades with pacman.

```bash
abs --check-update    # print whether HEAD is newer
abs --self-update     # install from ready packages, or fetch HEAD, compile, install
```

If `self_update_use_pacman = false` in `abs.toml`, it copies `abs` and `absgui` next to `self_update_install_path` (default `/usr/bin/abs`) instead of using pacman. When that path is under `/usr/bin`, it also installs the desktop file and icon. Shared compile machines should keep pacman self-update enabled.

With `check_for_update_on_startup = true` (the default), ABS checks in the background and, if HEAD is newer, prints a reminder **when that run exits**. `auto_update_on_startup` updates before doing anything else. `self_update_at_updates` updates ABS before `abs -U` / `-RU`.

---

## Several computers, one compile machine

Give every machine the **same** `ready_made_packages_path` (NFS, SMB, or another shared folder). Keep `ignore_already_made_packages = false` (the default) and `self_update_use_pacman = true`. The compile PC runs `abs` as usual; the others skip compilation when matching `.pkg.tar.*` files are already in that folder, then install. `abs --self-update` on the others installs the shared `abs` / `absgui` / `abs-full` packages and does not compile ABS again.

Use the same `abs.toml` package lists and `[packages.*]` overrides on every machine. Install `abs-full` on each PC so the self-update artifact set matches.

`packages_path` (git clones) can be shared too, but it is optional. Prefer **not** sharing it if the compile PC might still be cloning or building: consumers only need the ready folder to skip compiles. Leave `[ramdisk] packages = false` when `packages_path` is on a network share.

Do **not** point pacman’s download cache at `ready_made_packages_path`. A repo tarball with the same name as an ABS build would look like a finished compile and skip a rebuild.

On NFS, attribute caching can hide new files for a while. Mount with `actimeo=0` (or equivalent) so other PCs see packages as soon as the compile machine finishes.

### Pacman, yay, and makepkg on the same share

Create one network share with **separate subfolders**:

| What | Example path | Config |
| --- | --- | --- |
| ABS sources (optional) | `/mnt/pkgshare/abs/packages` | `[paths] packages_path` |
| ABS compiled packages (required) | `/mnt/pkgshare/abs/ready` | `[paths] ready_made_packages_path` |
| Pacman / yay repo downloads | `/mnt/pkgshare/pacman` | `/etc/pacman.conf` `CacheDir` |
| Yay AUR build trees (optional) | `/mnt/pkgshare/yay` | yay `buildDir` |

On every machine, in `/etc/pacman.conf`:

```
CacheDir = /mnt/pkgshare/pacman/
```

Yay uses that cache for repo packages automatically. Optional: put AUR build trees on the share in `~/.config/yay/config.json`:

```json
{
    "buildDir": "/mnt/pkgshare/yay"
}
```

Optional: send **yay / raw makepkg** packages into the ABS ready folder (ABS already sets `PKGDEST` for its own builds). In `/etc/makepkg.conf`:

```
PKGDEST=/mnt/pkgshare/abs/ready
```

The compile user needs write access to `abs/ready` (and `abs/packages` if you share sources). Other PCs only need to read `abs/ready` for `pacman -U`. Pacman writes `CacheDir` as root during `-Syu`, so that folder must be writable by root on each machine (watch NFS `root_squash`). Run `paccache` from one machine only.

Mount the share before running abs, pacman, or yay (`fstab` or a systemd `.mount`). Then on the compile PC:

```bash
abs --self-update
abs -RU
```

On the other PCs, after that finishes:

```bash
abs --self-update
abs mesa                 # skips compile when the ready folder already has this version
abs --install-only mesa  # install from the ready folder only
abs -U                   # system update; watched packages reuse ready artifacts
```

---

## Uninstall

Remove the pacman packages (config under `~/.config/abs` is **kept**):

```bash
sudo pacman -Rns abs-full    # also removes abs and absgui when nothing else needs them
```

Remove ABS itself **and** user config, state, cache, and the build directories from `abs.toml`:

```bash
abs --purge              # lists paths and asks
abs --purge --yes        # no prompt
abs --purge --dry-run
```

`--purge` does **not** uninstall packages you compiled and installed with `pacman -U` (mesa, a kernel, …). After a purge, a new ABS comes from git + `makepkg` / `make install` again, then `abs --self-update` as usual.

---

## Everyday use

```
abs [FLAGS] [PACKAGE...]
```

**`-h` is chroot build (`makechrootpkg`), not help.** Use `--help`.

Build one package (local `makepkg` unless you pass `-h` or the config default is chroot):

```bash
abs mesa
abs --repo=aur xray
abs -l firefox-pure          # force local
abs -h mesa                  # force chroot (needs devtools)
```

In zsh (and bash with globbing on), **quote arguments that contain `[`**:

```bash
abs -h 'firefox-pure[ramdisk=wcp]'
abs --ramdisk=wcp firefox-pure    # no quoting needed
```

**Watched packages and system updates**

- `abs -R` — refresh git clones for `manual_update_packages`, print PKGBUILD vs installed, then run the repo-sync command (no compile).
- `abs -U` — system update (and compile watched/held packages that qualify).
- `abs -RU` — `-R` plus compile what is newer, then the full system update command.

Add packages to watch without editing TOML by hand:

```bash
abs --list-add=manual_update_packages mesa linux-cachyos
abs --wizard=add --pkg-list=manual_update_packages go
abs --wizard                 # add / remove / edit / hold
```

`--config-wizard` edits **global** settings (paths, build, ramdisk, repos). `--wizard` edits **package lists and holds**. They are different commands.

List names (aliases in parentheses): `manual_update_packages` (`manual`, `watched`), `skip_install_packages` (`skip`, `skip_install`), `skip_install_packages_after_compilation` (`skip_after`), `ignore_packages` (`ignore`).

**Held packages** pin a `pkgver-pkgrel`. They are ignored during system update and skipped in `-R`/`-RU` version compares. Rebuild with `abs <pkg>`. Optional triggers recompile the held package on `-U` when those packages change outside ABS:

```bash
abs --hold libfoo --hold-version=1.2.3-1 --trigger=glibc,icu
abs --hold-check
abs --unhold libfoo
```

---

## absgui

Graphical editor for the same `abs.toml`, plus kernel PGO controls:

```bash
absgui
```

Set `ABS_BINARY` if `abs` is not on `PATH`. Window theme/size: `~/.config/abs/absgui-settings.toml`. Save the config before **Start PGO**. Kernel PGO details are in [Kernel PGO](#kernel-pgo-linux-cachyos) below.

---

## Reference

### Flags

| Flag | Description |
| ---- | ----------- |
| `-d` | Download sources only |
| `-l` | Local `makepkg` |
| `-h` | Chroot `makechrootpkg` (not help; use `--help`) |
| `-o` | Compile only; skip install prompt |
| `-t` | Skip tests (`--nocheck`) |
| `-n` | Force rebuild even if matching artifacts exist in PKGDEST |
| `-c` | Re-clone the package repo |
| `-u` | Run `updpkgsums` before build |
| `-e` | Full clean (repos, chroot, ready packages) |
| `-s` | Use sudo when deleting build artifacts |
| `-r` | Remove the configured chroot |
| `-k` | Install Arch / CachyOS keyrings |
| `-v` / `-i` | Verbose / silent |
| `-j` / `--jobs=N` | Default `-j` for this run (does not override per-package `compilation_threads`) |
| `-R` | Refresh `manual_update_packages` clones, version report, repo-sync command (no compile) |
| `-U` | System update; compile watched/held packages that qualify |
| `-RU` | `-R` + compile what qualifies + full system update |
| `--repo=NAME` | Default repository when the package has no `source=` / `[repo=…]` |
| `--ramdisk=wcp\|disabled` | Ramdisk targets for every package on this run (`w` workdir, `c` chroot, `p` packages) |
| `--install-only` | Install existing artifacts from `ready_made_packages_path` |
| `--clean-install` | Remove `src/` and `pkg/` before compile |
| `--dry-run` | Print commands without running them |
| `--list` | Print the resolved config |
| `--config-wizard` | Guided setup / reconfigure of `abs.toml` (copies an existing user file to `abs.toml.bak` first) |
| `--configure` / `--configure=EDITOR` | Open config in `$EDITOR` (tested: vim, nano, kate) |
| `--list-add=LIST` / `--list-remove=LIST` | Mutate a package list |
| `--wizard[=ACTION]` | Package-list / hold wizard (`add`, `remove`, `edit`, `hold`) |
| `--pkg-list=LIST` | Prefill list name for `--wizard` |
| `--hold` / `--hold-version=` / `--trigger=` / `--unhold` / `--hold-check` | Held packages |
| `--check-update` / `--self-update` | Compare / install newer ABS from git HEAD |
| `--kernel-build=PKG` | One-shot kernel build from `[packages.PKG.kernel]` (no PGO) |
| `--pgo` / `--pgo-resume` / `--pgo-status` / `--pgo-abort` / `--pgo-restart` | Kernel PGO pipeline |
| `--pgo-stage` / `--pgo-once` / `--pgo-goto` / `--pgo-keep-stage` / `--pgo-auto` | PGO stage control |
| `--ramdisk-shutdown` | Unmount the configured tmpfs |
| `--json` | Machine-readable output (PGO status / events) |
| `--event-log=PATH` | Append JSON-lines PGO events |
| `--purge` | Remove ABS binaries, config, cache, build dirs |
| `--yes` / `-y` | Skip `--purge` confirmation; on first run with no config, write example defaults |
| `--no-wait` | Skip “Press Enter to exit” (scripts / GUI) |
| `--help` | Help |

### Per-package bracket overrides

Put options in `[` `]` after the package name. Use `,` or `/` between options. Quote in zsh.

| Bracket key | Effect |
| ----------- | ------ |
| `repo=NAME` | Repository for this package only (overrides `--repo`) |
| `pkgver=`, `pkgrel=`, `epoch=`, … | Replace or append that PKGBUILD assignment before build |
| `local`, `chroot`, `build=local\|chroot` | Build environment for this package only |
| `nocheck` | Skip tests for this package only |
| `ramdisk=wcp`, `wcp`, `ramdisk=disabled` | Ramdisk targets; **disabled** = disk only |

When `pkgrel` is set explicitly, automatic pkgrel bump is skipped. Any PKGBUILD override triggers `updpkgsums` (same as `-u`). Global `--ramdisk=wcp` applies to every package unless that package has a bracket override.

### `-R` / `-RU` and AUR

Add AUR packages to `manual_update_packages` with `source = "aur"` under `[packages]`. On `-R` / `-RU`, ABS updates each AUR git clone, compares PKGBUILD versions to installed, and (with `-U`) rebuilds when newer.

Optional GitHub tracking when the PKGBUILD lags upstream:

```toml
manual_update_packages = ["xray"]

[packages.xray]
source = "aur"
upstream_github = "xtls/xray-core"
upstream_prereleases = true
```

On `-R` / `-RU`, after the git sync, ABS queries the GitHub API (`curl`). If upstream is newer, it sets `pkgver`, resets `pkgrel=1`, runs `updpkgsums`, then continues. Needs network and `curl`.

### `[ramdisk]`

Optional tmpfs to speed compiles and spare the SSD. Disk `[paths]` stay the permanent locations. Mounted on the **first task that needs it**, not at ABS startup. Unmounted on normal exit, Ctrl+C / SIGTERM, and fatal errors. `kill -9` cannot unmount — use `sudo umount -l /run/abs-ram` if needed.

| Key | Description |
| --- | ----------- |
| `enabled` | Allow tmpfs when a task requests targets (default `false`) |
| `mount_point` | Absolute mount path (default `/run/abs-ram`; last component must start with `abs`) |
| `size` | tmpfs `size=` cap, not pre-allocated (default `16G`) |
| `mode` | Mount directory mode (default `0755`) |
| `build_workdir` | `src/` / `pkg/` (and compiler caches) on tmpfs (default `false`) |
| `chroot` | Chroot rootfs on tmpfs (default `false`) |
| `packages` | Whole `packages_path` on tmpfs — high RAM (default `false`) |
| `seed_chroot_from` | Optional disk tree to rsync into the ram chroot (unset = fresh `mkarchroot`) |
| `sync_chroot_on_exit` | Rsync ram chroot back to `seed_chroot_from` (default `false`) |
| `min_free_ram_mb` | Refuse to mount if `MemAvailable` is below this (default `4096`) |
| `warn_packages_ram` | Warn when `packages = true` (default `true`) |
| `reclaim_mount_on_startup` | Unmount a stale tmpfs at `mount_point` before mounting (default `true`) |

Per-package: `[packages.NAME] ramdisk = "wcp"` (letters **w** / **c** / **p**) replaces the global flags for that build. CLI: `'mesa[wcp]'` or `abs --ramdisk=wcp mesa`.

```toml
[ramdisk]
enabled = true
mount_point = "/run/abs-ram"
size = "16G"
build_workdir = false
chroot = false
packages = false

[packages.mesa]
build_env = "chroot"
ramdisk = "wcp"
```

### `[build]`

| Key | Description |
| --- | ----------- |
| `default_environment` | `local` or `chroot` |
| `ignore_compilation_failures` | Continue the queue if one package fails |
| `compile_first_install_after` | Compile everything first, then install prompts |
| `clean_install_by_default` | Remove `src/` and `pkg/` before every compile |
| `ignore_already_made_packages` | Always rebuild even if PKGDEST already has this version (default `false`; `-n` per run). Per-package: `[packages.NAME] ignore_already_made_packages` |
| `clean_chroot_after_compilation` | Reset the idle chroot after each chroot build (default `true`) |
| `concurrent_compilations_limit` | Max packages compiling at once (default `1`) |
| `concurrent_repos_downloads_limit` | Max git clones/pulls at once (default `10`) |
| `fast_aur_rpc_update_checks` | Batch AUR version checks via the AUR RPC (default `true`) |
| `system_update_first` | Run the system update command before compiling (default `true`) |
| `global_cpu_threads_mode` | `strict` or `flexible` (default `strict`) |
| `global_cpu_threads_cap` | Max sum of active `-j` (strict hard cap; flexible soft target) |
| `maximum_cpu_threads_cap` | Flexible only: hard ceiling above the soft cap |
| `default_compilation_threads` | Default `-j` when a package has no `compilation_threads` (override with `abs -j`) |

When `compilation_threads` (or `-j`) is set, ABS applies parallel limiters (`MAKEFLAGS`, `NPROC`, `CMAKE_BUILD_PARALLEL_LEVEL`, `NINJAFLAGS`, `CARGO_BUILD_JOBS`, `MAX_JOBS`) via a wrapper `makepkg.conf` (local) or `makepkg.conf.d/abs-parallel.conf` (chroot). PKGBUILDs that hardcode `make -j$(nproc)` ignore those. Packages with no thread setting are not counted against the CPU caps; only `concurrent_compilations_limit` applies.

### Self-update config keys

Root-level (also accepted under `[build]` for old files):

| Key | Description |
| --- | ----------- |
| `check_for_update_on_startup` | Background check at start; remind at **exit** if HEAD is newer (default `true`) |
| `auto_update_on_startup` | Self-update before the rest of the run (default `false`) |
| `self_update_at_updates` | Self-update before `-U` / `-RU` (default `false`) |
| `self_update_raw_url` | Raw `Cargo.toml` URL used to read the remote version (default this repo’s `HEAD`) |
| `self_update_use_pacman` | `true` (default): `makepkg` in `aur/` with `PKGDEST` = `ready_made_packages_path`, then pacman. Reuses packages already in that folder (other machines on a shared ready path skip the compile). `false`: copy binaries next to `self_update_install_path` (plus desktop/icon when installing under `/usr/bin`) |
| `self_update_install_path` | `abs` path when not using pacman (default `/usr/bin/abs`; `absgui` is installed beside it) |

---

## Kernel PGO (linux-cachyos)

Three-stage CachyOS pipeline (debug build → AutoFDO → Propeller) with reboot checkpoints. Configure `[packages.linux-cachyos]` and `[packages.linux-cachyos.pgo]` (see `abs.toml.example`).

```bash
abs --pgo linux-cachyos
sudo reboot
abs --pgo-resume linux-cachyos
sudo reboot
abs --pgo-resume linux-cachyos
abs --pgo-status linux-cachyos --json
```

`profiles_archive_dir` is required. `ramdisk = "w"` keeps sources on disk and compile I/O on tmpfs.

Profiling uses the bundled `assets/pgo-benchmark.sh` (installed to `/usr/share/abs/pgo-benchmark.sh`) unless you set `benchmark_command`. From a `cargo build` without that file, ABS writes the same script to `~/.local/share/abs/pgo-benchmark.sh`. It expects `cachyos-benchmarker`, `sysbench`, and optionally `rg` on `PATH`.

absgui pages: **Kernels** (list + default template), **per-kernel** (PGO controls and build log), **ABS settings** (full `abs.toml`), **App settings** (theme, window). For a taskbar icon on Wayland, install the `.desktop` file and icon (see install commands above).

---

## Development

```bash
cargo check
cargo clippy -- -D warnings
cargo test
cargo build -p absgui
```

The `abs` crate uses Rust edition 2024; `absgui` uses edition 2021. Use a current stable toolchain from rustup.

---

## License

**CC BY 4.0** — use, modify, and share with attribution.

---

## For AI assistants

A human may paste this README and ask how to do something with ABS. Use only facts from this file and `abs.toml.example`. Do not invent GitHub Releases, a `--configure-wizard` flag, or `make uninstall`.

**What ABS is.** A local Arch/CachyOS package builder (emerge-like). It is not a distro. Working code is git `HEAD`.

**Config.** Read `$XDG_CONFIG_HOME/abs/abs.toml` (usually `~/.config/abs/abs.toml`), else `/etc/abs/abs.toml`. Create or reconfigure globals with `abs --config-wizard` (an existing user file is copied to `abs.toml.bak` before saving). Open in an editor with `abs --configure` or `abs --configure=nano` (`$EDITOR` / `$VISUAL` / `vi`; tested: vim, nano, kate). Per-package tables, holds, and compilers: `abs --wizard` or absgui. `--wizard` is not `--config-wizard`.

**Shell traps.** `-h` means chroot build. Help is `--help`. Quote `[` `]` in zsh.

**Map user intent**

| They want | Point them at |
| --------- | ------------- |
| First-time setup | `abs --config-wizard` |
| Watch packages and rebuild when newer | `manual_update_packages`, then `abs -RU` |
| Don’t install pre-built repo binaries (they will compile instead) | `skip_install_packages` |
| Don’t offer extra packages after a compile (e.g. `qemu-docs`) | `skip_install_packages_after_compilation` |
| Pin a version | `abs --hold` / `--wizard=hold` |
| Update ABS itself | `abs --self-update` (HEAD, not Releases) |
| Share compiled packages across PCs | Same `ready_made_packages_path` on every machine; see [Several computers, one compile machine](#several-computers-one-compile-machine) |
| Remove ABS packages but keep config | `sudo pacman -Rns abs-full` |
| Remove ABS and its config/cache/build dirs | `abs --purge` (does not remove packages they built for the system) |
| Compile in RAM | `[ramdisk]` + `enabled = true`; letters `w`/`c`/`p` |
| Kernel PGO | `abs --pgo` / absgui; needs `profiles_archive_dir` |

**Install facts.** `make install` already runs the release build. `cd aur && makepkg -si` installs `abs`, `absgui`, and `abs-full`. CLI-only: `makepkg -s` then `pacman -U` only `abs-[0-9]*.pkg.tar.zst` (not `absgui-…` or `abs-full-…`).

For key names and examples, prefer `abs.toml.example` and the Reference section above.
