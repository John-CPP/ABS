# ABS

Arch Linux / CachyOS package builder. Maybe works with other arch-based distros.
Main idea of ABS is add gentoo-emerge like functionality to arch-like systems.
Current code in repo is already a stable version so no releases necessary.

---

## Requirements

- Rust stable (edition 2024) — install via [rustup](https://rustup.rs/)
- `base-devel`, `git`, `sudo`, `pacman`
- `devtools` — required for chroot builds (`makechrootpkg`)

---

## Install

```bash
git clone https://github.com/John-CPP/ABS.git
cd ABS/aur
makepkg -si
```

This installs three pacman packages:

| Package    | Role                                      |
| ---------- | ----------------------------------------- |
| `abs`      | CLI, PGO benchmark script, documentation  |
| `absgui`   | GUI (depends on `abs`)                    |
| `abs-full` | Metapackage pulling in both               |

Install only the CLI: build with `makepkg -si` and install `abs-*.pkg.tar.zst` only.  
Install everything: install `abs`, `absgui`, and `abs-full` artifacts (or `pacman -U` all three after `makepkg -s`).

Remove pacman-tracked files:

```bash
sudo pacman -Rns abs-full   # also removes abs and absgui when nothing else needs them
```

User config under `~/.config/abs` is not removed by pacman; use `abs --purge --yes` for that.

Manual install (development):

```bash
cargo build --release
sudo install -Dm755 ./target/release/abs /usr/bin/abs
sudo install -Dm755 ./target/release/absgui /usr/bin/absgui
sudo install -Dm755 ./assets/pgo-benchmark.sh /usr/share/abs/pgo-benchmark.sh
sudo install -Dm644 ./absgui/assets/icon.png /usr/share/icons/hicolor/256x256/apps/absgui.png
sudo install -Dm644 ./absgui/absgui.desktop /usr/share/applications/absgui.desktop
```

---

## Configuration

ABS reads one TOML file (first match wins):

1. `$XDG_CONFIG_HOME/abs/abs.toml`
2. `/etc/abs/abs.toml`

On first run, if neither file exists, ABS creates `~/.config/abs/abs.toml` from
`[abs.toml.example](abs.toml.example)` and prints:

```
ABS config has been created from the example. Please configure using --configure
```

Edit the config with:

```bash
abs --configure              # uses $EDITOR (then $VISUAL, then vi)
abs --configure=kate         # uses a specific editor
```

See `[abs.toml.example](abs.toml.example)` for all available keys.

---

## Usage

```
abs [FLAGS] [PACKAGE...]
```

### Flags


| Flag                 | Description                                                              |
| -------------------- | ------------------------------------------------------------------------ |
| `-d`                 | Download sources only                                                    |
| `-l`                 | Local `makepkg` build                                                    |
| `-h`                 | Chroot `makechrootpkg` build                                             |
| `-o`                 | Compile only; skip install                                               |
| `-t`                 | Skip tests (`--nocheck`)                                                 |
| `-n`                 | Force rebuild                                                            |
| `-c`                 | Re-clone package repo                                                    |
| `-u`                 | Run `updpkgsums` before build                                            |
| `-e`                 | Full clean                                                               |
| `-s`                 | Sudo clean                                                               |
| `-r`                 | Remove chroot                                                            |
| `-k`                 | Install keyrings                                                         |
| `-v` / `-i`          | Verbose / silent                                                         |
| `-R`                 | Refresh all git remotes, print PKGBUILD vs installed report, no compile  |
| `-U`                 | Print pending updates, pre-build manuals, run system update              |
| `-RU`                | `-R` + compile qualifying manuals, then run system update                |
| `--repo`             | Default repository for packages without `[repo=...]` (e.g. `--repo=aur`) |
| `--install-only`     | Install existing packages from `ready_made_packages_path`                |
| `--clean-install`    | Remove `src/` and `pkg/` before compile                                  |
| `--dry-run`          | Print without executing                                                  |
| `--list`             | Dump resolved config                                                     |
| `--configure`        | Open user config in `$EDITOR`                                            |
| `--configure=EDITOR` | Open user config in the given editor (e.g. `--configure=kate`)           |
| `--check-update`     | Query latest remote version of ABS and check against local version       |
| `--self-update`      | Fetch, compile, and install the latest remote version of ABS             |
| `--pgo PKG`          | Start CachyOS kernel 3-stage PGO pipeline (debug → AutoFDO → Propeller)  |
| `--pgo-resume PKG`   | Resume PGO pipeline after reboot                                         |
| `--pgo-status PKG`   | Show current PGO stage (`--json` for machine-readable output)            |
| `--pgo-abort PKG`    | Abort PGO pipeline (releases system-update holds; use `--pgo-keep-stage` to preserve stage) |
| `--json`             | JSON output (with `--pgo-status` or PGO event stream)                    |
| `--purge`            | Remove ABS from the system (binaries, config, cache, build data)         |
| `--yes` / `-y`       | Skip confirmation when used with `--purge`                               |


### Uninstall

Remove installed binaries, user config, state, default cache, and configured build directories:

```bash
abs --purge          # lists paths and asks for confirmation
abs --purge --yes    # remove without prompting
abs --purge --dry-run
```

This removes `/usr/bin/abs`, `/usr/bin/absgui`, `/usr/share/abs/`, `~/.config/abs/`, `~/.local/state/abs/`, package/chroot/ready paths from `abs.toml`, PGO profile archives, and related data. It does **not** remove packages you installed to the system with `pacman -U`.

### Per-package overrides

Put options in square brackets after the package name. **In zsh (and bash with `glob` on), you must quote arguments that contain `[`** — otherwise the shell treats brackets as globs and the command fails before ABS runs:

```bash
abs -h 'firefox-pure[ramdisk=wcp]'
abs -h 'firefox-pure[wcp]'
abs -h --ramdisk=wcp firefox-pure    # no quoting needed
abs --repo=aur xray[pkgver=26.5.9,pkgrel=2] 'mesa[repo=cachyos]'
```

Note: `-h` is chroot build (makechrootpkg), not help. Use `--help` for usage.


| Bracket key                             | Effect                                                  |
| --------------------------------------- | ------------------------------------------------------- |
| `repo=NAME`                             | Repository for this package only (overrides `--repo`)   |
| `pkgver=`, `pkgrel=`, `epoch=`, …       | Replace or append that PKGBUILD assignment before build |
| `local`, `chroot`, `build=local|chroot` | Build environment for this package only                 |
| `nocheck`                               | Skip tests for this package only                        |
| `ramdisk=wcp`, `wcp`, `ramdisk=disabled` | Ramdisk targets: **w**=workdir, **c**=chroot, **p**=packages; **disabled**=disk only (overrides global `[ramdisk]` and `--ramdisk`) |


Use `,` or `/` between bracket options. When `pkgrel` is set explicitly, automatic pkgrel bump is skipped for that build. Any PKGBUILD override triggers `updpkgsums` before compile (same as `-u`).

Global **`--ramdisk=wcp`** applies to every package on the command line unless a bracket override is present for that package.

### `-RU` and AUR packages

Add AUR packages to `manual_update_packages` with `source = "aur"` in `[packages]`. On `**-R**` / `**-RU**`, ABS pulls each AUR git clone (same as official `arch` packages), compares PKGBUILD versions to installed, and rebuilds when newer.

Optional **upstream GitHub** tracking for packages that lag behind upstream (e.g. AUR only ships stable):

```toml
manual_update_packages = ["xray"]

[packages.xray]
source = "aur"
upstream_github = "xtls/xray-core"   # or https://github.com/xtls/xray-core
upstream_prereleases = true          # include GitHub prereleases (default: false)
```

On `**-R**` / `**-RU**`, after syncing the AUR clone, ABS queries the GitHub API (via `curl`). If upstream is newer than `pkgver` in the PKGBUILD, it sets `pkgver`, resets `pkgrel=1`, runs `updpkgsums`, then continues the normal version report / build flow. Requires network access and `curl`.

### `[ramdisk]` config keys

Optional tmpfs/ramdisk support to speed up compiles and reduce disk wear. Disk `[paths]` remain the persistent locations; the ramdisk holds ephemeral build state for the ABS session.


| Key                        | Description                                                                                                              |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| `enabled`                  | Allow tmpfs ramdisk when a package task requests targets (default: `false`; mounted lazily on first use, not at startup) |
| `mount_point`              | Absolute path for the tmpfs mount (default: `/run/abs-ram`)                                                              |
| `size`                     | tmpfs size passed to `mount -o size=` (default: `16G`)                                                                   |
| `mode`                     | Directory mode for the mount (default: `0755`)                                                                           |
| `build_workdir`            | Symlink each package's `src/` and `pkg/` to tmpfs during builds (default: `false`)                                       |
| `chroot`                   | Use tmpfs for `chroot_base_path` during chroot builds (default: `false`)                                                 |
| `packages`                 | Move entire `packages_path` to tmpfs — high RAM use (default: `false`)                                                   |
| `seed_chroot_from`         | Optional disk path to `rsync` into the ram chroot before first use (full copy). **Unset = fresh `mkarchroot` on RAM**    |
| `sync_chroot_on_exit`      | `rsync` ram chroot back to `seed_chroot_from` on exit (requires `seed_chroot_from`; default: `false`)                    |
| `min_free_ram_mb`          | Refuse mount when `MemAvailable` is below this (default: `4096`)                                                         |
| `warn_packages_ram`        | Print a warning when `packages = true` (default: `true`)                                                                 |
| `reclaim_mount_on_startup` | Unmount a stale tmpfs at `mount_point` before mounting (default: `true`)                                                 |


**Per-package overrides:** When `[ramdisk].enabled = true`, you can leave global `build_workdir`, `chroot`, and `packages` as `false` and opt in per package. In `[packages.NAME]` set `ramdisk = "wcp"` (letters: **w** = workdir, **c** = chroot, **p** = packages). The string **replaces** the global defaults for that build. CLI: `'mesa[ramdisk=wcp]'` / `'mesa[wcp]'` (quote in zsh), or `abs --ramdisk=wcp mesa`.

When `[ramdisk].enabled = true`, tmpfs is mounted on the **first package task** that needs ramdisk targets (global flags or per-package `ramdisk = "wcp"` / CLI `mesa[wcp]`). Runs that only refresh repos (`-R`), update the system, or build packages without ramdisk targets never mount tmpfs.

When a ramdisk session is active, ABS unmounts the tmpfs on normal exit, on `**Ctrl+C` / SIGTERM** (stops running subprocesses first), and on error exits (`die!`). Uses lazy unmount (`umount -l`) if the mount is busy. `kill -9` cannot be handled — use `sudo umount -l /run/abs-ram` manually if needed.

Example (ramdisk on, global targets off, heavy packages opt in):

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

Example (hybrid: git clones on disk, compile workdirs and chroot in RAM):

```toml
[paths]
packages_path = "/media/storage/abs/packages"
chroot_base_path = "/media/storage/abs/chroot"
ready_made_packages_path = "/media/storage/abs/ready"

[ramdisk]
enabled = true
mount_point = "/run/abs-ram"
size = "16G"
build_workdir = true
chroot = true
packages = false
# seed_chroot_from = "/media/storage/abs/chroot-minimal"  # optional; omit for fresh mkarchroot
```

### `[build]` config keys


| Key                                | Description                                                                      |
| ---------------------------------- | -------------------------------------------------------------------------------- |
| `default_environment`              | `local` or `chroot`                                                              |
| `ignore_compilation_failures`      | Log warning and continue on build failure instead of aborting                    |
| `compile_first_install_after`      | Build all packages first, then install — useful for unattended runs              |
| `clean_install_by_default`         | Remove `src/` and `pkg/` before every compile                                    |
| `clean_chroot_after_compilation`   | Reset devtools chroot after each chroot build (default: `true`)                  |
| `concurrent_compilations_limit`    | Max packages building at once (default: `1`)                                     |
| `global_cpu_threads_mode`          | `strict` or `flexible` — how concurrent `-j` sums are capped (default: `strict`) |
| `global_cpu_threads_cap`           | Max sum of active compilation threads (strict hard cap; flexible soft target)  |
| `maximum_cpu_threads_cap`          | Flexible mode only: hard ceiling above the soft cap                              |
| `default_compilation_threads`      | Default `-j` for packages without `compilation_threads` (override with `abs -j`) |
| `concurrent_repos_downloads_limit` | Maximum number of repository clones/updates to sync concurrently (default: `10`) |

When `compilation_threads` (or `-j`) is set, ABS applies parallel limiters (`MAKEFLAGS`, `NPROC`, `CMAKE_BUILD_PARALLEL_LEVEL`, `NINJAFLAGS`, `CARGO_BUILD_JOBS`, `MAX_JOBS`). For local/PGO builds it generates a temporary wrapper `makepkg.conf` (sourcing your normal config chain first) and passes it via `MAKEPKG_CONF`; for chroot builds it writes `makepkg.conf.d/abs-parallel.conf` into the worker copy. Both override distro defaults such as `MAKEFLAGS="-j$(nproc)"`.

PKGBUILDs that hardcode `make -j$(nproc)` ignore these env vars. Packages without any thread setting are not counted against `global_cpu_threads_cap` / `maximum_cpu_threads_cap`; only the `concurrent_compilations_limit` slot count constrains them.


### Self-Updates config keys

These are root-level properties (but are also parsed under the `[build]` section for backwards compatibility):


| Key                           | Description                                                                                                                             |
| ----------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| `check_for_update_on_startup` | Check for newer ABS versions silently in the background at startup, notifying at exit if newer (default: `true`)                        |
| `auto_update_on_startup`      | Check for newer ABS versions and automatically self-update synchronously at startup (default: `false`)                                  |
| `self_update_at_updates`      | Check for newer ABS versions synchronously when `-U` is used and update before system packages (default: `false`)                       |
| `self_update_raw_url`         | The raw Cargo.toml URL used to parse the latest version (default: `"https://raw.githubusercontent.com/John-CPP/ABS/master/Cargo.toml"`) |
| `self_update_use_pacman`      | When `true`, `--self-update` runs `makepkg` in `aur/` and upgrades pacman packages. When `false`, copies the `abs` binary to `self_update_install_path`. Unset = auto-detect from installed packages. |
| `self_update_install_path`    | Fallback binary path when not using pacman packages (default: `"/usr/bin/abs"`)                                                         |


---

## Kernel PGO (linux-cachyos)

ABS supports the 3-stage CachyOS kernel pipeline (debug build → AutoFDO profile → Propeller profile) with reboot checkpoints. Configure `[packages.linux-cachyos]` and `[packages.linux-cachyos.pgo]` in `abs.toml` (see `abs.toml.example`).

```bash
abs --pgo linux-cachyos
sudo reboot
abs --pgo-resume linux-cachyos
sudo reboot
abs --pgo-resume linux-cachyos
abs --pgo-status linux-cachyos --json
```

Profiles are archived to `profiles_archive_dir` (required; HDD path is fine). Use `ramdisk = "w"` for compile I/O on tmpfs while keeping sources on disk.

PGO profiling runs the bundled benchmark script (`assets/pgo-benchmark.sh`, installed to `/usr/share/abs/pgo-benchmark.sh`) unless you set `benchmark_command` in `[packages.PKG.pgo]`. When running from `cargo build` without installing that file, ABS materializes the same script at `~/.local/share/abs/pgo-benchmark.sh`. It expects `cachyos-benchmarker`, `sysbench`, and optionally `rg` on `PATH`.

### absgui

`absgui` is an iced-based GUI for editing `abs.toml` and driving the kernel PGO pipeline (installed with `abs` above).

```bash
absgui
```

It reads and writes the same config as the CLI (`~/.config/abs/abs.toml` by default). Set `ABS_BINARY` if `abs` is not on `PATH` (e.g. `ABS_BINARY=/usr/bin/abs absgui`).

- **Kernels** — CachyOS Kernel Manager–style list; **Edit defaults** opens the default kernel config template.
- **Per-kernel page** — kernel/PGO options, pipeline controls, and build log (log only appears here).
- **ABS settings** — full `abs.toml` editor (paths, build, self-update, package lists, system update, ramdisk, repositories, compilers). Folder/file browse buttons open native dialogs.
- **App settings** — theme (dark/light) and window size/position (restored on next launch).

Window position/size is stored in `~/.config/abs/absgui-settings.toml`. For the taskbar icon on Wayland, install the `.desktop` file and icon (see install commands above).

Save the config before **Start PGO**; the GUI writes `abs.toml` then runs `abs --pgo` or `abs --pgo-resume`.

---

## Development

```bash
cargo check
cargo clippy -- -D warnings
cargo test
cargo build -p absgui
```

---

## License

**CC BY 4.0** — use, modify, and share with attribution.
