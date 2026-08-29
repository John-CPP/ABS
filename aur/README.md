# ABS Arch packages

Split packages built from this PKGBUILD:

| Package    | Contents                                              |
| ---------- | ----------------------------------------------------- |
| `abs`      | CLI (`/usr/bin/abs`), PGO benchmark script, docs     |
| `absgui`   | GUI, desktop entry, icon (depends on `abs`)           |
| `abs-full` | Metapackage depending on `abs` + `absgui`             |

## Build and install (from this repository)

```bash
cd aur
./install.sh
```

`install.sh` runs `makepkg -s`, then `pacman -U`. On a first install (no `~/.config/abs/abs.toml` and no `install-prefs.toml`) it asks whether to install AbsGui. Later runs reuse `install_absgui` from `abs.toml` or `install-prefs.toml`. `ABS_INSTALL_GUI=0 ./install.sh` skips the GUI without asking.

`makepkg -si` still installs all three packages without asking.

### Migrating from a manual `cargo install` / `sudo install` setup

If pacman reports **conflicting files** (`/usr/bin/abs exists in filesystem`), remove the old manual files or overwrite when installing:

```bash
cd aur
makepkg -s
sudo pacman -U --overwrite '/usr/bin/abs,/usr/bin/absgui,/usr/share/abs/*,/usr/share/applications/absgui.desktop,/usr/share/icons/hicolor/*/apps/absgui.png' \
  abs-*.pkg.tar.zst absgui-*.pkg.tar.zst abs-full-*.pkg.tar.zst
```

Or remove the conflicting files first, then `makepkg -si`:

```bash
sudo rm -f /usr/bin/abs /usr/bin/absgui \
  /usr/share/applications/absgui.desktop \
  /usr/share/icons/hicolor/*/apps/absgui.png \
  /usr/share/abs/pgo-benchmark.sh \
  /usr/share/abs/build-generate-propeller-profiles.sh
cd aur && makepkg -si
```

Install only the CLI:

```bash
makepkg -si -- --holdver  # build all split packages
sudo pacman -U abs-*.pkg.tar.zst        # CLI only
```

Install everything (recommended):

```bash
makepkg -si
sudo pacman -U abs-*.pkg.tar.zst absgui-*.pkg.tar.zst abs-full-*.pkg.tar.zst
# or after both abs and absgui are installed:
sudo pacman -U abs-full-*.pkg.tar.zst
```

## Uninstall

Remove the metapackage and its dependencies (pacman-tracked files only):

```bash
sudo pacman -Rns abs-full
```

User config and build caches under `~/.config/abs` are not removed by pacman. To wipe those too:

```bash
abs --purge --yes
```

## AUR submission

1. Copy this directory to a new `abs` AUR git repo.
2. Ensure `source` points at the public git URL (already set when not building in-tree).
3. Run `makepkg --printsrcinfo > .SRCINFO` and commit both `PKGBUILD` and `.SRCINFO`.

`--self-update` / `--check-update` in abs expect pacman packages named `abs`, `absgui`, and/or `abs-full`.
