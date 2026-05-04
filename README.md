# ABS

<div align="center">

### Arch Build Script

Lightweight helper for building Arch Linux packages from multiple repositories, managing chroots, and handling system updates.

![Shell](https://img.shields.io/badge/bash-script-121011?style=for-the-badge&logo=gnu-bash)
![Arch Linux](https://img.shields.io/badge/arch-linux-1793D1?style=for-the-badge&logo=arch-linux&logoColor=white)
![License](https://img.shields.io/badge/license-CC--BY--4.0-green?style=for-the-badge)

</div>

---

## Features

- **Multi-Repo Support:** Define custom Git repositories in `abs.config` (e.g., Arch, CachyOS, custom Github repos).
- **System Update Integration (`-U`):** Run full system updates (`yay -Syu` by default) but intercept specific packages to be manually compiled *before* the rest of the system updates.
- **Smart Package Installation:** After a successful build, it displays a numbered menu for selecting which sub-packages to install (supports ranges like `1-3, 5`).
- **Skip Rules:** Specify packages (like `systemd-tests`) in `abs.config` to completely hide them from installation prompts.
- **Aliases:** Support mapping sub-packages to their base repository PKGBUILD name using `PACKAGE_ALIASES`.
- **Automatic GPG Handling:** Fetches missing PGP keys defined in the PKGBUILD automatically.
- **Hooks:** Run custom pre-build and post-install commands for specific packages.

---

## Configuration (`abs.config`)

The `abs.config` file allows you to customize paths, repositories, and update behavior.

```bash
# Example repositories
declare -A REPOSITORIES
REPOSITORIES["arch"]="https://gitlab.archlinux.org/archlinux/packaging/packages"
REPOSITORIES["cachyos"]="https://github.com/CachyOS/CachyOS-PKGBUILDS.git"

# System Update Command
SYSTEM_UPDATE_COMMAND="yay -Syu"

# Map packages to compile manually during -U
declare -A MANUAL_UPDATE_PACKAGES
MANUAL_UPDATE_PACKAGES["mkinitcpio"]="cachyos"

# Aliases: Map sub-packages to base package
declare -A PACKAGE_ALIASES
PACKAGE_ALIASES["qemu-full"]="qemu"

# Skip installation of specific sub-packages
SKIP_INSTALL_PACKAGES=("systemd-tests")
```

---

## Flags

| Flag | Description |
| --- | --- |
| `-d` | Download package sources only. Do not build. |
| `-l` | Build locally with `makepkg` (default mode). |
| `-h` | Build inside a chroot with `makechrootpkg`. |
| `-o` | Compile only. Skip the package installation prompt. |
| `-n` | Force a new build even if package artifacts already exist. |
| `-c` | Delete the existing package repository and clone it again. |
| `-e` | Run full cleaning, including chroot removal and cache cleanup. |
| `-s` | Use `sudo` when deleting repositories or build artifacts. |
| `-r` | Remove the configured chroot. |
| `-k` | Install and populate Arch Linux / CachyOS signing keys. |
| `-u` | Update PKGBUILD checksums before building. |
| `-v` | Enable verbose output. |
| `-i` | Silent mode. Hide normal status output. |
| `--repo=NAME` | Specify which repository to pull the package from (default: `arch`). |
| `-U` | Perform full system update with manual compilation of configured packages. |
| `--help` | Show help output. |

---

## Example

```bash
# Build a package in a chroot, update sums, force a new build, using the CachyOS repo
bash abs.sh -h -u -n --repo=cachyos package-name

# Run a system update, manually compiling any packages configured in abs.config first
bash abs.sh -U
```

---

## License

This project is licensed under **Creative Commons Attribution 4.0 International (CC BY 4.0)**.

That means people can use, modify, and share it, including commercially, as long as they give attribution and keep a link back to this project.
