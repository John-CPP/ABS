# Arch-Emerge (Alpha version. Currently very buggy. New version in progress, soon.)

<div align="center">

### Arch Linux / CachyOS package builder (Rust)

Clone sources, build with `makepkg` or `makechrootpkg`, install built `.pkg.tar.zst` artifacts, and optionally drive a system update (`-U`) while compiling selected packages first.

![Rust](https://img.shields.io/badge/rust-000000?style=for-the-badge&logo=rust&logoColor=white)
![Arch Linux](https://img.shields.io/badge/arch-linux-1793D1?style=for-the-badge&logo=arch-linux&logoColor=white)
![License](https://img.shields.io/badge/license-CC--BY--4.0-green?style=for-the-badge)

**Repository:** [https://github.com/John-CPP/Arch-Emerge.git](https://github.com/John-CPP/Arch-Emerge.git)

</div>

---

## What you need on your system

- **Rust:** a current stable toolchain with **edition 2024** support (install via [rustup](https://rustup.rs/) on Arch: `sudo pacman -S rustup` then `rustup default stable`).
- **Arch / CachyOS tooling:** `base-devel`, `git`, `sudo`, `pacman`, `devtools` (for chroot builds: `makechrootpkg`), and optionally an AUR helper if you set `system_update` to use it.

---

## Clone and compile

```bash
git clone https://github.com/John-CPP/Arch-Emerge.git
cd Arch-Emerge
cargo build --release
```

The binary is:

```text
./target/release/arch_emerge
```

Optional install into your user Cargo bin path:

```bash
cargo install --path . --locked
# then: arch_emerge --help
```

---

## Configuration before first run

The program reads **one** TOML file, searched in this order (first match wins):

1. `$XDG_CONFIG_HOME/arch-emerge/emerge.toml` (usually `~/.config/arch-emerge/emerge.toml`)
2. `/etc/arch-emerge/emerge.toml`
3. `./emerge.toml` in the **current working directory**

**Quick start from the clone directory** (good for testing):

```bash
mkdir -p /tmp/arch-emerge-demo/{sources,chroot,emerge_ready}
cp emerge.toml.example emerge.toml
# Paths in emerge.toml.example match /tmp/arch-emerge-demo/...
```

For daily use, prefer installing the config:

```bash
mkdir -p ~/.config/arch-emerge
cp emerge.toml.example ~/.config/arch-emerge/emerge.toml
$EDITOR ~/.config/arch-emerge/emerge.toml   # set real paths under your $HOME or /media/...
```

See [`emerge.toml.example`](emerge.toml.example) for all keys: repositories, `manual_update_packages`, per-package `[packages.NAME]` sections, custom build commands, hooks, etc.

---

## See it working (minimal smoke test)

After `cargo build --release` and creating `emerge.toml` as above:

```bash
# --help does not load emerge.toml; --list and builds do.

./target/release/arch_emerge --help
```

Load and print your configuration:

```bash
cd /path/to/Arch-Emerge   # if you rely on ./emerge.toml
./target/release/arch_emerge --list
```

Download **sources only** for a small package (uses `git`; needs network):

```bash
./target/release/arch_emerge -d vim
```

Full **local build** (longer; compiles dependencies as `makepkg` would):

```bash
./target/release/arch_emerge -l vim
```

**Chroot** build (requires a devtools chroot under `chroot_base_path` from your config):

```bash
./target/release/arch_emerge -h vim
```

**Compile only** (no install prompt):

```bash
./target/release/arch_emerge -lo vim
```

---

## CLI flags (summary)

| Flag | Description |
| --- | --- |
| `-d` | Download sources only |
| `-l` | Local `makepkg` build |
| `-h` | Chroot `makechrootpkg` build |
| `-o` | Compile only; skip install |
| `-t` | Skip tests (`--nocheck`) |
| `-n` | Force rebuild |
| `-c` | Re-clone package repo |
| `-e` / `-s` / `-r` / `-k` | Full clean / sudo clean / remove chroot / install keyrings |
| `-u` | `updpkgsums` before build |
| `-v` / `-i` | Verbose / silent |
| `-R` | Force refresh custom git repos |
| `-U` | System update flow |
| `--repo` | Override repository for this run |
| `--install-only` | Install existing packages from `ready_made_packages_path` |
| `--dry-run` | Print without executing |
| `--list` | Dump resolved config |

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
