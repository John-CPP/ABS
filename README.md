# ABS

<div align="center">

### Arch Build Script

Lightweight helper for building Arch Linux and CachyOS packages.

![Shell](https://img.shields.io/badge/bash-script-121011?style=for-the-badge&logo=gnu-bash)
![Arch Linux](https://img.shields.io/badge/arch-linux-1793D1?style=for-the-badge&logo=arch-linux&logoColor=white)
![License](https://img.shields.io/badge/license-CC--BY--4.0-green?style=for-the-badge)

</div>

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
| `--cachyos` | Use the CachyOS PKGBUILDS repository instead of Arch Linux packaging sources. |
| `--help` | Show help output. |

---

## Example

```bash
bash abs.sh -h -u -n package-name
```

### Handling broken tip commits

Arch package repositories sometimes land a test commit that does not build yet. ABS now handles this by pausing before any build starts, showing the last 5 commits for every requested package, and letting you choose the commit for each package first.

For example:

```bash
bash abs.sh systemd gimp
```

The script will:
1. Clone or update all requested package repositories.
2. Show the last 5 commits for `systemd`.
3. Show the last 5 commits for `gimp`.
4. Ask you to choose one commit for each package.
5. Only after all selections are made, continue with checksum preparation and builds.

---

## License

This project is licensed under **Creative Commons Attribution 4.0 International (CC BY 4.0)**.

That means people can use, modify, and share it, including commercially, as long as they give attribution and keep a link back to this project.
