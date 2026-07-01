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
    local w=$1 script ffmpegver kernver ycruncher_ver
    script="$(command -v cachyos-benchmarker)" || return 1
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
    if ! command -v cachyos-benchmarker >/dev/null 2>&1; then
        echo "error: cachyos-benchmarker not in PATH (ABS_PGO_BENCHMARK=cachyos)" >&2
        return 127
    fi
    install_quiet_wget_wrapper
    if ! cachyos_benchmarker_assets_cached "${WORKDIR}"; then
        echo "==> cachyos-benchmarker (opt-in): downloads + configures sources; first run is very slow"
    fi
    local progress_pid=""
    ( while sleep 120; do echo "==> cachyos-benchmarker still running ($(date +%H:%M:%S))…"; done ) &
    progress_pid=$!
    trap 'kill "${progress_pid}" 2>/dev/null || true' RETURN
    # checksys() prompts twice; feed defaults (no page-cache drop, default run name).
    if ! printf '\n\n' | cachyos-benchmarker "${WORKDIR}"; then
        local status=$?
        kill "${progress_pid}" 2>/dev/null || true
        wait "${progress_pid}" 2>/dev/null || true
        return "${status}"
    fi
    kill "${progress_pid}" 2>/dev/null || true
    wait "${progress_pid}" 2>/dev/null || true
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
