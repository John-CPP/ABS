# ABS

Arch Linux / CachyOS package builder written in Rust. Clones sources, builds with `makepkg` or `makechrootpkg`, installs built `.pkg.tar.zst` artifacts, and optionally drives a full system update (`-U`) while compiling selected packages first.

**Repository:** <https://github.com/John-CPP/ABS.git>

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

```bash
mkdir -p ~/.config/abs
cp abs.toml.example ~/.config/abs/abs.toml
$EDITOR ~/.config/abs/abs.toml
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
| `--repo`          | Override repository for this run                                        |
| `--install-only`  | Install existing packages from `ready_made_packages_path`               |
| `--clean-install` | Remove `src/` and `pkg/` before compile                                 |
| `--dry-run`       | Print without executing                                                 |
| `--list`          | Dump resolved config                                                    |

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