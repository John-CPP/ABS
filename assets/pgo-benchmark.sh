#!/usr/bin/env bash
# Bundled with ABS — default PGO profiling workload.
# ABS_PGO_BENCHMARK=fast (default): sysbench + stress-ng, no downloads, fully unattended.
# ABS_PGO_BENCHMARK=cachyos: full cachyos-benchmarker (multi-GB download, 30–60+ min, opt-in only).
set -euo pipefail

WORKDIR="${ABS_PGO_BENCHMARK_DIR:-${ABS_PGO_PROFILE_DIR:-${TMPDIR:-/tmp}/abs-pgo-profile}}"
MODE="${ABS_PGO_BENCHMARK:-fast}"
NPROC="$(nproc)"

echo "ABS PGO benchmark (mode=${MODE})"
mkdir -p "${WORKDIR}"
cd "${WORKDIR}"

run_sysbench() {
    local label=$1
    shift
    echo "==> sysbench: ${label} ($(date +%H:%M:%S))"
    sysbench "$@"
    echo "==> sysbench: ${label} done"
}

run_fast_benchmark() {
    echo "==> fast profiling workload (no downloads, unattended)"

    if command -v stress-ng >/dev/null 2>&1; then
        echo "==> stress-ng: cpu/memory ($(date +%H:%M:%S))"
        stress-ng --cpu "${NPROC}" --cpu-method matrixprod --timeout 45s --metrics-brief >/dev/null 2>&1 || true
        stress-ng --vm 2 --vm-bytes 40% --timeout 30s --metrics-brief >/dev/null 2>&1 || true
        echo "==> stress-ng done"
    else
        echo "warning: stress-ng not in PATH; skipping" >&2
    fi

    echo "==> sysbench suite"
    run_sysbench "cpu" --time=45 cpu --cpu-max-prime=50000 --threads="${NPROC}" run >/dev/null

    local mem_mb
    mem_mb="$(awk '/MemAvailable:/ {printf "%d", int($2/1024*3/4)}' /proc/meminfo)"
    if [[ -z "${mem_mb}" || "${mem_mb}" -lt 512 ]]; then
        mem_mb=512
    elif [[ "${mem_mb}" -gt 16384 ]]; then
        mem_mb=16384
    fi
    echo "==> sysbench memory using ${mem_mb} MiB"
    run_sysbench "memory (write)" memory --memory-block-size=1M --memory-total-size="${mem_mb}M" run >/dev/null
    run_sysbench "memory (read)" memory --memory-block-size=1M --memory-total-size="${mem_mb}M" \
        --memory-oper=read --threads=16 run >/dev/null

    local io_mb=2048
    if awk -v need=4096 '/MemAvailable:/ {exit !($2/1024 >= need)}' /proc/meminfo; then
        io_mb=5120
    fi
    echo "==> sysbench fileio (${io_mb} MiB total)"
    run_sysbench "fileio prepare" fileio --file-total-size="${io_mb}M" --file-num=4 prepare >/dev/null
    run_sysbench "fileio random read" fileio --file-total-size="${io_mb}M" --file-num=4 \
        --file-fsync-freq=0 --file-test-mode=rndrd --file-block-size=4K run >/dev/null
    run_sysbench "fileio sequential write" fileio --file-total-size="${io_mb}M" --file-num=4 \
        --file-fsync-freq=0 --file-test-mode=seqwr --file-block-size=1M run >/dev/null
    sysbench fileio --file-total-size="${io_mb}M" --file-num=4 cleanup >/dev/null 2>&1 || true

    echo "==> misc I/O and search"
    find /usr/include -type f -name '*.h' 2>/dev/null | head -n 5000 >/dev/null || true
    if command -v rg >/dev/null 2>&1; then
        rg -l 'kernel|sched' /usr/include 2>/dev/null | head -n 200 >/dev/null || true
    fi

    echo "==> fast profiling workload finished"
}

# Persistent workdirs keep .abs-bin/cachyos-benchmarker from the last run.
# The wget wrapper prepends that directory to PATH, so `command -v` must
# skip the leftover ABS shim and keep the real CachyOS script.
find_real_cachyos_benchmarker() {
    local bindir="${WORKDIR}/.abs-bin"
    local dir candidate resolved
    local -a dirs
    IFS=':' read -ra dirs <<< "${PATH}"
    for dir in "${dirs[@]}"; do
        [[ -n "${dir}" ]] || continue
        candidate="${dir}/cachyos-benchmarker"
        [[ -x "${candidate}" && ! -d "${candidate}" ]] || continue
        resolved="$(readlink -f "${candidate}")"
        [[ "${resolved}" != "${bindir}/cachyos-benchmarker" ]] || continue
        printf '%s\n' "${resolved}"
        return 0
    done
    return 1
}

# CachyOS ends every run by invoking benchmark_scraper.py (matplotlib). That
# script crashes when the persistent ABS workdir mixes logs with different
# test counts. ABS writes charts in Rust — patch the scraper call out of a
# copy of cachyos-benchmarker. Do not put python on PATH.
install_skip_scraper_wrapper() {
    local bindir="${WORKDIR}/.abs-bin"
    mkdir -p "${bindir}"
    local real="${1:-}"
    if [[ -z "${real}" ]]; then
        real="$(find_real_cachyos_benchmarker)" || return 1
    fi
    real="$(readlink -f "${real}")"
    if [[ "${real}" == "${bindir}/cachyos-benchmarker" ]]; then
        echo "error: refusing to wrap the ABS cachyos-benchmarker shim" >&2
        return 1
    fi
    cat > "${bindir}/cachyos-benchmarker" << 'EOF'
#!/usr/bin/env bash
set -euo pipefail
real="${ABS_CACHYOS_BENCHMARKER_REAL:?}"
patched="$(mktemp)"
trap 'rm -f "${patched}"' EXIT
scriptdir="$(dirname "${real}")"
sed -E \
    -e "s|^SCRIPTDIR=.*|SCRIPTDIR=\"${scriptdir}\"|" \
    -e 's|^[[:space:]]*python3?[[:space:]].*benchmark_scraper\.py.*$|echo "==> skipping CachyOS benchmark_scraper.py (ABS writes comparison charts)"; true|' \
    "${real}" > "${patched}"
chmod +x "${patched}"
exec bash "${patched}" "$@"
EOF
    chmod +x "${bindir}/cachyos-benchmarker"
    export ABS_CACHYOS_BENCHMARKER_REAL="${real}"
    export PATH="${bindir}:${PATH}"
}

install_quiet_wget_wrapper() {
    local bindir="${WORKDIR}/.abs-bin"
    mkdir -p "${bindir}"
    cat > "${bindir}/wget" << 'EOF'
#!/usr/bin/env bash
set -euo pipefail
real=/usr/bin/wget
[[ -x "${real}" ]] || real="$(command -v wget 2>/dev/null || true)"
[[ -n "${real}" && -x "${real}" ]] || { echo "wget not found" >&2; exit 127; }
args=()
dest=""
prev=""
for a in "$@"; do
    case "$a" in
        --show-progress|--progress=bar*|--progress=dot*) continue ;;
    esac
    if [[ "${prev}" == "-O" || "${prev}" == "-qO" ]]; then dest="$a"; fi
    prev="$a"
    args+=("$a")
done
echo "==> wget: ${dest##*/} ($(date +%H:%M:%S))"
exec "${real}" -q "${args[@]}"
EOF
    chmod +x "${bindir}/wget"
    export PATH="${bindir}:${PATH}"
}

# True when cachyos-benchmarker would skip its large wget/tar steps (same paths as /usr/bin/cachyos-benchmarker).
cachyos_benchmarker_assets_cached() {
    local w=$1 script="${2:-}" ffmpegver kernver ycruncher_ver
    if [[ -z "${script}" ]]; then
        script="$(find_real_cachyos_benchmarker)" || return 1
    fi
    ffmpegver="$(sed -n 's/^FFMPEGVER="\([^"]*\)".*/\1/p' "${script}" | head -1)"
    ycruncher_ver="$(sed -n 's/^YCRUNCHER_VER="\([^"]*\)".*/\1/p' "${script}" | head -1)"
    kernver="$(sed -n 's/^KERNVER="\([^"]*\)".*/\1/p' "${script}" | head -1)"
    [[ -n "${ffmpegver}" && -n "${ycruncher_ver}" && -n "${kernver}" ]] || return 1
    [[ -d "${w}/ffmpeg-${ffmpegver}" ]] \
        && [[ -d "${w}/linux-${kernver}" ]] \
        && [[ -d "${w}/y-cruncher v${ycruncher_ver}-static" ]] \
        && [[ -d "${w}/namd" ]] \
        && [[ -f "${w}/bosphorus_hd.y4m" ]] \
        && [[ -f "${w}/bmw_cpu_mod.blend" ]] \
        && [[ -f "${w}/firefox102.tar" ]]
}

run_cachyos_benchmarker() {
    local real
    real="$(find_real_cachyos_benchmarker)" || {
        echo "error: cachyos-benchmarker not in PATH (ABS_PGO_BENCHMARK=cachyos)" >&2
        return 127
    }
    install_quiet_wget_wrapper
    # Hide prior logs so a leaked scraper still sees only this run.
    mkdir -p "${WORKDIR}/.abs-benchie-prev"
    shopt -s nullglob
    local prev_logs=("${WORKDIR}"/benchie_*.log)
    if ((${#prev_logs[@]} > 0)); then
        mv -f "${prev_logs[@]}" "${WORKDIR}/.abs-benchie-prev/" || true
    fi
    shopt -u nullglob
    if ! cachyos_benchmarker_assets_cached "${WORKDIR}" "${real}"; then
        echo "==> cachyos-benchmarker (opt-in): downloads + configures sources; first run is very slow"
    fi
    install_skip_scraper_wrapper "${real}"
    local progress_pid=""
    # `wait` (builtin) is interruptible; a foreground `sleep` is not. Killing
    # the reporter must also reap its sleep child so piped ABS logs close.
    (
        trap 'kill "${sp:-}" 2>/dev/null; exit 0' TERM INT
        while true; do
            sleep 120 &
            sp=$!
            wait "${sp}" || true
            echo "==> cachyos-benchmarker still running ($(date +%H:%M:%S))…"
        done
    ) &
    progress_pid=$!
    stop_progress() {
        if [[ -n "${progress_pid}" ]]; then
            kill "${progress_pid}" 2>/dev/null || true
            wait "${progress_pid}" 2>/dev/null || true
            progress_pid=""
        fi
    }
    trap 'stop_progress' RETURN
    # checksys() prompts twice: page-cache drop (empty = no), then run name.
    # ABS_PGO_COMPARE_LABEL is set when this run is also the comparison chart series.
    local label="${ABS_PGO_COMPARE_LABEL:-}"
    if ! printf '%s\n' '' "${label}" | cachyos-benchmarker "${WORKDIR}"; then
        local status=$?
        stop_progress
        return "${status}"
    fi
    stop_progress
}

case "${MODE}" in
    fast|"") run_fast_benchmark ;;
    cachyos|full) run_cachyos_benchmarker ;;
    *)
        echo "error: unknown ABS_PGO_BENCHMARK='${MODE}' (use fast or cachyos)" >&2
        exit 2
        ;;
esac

echo "All tests completed."
