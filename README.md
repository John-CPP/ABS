# ABS Script

> A Bash helper for fetching, preparing, building, and optionally installing Arch Linux or CachyOS packages from PKGBUILD sources.

`README.md` is the correct filename for GitHub's front page documentation, and the repository already uses that convention. This version expands it into full project documentation so the GitHub landing page is useful for setup, daily usage, and troubleshooting.

## Overview

`abs.sh` automates a typical Arch package build workflow:

- Loads build paths and repository settings from `abs.config`.
- Fetches package sources from the official Arch Linux packaging repositories.
- Optionally switches to the CachyOS PKGBUILDS repository.
- Updates checksums when requested.
- Bumps `pkgrel` automatically after repository preparation.
- Builds packages either locally with `makepkg` or inside a clean chroot with `makechrootpkg`.
- Detects missing PGP keys during builds and attempts to import them automatically.
- Stores built packages in a configurable output directory.
- Optionally prompts to install the resulting package artifacts with `pacman -U`.

## Features

- **Local or chroot builds** using a simple CLI flag.
- **Arch Linux and CachyOS support** with one switch.
- **Download-only mode** for fetching package sources without compiling.
- **New build mode** to force a rebuild even if artifacts already exist.
- **Repository cleanup options** for stale checkout recovery.
- **Full cache cleanup mode** for difficult dependency or build-cache issues.
- **Verbose and silent logging modes** for different workflows.
- **Automatic keyring/bootstrap helpers** for common packaging signature problems.
- **Config-driven paths** so you can keep package sources, chroots, and output artifacts wherever you want.

## Repository Layout

```text
.
├── abs.sh        # Main automation script
├── abs.config    # User-editable configuration
└── README.md     # Project documentation
```

## Requirements

This script is intended for **Arch Linux-based systems** and expects the standard Arch packaging toolchain to be available.

### Core tools

Install or verify availability of:

- `bash`
- `git`
- `sudo`
- `gpg`
- `pacman`
- `makepkg`
- `updpkgsums`
- `mkarchroot`
- `arch-nspawn`
- `makechrootpkg`

These usually come from packages such as:

- `base-devel`
- `devtools`
- `git`
- `gnupg`

### Optional ecosystem tools used by cleanup mode

The **full cleaning** workflow also calls:

- `go`
- `npm`

If those commands are not installed, that cleaning path may fail unless you adapt the script for your environment.

## Configuration

The script reads settings from `abs.config` located in the repository root.

Current configuration keys:

```bash
PACKAGES_PATH="/media/storage/packages/my"
CHROOT_BASE_PATH="/media/storage/packages/my/chroot"
READY_MADE_PACKAGES_PATH="/media/storage/packages/abs_ready"
MASTER_CHROOT="${CHROOT_BASE_PATH}/base"
CACHYOS_REPO_URL="https://github.com/CachyOS/CachyOS-PKGBUILDS.git"
CACHYOS_PACKAGES_PATH="${PACKAGES_PATH}/CachyOS-PKGBUILDS"
```

### What each setting does

| Variable | Description |
| --- | --- |
| `PACKAGES_PATH` | Directory where package source repositories are cloned. |
| `CHROOT_BASE_PATH` | Base location for the build chroot environment. |
| `READY_MADE_PACKAGES_PATH` | Destination folder for completed package artifacts. |
| `MASTER_CHROOT` | Derived path for the reusable chroot root. |
| `CACHYOS_REPO_URL` | Git repository used when `--cachyos` is enabled. |
| `CACHYOS_PACKAGES_PATH` | Local checkout path for the CachyOS PKGBUILDS repository. |

### Recommended first-time setup

1. Clone this repository.
2. Open `abs.config`.
3. Replace the sample paths with directories that exist on your machine.
4. Ensure your user can build packages and use `sudo` where needed.
5. Run `bash abs.sh --help` to verify the script is reachable.

## Usage

### Basic syntax

```bash
bash abs.sh [options] pkgname...
```

### Built-in help

```bash
bash abs.sh --help
```

### CLI options

| Flag | Meaning |
| --- | --- |
| `-d` | Download only. Prepare repositories but skip the build step. |
| `-l` | Build locally with `makepkg`. This is the default mode. |
| `-h` | Build in chroot mode with `makechrootpkg`. |
| `-o` | Compile only; do not prompt to install built packages. |
| `-n` | Force a new build even if package artifacts already exist. |
| `-c` | Clean the local repository checkout before cloning again. |
| `-e` | Perform full cleaning, including chroot removal and cache cleanup. |
| `-s` | Use `sudo` when deleting repositories or stale build artifacts. |
| `-r` | Remove the configured chroot directory. |
| `-k` | Install and populate Arch/CachyOS signing keys. |
| `-u` | Run `updpkgsums` before building. |
| `-v` | Verbose mode. |
| `-i` | Silent mode for normal status logs. |
| `--cachyos` | Use CachyOS PKGBUILDS instead of the Arch Linux packaging repository. |
| `--help` | Show usage information. |

> Flags can be combined, for example: `bash abs.sh -hn package-name` is supported by the parser, and combined short flags are the recommended style.

## Common Workflows

### 1. Build a package locally

```bash
bash abs.sh neovim
```

What happens:

1. The package repository is cloned or updated under `PACKAGES_PATH`.
2. The script prepares checksums and bumps `pkgrel`.
3. `makepkg --syncdeps --noconfirm --needed -f` is executed.
4. Built artifacts are placed in `READY_MADE_PACKAGES_PATH`.
5. You are prompted to install the generated package.

### 2. Build inside a clean chroot

```bash
bash abs.sh -h neovim
```

Use this when you want a cleaner and more reproducible build environment.

### 3. Prepare or refresh the chroot without building a package

```bash
bash abs.sh -h
```

If no package names are supplied in chroot mode, the script initializes or updates the master chroot and exits.

### 4. Download package sources only

```bash
bash abs.sh -d neovim
```

This is useful when you only want the PKGBUILD checkout and related files.

### 5. Force a rebuild

```bash
bash abs.sh -n neovim
```

This ignores existing package artifacts and rebuilds the package.

### 6. Update checksums before building

```bash
bash abs.sh -u neovim
```

Use this when sources changed and the PKGBUILD needs refreshed checksums.

### 7. Build without installation prompts

```bash
bash abs.sh -o neovim
```

The package is compiled, but the script will not ask to install it afterward.

### 8. Use CachyOS PKGBUILDS

```bash
bash abs.sh --cachyos scx-scheds
```

The script clones or updates the CachyOS PKGBUILDS repository and searches it for the requested package.

### 9. Re-clone a broken package checkout

```bash
bash abs.sh -c neovim
```

This deletes the existing source checkout first, then clones it again.

### 10. Remove the chroot

```bash
bash abs.sh -r
```

This removes the configured master chroot directory.

### 11. Perform aggressive cleanup

```bash
bash abs.sh -e -s
```

This workflow:

- Removes the chroot.
- Clears several language/package caches.
- Enables repository cleanup.
- Forces a new build on the next run.

Because it deletes caches and can call privileged package-manager cleanup operations, review the script before using this mode on a shared or critical machine.

## How the Script Works

### 1. Startup and configuration

At launch, `abs.sh`:

- Enables `set -e` so most command failures stop execution.
- Loads `abs.config` from the current repository directory.
- Creates the configured package, chroot, and output directories if they do not exist.

### 2. Repository preparation

For each requested package, the script:

- Chooses the Arch Linux repo path or the CachyOS PKGBUILDS repo.
- Clones the repository if missing.
- Runs `git pull` if the local checkout already exists.
- Optionally refreshes checksums with `updpkgsums`.
- Bumps `pkgrel` in the current `PKGBUILD`.

### 3. Build execution

Depending on mode:

- **Local mode** uses `makepkg` directly in the package directory.
- **Chroot mode** ensures a master chroot exists, updates it, then runs `makechrootpkg`.

In both cases, the script first checks whether expected package artifacts already exist and skips unnecessary rebuilds unless `-n` is set.

### 4. Key handling

When package signature verification fails due to unknown public keys, the script parses the build log, imports missing keys from `keyserver.ubuntu.com`, and retries the build command.

### 5. Installation prompt

If compilation succeeds and `-o` is not set, the script asks whether each produced package file should be installed with `sudo pacman -U`.

## Notes and Caveats

- The script uses **interactive install prompts**, so the final installation step is not fully unattended.
- `abs.config` currently contains **machine-specific sample paths**. Update them before use.
- `CONFIG_FILE` is set to `./abs.config`, so you should usually run the script from the repository root.
- The script assumes an **Arch-based environment** and will not work as-is on non-Arch distributions.
- Full cleanup calls `sudo pacman -Scc --noconfirm`, which removes cached packages system-wide.
- CachyOS package lookup currently searches PKGBUILD directories with `find` and `grep`, so similarly named packages could require attention.
- In local build mode, built files are expected in the output path set by `PKGDEST`.

## Troubleshooting

### `ERROR: Config file './abs.config' not found`

Run the script from the repository root, or modify the script to use an absolute config path.

### Build fails with unknown public key

Try:

```bash
bash abs.sh -k
```

Then rerun the build. The script also attempts to import missing keys automatically during builds.

### `Package <name> not found in CachyOS repo`

The package name may not exist in the CachyOS PKGBUILDS repository, or the simple search logic may not match the exact package path. Verify the package exists upstream.

### Chroot tools are missing

Install `devtools` and verify commands such as `mkarchroot`, `arch-nspawn`, and `makechrootpkg` are available in your `PATH`.

### Cleanup mode fails because `go` or `npm` are missing

Either install those tools or edit `remove_all_cache()` to fit your local development environment.

## Example Session

```bash
# Build locally
bash abs.sh -v fastfetch

# Build in chroot and skip install prompts
bash abs.sh -h -o fastfetch

# Refresh checksums and force rebuild
bash abs.sh -u -n fastfetch

# Fetch from CachyOS sources
bash abs.sh --cachyos fastfetch
```

## Suggested Improvements

If you plan to extend the project, good next enhancements would be:

- Support passing a custom config file path.
- Add dependency checks and friendlier startup validation.
- Document package naming expectations for CachyOS lookups.
- Add non-interactive install flags.
- Add shellcheck and CI validation.
- Separate cleanup actions so language-specific cache clearing is optional.

## License / Project Status

No explicit license file is currently included in this repository. If you intend to share or distribute the project publicly, consider adding a `LICENSE` file and a short project status note.
