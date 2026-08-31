#!/usr/bin/env bash
# Bundled with ABS — PGO profiling and comparison workloads.
#
# Training (under perf record, `:k` samples). Userspace suites (y-cruncher, blender,
# sysbench cpu) leave syscall/VFS/net unsampled; AutoFDO then treats those paths as
# cold. Only `kernel` is supported — it drives those paths.
#   ABS_PGO_BENCHMARK=kernel  (default)
#
# Scoring (no perf; comparison charts). Keep this shorter than training:
#   ABS_PGO_BENCHMARK=kbench            kernel micro-benchmarks → benchie_*.log
#   ABS_PGO_BENCHMARK=warmup            CPU turbo warm-up before a scored run
#
# ABS_PGO_PROFILE=short|sweet|long  (sweet default) sets the budget when
# ABS_PGO_KERNEL_SECS is unset:
#   short  ~10 min train / ~3 min kbench
#   sweet  ~20 min train / ~6 min kbench   (default)
#   long   ~60 min train / ~10 min kbench
# ABS_PGO_KBENCH_REPEATS / ABS_PGO_KBENCH_SECS override comparison length.
set -euo pipefail

WORKDIR="${ABS_PGO_BENCHMARK_DIR:-${ABS_PGO_PROFILE_DIR:-${TMPDIR:-/tmp}/abs-pgo-profile}}"
MODE="${ABS_PGO_BENCHMARK:-kernel}"
NPROC="$(nproc)"
SNG_TMP="${WORKDIR}/.abs-sng"

# ABS_PGO_DRY_RUN=1 prints the workload plan and exits without loading the
# machine. Use it to review a config before spending a profiling stage on it.
DRY_RUN=0
if [[ "${ABS_PGO_DRY_RUN:-0}" == "1" ]]; then
    DRY_RUN=1
fi

# stress-ng spawns workers *per stressor*, so a long list at one worker per CPU
# forks hundreds of tasks and can livelock a desktop. Cap total concurrency.
SNG_BATCH="${ABS_PGO_SNG_BATCH:-4}"
SNG_WORKER_CAP="${ABS_PGO_SNG_WORKERS:-${NPROC}}"
SNG_NICE="${ABS_PGO_NICE:-0}"
# sock/sockpair/udp hold skbuffs in unreclaimable kernel slab (not process RSS).
# nproc/2 sockpair workers exhausted 96G after reboot; keep this small.
SNG_NET_WORKERS="${ABS_PGO_SNG_NET_WORKERS:-4}"
SNG_OOM_AVOID_BYTES="${ABS_PGO_OOM_AVOID_BYTES:-1G}"
SNG_MAX_FD="${ABS_PGO_SNG_MAX_FD:-1024}"
PROFILE="$(printf '%s' "${ABS_PGO_PROFILE:-sweet}" | tr '[:upper:]' '[:lower:]')"
if ((SNG_BATCH < 1)); then
    SNG_BATCH=1
fi
if ((SNG_WORKER_CAP < 1)); then
    SNG_WORKER_CAP=1
fi
if ! [[ "${SNG_NICE}" =~ ^-?[0-9]+$ ]]; then
    SNG_NICE=0
fi
if ! [[ "${SNG_NET_WORKERS}" =~ ^[0-9]+$ ]] || ((SNG_NET_WORKERS < 1)); then
    SNG_NET_WORKERS=1
fi
if ((SNG_NET_WORKERS > SNG_WORKER_CAP)); then
    SNG_NET_WORKERS=${SNG_WORKER_CAP}
fi
if ! [[ "${SNG_MAX_FD}" =~ ^[0-9]+$ ]] || ((SNG_MAX_FD < 64)); then
    SNG_MAX_FD=1024
fi

echo "ABS PGO benchmark (mode=${MODE})"
mkdir -p "${WORKDIR}"
cd "${WORKDIR}"

run_cpu_warmup() {
    echo "==> CPU warm-up (no disk) so scored runs start at turbo with a cold page cache"
    if ((DRY_RUN)); then
        echo "    [dry run] stress-ng cpu 20s, sysbench cpu 15s"
        return 0
    fi
    if command -v stress-ng >/dev/null 2>&1; then
        stress-ng --cpu "${NPROC}" --cpu-method matrixprod --timeout 20s --metrics-brief >/dev/null 2>&1 || true
    fi
    if command -v sysbench >/dev/null 2>&1; then
        sysbench --time=15 cpu --cpu-max-prime=20000 --threads="${NPROC}" run >/dev/null || true
    fi
    echo "==> CPU warm-up done"
}

# ---------------------------------------------------------------------------
# stress-ng plumbing
#
# Stressor names differ across stress-ng releases, so every list is filtered
# against `stress-ng --stressors` before use. Only unprivileged, non-fault-
# injection stressors are listed: deliberate faults (sigsegv, sysbadaddr,
# bad-altstack) and root-only ones would skew the profile toward error paths.
# ---------------------------------------------------------------------------

declare -A SNG_OK=()
SNG_LOADED=0

sng_load_supported() {
    if ((SNG_LOADED)); then
        return 0
    fi
    SNG_LOADED=1
    if ! command -v stress-ng >/dev/null 2>&1; then
        return 0
    fi
    local s
    for s in $(stress-ng --stressors 2>/dev/null || true); do
        SNG_OK["${s}"]=1
    done
}

sng_have() { [[ -n "${SNG_OK[$1]:-}" ]]; }

# Kernel skbuffs/unix socks do not show up in worker RSS, so --oom-avoid (MemAvailable)
# is the backstop when a stressor still allocates faster than it frees.
SNG_OOM_FLAGS=(--oom-avoid --oom-avoid-bytes "${SNG_OOM_AVOID_BYTES}")

# Run a stressor list in batches of SNG_BATCH. Time is split across batches and
# never inflated past `secs` (a per-batch floor would make a "20 min" profile
# run 25). Total workers stay <= SNG_WORKER_CAP.
sng_group() {
    local label=$1 secs=$2 workers_total=$3
    shift 3
    if ((secs <= 0)); then
        return 0
    fi
    if ((workers_total < 1)); then
        workers_total=1
    fi
    if ((workers_total > SNG_WORKER_CAP)); then
        workers_total=${SNG_WORKER_CAP}
    fi

    local -a list=()
    local s
    for s in "$@"; do
        if sng_have "${s}"; then
            list+=("${s}")
        fi
    done
    if ((${#list[@]} == 0)); then
        echo "==> kernel group ${label}: no supported stressors, skipping"
        return 0
    fi

    local batch=${SNG_BATCH}
    if ((batch > ${#list[@]})); then
        batch=${#list[@]}
    fi
    local nbatches=$(((${#list[@]} + batch - 1) / batch))
    local batch_secs=$((secs / nbatches))
    if ((batch_secs < 1)); then
        batch_secs=1
    fi
    local per_worker=$((workers_total / batch))
    if ((per_worker < 1)); then
        per_worker=1
    fi

    echo "==> kernel group ${label}: ${#list[@]} stressors in ${nbatches} batches," \
        "${batch_secs}s x ${per_worker} workers each ($(date +%H:%M:%S))"
    mkdir -p "${SNG_TMP}"

    local i=0 n=1 failed=0
    while ((i < ${#list[@]})); do
        local -a chunk=("${list[@]:i:batch}")
        local -a flags=()
        for s in "${chunk[@]}"; do
            flags+=("--${s}" "${per_worker}")
        done
        echo "    [${n}/${nbatches}] ${chunk[*]}"
        if ((DRY_RUN == 0)); then
            local -a extra=("${SNG_OOM_FLAGS[@]}")
            if [[ "${label}" == "ipc-net" ]]; then
                extra+=(--max-fd "${SNG_MAX_FD}")
            fi
            if ! timeout -s INT -k 30 "$((batch_secs + 120))" \
                nice -n "${SNG_NICE}" stress-ng "${flags[@]}" "${extra[@]}" --timeout "${batch_secs}s" \
                --temp-path "${SNG_TMP}" --metrics-brief >/dev/null 2>&1; then
                failed=$((failed + 1))
            fi
        fi
        i=$((i + batch))
        n=$((n + 1))
    done
    if ((failed == nbatches && DRY_RUN == 0)); then
        echo "warning: kernel group ${label}: every stress-ng batch failed" >&2
    fi
}

# ---------------------------------------------------------------------------
# Training workload: drive kernel code, not userspace math.
# ---------------------------------------------------------------------------

# Deliberately excluded, even though they touch kernel code: memory balloons
# (malloc, stack, bigheap), resource exhaustion (resources, loadavg, sockmany,
# epollmany, dirmany), scheduling-policy changes (schedmix, cpu-sched, nice),
# IPI storms (tlb-shootdown), detaching or unreaped processes (daemon, session,
# zombie, forkheavy) and fault injection (sigsegv, sysbadaddr, bad-altstack).
# They destabilise an interactive system and bias the profile toward error and
# reclaim paths rather than the hot paths worth optimising.
# Hot paths first: relative frequency is what AutoFDO uses. Equal time on umask
# and futex teaches the compiler they are equally hot. Unsupported names are
# dropped by sng_have. Sweet/long add a short fork/exec slice; long also adds
# io-uring. Bulk disk write stays in the fio composite, not stress-ng hdd/write.
# Excluded: malloc/stack/tlb-shootdown/loadavg/forkheavy/sockmany/dirmany/schedmix
# /zombie/daemon and fault injection — they livelock a desktop or bias error paths.
# sockpair/udp stay in ipc-net but run with SNG_NET_WORKERS (not nproc/2): each
# socket's skbuffs live in unreclaimable slab and will OOM a swapless box.
SNG_SYSCALL=(syscall get close fcntl dup)
SNG_SCHED=(switch futex pipeherd pthread)
SNG_IPCNET=(sock sockpair udp epoll pipe eventfd)
SNG_MM=(fault mmap munmap mprotect mremap)
SNG_VFS=(open dentry dentrycache stat fstat statx access lseek seek)
SNG_TAIL_BLOCK=(io-uring iomix)
SNG_TAIL_PROC=(fork exec)

# Training under perf. Same kernel-path mix past 60 min mostly re-counts the same
# edges; long sits at that ceiling. Sweet stays the default for normal runs.
KERNEL_TRAIN_CAP_SECS=3600

kernel_profile_budget() {
    case "${PROFILE}" in
        short|quick) echo 600 ;;                          # 10 min
        long|maximum|max|perfect) echo "${KERNEL_TRAIN_CAP_SECS}" ;;  # 60 min
        *) echo 1200 ;;                                   # sweet: 20 min
    esac
}

run_kernel_benchmark() {
    local budget="${ABS_PGO_KERNEL_SECS:-}"
    if ! [[ "${budget}" =~ ^[0-9]+$ ]] || ((budget == 0)); then
        budget="$(kernel_profile_budget)"
    fi
    if ((budget < 180)); then
        budget=180
    fi
    if ((budget > KERNEL_TRAIN_CAP_SECS)); then
        budget=${KERNEL_TRAIN_CAP_SECS}
    fi
    sng_load_supported
    if ! command -v stress-ng >/dev/null 2>&1; then
        echo "warning: stress-ng not in PATH — kernel training falls back to composite phase only" >&2
    fi

    echo "==> kernel-path training (${PROFILE}), budget ${budget}s"
    echo "    concurrency: <= ${SNG_WORKER_CAP} workers, ${SNG_BATCH} stressors per batch, nice ${SNG_NICE}"
    if ((DRY_RUN)); then
        echo "    DRY RUN — printing the plan only, nothing will be executed"
    else
        echo "    this saturates the machine; expect an unresponsive desktop until it finishes"
    fi
    mkdir -p "${SNG_TMP}"

    local half=$((NPROC / 2)) quarter=$((NPROC / 4))
    if ((half < 2)); then half=2; fi
    if ((quarter < 2)); then quarter=2; fi

    # ~80% synthetic hot paths, ~20% composite. Sweet/long spend 5% on fork/exec
    # (4 workers — not nproc, or clone storms livelock the box). Long adds 5%
    # io-uring/iomix; sweet already hits the block layer via fio.
    local proc_secs=0 block_secs=0
    case "${PROFILE}" in
        short|quick) ;;
        long|maximum|max|perfect)
            proc_secs=$((budget * 5 / 100))
            block_secs=$((budget * 5 / 100))
            ;;
        *)
            proc_secs=$((budget * 5 / 100))
            ;;
    esac
    local rest=$((budget - proc_secs - block_secs))
    local sng_budget=$((rest * 80 / 100))
    local comp_budget=$((rest - sng_budget))

    sng_group syscall "$((sng_budget * 22 / 100))" "${NPROC}" "${SNG_SYSCALL[@]}"
    sng_group sched "$((sng_budget * 22 / 100))" "${NPROC}" "${SNG_SCHED[@]}"
    sng_group ipc-net "$((sng_budget * 20 / 100))" "${SNG_NET_WORKERS}" "${SNG_IPCNET[@]}"
    if mm_stress_is_safe; then
        sng_group mm-fault "$((sng_budget * 18 / 100))" "${half}" "${SNG_MM[@]}"
    fi
    sng_group vfs-dcache "$((sng_budget * 18 / 100))" "${quarter}" "${SNG_VFS[@]}"
    if ((block_secs > 0)); then
        sng_group block-io "${block_secs}" 4 "${SNG_TAIL_BLOCK[@]}"
    fi
    if ((proc_secs > 0)); then
        sng_group proc-lifecycle "${proc_secs}" 4 "${SNG_TAIL_PROC[@]}"
    fi

    kernel_composite_phase "${comp_budget}"
    rm -rf "${SNG_TMP}" 2>/dev/null || true
    echo "==> kernel-path training workload finished"
}

# The mm group maps and faults memory in every worker. Skip it when free memory
# is already tight rather than pushing the box into reclaim or the OOM killer.
mm_stress_is_safe() {
    local avail_mb
    avail_mb="$(awk '/MemAvailable:/ { printf "%d", $2 / 1024; exit }' /proc/meminfo 2>/dev/null || true)"
    if [[ -z "${avail_mb}" ]]; then
        return 0
    fi
    if ((avail_mb < 2048)); then
        echo "==> kernel group mm-fault: only ${avail_mb} MiB available, skipping"
        return 1
    fi
    return 0
}

# Real-world kernel pressure: IPC, TCP loopback, a metadata walk, block I/O, and
# a sequential cc/clang loop (exec, loader mmap, page cache — not a kernel tree).
kernel_composite_phase() {
    local budget=$1
    if ((budget <= 0)); then
        return 0
    fi
    echo "==> composite kernel phase, budget ${budget}s"
    # Short: IPC + TCP only (highest sample density per second).
    # Sweet/long: metadata walk, buffered I/O, tiny compiles.
    local ipc tcp extra
    case "${PROFILE}" in
        short|quick)
            ipc=$((budget * 60 / 100))
            tcp=$((budget - ipc))
            extra=0
            ;;
        *)
            extra=$((budget * 40 / 100))
            ipc=$(((budget - extra) * 60 / 100))
            tcp=$((budget - extra - ipc))
            ;;
    esac
    if ((DRY_RUN)); then
        echo "    [dry run] hackbench/perf bench ${ipc}s, TCP loopback ${tcp}s"
        if ((extra > 0)); then
            echo "    [dry run] tar/find + fio + tiny cc ${extra}s"
        fi
        return 0
    fi

    composite_ipc_bench "${ipc}"
    composite_loopback_net "${tcp}"
    if ((extra > 0)); then
        local third=$((extra / 3))
        composite_archive_and_search "${third}"
        composite_block_io "${third}"
        composite_compile_loop "$((extra - third - third))"
    fi
}

# One compiler, one file, loop until the timer. Trains execve, the loader, and
# text-file page cache without a -jN kernel build.
composite_compile_loop() {
    local secs=$1
    local cc=""
    local c
    for c in clang cc gcc; do
        if command -v "${c}" >/dev/null 2>&1; then
            cc="${c}"
            break
        fi
    done
    if [[ -z "${cc}" ]]; then
        echo "==> composite: tiny compile skipped (no clang/cc/gcc)"
        return 0
    fi
    local dir="${WORKDIR}/.abs-cc"
    mkdir -p "${dir}"
    cat > "${dir}/t.c" << 'EOF'
int add(int a, int b) { return a + b; }
int main(void) { return add(1, 2) - 3; }
EOF
    echo "==> composite: tiny ${cc} compile (${secs}s cap, $(date +%H:%M:%S))"
    timeout -s INT -k 15 "${secs}" bash -c '
        cc=$1
        dir=$2
        while :; do
            "${cc}" -O2 -c "${dir}/t.c" -o "${dir}/t.o" || exit 0
            "${cc}" -O2 "${dir}/t.o" -o "${dir}/t" || exit 0
            "${dir}/t" >/dev/null 2>&1 || true
        done' _ "${cc}" "${dir}" >/dev/null 2>&1 || true
    rm -rf "${dir}" 2>/dev/null || true
}

composite_archive_and_search() {
    local secs=$1
    local dir="${WORKDIR}/.abs-arch"
    echo "==> composite: archive + metadata search (${secs}s cap, $(date +%H:%M:%S))"
    mkdir -p "${dir}"
    local half=$((secs / 2))
    if ((half < 5)); then
        half=5
    fi
    # tar reads thousands of small files (dcache, inode, readahead) and writes
    # back through the page cache; extraction hammers create/unlink.
    timeout -s INT -k 15 "${half}" bash -c '
        set -o pipefail
        while :; do
            tar -cf "$1/t.tar" -C /usr include 2>/dev/null || exit 0
            rm -rf "$1/x"; mkdir -p "$1/x"
            tar -xf "$1/t.tar" -C "$1/x" 2>/dev/null || exit 0
        done' _ "${dir}" >/dev/null 2>&1 || true
    timeout -s INT -k 15 "${half}" bash -c '
        while :; do
            find /usr/include /usr/lib -xdev -type f -printf "" 2>/dev/null
            find /usr/share -xdev -name "*.h" 2>/dev/null | head -n 20000 >/dev/null
        done' >/dev/null 2>&1 || true
    rm -rf "${dir}" 2>/dev/null || true
}

composite_block_io() {
    local secs=$1
    local dir="${WORKDIR}/.abs-io"
    mkdir -p "${dir}"
    if command -v fio >/dev/null 2>&1; then
        echo "==> composite: fio buffered + O_DIRECT (${secs}s cap, $(date +%H:%M:%S))"
        local per=$((secs / 2))
        if ((per < 5)); then
            per=5
        fi
        timeout -s INT -k 30 "$((per + 60))" fio --name=buffered --directory="${dir}" \
            --rw=randrw --bs=4k --size=512M --numjobs=4 --iodepth=32 --ioengine=psync \
            --runtime="${per}" --time_based --group_reporting >/dev/null 2>&1 || true
        timeout -s INT -k 30 "$((per + 60))" fio --name=direct --directory="${dir}" \
            --rw=randread --bs=4k --size=512M --numjobs=4 --iodepth=32 --direct=1 \
            --ioengine=libaio --runtime="${per}" --time_based --group_reporting \
            >/dev/null 2>&1 || true
    else
        echo "==> composite: dd read/write (fio not installed)"
        timeout -s INT -k 15 "${secs}" bash -c '
            while :; do
                dd if=/dev/zero of="$1/blob" bs=1M count=1024 conv=fsync 2>/dev/null || exit 0
                dd if="$1/blob" of=/dev/null bs=4k 2>/dev/null || exit 0
            done' _ "${dir}" >/dev/null 2>&1 || true
    fi
    rm -rf "${dir}" 2>/dev/null || true
}

composite_ipc_bench() {
    local secs=$1
    echo "==> composite: scheduler/IPC benchmarks (${secs}s cap, $(date +%H:%M:%S))"
    local half=$((secs / 2))
    if ((half < 5)); then
        half=5
    fi
    # Loop until timeout so a longer profile actually spends the budget.
    if command -v hackbench >/dev/null 2>&1; then
        timeout -s INT -k 20 "${half}" bash -c '
            while :; do
                hackbench -p -g 20 -l 4000 >/dev/null 2>&1 || exit 0
                hackbench -s 512 -g 10 -l 4000 >/dev/null 2>&1 || exit 0
            done' >/dev/null 2>&1 || true
    else
        half=0
    fi
    if command -v perf >/dev/null 2>&1; then
        local rest=$((secs - half))
        if ((rest < 5)); then
            rest=5
        fi
        timeout -s INT -k 20 "${rest}" bash -c '
            while :; do
                perf bench sched messaging -l 3000 -g 20 >/dev/null 2>&1 || true
                perf bench sched pipe -l 500000 >/dev/null 2>&1 || true
                perf bench syscall basic -l 3000000 >/dev/null 2>&1 || true
                perf bench futex hash -r 3 >/dev/null 2>&1 || true
                perf bench epoll wait -r 3 >/dev/null 2>&1 || true
            done' >/dev/null 2>&1 || true
    fi
}

composite_loopback_net() {
    local secs=$1
    if ! command -v socat >/dev/null 2>&1; then
        return 0
    fi
    echo "==> composite: TCP loopback transfer (${secs}s cap, $(date +%H:%M:%S))"
    # Unprivileged high port; both ends local so this exercises the loopback
    # TCP send/receive path without touching a real NIC.
    local port=$((20000 + RANDOM % 20000))
    timeout -s INT -k 15 "${secs}" bash -c '
        port=$1
        socat -u TCP-LISTEN:"${port}",reuseaddr,fork /dev/null >/dev/null 2>&1 &
        listener=$!
        trap "kill ${listener} 2>/dev/null" EXIT
        sleep 1
        while :; do
            head -c 268435456 /dev/zero | socat -u - TCP:127.0.0.1:"${port}" >/dev/null 2>&1 || exit 0
        done' _ "${port}" >/dev/null 2>&1 || true
}

# ---------------------------------------------------------------------------
# Scoring workload: kernel-sensitive metrics in benchie_*.log format.
#
# Userspace compute (y-cruncher, blender, ffmpeg) barely moves with a faster
# kernel. These metrics sit on syscall, scheduler, VFS and network paths, so
# kernel gains are actually visible. Names must match src/pgo_scraper.rs.
# ---------------------------------------------------------------------------

KB_OUT=""
KB_REPEATS=1

kb_emit() {
    printf '%s: %s\n' "$1" "$2" >> "${KB_OUT}"
    printf '    %-28s %s\n' "$1" "$2"
}

kb_scale() {
    awk -v v="$1" -v d="$2" 'BEGIN { if (d == 0) d = 1; printf "%.3f", v / d }'
}

# One stressor, reported as mean bogo-ops/s over ABS_PGO_KBENCH_REPEATS runs.
kb_sng() {
    local label=$1 stressor=$2 workers=$3 secs=$4 div=${5:-1000}
    if ! sng_have "${stressor}"; then
        return 0
    fi
    mkdir -p "${SNG_TMP}"
    local yaml="${SNG_TMP}/kb-${stressor}.yaml"
    local -a rates=()
    local i rate
    for ((i = 0; i < KB_REPEATS; i++)); do
        rm -f "${yaml}"
        local -a extra=("${SNG_OOM_FLAGS[@]}")
        case "${stressor}" in
            sock|sockpair|udp|epoll) extra+=(--max-fd "${SNG_MAX_FD}") ;;
        esac
        timeout -s INT -k 20 "$((secs + 90))" \
            nice -n "${SNG_NICE}" stress-ng --"${stressor}" "${workers}" "${extra[@]}" \
            --timeout "${secs}s" --temp-path "${SNG_TMP}" --metrics-brief --yaml "${yaml}" \
            >/dev/null 2>&1 || true
        rate="$(awk '/bogo-ops-per-second-real-time:/ { print $2; exit }' "${yaml}" 2>/dev/null || true)"
        if [[ -n "${rate}" ]]; then
            rates+=("${rate}")
        fi
    done
    rm -f "${yaml}"
    if ((${#rates[@]} == 0)); then
        return 0
    fi
    rate="$(awk -v d="${div}" 'BEGIN { s=0; n=0 } { s+=$1; n++ } END { if (n==0) exit 1; printf "%.3f", (s/n)/d }' <<< "$(printf '%s\n' "${rates[@]}")")"
    kb_emit "${label}" "${rate}"
}

# `perf bench` reports either "N ops/sec" or "Averaged N operations/sec".
kb_perf_ops() {
    local label=$1 div=$2
    shift 2
    if ! command -v perf >/dev/null 2>&1; then
        return 0
    fi
    local out ops
    out="$(timeout -s INT -k 20 180 perf bench "$@" 2>&1 || true)"
    ops="$(awk '
        /Averaged .* operations\/sec/ { gsub(/,/, "", $2); v = $2 }
        / ops\/sec/ && !/\[/ { gsub(/,/, "", $1); if (v == "") v = $1 }
        END { if (v != "") print v }' <<< "${out}")"
    if [[ -z "${ops}" ]]; then
        return 0
    fi
    kb_emit "${label}" "$(kb_scale "${ops}" "${div}")"
}

kb_perf_seconds() {
    local label=$1
    shift
    if ! command -v perf >/dev/null 2>&1; then
        return 0
    fi
    local out secs
    out="$(timeout -s INT -k 20 180 perf bench "$@" 2>&1 || true)"
    secs="$(awk '/Total time:/ { print $3; exit }' <<< "${out}")"
    if [[ -z "${secs}" ]]; then
        return 0
    fi
    kb_emit "${label}" "${secs}"
}

kb_hackbench() {
    if ! command -v hackbench >/dev/null 2>&1; then
        return 0
    fi
    local out secs
    out="$(timeout -s INT -k 20 240 hackbench -p -g 20 -l 8000 2>&1 || true)"
    secs="$(awk '/^Time:/ { print $2; exit }' <<< "${out}")"
    if [[ -z "${secs}" ]]; then
        return 0
    fi
    kb_emit "hackbench pipes (s)" "${secs}"
}

collect_kernel_metrics() {
    local secs="${ABS_PGO_KBENCH_SECS:-}"
    KB_REPEATS="${ABS_PGO_KBENCH_REPEATS:-}"
    if ! [[ "${secs}" =~ ^[0-9]+$ ]] || ((secs < 1)); then
        case "${PROFILE}" in
            short|quick) secs=8 ;;
            long|maximum|max|perfect) secs=12 ;;
            *) secs=8 ;;
        esac
    fi
    if ! [[ "${KB_REPEATS}" =~ ^[0-9]+$ ]] || ((KB_REPEATS < 1)); then
        case "${PROFILE}" in
            short|quick) KB_REPEATS=1 ;;
            *) KB_REPEATS=3 ;;
        esac
    fi
    local half=$((NPROC / 2))
    if ((half < 2)); then
        half=2
    fi
    local net=${SNG_NET_WORKERS}
    sng_load_supported
    echo "==> kernel micro-benchmarks (${secs}s × ${KB_REPEATS} run(s) per metric)"
    if ((DRY_RUN)); then
        echo "    [dry run] perf bench syscall/sched/futex/epoll, hackbench, and" \
            "stress-ng switch/fault/mmap/open/dentry/sockpair/udp/pipe/exec/io-uring/iomix"
        echo "    sockpair/udp workers: ${net} (capped; kernel skbuffs are not process RSS)"
        return 0
    fi

    kb_perf_ops "syscall getppid (Mops/s)" 1000000 syscall basic -l 5000000
    kb_perf_ops "sched pipe (Kops/s)" 1000 sched pipe -l 500000
    kb_perf_seconds "sched messaging (s)" sched messaging -l 5000 -g 20
    kb_perf_ops "futex hash (Kops/s)" 1000 futex hash -r "${KB_REPEATS}"
    kb_perf_ops "epoll wait (Kops/s)" 1000 epoll wait -r "${KB_REPEATS}"
    kb_hackbench

    kb_sng "context switch (Kops/s)" switch "${NPROC}" "${secs}"
    kb_sng "page fault (Kops/s)" fault "${half}" "${secs}"
    kb_sng "mmap/munmap (Kops/s)" mmap "${half}" "${secs}"
    kb_sng "vfs open/close (Kops/s)" open "${half}" "${secs}"
    kb_sng "dentry lookup (Kops/s)" dentry "${half}" "${secs}"
    kb_sng "unix socket (Kops/s)" sockpair "${net}" "${secs}"
    kb_sng "udp loopback (Kops/s)" udp "${net}" "${secs}"
    kb_sng "pipe throughput (Kops/s)" pipe "${half}" "${secs}"
    kb_sng "fork+exec (ops/s)" exec 4 "${secs}" 1
    kb_sng "io_uring (Kops/s)" io-uring 4 "${secs}"
    kb_sng "buffered file io (Kops/s)" iomix 4 "${secs}"
}

# Standalone scored run: emit a benchie_*.log the ABS scraper can chart.
run_kbench() {
    local label="${ABS_PGO_COMPARE_LABEL:-abs-kbench}"
    local stamp
    stamp="$(date +%Y%m%d-%H%M%S)"
    KB_OUT="${WORKDIR}/.abs-kbench-${stamp}.txt"
    : > "${KB_OUT}"
    collect_kernel_metrics
    if ((DRY_RUN)); then
        rm -f "${KB_OUT}"
        return 0
    fi
    if [[ ! -s "${KB_OUT}" ]]; then
        echo "error: no kernel metrics collected (need perf and/or stress-ng)" >&2
        rm -f "${KB_OUT}"
        return 1
    fi
    local log="${WORKDIR}/benchie_${label}_kbench-${stamp}.log"
    {
        printf 'Kernel: %s\n' "$(uname -r)"
        printf 'SCX Scheduler: none\n'
        printf 'SCX Version: none\n'
        cat "${KB_OUT}"
    } > "${log}"
    rm -f "${KB_OUT}" "${SNG_TMP}"/kb-*.yaml 2>/dev/null || true
    rm -rf "${SNG_TMP}" 2>/dev/null || true
    echo "==> kernel metrics written to ${log##*/}"
}

case "${MODE}" in
    kernel|"") run_kernel_benchmark ;;
    kbench) run_kbench ;;
    warmup) run_cpu_warmup ;;
    *)
        echo "error: unknown ABS_PGO_BENCHMARK='${MODE}' (use kernel, kbench, or warmup)" >&2
        exit 2
        ;;
esac

echo "All tests completed."
