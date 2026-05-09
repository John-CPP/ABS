# ABS



### Arch Linux / CachyOS package builder (Rust)

Clone sources, build with `makepkg` or `makechrootpkg`, install built `.pkg.tar.zst` artifacts, and optionally drive a system update (`-U`) while compiling selected packages first.

Rust
Arch Linux
License

**Repository:** [https://github.com/John-CPP/ABS.git](https://github.com/John-CPP/ABS.git)



---

## What you need on your system

- **Rust:** a current stable toolchain with **edition 2024** support (install via [rustup](https://rustup.rs/) on Arch: `sudo pacman -S rustup` then `rustup default stable`).
- **Arch / CachyOS tooling:** `base-devel`, `git`, `sudo`, `pacman`, `devtools` (for chroot builds: `makechrootpkg`), and optionally an AUR helper if you set `system_update` to use it.

---

## Clone and compile

```bash
git clone https://github.com/John-CPP/ABS.git
cd ABS
cargo build --release
```

The binary is `./target/release/abs`.

Optional install into your user Cargo bin path:

```bash
cargo install --path . --locked
# then: abs --help
```

To install `abs` system-wide (example):

```bash
sudo install -Dm755 ./target/release/abs /usr/bin/abs
```

Then create config as in the next section (`~/.config/abs/abs.toml` or `/etc/abs/abs.toml`).

---

## Configuration before first run

The program reads **one** TOML file, searched in this order (first match wins):

1. `$XDG_CONFIG_HOME/abs/abs.toml` (usually `~/.config/abs/abs.toml`)
2. `/etc/abs/abs.toml`

**Quick start from the clone directory** (good for testing):

```bash
mkdir -p ~/.config/abs
cp abs.toml.example ~/.config/abs/abs.toml
```

For daily use, prefer installing the config:

```bash
$EDITOR ~/.config/abs/abs.toml
```

See `[abs.toml.example](abs.toml.example)` for all keys: repositories, `manual_update_packages`, per-package `[packages.NAME]` sections, custom build commands, hooks, etc.

### Sudo

`--list` only prints configuration and exits; it does not run `sudo`. Any real run (except `--dry-run`) starts with `sudo -v`, and a background refresh keeps the sudo timestamp alive during long builds.

---

## See it working (minimal smoke test)

After `cargo build --release` and creating `abs.toml` as above:

```bash
# --help does not load abs.toml; --list and builds do.

./target/release/abs --help
```

Load and print your configuration:

```bash
cd /path/to/ABS
./target/release/abs --list
```

Download **sources only** for a small package (uses `git`; needs network):

```bash
./target/release/abs -d vim
```

Full **local build** (longer; compiles dependencies as `makepkg` would):

```bash
./target/release/abs -l vim
```

**Chroot** build (`makechrootpkg`): if `{chroot_base_path}/base/root` is missing, `abs` runs `sudo mkarchroot …/base/root base-devel` on first use (install the `devtools` package first; layout matches makechrootpkg(1)).

```bash
./target/release/abs -h vim
```

**Compile only** (no install prompt):

```bash
./target/release/abs -lo vim
```

---

## CLI flags (summary)


| Flag                      | Description                                                                                                                                                                                                         |
| ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `-d`                      | Download sources only                                                                                                                                                                                               |
| `-l`                      | Local `makepkg` build                                                                                                                                                                                               |
| `-h`                      | Chroot `makechrootpkg` build                                                                                                                                                                                        |
| `-o`                      | Compile only; skip install                                                                                                                                                                                          |
| `-t`                      | Skip tests (`--nocheck`)                                                                                                                                                                                            |
| `-n`                      | Force rebuild                                                                                                                                                                                                       |
| `-c`                      | Re-clone package repo                                                                                                                                                                                               |
| `-e` / `-s` / `-r` / `-k` | Full clean / sudo clean / remove chroot / install keyrings                                                                                                                                                          |
| `-u`                      | `updpkgsums` before build                                                                                                                                                                                           |
| `-v` / `-i`               | Verbose / silent                                                                                                                                                                                                    |
| `-R`                      | Refresh **all** git remotes used by `**manual_update_packages`**, print a colored **PKGBUILD vs installed** report, then run `**[system_update].command_to_update_repositories`** with ignores — **no** compile     |
| `-U`                      | Print pending updates, maybe pre-build manuals, then run `**command_to_update_repositories`** with ignores                                                                                                          |
| `-RU`                     | Same as `**-R**` plus **compile** manuals that qualify, then run `**command_to_perform_system_update`** with ignores                                                                                                |
| `--repo`                  | Override repository for this run                                                                                                                                                                                    |
| `--install-only`          | Install existing packages from `ready_made_packages_path`                                                                                                                                                           |
| `--clean-install`         | Before `**makepkg**`, remove `**src/**` and `**pkg/**` under the package directory (e.g. `…/ventureoo/firefox-pure/`). Enables clean install for this run even if `**clean_install_by_default**` is false in config |
| `--dry-run`               | Print without executing                                                                                                                                                                                             |
| `--list`                  | Dump resolved config                                                                                                                                                                                                |


`**manual_update_packages**` drives repo refreshes and optional pre-builds. Every run of `**command_to_update_repositories**` or `**command_to_perform_system_update**` appends `**ignore_flag**` for each name in `**ignore_packages**` and `**manual_update_packages**` (deduped). Legacy TOML keys `**command**` and `**command_with_refresh**` are still accepted as aliases.

With `**-v**`, the exact shell line for the system update (e.g. `yay -Sy … --ignore …`) is printed before it runs; without `**-v**` that line is omitted.

### `[build]` options (TOML)


| Key                           | Meaning                                                                                                                                                                                                                                                                 |
| ----------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `default_environment`         | `local` or `chroot`                                                                                                                                                                                                                                                     |
| `ignore_compilation_failures` | If true, a failed `**makepkg**` / custom build logs a warning and the run continues with the next package (aliases: `**IGNORE_COMPILATION_FAILURES**`)                                                                                                                  |
| `compile_first_install_after` | If true, build every scheduled package first, then run install prompts / `**pacman -U**` for all of them (good for unattended compile). Not used with `**--install-only**`, `**--download-only**`, or `**--compile-only**` (aliases: `**COMPILE_FIRST_INSTALL_AFTER**`) |
| `clean_install_by_default`    | If true, remove `**src/**` and `**pkg/**` before each compile (same as `**--clean-install**`). `**--clean-install**` turns this on for one run even when the config is false                                                                                            |


Pre-build rules for `**-U**` / `**-RU**`: `**arch**` uses `**checkupdates`/`yay -Qu**` line prefix matching (or `**-n**`). **Non-`arch`** with `**-RU**` compares the PKGBUILD tree version (**.SRCINFO** / `**PKGBUILD`**, with `**makepkg --printsrcinfo**` as fallback) to `**pacman -Q**` after the git refresh; with `**-U**` only (no `**-R**`), non-`arch` manuals still follow the helper list only. Use `**-n**` to force a manual rebuild.

Per-package `**pre_update_command**` runs after clone/pull and before `**makepkg**` (e.g. `**rm -rf mozbuild**` for some Firefox PKGBUILDs). `**--clean-install**` / `**clean_install_by_default**` remove `**src/**` and `**pkg/**` after the `**PKGBUILD**` backup and before `**pre_update_command**`.

---

## Development

```bash
cargo check
cargo clippy -- -D warnings
cargo test
```

---

## License

**Creative Commons Attribution 4.0 International (CC BY 4.0)** — use, modify, and share with attribution and a link back to this project.