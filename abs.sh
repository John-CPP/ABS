#!/bin/bash
set -e

# -------------------------------------------------
# Paths
# -------------------------------------------------
PACKAGES_PATH="/media/storage/packages/my"
CHROOT_BASE_PATH="/media/storage/packages/my/chroot"
READY_MADE_PACKAGES_PATH="/media/storage/packages/abs_ready"
MASTER_CHROOT="${CHROOT_BASE_PATH}/base"

CACHYOS_REPO_URL="https://github.com/CachyOS/CachyOS-PKGBUILDS.git"
CACHYOS_PACKAGES_PATH="${PACKAGES_PATH}/CachyOS-PKGBUILDS"

mkdir -p "$PACKAGES_PATH" "$CHROOT_BASE_PATH" "$READY_MADE_PACKAGES_PATH"

# -------------------------------------------------
# Defaults
# -------------------------------------------------
MODE="local"
DOWNLOAD_ONLY=0
NEWBUILD=0
CLEAN=0
SUDO=0
USE_CACHYOS=0
INSTALL_KEYS=0
UPDATE_PKGSUMS=0
VERBOSE=0
SILENT=0
COMPILE_ONLY=0
REMOVE_CHROOT=0
DO_FULL_CLEANING=0

# -------------------------------------------------
# Verbose helper
# -------------------------------------------------
vlog() {
   if [[ "$VERBOSE" -eq 1 ]]; then
        echo "$@"
        return
   fi
}

blog() {
    if [[ "$SILENT" -eq 0 ]]; then
        echo "$@"
        return
   fi
}


# -------------------------------------------------
# Usage
# -------------------------------------------------
usage() {
    cat <<EOF
Usage: $0 [options] pkgname...

Options:
  -d    Download only (no build)
  -l    Build locally (default)
  -h    Build in chroot
  -o    Only compiles, doesn't install built packages
  -n    Force new build
  -c    Clean repo (delete + reclone)
  -e    Do Full Cleaning (Sometimes things hang because of some cache)
  -s    Use sudo for cleaning repo
  -r    Remove Chroot
  -k    Populate Keys (to fix unknown public key)
  -u    Update pkgsums before building
  -v    Verbose mode (show script comments)
  -i    Silent Mode
  --cachyos  Use CachyOS-PKGBUILDS repo instead of Arch Linux

Flags can be combined (e.g. -ch, -hnc).
EOF
    exit 1
}

# -------------------------------------------------
# Parse flags
# -------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        --) shift; break ;;
        --cachyos) USE_CACHYOS=1 ;;
        -*)
            flags="${1#-}"
            for (( i=0; i<${#flags}; i++ )); do
                f="${flags:$i:1}"
                case "$f" in
                    d) DOWNLOAD_ONLY=1 ;;
                    l) MODE="local" ;;
                    h) MODE="chroot" ;;
                    n) NEWBUILD=1 ;;
                    c) CLEAN=1 ;;
                    s) SUDO=1 ;;
                    k) INSTALL_KEYS=1 ;;
                    u) UPDATE_PKGSUMS=1 ;;
                    v) VERBOSE=1 ;;
                    i) SILENT=1 ;;
                    o) COMPILE_ONLY=1 ;;
                    r) REMOVE_CHROOT=1 ;;
                    e) DO_FULL_CLEANING=1 ;;
                    *) usage ;;
                esac
            done
            ;;
        *) break ;;
    esac
    shift
done

PKG_ARRAY=("$@")

#--------------------------------------
# Cleaners
#--------------------------------------
do_full_cleaning() {
    remove_chroot
    remove_all_cache
    CLEAN=1;
    NEWBUILD=1;
}

remove_chroot() {
    check_sudo_removal "$MASTER_CHROOT"
    mkdir -p "$MASTER_CHROOT"
}

remove_all_cache() {
    rm -rf ~/.cargo/registry/cache

    go clean -modcache
    go clean -cache

    npm cache clean --force

    sudo sudo pacman -Scc
}

check_sudo_removal() {
    local cmd=("$@")

    if [[ "$SUDO" -eq 1 ]]; then
            sudo rm -rf "$cmd"
        else
            rm -rf "$cmd"
    fi
}


# -------------------------------------------------
# Helpers
# -------------------------------------------------
bump_pkgrel() {
    local current base
    if [[ ! -f PKGBUILD ]]; then
        vlog "PKGBUILD not found, skipping pkgrel bump"
        return
    fi

    current=$(grep -E '^pkgrel=' PKGBUILD | cut -d= -f2 || true)
    if [[ -z "$current" ]]; then
        vlog "pkgrel=1.2" >> PKGBUILD
        return
    fi

    base="${current%%.*}"
    sed -i "s/^pkgrel=.*/pkgrel=${base}.2/" PKGBUILD || vlog "Failed to bump pkgrel, skipping"
}


install_all_keys() {
    vlog "==> Installing Arch Linux and CachyOS keyrings"
    sudo pacman -Sy --noconfirm archlinux-keyring cachyos-keyring || true

    vlog "==> Populating keys for archlinux and cachyos"
    sudo pacman-key --populate archlinux
    sudo pacman-key --populate cachyos || true

    vlog "==> Refreshing keys from keyserver"
    sudo pacman-key --keyserver hkps://keyserver.ubuntu.com --refresh-keys || true

    vlog "==> All keys installed and refreshed"
}

prepare_sums_pkgrel() {
    vlog "==> Package folder: $PKG_DIR"
    vlog "==> Preparing pkgsums..."
    prepare_pkgsums
    vlog "==> Bumping pkgrel..."
    bump_pkgrel
    vlog "==> Repo preparation done"

}

prepare_pkgsums() {
    if [[ "$UPDATE_PKGSUMS" -eq 1 ]]; then
        vlog "==> Updating PKGBUILD checksums..."
        updpkgsums || vlog "==> updpkgsums failed, continuing..."
    else
        vlog "==> pkgsums not requested to update"
    fi
}

# ----------------- Key Helpers -----------------
import_keys_from_pkgbuild() {
    local chroot_root="$1"
    local pkg_dir="$2"
    vlog "==> Importing PKGBUILD-specific keys into chroot $chroot_root"

    # Collect all keys from PKGBUILD
    local keys
    keys=$(grep '^validpgpkeys=' "$pkg_dir/PKGBUILD" \
           | sed -E "s/validpgpkeys=\(?['\"]?(.*)['\"]?\)?/\1/" \
           | tr ',' ' ')

    [[ -z "$keys" ]] && return 0

    vlog "==> Importing keys: $keys"
    for key in $keys; do
        gpg --keyserver hkps://keyserver.ubuntu.com --recv-keys $key
    done

    vlog "==> PKGBUILD keys imported"
}


fix_unknown_keys() {
    local seen_keys=""
    local cmd=("$@")  # Take the command as arguments

    while true; do
        vlog "==> Running command: ${cmd[*]}"

        # Run the command, tee output to log
        "${cmd[@]}" 2>&1 | tee /tmp/abs_script.log
        local exit_code=${PIPESTATUS[0]}

        if [[ $exit_code -eq 0 ]]; then
            vlog "==> Command succeeded"
            break
        fi

        # Extract missing keys
        local missing_keys
        missing_keys=$(grep -oP 'unknown public key \K[0-9A-F]+' /tmp/abs_script.log || true)

        # Filter out keys we've already imported
        missing_keys=$(comm -23 <(echo "$missing_keys" | sort) <(echo "$seen_keys" | tr ' ' '\n' | sort))

        if [[ -z "$missing_keys" ]]; then
            vlog "==> Build failed, no new missing keys detected. Giving up."
            return $exit_code
        fi

        vlog "==> Missing keys detected: $missing_keys"
        for key in $missing_keys; do
            vlog "==> Importing missing key $key..."
            gpg --keyserver hkps://keyserver.ubuntu.com --recv-keys "$key"
        done

        # Add newly imported keys to seen_keys
        seen_keys="$seen_keys $missing_keys"

        vlog "==> Retrying command after importing missing keys..."
    done
}


# ----------------- Arch Repo -----------------
prepare_arch_repo() {
    local pkg="$1"
    local pkg_dir="${PACKAGES_PATH}/${pkg}"

    if [[ "$CLEAN" -eq 1 && -d "$pkg_dir" ]]; then
        vlog "==> Cleaning repo for $pkg"
        check_sudo_removal "$pkg_dir"
    fi

    if [[ -d "$pkg_dir" ]]; then
        vlog "==> Updating repo for $pkg"
        cd "$pkg_dir"
        git pull  || true
        prepare_sums_pkgrel
    else
        vlog "==> Cloning repo for $pkg"
        git clone "https://gitlab.archlinux.org/archlinux/packaging/packages/${pkg}.git" "$pkg_dir"
        cd "$pkg_dir"
        prepare_sums_pkgrel
    fi
}

# ----------------- CachyOS Repo -----------------
prepare_cachyos_repo() {
    local pkg="$1"

    mkdir -p "$CACHYOS_PACKAGES_PATH"

    if [[ "$CLEAN" -eq 1 && -d "$CACHYOS_PACKAGES_PATH" ]]; then
        vlog "==> Cleaning CachyOS repo"
        if [[ "$SUDO" -eq 1 ]]; then
            sudo rm -rf "$CACHYOS_PACKAGES_PATH"
        else
            rm -rf "$CACHYOS_PACKAGES_PATH"
        fi
    fi

    if [[ -d "$CACHYOS_PACKAGES_PATH/.git" ]]; then
        vlog "==> Updating CachyOS-PKGBUILDS repo"
        cd "$CACHYOS_PACKAGES_PATH"
        git pull --ff-only || true
    else
        vlog "==> Cloning CachyOS-PKGBUILDS repo"
        git clone "$CACHYOS_REPO_URL" "$CACHYOS_PACKAGES_PATH"
    fi

    # Locate package folder by PKGBUILD
    PKG_DIR=$(find "$CACHYOS_PACKAGES_PATH" -type f -name "PKGBUILD" -exec dirname {} \; | grep -i "$pkg" | head -n1)
    if [[ -z "$PKG_DIR" ]]; then
        blog "Package $pkg not found in CachyOS repo"
        exit 1
    fi

    cd "$PKG_DIR"
    prepare_sums_pkgrel
}

prepare_repo() {
    local pkg="$1"
    if [[ "$USE_CACHYOS" -eq 1 ]]; then
        prepare_cachyos_repo "$pkg"
    else
        prepare_arch_repo "$pkg"
    fi
}

# ----------------- Chroot Helpers -----------------
ensure_master_chroot() {
    if [[ ! -d "${MASTER_CHROOT}/root" ]]; then
        vlog "==> Creating master chroot"
        mkdir -p "$MASTER_CHROOT"
        mkarchroot "${MASTER_CHROOT}/root" base base-devel
    fi
}

update_chroot() {
    vlog "==> Updating chroot"
    arch-nspawn "${MASTER_CHROOT}/root" pacman -Syu --noconfirm
}


# ----------------- Build Helpers -----------------
build_local() {
    local pkg="$1"
    vlog "==> Building $pkg locally"
    export PKGDEST="$READY_MADE_PACKAGES_PATH"

    shopt -s nullglob
    local files=("${READY_MADE_PACKAGES_PATH}/${pkg}-"*.pkg.tar.zst)
    shopt -u nullglob

    if [[ ${#files[@]} -gt 0 && "$NEWBUILD" -eq 0 ]]; then
        vlog "==> Package already built, skipping"
    else
        fix_unknown_keys makepkg --syncdeps --noconfirm --needed -f
    fi
}

build_chroot() {
    local pkg="$1"
    vlog "==> Building $pkg in chroot"

    ensure_master_chroot
    update_chroot

    shopt -s nullglob
    local files=("${READY_MADE_PACKAGES_PATH}/${pkg}-"*.pkg.tar.zst)
    shopt -u nullglob

    if [[ ${#files[@]} -gt 0 && "$NEWBUILD" -eq 0 ]]; then
        vlog "==> Package already built, skipping"
        return 0
    fi

    [[ "$NEWBUILD" -eq 1 ]] && check_sudo_removal "${READY_MADE_PACKAGES_PATH}/${pkg}-"*.pkg.tar.zst
    export PKGDEST="$READY_MADE_PACKAGES_PATH"

    # Import known keys before build
    import_keys_from_pkgbuild "${MASTER_CHROOT}/root" "$PWD"

    fix_unknown_keys makechrootpkg -c -r "$MASTER_CHROOT" -d "$PWD"
}

install_built_packages() {
    local pkg="$1"
    shopt -s nullglob
    local files=("${READY_MADE_PACKAGES_PATH}/${pkg}-"*.pkg.tar.zst)
    shopt -u nullglob

    [[ ${#files[@]} -eq 0 ]] && return

    for f in "${files[@]}"; do
        while true; do
            read -rp "Install $f ? [Y/n] " yn
            case "$yn" in
                [Yy]*|"")
                    sudo pacman -U "$f" || sudo pacman -U "$f"
                    break
                    ;;
                [Nn]*) break ;;
                *) echo "Answer Y or N" ;;
            esac
        done
    done
}


# -------------------------------------------------
# Main
# -------------------------------------------------

if [[ "$INSTALL_KEYS" -eq 1 ]]; then
    install_all_keys
    blog "==> Keys installed."
fi

if [[ "$REMOVE_CHROOT" -eq 1 ]]; then
    remove_chroot
    blog "==> Chroot Removed. "
fi

if [[ "$DO_FULL_CLEANING" -eq 1 ]]; then
    do_full_cleaning
    blog "==> Full cleaning done."
fi

if [[ ${#PKG_ARRAY[@]} -eq 0 && "$MODE" != "chroot" ]]; then
    blog "No packages to build."
    exit 1
fi


[[ ${#PKG_ARRAY[@]} -eq 0 ]] && {
    if [[ "$MODE" == "chroot" ]]; then
        blog "==> No packages specified, preparing/updating chroot"
        ensure_master_chroot
        update_chroot
        vlog "==> Chroot ready"
        exit 1
    else
        usage
    fi
}

for pkg in "${PKG_ARRAY[@]}"; do
    prepare_repo "$pkg"

    [[ "$DOWNLOAD_ONLY" -eq 1 ]] && continue

    vlog "==> MODE=$MODE, building package $pkg..."
    if [[ "$MODE" == "local" ]]; then
        build_local "$pkg"
    else
        build_chroot "$pkg"
    fi

    if [[ "$COMPILE_ONLY" == 0 ]]; then
        install_built_packages "$pkg"
    fi
done

blog "==> All requested packages processed successfully"
