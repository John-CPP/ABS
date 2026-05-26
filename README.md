# ABS

Arch Linux / CachyOS package builder. Maybe works with other arch-based distros.
Main idea of ABS is add gentoo-emerge like functionality to arch-like systems.

---

## Requirements

- Rust stable (edition 2024) — install via [rustup](https://rustup.rs/)
- `base-devel`, `git`, `sudo`, `pacman`
- `devtools` — required for chroot builds (`makechrootpkg`)

---

## Install

```bash
git clone https://github.com/John-CPP/ABS.git
cd ABS
cargo build --release
sudo install -Dm755 ./target/release/abs /usr/bin/abs
```

---

## Configuration

ABS reads one TOML file (first match wins):

1. `$XDG_CONFIG_HOME/abs/abs.toml`
2. `/etc/abs/abs.toml`

On first run, if neither file exists, ABS creates `~/.config/abs/abs.toml` from
[`abs.toml.example`](abs.toml.example) and prints:

```
ABS config has been created from the example. Please configure using --configure
```

Edit the config with:

```bash
abs --configure              # uses $EDITOR (then $VISUAL, then vi)
abs --configure=kate         # uses a specific editor
```

See [`abs.toml.example`](abs.toml.example) for all available keys.

---

## Usage

```
abs [FLAGS] [PACKAGE...]
```

### Flags

| Flag              | Description                                                             |
| ----------------- | ----------------------------------------------------------------------- |
| `-d`              | Download sources only                                                   |
| `-l`              | Local `makepkg` build                                                   |
| `-h`              | Chroot `makechrootpkg` build                                            |
| `-o`              | Compile only; skip install                                              |
| `-t`              | Skip tests (`--nocheck`)                                                |
| `-n`              | Force rebuild                                                           |
| `-c`              | Re-clone package repo                                                   |
| `-u`              | Run `updpkgsums` before build                                           |
| `-e`              | Full clean                                                              |
| `-s`              | Sudo clean                                                              |
| `-r`              | Remove chroot                                                           |
| `-k`              | Install keyrings                                                        |
| `-v` / `-i`       | Verbose / silent                                                        |
| `-R`              | Refresh all git remotes, print PKGBUILD vs installed report, no compile |
| `-U`              | Print pending updates, pre-build manuals, run system update             |
| `-RU`             | `-R` + compile qualifying manuals, then run system update               |
| `--repo`          | Default repository for packages without `[repo=...]` (e.g. `--repo=aur`) |
| `--install-only`  | Install existing packages from `ready_made_packages_path`               |
| `--clean-install` | Remove `src/` and `pkg/` before compile                                 |
| `--dry-run`       | Print without executing                                                 |
| `--list`          | Dump resolved config                                                    |
| `--configure`     | Open user config in `$EDITOR`                                           |
| `--configure=EDITOR` | Open user config in the given editor (e.g. `--configure=kate`)       |

### Per-package overrides

Put options in square brackets after the package name (quote the argument in the shell when it contains `[`):

```bash
abs -h xray[repo=aur,pkgver=26.5.9,pkgrel=2] 'mesa[repo=cachyos]'
abs --repo=aur xray[pkgver=26.5.9,pkgrel=2] mesa[repo=cachyos]
```

| Bracket key | Effect |
| ----------- | ------ |
| `repo=NAME` | Repository for this package only (overrides `--repo`) |
| `pkgver=`, `pkgrel=`, `epoch=`, … | Replace or append that PKGBUILD assignment before build |
| `local`, `chroot`, `build=local\|chroot` | Build environment for this package only |
| `nocheck` | Skip tests for this package only |

Use `,` or `/` between bracket options. When `pkgrel` is set explicitly, automatic pkgrel bump is skipped for that build. Any PKGBUILD override triggers `updpkgsums` before compile (same as `-u`).

### `[build]` config keys

| Key                           | Description                                                         |
| ----------------------------- | ------------------------------------------------------------------- |
| `default_environment`         | `local` or `chroot`                                                 |
| `ignore_compilation_failures` | Log warning and continue on build failure instead of aborting       |
| `compile_first_install_after` | Build all packages first, then install — useful for unattended runs |
| `clean_install_by_default`    | Remove `src/` and `pkg/` before every compile                       |

---

## Development

```bash
cargo check
cargo clippy -- -D warnings
cargo test
```

---

## License

**CC BY 4.0** — use, modify, and share with attribution.