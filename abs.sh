#!/bin/bash
set -e

# -------------------------------------------------
# Paths
# -------------------------------------------------


CONFIG_FILE="./abs.config"

if [[ -f "$CONFIG_FILE" ]]; then
    source "$CONFIG_FILE"
else
    echo "ERROR: Config file '$CONFIG_FILE' not found"
    exit 1
fi

mkdir -p "$PACKAGES_PATH" "$CHROOT_BASE_PATH" "$READY_MADE_PACKAGES_PATH"

# -------------------------------------------------
# Defaults
# -------------------------------------------------
MODE="" # Leave empty to use DEFAULT_BUILD_ENVIRONMENT
DOWNLOAD_ONLY=0
NEWBUILD=0
CLEAN=0
SUDO=0
INSTALL_KEYS=0
UPDATE_PKGSUMS=0
VERBOSE=0
SILENT=0
COMPILE_ONLY=0
REMOVE_CHROOT=0
DO_FULL_CLEANING=0
SYSTEM_UPDATE=0
FORCE_REPO_UPDATE=0

# Default system update command if not set in config
SYSTEM_UPDATE_COMMAND="${SYSTEM_UPDATE_COMMAND:-sudo pacman -Syu}"
SYSTEM_UPDATE_WITH_REPOSITORY_REFRESH_COMMAND="${SYSTEM_UPDATE_WITH_REPOSITORY_REFRESH_COMMAND:-sudo pacman -Syu}"
SYSTEM_UPDATE_IGNORE_FLAG="${SYSTEM_UPDATE_IGNORE_FLAG:---ignore}"
DEFAULT_REPOSITORY="${DEFAULT_REPOSITORY:-arch}"
DEFAULT_BUILD_ENVIRONMENT="${DEFAULT_BUILD_ENVIRONMENT:-local}"

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
  -l    Build locally
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
  -R    Force git pull / repository refresh for all custom repositories
  --repo=NAME  Use a specific repository from config (default: ${DEFAULT_REPOSITORY})
  -U    Perform full system update (${SYSTEM_UPDATE_COMMAND} or ${SYSTEM_UPDATE_WITH_REPOSITORY_REFRESH_COMMAND} if -R used)

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
        --repo=*) DEFAULT_REPOSITORY="${1#*=}" ;;
        --help) usage ;;
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
                    U) SYSTEM_UPDATE=1 ;;
                    R) FORCE_REPO_UPDATE=1 ;;
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
    remove_abs_artifacts
    CLEAN=1;
    NEWBUILD=1;
}

remove_chroot() {
    check_sudo_removal "$MASTER_CHROOT"
}

remove_abs_artifacts() {
    vlog "==> Removing all downloaded repositories and built packages..."
    check_sudo_removal "$PACKAGES_PATH"
    check_sudo_removal "$READY_MADE_PACKAGES_PATH"

    # Re-create empty base paths so subsequent commands don't fail
    mkdir -p "$PACKAGES_PATH" "$READY_MADE_PACKAGES_PATH"
}

remove_all_cache() {
    rm -rf ~/.cargo/registry/cache

    go clean -modcache
    go clean -cache

    npm cache clean --force

    sudo pacman -Scc --noconfirm
}

check_sudo_removal() {
    local cmd=("$@")

    if [[ "$SUDO" -eq 1 ]]; then
            sudo rm -rf "${cmd[@]}"
        else
            rm -rf "${cmd[@]}"
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
        echo "pkgrel=1.2" >> PKGBUILD
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
    vlog "==> Package folder: $PWD"
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
        eval "${cmd[@]}" 2>&1 | tee /tmp/abs_script.log
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


# ----------------- Repo Helpers -----------------
prepare_git_repo() {
    local repo_name="$1"
    local repo_url="${REPOSITORIES[$repo_name]}"
    local pkg_input="$2"

    # Resolve the actual package base name to clone, if an alias exists
    local pkg="${PACKAGE_ALIASES[$pkg_input]:-$pkg_input}"

    local REPO_DIR="${PACKAGES_PATH}/${repo_name}"
    local PKG_DIR

    if [[ -z "$repo_url" ]]; then
        blog "Error: Repository '$repo_name' not found in config."
        exit 1
    fi

    if [[ "$repo_name" == "arch" ]]; then
        # Arch uses a different structure (one git repo per package)
        PKG_DIR="${PACKAGES_PATH}/arch/${pkg}"
        if [[ "$CLEAN" -eq 1 && -d "$PKG_DIR" ]]; then
            vlog "==> Cleaning arch repo for $pkg"
            check_sudo_removal "$PKG_DIR"
        fi

        if [[ -d "$PKG_DIR" ]]; then
            vlog "==> Arch repo exists for $pkg. Skipping update since arch packages are updated via checkupdates"
            cd "$PKG_DIR"
            # We don't pull here anymore as requested, just cd
        else
            vlog "==> Cloning arch repo for $pkg"
            mkdir -p "${PACKAGES_PATH}/arch"
            git clone "${repo_url}/${pkg}.git" "$PKG_DIR"
            cd "$PKG_DIR"
        fi

        return
    fi

    # Other repos (CachyOS, ventureoo) use a monolithic repo containing many packages
    mkdir -p "$REPO_DIR"

    if [[ "$CLEAN" -eq 1 && -d "$REPO_DIR" ]]; then
        vlog "==> Cleaning repo $repo_name"
        check_sudo_removal "$REPO_DIR"
    fi

    if [[ -d "$REPO_DIR/.git" ]]; then
        if [[ "$FORCE_REPO_UPDATE" -eq 1 ]]; then
            vlog "==> Updating repo $repo_name (R flag used)"
            cd "$REPO_DIR"
            git pull --ff-only || true
        else
            vlog "==> Repo $repo_name exists. Skipping update (No R flag used)"
            cd "$REPO_DIR"
        fi
    else
        vlog "==> Cloning repo $repo_name"
        git clone "$repo_url" "$REPO_DIR"
    fi

    # Locate package folder by PKGBUILD
    # Modified search: first find all PKGBUILDs, then grab their directories,
    # then grep for an exact directory match ending in /$pkg
    PKG_DIR=$(find "$REPO_DIR" -type f -name "PKGBUILD" -exec dirname {} \; | grep -E "/${pkg}$" | head -n1)

    if [[ -z "$PKG_DIR" ]]; then
        # Try finding anywhere if exact match fails
        PKG_DIR=$(find "$REPO_DIR" -type f -name "PKGBUILD" -exec dirname {} \; | grep -i "$pkg" | head -n1)
    fi

    if [[ -z "$PKG_DIR" ]]; then
        blog "Package $pkg not found in repo $repo_name"
        exit 1
    fi

    cd "$PKG_DIR"
}


prepare_repo() {
    local pkg="$1"

    # Custom logic to determine repo based on PACKAGE_SOURCES
    # First check command-line --repo argument. If not DEFAULT_REPOSITORY, use it.
    local custom_repo="$2"
    local repo_to_use="$DEFAULT_REPOSITORY"

    if [[ -n "$custom_repo" ]]; then
        repo_to_use="$custom_repo"
    elif [[ -n "${PACKAGE_SOURCES[$pkg]}" ]]; then
        repo_to_use="${PACKAGE_SOURCES[$pkg]}"
    fi

    prepare_git_repo "$repo_to_use" "$pkg"
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
    local pkg_input="$1"
    local pkg="${PACKAGE_ALIASES[$pkg_input]:-$pkg_input}"
    vlog "==> Building $pkg locally"
    export PKGDEST="$READY_MADE_PACKAGES_PATH"

    local expected_files=()
    mapfile -t expected_files < <(makepkg --packagelist 2>/dev/null || true)

    local all_expected_exist=1
    if [[ ${#expected_files[@]} -eq 0 ]]; then
        all_expected_exist=0
    else
        local expected_file
        for expected_file in "${expected_files[@]}"; do
            [[ -f "$expected_file" ]] || {
                all_expected_exist=0
                break
            }
        done
    fi

    if [[ "$all_expected_exist" -eq 1 && "$NEWBUILD" -eq 0 ]]; then
        vlog "==> Package already built, skipping"
    else
        local build_cmd="${CUSTOM_LOCAL_BUILD_COMMANDS[$pkg_input]}"
        if [[ -z "$build_cmd" ]]; then
             build_cmd="makepkg --syncdeps --noconfirm --needed -f"
        fi

        fix_unknown_keys "$build_cmd"
    fi
}

build_chroot() {
    local pkg_input="$1"
    local pkg="${PACKAGE_ALIASES[$pkg_input]:-$pkg_input}"
    vlog "==> Building $pkg in chroot"

    ensure_master_chroot
    update_chroot

    local expected_files=()
    mapfile -t expected_files < <(makepkg --packagelist 2>/dev/null || true)

    local all_expected_exist=1
    if [[ ${#expected_files[@]} -eq 0 ]]; then
        all_expected_exist=0
    else
        local expected_file
        for expected_file in "${expected_files[@]}"; do
            [[ -f "$expected_file" ]] || {
                all_expected_exist=0
                break
            }
        done
    fi

    if [[ "$all_expected_exist" -eq 1 && "$NEWBUILD" -eq 0 ]]; then
        vlog "==> Package already built, skipping"
        return 0
    fi

    if [[ "$NEWBUILD" -eq 1 ]]; then
        local stale_files=()
        mapfile -t stale_files < <(makepkg --packagelist 2>/dev/null || true)
        [[ ${#stale_files[@]} -gt 0 ]] && check_sudo_removal "${stale_files[@]}"
    fi
    export PKGDEST="$READY_MADE_PACKAGES_PATH"

    # Import known keys before build
    import_keys_from_pkgbuild "${MASTER_CHROOT}/root" "$PWD"

    local build_cmd="${CUSTOM_CHROOT_BUILD_COMMANDS[$pkg_input]}"
    if [[ -z "$build_cmd" ]]; then
         build_cmd="makechrootpkg -c -r \"$MASTER_CHROOT\" -d \"$PWD\""
    fi

    fix_unknown_keys "$build_cmd"
}

should_skip_install() {
    local pkg_file="$1"
    local pkg_name

    # Use pacman to get the actual package name. It's the most reliable method.
    pkg_name=$(pacman -Qp "$pkg_file" 2>/dev/null | awk '{print $1}')

    if [[ -z "$pkg_name" ]]; then
        return 1 # Don't skip if we can't identify it
    fi

    for skip_pkg in "${SKIP_INSTALL_PACKAGES[@]}"; do
        if [[ "$pkg_name" == "$skip_pkg" ]]; then
            return 0 # Should skip
        fi
    done

    return 1 # Should not skip
}

install_built_packages() {
    local pkg_input="$1"

    # Use the resolved package name to look for output files
    local pkg="${PACKAGE_ALIASES[$pkg_input]:-$pkg_input}"

    local files=()
    mapfile -t files < <(makepkg --packagelist 2>/dev/null || true)

    if [[ ${#files[@]} -eq 0 ]]; then
        shopt -s nullglob
        files=("${READY_MADE_PACKAGES_PATH}/${pkg}-"*.pkg.tar.zst)
        shopt -u nullglob
    fi

    [[ ${#files[@]} -eq 0 ]] && return

    # Filter out skipped packages first
    local valid_files=()
    for f in "${files[@]}"; do
        if should_skip_install "$f"; then
            vlog "==> Skipping installation of ignored package: $(basename "$f")"
        else
            valid_files+=("$f")
        fi
    done

    if [[ ${#valid_files[@]} -eq 0 ]]; then
        return
    fi

    echo "==> Packages available for installation:"
    local i=1
    for f in "${valid_files[@]}"; do
        echo "  $i) $(basename "$f")"
        ((i++))
    done

    while true; do
        read -rp "Enter numbers of packages to install (e.g. 1,2,3 or 1-3, 4) [leave empty to install all, 'n' to skip]: " choice

        if [[ -z "$choice" ]]; then
            # Install all
            sudo pacman -U "${valid_files[@]}" || sudo pacman -U "${valid_files[@]}"
            break
        elif [[ "$choice" =~ ^[Nn]$ ]]; then
            echo "Skipping installation."
            break
        elif [[ "$choice" =~ ^[-0-9,[:space:]]+$ ]]; then
            # Parse ranges and comma separated values
            local -a selected_indices=()

            # Remove spaces
            choice="${choice// /}"

            IFS=',' read -ra parts <<< "$choice"
            for part in "${parts[@]}"; do
                if [[ "$part" =~ ^([0-9]+)-([0-9]+)$ ]]; then
                    local start="${BASH_REMATCH[1]}"
                    local end="${BASH_REMATCH[2]}"
                    for (( j=start; j<=end; j++ )); do
                        selected_indices+=("$j")
                    done
                else
                    selected_indices+=("$part")
                fi
            done

            # Collect selected files
            local -a files_to_install=()
            for idx in "${selected_indices[@]}"; do
                # Convert 1-based index to 0-based
                local array_idx=$((idx - 1))
                if [[ $array_idx -ge 0 && $array_idx -lt ${#valid_files[@]} ]]; then
                    files_to_install+=("${valid_files[$array_idx]}")
                else
                    echo "Warning: Number $idx is out of range."
                fi
            done

            if [[ ${#files_to_install[@]} -gt 0 ]]; then
                sudo pacman -U "${files_to_install[@]}" || sudo pacman -U "${files_to_install[@]}"
                break
            else
                echo "No valid packages selected."
            fi
        else
            echo "Invalid input format. Please use numbers, commas, and hyphens (e.g. 1,2,3 or 1-3)."
        fi
    done
}

process_package() {
    local pkg_input="$1"
    local custom_repo="$2"

    local pkg="${PACKAGE_ALIASES[$pkg_input]:-$pkg_input}"

    (
        prepare_repo "$pkg_input" "$custom_repo"

        # Execute pre-build commands if any
        if [[ -n "${PRE_UPDATE_COMMANDS[$pkg_input]}" ]]; then
            vlog "==> Running pre-update commands for $pkg_input"
            eval "${PRE_UPDATE_COMMANDS[$pkg_input]}"
        fi

        prepare_sums_pkgrel

        if [[ "$DOWNLOAD_ONLY" -eq 1 ]]; then
            vlog "==> Download-only mode, skipping build for $pkg"
        else
            # Determine build mode
            local current_mode="$MODE"
            if [[ -z "$current_mode" ]]; then
                if [[ -n "${PACKAGE_BUILDING_ENVIRONMENT[$pkg_input]}" ]]; then
                    current_mode="${PACKAGE_BUILDING_ENVIRONMENT[$pkg_input]}"
                else
                    current_mode="$DEFAULT_BUILD_ENVIRONMENT"
                fi
            fi

            vlog "==> MODE=$current_mode, building package $pkg..."

            if [[ "$current_mode" == "local" ]]; then
                build_local "$pkg_input"
            elif [[ "$current_mode" == "chroot" ]]; then
                build_chroot "$pkg_input"
            else
                echo "ERROR: Invalid build mode: $current_mode"
                exit 1
            fi

            if [[ "$COMPILE_ONLY" -eq 0 ]]; then
                install_built_packages "$pkg_input"

                # Execute post-build commands if any
                if [[ -n "${POST_UPDATE_COMMANDS[$pkg_input]}" ]]; then
                    vlog "==> Running post-update commands for $pkg_input"
                    eval "${POST_UPDATE_COMMANDS[$pkg_input]}"
                fi
            fi
        fi
    )
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

# System Update logic
if [[ "$SYSTEM_UPDATE" -eq 1 ]]; then
    blog "==> Checking for system updates..."

    # Determine command to use based on R flag
    cmd_to_use="$SYSTEM_UPDATE_COMMAND"
    if [[ "$FORCE_REPO_UPDATE" -eq 1 ]]; then
        cmd_to_use="$SYSTEM_UPDATE_WITH_REPOSITORY_REFRESH_COMMAND"
    fi

    # Get list of packages that need updating (from arch repos)
    # CheckUpdates returns non-zero if no updates, so we ignore failures
    updates_available=$(checkupdates 2>/dev/null || true)

    declare -a pkgs_to_compile=()

    if [[ -n "$updates_available" ]]; then
        while read -r update_line; do
            pkg_name=$(echo "$update_line" | awk '{print $1}')

            # Check if this package is in our manual update list
            for manual_pkg in "${MANUAL_UPDATE_PACKAGES[@]}"; do
                if [[ "$pkg_name" == "$manual_pkg" ]]; then
                    # Avoid duplicates
                    already_added=0
                    for p in "${pkgs_to_compile[@]}"; do
                        if [[ "$p" == "$pkg_name" ]]; then
                            already_added=1
                            break
                        fi
                    done
                    if [[ "$already_added" -eq 0 ]]; then
                        pkgs_to_compile+=("$pkg_name")
                    fi
                    break
                fi
            done
        done <<< "$updates_available"
    fi

    # If -R is passed, check custom repositories for updates
    if [[ "$FORCE_REPO_UPDATE" -eq 1 ]]; then
        for manual_pkg in "${MANUAL_UPDATE_PACKAGES[@]}"; do
            already_added=0
            for p in "${pkgs_to_compile[@]}"; do
                if [[ "$p" == "$manual_pkg" ]]; then
                    already_added=1
                    break
                fi
            done
            [[ "$already_added" -eq 1 ]] && continue

            repo_to_use="${PACKAGE_SOURCES[$manual_pkg]:-$DEFAULT_REPOSITORY}"

            if [[ "$repo_to_use" != "arch" ]]; then
                blog "==> Checking custom repository ($repo_to_use) for $manual_pkg updates..."

                needs_update=0
                if (
                    prepare_repo "$manual_pkg"
                    if [[ -f PKGBUILD ]]; then
                        source PKGBUILD >/dev/null 2>&1 || true
                        full_ver=""
                        [[ -n "$epoch" ]] && full_ver="${epoch}:"

                        # Fallback for missing pkgver/pkgrel which shouldn't happen but keeps shellcheck quiet
                        safe_pkgver="${pkgver:-1.0}"
                        safe_pkgrel="${pkgrel:-1}"
                        full_ver="${full_ver}${safe_pkgver}-${safe_pkgrel}"

                        pkg_base="${PACKAGE_ALIASES[$manual_pkg]:-$manual_pkg}"

                        inst_ver=$(pacman -Q "$pkg_base" 2>/dev/null | awk '{print $2}' || true)

                        if [[ -z "$inst_ver" ]] || [[ $(vercmp "$full_ver" "$inst_ver") -gt 0 ]]; then
                            exit 100
                        fi
                    fi
                    exit 0
                ); then
                    needs_update=0
                else
                    if [[ $? -eq 100 ]]; then
                        needs_update=1
                    fi
                fi

                if [[ "$needs_update" -eq 1 ]]; then
                    pkgs_to_compile+=("$manual_pkg")
                fi
            fi
        done
    fi

    if [[ ${#pkgs_to_compile[@]} -gt 0 ]]; then
        blog "==> The following packages will be manually compiled:"
        for p in "${pkgs_to_compile[@]}"; do
            blog "  -> $p"
        done

        # Then compile and install manual packages first
        blog "==> Compiling manual packages..."
        for p in "${pkgs_to_compile[@]}"; do
            process_package "$p" ""
        done

        # Finally, update all other packages standard way
        blog "==> Updating standard system packages..."

        # 1. We add the configured ignore flag for all manually compiled packages
        ignore_args=""
        for p in "${pkgs_to_compile[@]}"; do
            ignore_args="$ignore_args ${SYSTEM_UPDATE_IGNORE_FLAG} $p"
        done

        # 2. We add the configured ignore flag for all explicitly ignored packages from config
        for p in "${SYSTEM_UPDATE_IGNORE_PACKAGES[@]}"; do
            ignore_args="$ignore_args ${SYSTEM_UPDATE_IGNORE_FLAG} $p"
        done

        eval "${cmd_to_use} ${ignore_args}"

    else
        blog "==> No manual compile packages need updating. Running standard update..."
        # Apply configured ignore packages even if no manual compilations were done
        ignore_args=""
        for p in "${SYSTEM_UPDATE_IGNORE_PACKAGES[@]}"; do
            ignore_args="$ignore_args ${SYSTEM_UPDATE_IGNORE_FLAG} $p"
        done

        eval "${cmd_to_use} ${ignore_args}"
    fi
    exit 0
fi

if [[ ${#PKG_ARRAY[@]} -eq 0 && "$MODE" != "chroot" ]]; then
    blog "No packages to build."
    exit 1
fi

if [[ ${#PKG_ARRAY[@]} -eq 0 ]]; then
    if [[ "$MODE" == "chroot" ]]; then
        blog "==> No packages specified, preparing/updating chroot"
        ensure_master_chroot
        update_chroot
        vlog "==> Chroot ready"
        exit 0
    else
        usage
    fi
fi

for pkg in "${PKG_ARRAY[@]}"; do
    process_package "$pkg" ""
done

blog "==> All requested packages processed successfully"
