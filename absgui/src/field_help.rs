//! Short explanations shown under form fields (kernel + ABS settings pages).

// --- Kernel build (CachyOS PKGBUILD env) ---

pub const CPUSCHED: &str =
    "Kernel scheduler flavour passed as _cpusched to the CachyOS PKGBUILD (e.g. bore, eevdf, rt).";
pub const PROCESSOR_OPT: &str =
    "CPU micro-architecture level for -march (native or x86-64-v2/v3/v4). Maps to _processor_opt.";
pub const LLVM_LTO: &str =
    "Compiler/LTO (_use_llvm_lto): none = GCC build, thin/full = Clang with LTO. \
     Applies to one-shot builds; PGO runs force their own per-stage value.";
pub const HZ_TICKS: &str = "Timer interrupt frequency in Hz (_HZ_ticks). Affects latency vs power.";
pub const TICKRATE: &str =
    "Tickless behaviour: full (always tickless), idle, or periodic. Maps to _tickrate.";
pub const PREEMPT: &str =
    "Kernel preemption model (full, voluntary, server, lazy). Maps to _preempt.";
pub const HUGEPAGE: &str =
    "Transparent hugepage policy: always or madvise. Maps to _hugepage.";
pub const CC_HARDER: &str =
    "Enable stricter compiler flags in the PKGBUILD (_cc_harder). May increase build time.";
pub const LTO_SUFFIX: &str =
    "Append -lto to the package name (_use_lto_suffix). One-shot builds only; PGO stages manage suffixes themselves.";
pub const GCC_SUFFIX: &str =
    "Append -gcc to the package name when building with GCC (_use_gcc_suffix).";
pub const KCFI: &str =
    "Enable kernel Control Flow Integrity (_use_kcfi). Requires a Clang/LLVM build.";

// --- Per-package ABS build ---

pub const SOURCE: &str =
    "Where ABS fetches the PKGBUILD: aur, cachyos, or arch official repos.";
pub const BUILD_ENV: &str =
    "Build environment: local (host makepkg) or chroot (isolated rootfs).";
pub const PACKAGE_ALIAS: &str =
    "Alternate upstream package name checked for updates (e.g. the repo package this AUR package tracks).";
pub const PACKAGE_COMPILER: &str =
    "Named compiler set from [compilers] used for this package; empty = default_compiler / makepkg default.";
pub const PACKAGE_TESTS: &str =
    "Run the PKGBUILD check() phase. Default follows makepkg; false passes --nocheck.";
pub const PACKAGE_UPSTREAM_GITHUB: &str =
    "GitHub owner/repo (or URL) checked on -R/-RU when the PKGBUILD lags behind upstream releases.";
pub const PACKAGE_UPSTREAM_PRERELEASES: &str =
    "Also consider GitHub prereleases when choosing the newest upstream version.";
pub const PACKAGE_PRE_UPDATE_CMD: &str =
    "Shell command run before this package is built (e.g. stop a service).";
pub const PACKAGE_POST_UPDATE_CMD: &str =
    "Shell command run after this package is installed (e.g. restart a service).";
pub const PACKAGE_CUSTOM_LOCAL_CMD: &str =
    "Replaces the makepkg invocation for local builds. Runs in the package directory; empty = default makepkg.";
pub const PACKAGE_CUSTOM_CHROOT_CMD: &str =
    "Replaces the makechrootpkg invocation for chroot builds. Runs in the package directory; empty = default.";
pub const RAMDISK_TARGETS: &str =
    "Place parts of the build on tmpfs (ramdisk). Letters combine into the ramdisk= value in abs.toml.";
pub const RAMDISK_W: &str =
    "Build workdir (src/, pkg/) on tmpfs — speeds compile I/O for large trees like the kernel.";
pub const RAMDISK_C: &str = "Chroot root filesystem on tmpfs — faster package installs inside chroot.";
pub const RAMDISK_P: &str =
    "Git clone / extract under packages_path on tmpfs. Uses a lot of RAM; enable only if you have spare memory.";
pub const RAMDISK_R: &str =
    "PGO profile collection scratch (perf.data) on tmpfs; profiles are still copied to profiles_archive_dir after each stage.";

pub const KERNEL_RAMDISK_TARGETS: &str =
    "Kernel/PGO ramdisk options. PKGBUILD and source tarballs stay on disk unless repo-on-ramdisk (p) is enabled.";
pub const KERNEL_RAMDISK_W: &str =
    "Compile on ramdisk: git repo and downloads stay on disk; src/ and pkg/ (extracted tree) use tmpfs.";
pub const KERNEL_RAMDISK_P: &str =
    "Repo on ramdisk: clone the full git tree on tmpfs (re-downloads on each PGO stage — not recommended).";
pub const KERNEL_RAMDISK_R: &str =
    "Write perf.data to tmpfs during profiling; cachyos-benchmarker downloads are cached on disk under profiles_archive_dir/benchmark-workdir.";
pub const PGO_BENCHMARK_WORKDIR: &str =
    "Optional persistent cache for cachyos-benchmarker downloads. Default: {profiles_archive_dir}/benchmark-workdir.";

// --- PGO ---

pub const PGO_ENABLED: &str =
    "Run the multi-stage AutoFDO + Propeller pipeline instead of a single build when using abs --pgo.";
pub const PGO_ARCHIVE_DIR: &str =
    "Required. Directory where PGO profile archives and intermediate artifacts are stored between stages and reboots.";
pub const PGO_BENCHMARK: &str =
    "Optional. Override the default ABS profiling script. Leave empty to use the bundled script (refreshed on each run).";
pub const PGO_BENCHMARK_PRESET: &str =
    "Profiling workload when quality is standard: fast = sysbench + stress-ng; \
     cachyos = full cachyos-benchmarker. Maximum quality always uses cachyos.";
pub const PGO_PROFILING_QUALITY: &str =
    "standard = balanced sampling (-c 56000) and your benchmark preset. \
     maximum (default) = densest sampling (-c 48000) + full cachyos-benchmarker for llvm-profgen-quality profiles.";
pub const PGO_BUILD_USER: &str =
    "Unix user that runs profiling workloads after reboot (must match the user in your benchmark script).";
pub const PGO_SYSCTL: &str =
    "Command invoked before profiling to tune sysctl knobs (e.g. cachyos-perf-sysctl).";
pub const PGO_AUTO_RESTART: &str =
    "abs reboots at each PGO wait stage and resumes after boot via a user systemd unit until the pipeline finishes. \
     absgui passes --pgo-auto; requires linger or a graphical login for user systemd.";
pub const PGO_PRESET: &str =
    "PGO pipeline preset name (usually cachyos-kernel). Reserved for future pipeline variants.";
pub const PGO_PROFILE_SCRATCH: &str =
    "Directory for perf.data during profiling. auto = ramdisk when enabled, else a temp path under /tmp or the archive dir.";
pub const PGO_PERF_DATA_ON_RAM: &str =
    "Write perf.data to tmpfs (ramdisk profile scratch) during profiling; copies to the repo/archive after each stage.";
pub const PGO_VERIFY_BOOT: &str =
    "After reboot wait stages, verify uname -r matches the expected kernel before continuing the pipeline.";
pub const PGO_PERF_EVENT_ARGS: &str =
    "perf record event arguments. auto = detect Zen (--pfm-events) or Intel (BR_INST_RETIRED.NEAR_TAKEN) branch events from gcc -march=native.";
pub const PGO_PERF_EXTRA_ARGS: &str =
    "perf record flags after events (mmap-pages, -a, -N, -b, -c). profiling_quality overrides -c unless you set a custom value here.";
pub const PGO_VMLINUX: &str =
    "Path to DWARF vmlinux for llvm-profgen. auto = /usr/src/debug/<pkgbase>/vmlinux from the installed -dbg package.";
pub const PGO_AFDO_TOOL: &str = "Profile conversion tool for stage 2 (default llvm-profgen).";
pub const PGO_PROPELLER_TOOL: &str = "Profile conversion tool for stage 3 (default create_llvm_prof).";
pub const PGO_AFDO_PROFILE_NAME: &str =
    "Filename of the AutoFDO profile copied into the kernel source tree (default kernel-compilation.afdo).";
pub const PGO_STATE_FILE: &str =
    "Optional override for PGO pipeline JSON state. Default: ~/.config/abs/pgo/<package>.json";

// --- ABS global paths ---

pub const PATH_PACKAGES: &str =
    "Directory where ABS stores cloned sources and built package trees.";
pub const PATH_CHROOT: &str = "Base directory for chroot build roots (one subfolder per environment).";
pub const PATH_READY: &str =
    "Directory for pre-built .pkg.tar.zst files installed without recompiling.";
pub const PATH_CHROOT_MAKEPKG: &str =
    "Optional makepkg.conf copied into chroots; leave empty to use the chroot default.";

// --- ABS build section ---

pub const DEFAULT_ENV: &str = "Default build environment when a package does not set build_env.";
pub const DEFAULT_COMPILER: &str =
    "Default compiler set name from [compilers]; leave empty to use makepkg defaults.";
pub const CONCURRENT_REPOS: &str = "Maximum parallel repository/package downloads.";
pub const CONCURRENT_COMPILATIONS: &str = "Maximum parallel compile jobs across packages.";
pub const SYSTEM_UPDATE_FIRST: &str =
    "Run a system update before starting a build batch (-R / -RU).";
pub const IGNORE_FAILURES: &str =
    "Continue building other packages if one package fails to compile.";
pub const COMPILE_FIRST_INSTALL: &str =
    "After a successful compile, install the package before moving to dependents.";
pub const CLEAN_INSTALL_DEFAULT: &str =
    "Default for clean install (-C) when not specified on the command line.";
pub const IGNORE_ALREADY_MADE: &str =
    "Always rebuild even when a ready package at the PKGBUILD version (or newer) already exists. When off, skip compilation and reuse those artifacts (unless -n). Stale older ready packages do not skip a rebuild.";
pub const FAST_AUR_RPC: &str =
    "Use fast AUR RPC checks for update detection (fewer requests, may miss edge cases).";
pub const CLEAN_CHROOT_AFTER: &str =
    "Remove chroot build artifacts after each successful chroot compilation.";
pub const GLOBAL_CPU_THREADS_MODE: &str =
    "CPU thread cap mode: strict (hard cap) or flexible (soft cap with optional burst ceiling).";
pub const GLOBAL_CPU_THREADS_CAP: &str =
    "Max sum of active compilation threads across concurrent builds (strict hard cap; flexible soft target).";
pub const MAXIMUM_CPU_THREADS_CAP: &str =
    "Flexible mode only: hard ceiling when pairing exceeds the soft cap.";
pub const DEFAULT_COMPILATION_THREADS: &str =
    "Default -j for packages without per-package compilation_threads (overridable with abs -j).";
pub const PACKAGE_COMPILATION_THREADS: &str =
    "Per-package -j thread count (sacred; scheduler never reduces it).";
pub const PACKAGE_COMPILE_ALONE: &str =
    "When true, this package runs with no other package compiling at the same time.";
pub const PACKAGE_COMPILATION_PRIORITY: &str =
    "Higher value schedules this package earlier among ready builds.";

// --- Self-update & startup ---

pub const CHECK_UPDATE_STARTUP: &str = "Check for a new ABS release when abs starts.";
pub const AUTO_UPDATE_STARTUP: &str = "Automatically install a new ABS release on startup when found.";
pub const SELF_UPDATE_AT_UPDATES: &str =
    "Also check for ABS updates when running repository update commands.";
pub const INSTALL_TESTING: &str =
    "Allow installing Arch testing/staging packages when resolving dependencies.";
pub const SELF_UPDATE_RAW: &str =
    "Raw URL of Cargo.toml checked for the latest version string.";
pub const SELF_UPDATE_INSTALL: &str = "Fallback path for the abs binary when not using pacman packages (legacy manual install).";
pub const SELF_UPDATE_USE_PACMAN: &str =
    "When true, --self-update builds aur/PKGBUILD and upgrades pacman packages. When false, copies the abs binary only. Default (auto) detects installed abs/absgui packages.";

// --- Package lists ---

pub const MANUAL_UPDATE: &str =
    "Packages always rebuilt on -R/-RU even when ABS thinks they are up to date.";
pub const SKIP_INSTALL: &str =
    "Packages compiled but not installed to the live system. Supports globs such as qemu*.";
pub const SKIP_INSTALL_AFTER: &str =
    "Separate skip-install list used only after compilation in a batch run. Supports globs such as qemu*.";
pub const USE_SEPARATE_SKIP_AFTER: &str =
    "Maintain a distinct skip_install_packages_after_compilation list in abs.toml.";

// --- System update ---

pub const SYS_REPOS_CMD: &str = "Shell command to refresh package databases (e.g. pacman -Sy).";
pub const SYS_FULL_CMD: &str = "Shell command for a full system upgrade (e.g. pacman -Syu).";
pub const SYS_NO_REFRESH_CMD: &str =
    "Upgrade without refreshing databases; derived from full command if empty.";
pub const SYS_IGNORE_FLAG: &str = "Flag passed to pacman to skip specific packages during upgrade.";
pub const SYS_IGNORE_PACKAGES: &str =
    "Package names excluded from system upgrades (paired with ignore_flag). Supports globs such as qemu*.";

// --- Ramdisk (global) ---

pub const RAMDISK_ENABLED: &str =
    "Master switch for tmpfs mounts configured below and per-package ramdisk targets.";
pub const RAMDISK_MOUNT: &str = "Filesystem path where the tmpfs ramdisk is mounted.";
pub const RAMDISK_SIZE: &str = "Tmpfs size limit (e.g. 16G). Passed to mount -o size=.";
pub const RAMDISK_MODE: &str = "Directory permissions mode for the mount point (octal).";
pub const RAMDISK_GLOBAL_W: &str = "Default: put build workdirs (src/, pkg/) on tmpfs for packages that use ramdisk w.";
pub const RAMDISK_GLOBAL_C: &str = "Default: put chroot rootfs on tmpfs for packages that use ramdisk c.";
pub const RAMDISK_GLOBAL_P: &str =
    "Default: clone sources under packages_path on tmpfs for packages that use ramdisk p.";
pub const RAMDISK_SEED: &str =
    "Optional existing chroot directory copied to seed a fresh ramdisk chroot.";
pub const RAMDISK_SYNC: &str =
    "Sync ramdisk chroot contents back to disk when the build finishes.";
pub const RAMDISK_MIN_FREE: &str =
    "Minimum free RAM (MiB) required before mounting; ABS aborts if below this.";
pub const RAMDISK_WARN_PACKAGES: &str =
    "Warn when packages_path on tmpfs may exceed available RAM.";
pub const RAMDISK_RECLAIM: &str =
    "Unmount an existing mount at mount_point before mounting (recover from crashed runs).";

// --- Repositories & compilers ---

pub const REPO_URL: &str = "Pacman repository URL for this named repo.";
pub const COMPILER_CC: &str = "C compiler executable for this named compiler set.";
pub const COMPILER_CXX: &str = "C++ compiler executable for this named compiler set.";
