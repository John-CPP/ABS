use crate::die;
use crate::utils::{run_command, sh_single_quote};
use colored::Colorize;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default = "default_config_version")]
    pub config_version: u32,
    pub paths: PathsConfig,
    pub build: BuildConfig,
    pub system_update: SystemUpdateConfig,
    pub repositories: HashMap<String, String>,
    pub manual_update_packages: Vec<String>,
    /// Packages excluded from system update commands (repo binaries).
    pub skip_install_packages: Vec<String>,
    /// Packages excluded from `pacman -U` after manual compilation. When unset, falls back to
    /// `skip_install_packages` for backward compatibility with older configs.
    #[serde(default)]
    pub skip_install_packages_after_compilation: Option<Vec<String>>,
    pub packages: HashMap<String, PackageConfig>,
    #[serde(default = "default_check_for_update_on_startup")]
    pub check_for_update_on_startup: bool,
    #[serde(default = "default_auto_update_on_startup")]
    pub auto_update_on_startup: bool,
    #[serde(default = "default_self_update_raw_url")]
    pub self_update_raw_url: String,
    #[serde(default = "default_self_update_install_path")]
    pub self_update_install_path: String,
    /// When `true`, `--self-update` builds `aur/PKGBUILD` and installs with pacman.
    /// When `false`, copies the compiled `abs` binary to `self_update_install_path`.
    /// Default (`null`/unset): auto-detect from installed pacman packages.
    #[serde(default)]
    pub self_update_use_pacman: Option<bool>,
    #[serde(default = "default_self_update_at_updates")]
    pub self_update_at_updates: bool,
    #[serde(default = "default_install_testing_phase_archlinux_packages")]
    pub install_testing_phase_archlinux_packages: bool,
    #[serde(default)]
    pub compilers: HashMap<String, CompilerConfig>,
    #[serde(default)]
    pub ramdisk: RamdiskConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CompilerConfig {
    pub cc: String,
    pub cxx: String,
}

fn default_config_version() -> u32 {
    1
}

fn default_check_for_update_on_startup() -> bool {
    true
}

fn default_auto_update_on_startup() -> bool {
    false
}

fn default_self_update_at_updates() -> bool {
    false
}

fn default_install_testing_phase_archlinux_packages() -> bool {
    false
}

fn default_self_update_install_path() -> String {
    "/usr/bin/abs".to_string()
}

fn default_self_update_raw_url() -> String {
    "https://raw.githubusercontent.com/John-CPP/ABS/HEAD/Cargo.toml".to_string()
}

fn default_ramdisk_enabled() -> bool {
    false
}

fn default_ramdisk_mount_point() -> String {
    "/run/abs-ram".to_string()
}

fn default_ramdisk_size() -> String {
    "16G".to_string()
}

fn default_ramdisk_mode() -> String {
    "0755".to_string()
}

fn default_ramdisk_build_workdir() -> bool {
    false
}

fn default_ramdisk_chroot() -> bool {
    false
}

fn default_ramdisk_min_free_ram_mb() -> u64 {
    4096
}

fn default_ramdisk_warn_packages_ram() -> bool {
    true
}

fn default_ramdisk_reclaim_mount_on_startup() -> bool {
    true
}

/// Optional tmpfs/ramdisk for chroot rootfs and per-package build workdirs (`src/`, `pkg/`).
#[derive(Debug, Deserialize, Clone)]
pub struct RamdiskConfig {
    #[serde(default = "default_ramdisk_enabled")]
    pub enabled: bool,
    #[serde(default = "default_ramdisk_mount_point")]
    pub mount_point: String,
    #[serde(default = "default_ramdisk_size")]
    pub size: String,
    #[serde(default = "default_ramdisk_mode")]
    pub mode: String,
    #[serde(default = "default_ramdisk_build_workdir")]
    pub build_workdir: bool,
    #[serde(default = "default_ramdisk_chroot")]
    pub chroot: bool,
    #[serde(default)]
    pub packages: bool,
    #[serde(default)]
    pub seed_chroot_from: Option<String>,
    #[serde(default)]
    pub sync_chroot_on_exit: bool,
    #[serde(default = "default_ramdisk_min_free_ram_mb")]
    pub min_free_ram_mb: u64,
    #[serde(default = "default_ramdisk_warn_packages_ram")]
    pub warn_packages_ram: bool,
    /// Unmount `mount_point` before mounting when it is already mounted (e.g. after a crashed ABS run).
    #[serde(default = "default_ramdisk_reclaim_mount_on_startup")]
    pub reclaim_mount_on_startup: bool,
}

impl Default for RamdiskConfig {
    fn default() -> Self {
        Self {
            enabled: default_ramdisk_enabled(),
            mount_point: default_ramdisk_mount_point(),
            size: default_ramdisk_size(),
            mode: default_ramdisk_mode(),
            build_workdir: default_ramdisk_build_workdir(),
            chroot: default_ramdisk_chroot(),
            packages: false,
            seed_chroot_from: None,
            sync_chroot_on_exit: false,
            min_free_ram_mb: default_ramdisk_min_free_ram_mb(),
            warn_packages_ram: default_ramdisk_warn_packages_ram(),
            reclaim_mount_on_startup: default_ramdisk_reclaim_mount_on_startup(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct PathsConfig {
    pub packages_path: String,
    pub chroot_base_path: String,
    pub ready_made_packages_path: String,
    #[serde(default)]
    pub chroot_makepkg_conf: Option<String>,
}

fn default_concurrent_repos_downloads_limit() -> usize {
    10
}

fn default_concurrent_compilations_limit() -> usize {
    1
}

fn default_fast_aur_rpc_update_checks() -> bool {
    true
}

fn default_system_update_first() -> bool {
    true
}

fn default_clean_chroot_after_compilation() -> bool {
    true
}

fn default_global_cpu_threads_mode() -> String {
    "strict".to_string()
}

fn default_compilation_priority() -> usize {
    1
}

#[derive(Debug, Deserialize)]
pub struct BuildConfig {
    pub default_environment: String,
    /// Continue with the next package when a build fails instead of exiting.
    #[serde(default, alias = "IGNORE_COMPILATION_FAILURES")]
    pub ignore_compilation_failures: bool,
    /// Build every scheduled package first, then run install prompts (so long unattended compile runs finish before any questions).
    #[serde(default, alias = "COMPILE_FIRST_INSTALL_AFTER")]
    pub compile_first_install_after: bool,
    /// Before **`makepkg`**, remove **`src/`** and **`pkg/`** in the package directory. **`--clean-install`** enables the same for that invocation even when this is false.
    #[serde(default)]
    pub clean_install_by_default: bool,
    /// When true, always rebuild even if version-matching artifacts already exist in
    /// `ready_made_packages_path`. When false (default), skip compilation and reuse those
    /// artifacts (still install unless `-o`). Overridden by `-n` and per-package settings.
    #[serde(default)]
    pub ignore_already_made_packages: bool,
    /// Maximum number of repository directories to sync concurrently.
    #[serde(default = "default_concurrent_repos_downloads_limit")]
    pub concurrent_repos_downloads_limit: usize,
    /// Maximum number of clean chroot compilations to run concurrently.
    #[serde(default = "default_concurrent_compilations_limit")]
    pub concurrent_compilations_limit: usize,
    /// Whether to check AUR package versions using the AUR RPC API in batch.
    #[serde(default = "default_fast_aur_rpc_update_checks")]
    pub fast_aur_rpc_update_checks: bool,
    #[serde(default)]
    pub default_compiler: Option<String>,
    /// Perform system update before compiling packages (highly recommended to prevent broken shared libraries).
    #[serde(default = "default_system_update_first")]
    pub system_update_first: bool,
    /// After each chroot (`makechrootpkg`) build, remove the per-build working copy and reset the
    /// devtools chroot when no other chroot build is running (avoids unbounded chroot growth).
    #[serde(default = "default_clean_chroot_after_compilation")]
    pub clean_chroot_after_compilation: bool,
    /// How concurrent compilation thread sums are capped: `"strict"` or `"flexible"`.
    #[serde(default = "default_global_cpu_threads_mode")]
    pub global_cpu_threads_mode: String,
    /// Strict: hard max sum of active threads. Flexible: soft pairing target.
    #[serde(default)]
    pub global_cpu_threads_cap: Option<usize>,
    /// Flexible mode only: hard ceiling for concurrent thread sum.
    #[serde(default)]
    pub maximum_cpu_threads_cap: Option<usize>,
    /// Fallback `-j` for packages without per-package `compilation_threads`.
    #[serde(default)]
    pub default_compilation_threads: Option<usize>,

    // Optional self-update fields for backwards-compatibility/placement under [build]
    pub check_for_update_on_startup: Option<bool>,
    pub auto_update_on_startup: Option<bool>,
    pub self_update_at_updates: Option<bool>,
    pub self_update_raw_url: Option<String>,
    pub self_update_install_path: Option<String>,
    pub install_testing_phase_archlinux_packages: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct SystemUpdateConfig {
    /// Shown with **`-R`** / **`-U`** (no full refresh). TOML key: `command_to_update_repositories`
    /// (alias: `command`).
    #[serde(alias = "command")]
    pub command_to_update_repositories: String,
    /// Shown with **`-RU`**. TOML key: `command_to_perform_system_update` (alias: `command_with_refresh`).
    #[serde(alias = "command_with_refresh")]
    pub command_to_perform_system_update: String,
    /// Shown with **`-RU`** (after initial refresh has run). TOML key: `command_to_perform_system_update_no_refresh` (alias: `command_no_refresh`).
    #[serde(default)]
    #[serde(alias = "command_no_refresh")]
    pub command_to_perform_system_update_no_refresh: Option<String>,
    pub ignore_flag: String,
    pub ignore_packages: Vec<String>,
}

impl SystemUpdateConfig {
    pub fn get_command_to_perform_system_update_no_refresh(&self) -> String {
        if let Some(cmd) = &self.command_to_perform_system_update_no_refresh {
            cmd.clone()
        } else {
            let with_refresh = &self.command_to_perform_system_update;
            if with_refresh.contains("-Syu") {
                with_refresh.replace("-Syu", "-Su")
            } else if with_refresh.contains("-Sy") {
                with_refresh.replace("-Sy", "-S")
            } else {
                with_refresh.clone()
            }
        }
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct PackageConfig {
    pub source: Option<String>,
    pub build_env: Option<String>,
    pub tests: Option<bool>,
    pub alias: Option<String>,
    pub custom_local_build_command: Option<String>,
    pub custom_chroot_build_command: Option<String>,
    pub pre_update_command: Option<String>,
    pub post_update_command: Option<String>,
    /// GitHub `owner/repo` (or `https://github.com/owner/repo`) checked on **`-R`** / **`-RU`** when
    /// the AUR (or other) PKGBUILD lags behind upstream releases.
    #[serde(default)]
    pub upstream_github: Option<String>,
    /// When true, consider GitHub prereleases when choosing the newest upstream version.
    #[serde(default)]
    pub upstream_prereleases: bool,
    pub compiler: Option<String>,
    /// Per-package ramdisk targets: `w` = build workdir, `c` = chroot, `p` = packages (e.g. `"wcp"`).
    #[serde(default)]
    pub ramdisk: Option<String>,
    /// CachyOS kernel PKGBUILD env overrides (maps to `_cpusched`, `_processor_opt`, etc.).
    #[serde(default)]
    pub kernel: Option<KernelBuildConfig>,
    /// Multi-stage kernel PGO pipeline (AutoFDO + Propeller).
    #[serde(default)]
    pub pgo: Option<PgoConfig>,
    /// Sacred per-package `-j` thread count (never reduced by the scheduler).
    #[serde(default)]
    pub compilation_threads: Option<usize>,
    /// When true, no other package compiles concurrently with this one.
    #[serde(default)]
    pub compile_alone: bool,
    /// Higher value → scheduled earlier among ready packages.
    #[serde(default = "default_compilation_priority")]
    pub compilation_priority: usize,
    /// When set, overrides `[build].ignore_already_made_packages` for this package.
    /// `true` = always rebuild; `false` = reuse PKGDEST artifacts when present.
    #[serde(default)]
    pub ignore_already_made_packages: Option<bool>,
}

/// GUI-friendly kernel build options; each field maps to a CachyOS PKGBUILD env var.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct KernelBuildConfig {
    #[serde(default, rename = "_cpusched")]
    pub cpusched: Option<String>,
    #[serde(default, rename = "_processor_opt")]
    pub processor_opt: Option<String>,
    // LTO / suffix / KCFI are controlled per-stage by the PGO pipeline, but a one-shot
    // (non-PGO) kernel build applies them verbatim from the user's config.
    #[serde(default, rename = "_use_llvm_lto")]
    pub use_llvm_lto: Option<String>,
    #[serde(default, rename = "_use_lto_suffix")]
    pub use_lto_suffix: Option<String>,
    #[serde(default, rename = "_use_gcc_suffix")]
    pub use_gcc_suffix: Option<String>,
    #[serde(default, rename = "_use_kcfi")]
    pub use_kcfi: Option<String>,
    #[serde(default, rename = "_HZ_ticks")]
    pub hz_ticks: Option<String>,
    #[serde(default, rename = "_tickrate")]
    pub tickrate: Option<String>,
    #[serde(default, rename = "_preempt")]
    pub preempt: Option<String>,
    #[serde(default, rename = "_hugepage")]
    pub hugepage: Option<String>,
    #[serde(default, rename = "_cc_harder")]
    pub cc_harder: Option<String>,
}

/// All kernel options a user can set, including compiler/LTO. Used by one-shot (non-PGO) builds
/// where the user's choices (e.g. GCC vs LLVM/LTO) must be applied verbatim.
pub fn kernel_override_pairs(kernel: &KernelBuildConfig) -> [(&str, &Option<String>); 11] {
    [
        ("_cpusched", &kernel.cpusched),
        ("_processor_opt", &kernel.processor_opt),
        ("_use_llvm_lto", &kernel.use_llvm_lto),
        ("_use_lto_suffix", &kernel.use_lto_suffix),
        ("_use_gcc_suffix", &kernel.use_gcc_suffix),
        ("_use_kcfi", &kernel.use_kcfi),
        ("_HZ_ticks", &kernel.hz_ticks),
        ("_tickrate", &kernel.tickrate),
        ("_preempt", &kernel.preempt),
        ("_hugepage", &kernel.hugepage),
        ("_cc_harder", &kernel.cc_harder),
    ]
}

/// Kernel options exposed in absgui / user config. PGO stages set LTO, AutoFDO, KCFI, etc. separately.
pub fn kernel_user_override_pairs(kernel: &KernelBuildConfig) -> [(&str, &Option<String>); 7] {
    [
        ("_cpusched", &kernel.cpusched),
        ("_processor_opt", &kernel.processor_opt),
        ("_HZ_ticks", &kernel.hz_ticks),
        ("_tickrate", &kernel.tickrate),
        ("_preempt", &kernel.preempt),
        ("_hugepage", &kernel.hugepage),
        ("_cc_harder", &kernel.cc_harder),
    ]
}

fn default_pgo_preset() -> String {
    "cachyos-kernel".to_string()
}

fn default_auto_str() -> String {
    "auto".to_string()
}

fn default_profiling_quality() -> String {
    "maximum".to_string()
}

/// `perf record` flags after event args when `perf_extra_args` is left at the serde default.
/// `standard`: stage scripts (`-c 100000`) scaled for llvm-profgen (~1.8×).
/// `maximum`: further scaled (~1.1× headroom after a ~56000 run).
fn default_perf_extra_args() -> String {
    "--mmap-pages 131072 -a -N -b -c 56000".to_string()
}

pub const PERF_EXTRA_ARGS_STANDARD: &str = "--mmap-pages 131072 -a -N -b -c 56000";
pub const PERF_EXTRA_ARGS_MAXIMUM: &str = "--mmap-pages 131072 -a -N -b -c 48000";

fn default_afdo_tool() -> String {
    "llvm-profgen".to_string()
}

fn default_propeller_tool() -> String {
    "create_llvm_prof".to_string()
}

fn default_afdo_profile_name() -> String {
    "kernel-compilation.afdo".to_string()
}

fn default_benchmark_preset() -> String {
    "fast".to_string()
}

/// Per-package multi-stage kernel PGO configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct PgoConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_pgo_preset")]
    pub preset: String,
    /// Required when running PGO: persistent profile archive (HDD path is fine).
    pub profiles_archive_dir: Option<String>,
    #[serde(default = "default_auto_str")]
    pub profile_scratch_dir: String,
    #[serde(default = "default_true")]
    pub perf_data_on_ram: bool,
    /// Optional override for the bundled PGO benchmark script (`assets/pgo-benchmark.sh`).
    pub benchmark_command: Option<String>,
    /// Profiling workload: `fast` (sysbench + stress-ng) or `cachyos` (full cachyos-benchmarker).
    /// Ignored during profiling when `profiling_quality = "maximum"` (always uses `cachyos`).
    #[serde(default = "default_benchmark_preset")]
    pub benchmark_preset: String,
    /// `standard` or `maximum` (default). Maximum uses denser perf sampling and the full
    /// cachyos-benchmarker workload for llvm-profgen-quality kernel profiles.
    #[serde(default = "default_profiling_quality")]
    pub profiling_quality: String,
    /// Persistent directory for cachyos-benchmarker downloads (ffmpeg, kernel test tree, etc.).
    /// When unset, uses `{profiles_archive_dir}/benchmark-workdir` or `~/.cache/abs/pgo-benchmark/PKG`.
    pub benchmark_workdir: Option<String>,
    pub build_user: Option<String>,
    #[serde(default = "default_auto_str")]
    pub perf_event_args: String,
    #[serde(default = "default_perf_extra_args")]
    pub perf_extra_args: String,
    pub sysctl_command: Option<String>,
    #[serde(default = "default_auto_str")]
    pub vmlinux: String,
    #[serde(default = "default_afdo_tool")]
    pub afdo_tool: String,
    #[serde(default = "default_propeller_tool")]
    pub propeller_tool: String,
    #[serde(default = "default_afdo_profile_name")]
    pub afdo_profile_name: String,
    #[serde(default = "default_true")]
    pub verify_boot: bool,
    /// When true, abs reboots at PGO wait stages and resumes via a user systemd unit until done.
    #[serde(default)]
    pub auto_restart: bool,
    pub state_file: Option<String>,
}

fn default_true() -> bool {
    true
}

impl PgoConfig {
    pub fn resolved_state_file(&self, package: &str) -> PathBuf {
        if let Some(path) = &self.state_file {
            expand_user_path(path)
        } else {
            dirs::config_dir()
                .map(|d| d.join("abs").join("pgo").join(format!("{package}.json")))
                .unwrap_or_else(|| PathBuf::from(format!("/tmp/abs-pgo-{package}.json")))
        }
    }

    pub fn resolved_archive_dir(&self) -> Option<PathBuf> {
        self.profiles_archive_dir
            .as_ref()
            .map(|p| expand_user_path(p))
    }

    /// On-disk cache for cachyos-benchmarker assets (separate from ephemeral profile scratch on tmpfs).
    pub fn resolved_benchmark_workdir(&self, package: &str) -> PathBuf {
        if let Some(path) = &self.benchmark_workdir {
            let expanded = expand_user_path(path);
            if expanded.as_os_str() != "auto" {
                return expanded;
            }
        }
        if let Some(archive) = self.resolved_archive_dir() {
            return archive.join("benchmark-workdir");
        }
        dirs::cache_dir()
            .map(|d| d.join("abs").join("pgo-benchmark").join(package))
            .unwrap_or_else(|| PathBuf::from(format!("/tmp/abs-pgo-benchmark-{package}")))
    }
}

/// Expand `~` and `$HOME` / `$XDG_CONFIG_HOME` in config paths.
pub fn expand_user_path(raw: &str) -> PathBuf {
    let mut s = raw.to_string();
    if s.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            s = home.join(&s[2..]).to_string_lossy().into_owned();
        }
    } else if s == "~"
        && let Some(home) = dirs::home_dir()
    {
        return home;
    }
    for (var, val) in [
        ("$HOME", dirs::home_dir()),
        ("$XDG_CONFIG_HOME", dirs::config_dir()),
    ] {
        if let Some(ref path) = val {
            s = s.replace(var, &path.to_string_lossy());
        }
    }
    PathBuf::from(s)
}

const CONFIG_TEMPLATE: &str = include_str!("../abs.toml.example");

fn user_config_path() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("abs").join("abs.toml"))
        .unwrap_or_else(|| die!("Could not determine config directory ($XDG_CONFIG_HOME)"))
}

fn ensure_user_config_exists() -> PathBuf {
    let path = user_config_path();
    if path.exists() {
        return path;
    }

    if let Some(parent) = path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        die!("Failed to create config directory '{}': {}", parent.display(), e);
    }

    if let Err(e) = fs::write(&path, CONFIG_TEMPLATE) {
        die!("Failed to write config file '{}': {}", path.display(), e);
    }

    path
}

fn resolve_editor(explicit: Option<&str>) -> String {
    if let Some(editor) = explicit.filter(|s| !s.is_empty()) {
        return editor.to_string();
    }

    std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string())
}

fn run_editor(editor: &str, path: &Path) {
    let path_str = path.to_string_lossy();
    let editor_trimmed = editor.trim();
    let cmd_name = editor_trimmed.split_whitespace().next().unwrap_or(editor_trimmed);

    if cmd_name == "kate" {
        // Spawn a background instance of Kate to guarantee a running instance exists.
        // If an instance is already running, this is a fast no-op.
        let _ = std::process::Command::new("kate").spawn();
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    let result = if editor.chars().any(char::is_whitespace) {
        let script = format!("{} {}", editor, sh_single_quote(&path_str));
        run_command("sh", &["-c", &script], None::<&str>)
    } else {
        let mut args = Vec::new();
        if cmd_name == "kate" {
            args.push("-b");
        } else if cmd_name == "code"
            || cmd_name == "vscode"
            || cmd_name == "codium"
            || cmd_name == "vscodium"
            || cmd_name == "cursor"
            || cmd_name == "subl"
            || cmd_name == "sublime-text"
            || cmd_name == "gedit"
            || cmd_name == "pluma"
            || cmd_name == "xed"
            || cmd_name == "atom"
            || cmd_name == "lumiere"
        {
            args.push("-w");
        }
        args.push(path_str.as_ref());
        run_command(editor, &args, None::<&str>)
    };

    if let Err(e) = result {
        die!("Failed to open config in editor: {}", e);
    }
}

impl Config {
    /// Packages to skip when offering `pacman -U` after a manual build.
    pub fn skip_install_after_compilation(&self) -> &[String] {
        self.skip_install_packages_after_compilation
            .as_ref()
            .unwrap_or(&self.skip_install_packages)
    }

    pub fn open_in_editor(editor: Option<&str>) {
        use std::io::{self, Write};
        let path = ensure_user_config_exists();
        let editor_str = resolve_editor(editor);
        loop {
            run_editor(&editor_str, &path);

            let config_content = match fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => {
                    println!("==> ERROR: Failed to read config file for validation.");
                    break;
                }
            };
            match toml::from_str::<Config>(&config_content) {
                Ok(config) => {
                    let env = config.build.default_environment.as_str();
                    if env != "local" && env != "chroot" {
                        println!("{} Invalid [build] default_environment: {:?} (expected \"local\" or \"chroot\")", "==> ERROR:".red(), env);
                    } else {
                        println!("{}", "==> Configuration validated successfully!".green());
                        break;
                    }
                }
                Err(e) => {
                    println!("{} Failed to parse configuration file: {}", "==> ERROR:".red(), e);
                }
            }

            print!("Would you like to re-open the editor to fix the configuration? [Y/n]: ");
            let _ = io::stdout().flush();
            let mut input = String::new();
            if io::stdin().read_line(&mut input).is_err() {
                break;
            }
            let v = input.trim().to_lowercase();
            if v == "n" || v == "no" {
                break;
            }
        }
    }

    pub fn load_config() -> Config {
        // Same order as README: XDG config dir, then /etc.
        let user_config = user_config_path();
        let etc_config = PathBuf::from("/etc/abs/abs.toml");

        let config_path = if user_config.exists() {
            user_config
        } else if etc_config.exists() {
            etc_config
        } else {
            let path = ensure_user_config_exists();
            println!(
                "ABS config has been created from the example. Please configure using --configure"
            );
            path
        };

        let config_content = match fs::read_to_string(&config_path) {
            Ok(c) => c,
            Err(_) => {
                die!("Failed to read config file at {:?}", config_path);
            }
        };

        let mut config: Config = match toml::from_str(&config_content) {
            Ok(c) => c,
            Err(e) => {
                die!("Failed to parse config '{:?}': {}", config_path, e);
            }
        };

        // Merge self-update settings parsed under [build] for backwards-compatibility
        Self::merge_legacy_build_fields(&mut config);

        config.validate();
        config
    }

    /// Load an existing config without creating a default file (for `--purge`).
    pub fn try_load_existing() -> Option<Config> {
        let user_config = user_config_path();
        let etc_config = PathBuf::from("/etc/abs/abs.toml");
        let config_path = if user_config.exists() {
            user_config
        } else if etc_config.exists() {
            etc_config
        } else {
            return None;
        };

        let config_content = fs::read_to_string(&config_path).ok()?;
        let mut config: Config = toml::from_str(&config_content).ok()?;
        Self::merge_legacy_build_fields(&mut config);
        Some(config)
    }

    fn merge_legacy_build_fields(config: &mut Config) {
        if let Some(val) = config.build.check_for_update_on_startup {
            config.check_for_update_on_startup = val;
        }
        if let Some(val) = config.build.auto_update_on_startup {
            config.auto_update_on_startup = val;
        }
        if let Some(val) = config.build.self_update_at_updates {
            config.self_update_at_updates = val;
        }
        if let Some(val) = &config.build.self_update_raw_url {
            config.self_update_raw_url = val.clone();
        }
        if let Some(val) = &config.build.self_update_install_path {
            config.self_update_install_path = val.clone();
        }
        if let Some(val) = config.build.install_testing_phase_archlinux_packages {
            config.install_testing_phase_archlinux_packages = val;
        }
    }

    fn validate(&self) {
        if self.config_version == 0 {
            die!("Invalid config_version: 0 (expected >= 1)");
        }
        let env = self.build.default_environment.as_str();
        if env != "local" && env != "chroot" {
            die!(
                "Invalid [build] default_environment: {:?} (expected \"local\" or \"chroot\")",
                env
            );
        }
        match self.build.global_cpu_threads_mode.as_str() {
            "flexible" => {
                if let (Some(soft), Some(hard)) = (
                    self.build.global_cpu_threads_cap,
                    self.build.maximum_cpu_threads_cap,
                ) && hard < soft
                {
                    die!(
                        "Invalid [build] maximum_cpu_threads_cap ({hard}) must be >= global_cpu_threads_cap ({soft})"
                    );
                }
            }
            "strict" => {
                if self.build.maximum_cpu_threads_cap.is_some() {
                    crate::ewarn!(
                        "[build] maximum_cpu_threads_cap is ignored in \"strict\" mode (only used by \"flexible\")."
                    );
                }
            }
            cpu_mode => die!(
                "Invalid [build] global_cpu_threads_mode: {:?} (expected \"strict\" or \"flexible\")",
                cpu_mode
            ),
        }
        for (pkg_name, pkg) in &self.packages {
            if let Some(be) = &pkg.build_env {
                let be = be.as_str();
                if be != "local" && be != "chroot" {
                    die!(
                        "Invalid build_env for package {:?}: {:?} (expected \"local\" or \"chroot\")",
                        pkg_name,
                        be
                    );
                }
            }
            if let Some(code) = &pkg.ramdisk
                && !crate::ramdisk::is_ramdisk_disabled(code)
                && let Err(e) = crate::ramdisk::parse_ramdisk_targets(code)
            {
                die!("Invalid ramdisk for package {:?}: {}", pkg_name, e);
            }
        }
        for (key, path) in [
            ("paths.packages_path", self.paths.packages_path.as_str()),
            ("paths.chroot_base_path", self.paths.chroot_base_path.as_str()),
            (
                "paths.ready_made_packages_path",
                self.paths.ready_made_packages_path.as_str(),
            ),
        ] {
            if let Err(e) = crate::utils::validate_config_path(key, path) {
                die!("{}", e);
            }
        }
        if self.ramdisk.enabled {
            if let Err(e) = crate::utils::validate_config_path(
                "ramdisk.mount_point",
                self.ramdisk.mount_point.as_str(),
            ) {
                die!("{}", e);
            }
            if self.ramdisk.size.trim().is_empty() {
                die!("ramdisk.size cannot be empty when [ramdisk] is enabled");
            }
            if let Some(seed) = &self.ramdisk.seed_chroot_from {
                if seed.trim().is_empty() {
                    die!("ramdisk.seed_chroot_from cannot be empty when set");
                }
                if let Err(e) =
                    crate::utils::validate_config_path("ramdisk.seed_chroot_from", seed.as_str())
                {
                    die!("{}", e);
                }
            }
        }
        if let Err(e) = crate::utils::init_deletable_roots(
            &self.paths.packages_path,
            &self.paths.chroot_base_path,
            &self.paths.ready_made_packages_path,
            &[],
        ) {
            die!("{}", e);
        }
    }

    pub fn print_human_readable(&self) {
        println!("{}", "ABS Configuration".blue().bold());
        println!("{}", "-------------------------".blue());
        println!("config_version: {}", self.config_version);

        println!("\n{}", "Paths".green().bold());
        println!("  packages_path: {}", self.paths.packages_path);
        println!("  chroot_base_path: {}", self.paths.chroot_base_path);
        println!(
            "  ready_made_packages_path: {}",
            self.paths.ready_made_packages_path
        );
        println!(
            "  chroot_makepkg_conf: {}",
            self.paths.chroot_makepkg_conf.as_deref().unwrap_or("(none)")
        );

        println!("\n{}", "Ramdisk".green().bold());
        println!("  enabled: {}", self.ramdisk.enabled);
        if self.ramdisk.enabled {
            println!("  mount_point: {}", self.ramdisk.mount_point);
            println!("  size: {}", self.ramdisk.size);
            println!("  mode: {}", self.ramdisk.mode);
            println!("  build_workdir: {}", self.ramdisk.build_workdir);
            println!("  chroot: {}", self.ramdisk.chroot);
            println!("  packages: {}", self.ramdisk.packages);
            println!(
                "  seed_chroot_from: {}",
                self.ramdisk
                    .seed_chroot_from
                    .as_deref()
                    .unwrap_or("(none)")
            );
            println!("  sync_chroot_on_exit: {}", self.ramdisk.sync_chroot_on_exit);
            println!("  min_free_ram_mb: {}", self.ramdisk.min_free_ram_mb);
            println!(
                "  reclaim_mount_on_startup: {}",
                self.ramdisk.reclaim_mount_on_startup
            );
        }

        println!("\n{}", "Build".green().bold());
        println!("  default_environment: {}", self.build.default_environment);
        println!(
            "  ignore_compilation_failures: {}",
            self.build.ignore_compilation_failures
        );
        println!(
            "  compile_first_install_after: {}",
            self.build.compile_first_install_after
        );
        println!(
            "  clean_install_by_default: {}",
            self.build.clean_install_by_default
        );
        println!(
            "  ignore_already_made_packages: {}",
            self.build.ignore_already_made_packages
        );
        println!(
            "  concurrent_repos_downloads_limit: {}",
            self.build.concurrent_repos_downloads_limit
        );
        println!(
            "  concurrent_compilations_limit: {}",
            self.build.concurrent_compilations_limit
        );
        println!(
            "  fast_aur_rpc_update_checks: {}",
            self.build.fast_aur_rpc_update_checks
        );
        println!(
            "  default_compiler: {}",
            self.build.default_compiler.as_deref().unwrap_or("(none)")
        );
        println!(
            "  system_update_first: {}",
            self.build.system_update_first
        );
        println!(
            "  clean_chroot_after_compilation: {}",
            self.build.clean_chroot_after_compilation
        );
        println!(
            "  global_cpu_threads_mode: {}",
            self.build.global_cpu_threads_mode
        );
        println!(
            "  global_cpu_threads_cap: {}",
            self.build
                .global_cpu_threads_cap
                .map(|n| n.to_string())
                .unwrap_or_else(|| "(unset)".to_string())
        );
        println!(
            "  maximum_cpu_threads_cap: {}",
            self.build
                .maximum_cpu_threads_cap
                .map(|n| n.to_string())
                .unwrap_or_else(|| "(unset)".to_string())
        );
        println!(
            "  default_compilation_threads: {}",
            self.build
                .default_compilation_threads
                .map(|n| n.to_string())
                .unwrap_or_else(|| "(unset)".to_string())
        );

        println!("\n{}", "System Update".green().bold());
        println!(
            "  command_to_update_repositories: {}",
            self.system_update.command_to_update_repositories
        );
        println!(
            "  command_to_perform_system_update: {}",
            self.system_update.command_to_perform_system_update
        );
        println!(
            "  command_to_perform_system_update_no_refresh: {}",
            self.system_update.get_command_to_perform_system_update_no_refresh()
        );
        println!("  ignore_flag: {}", self.system_update.ignore_flag);
        if self.system_update.ignore_packages.is_empty() {
            println!("  ignore_packages: (none)");
        } else {
            println!("  ignore_packages:");
            for pkg in &self.system_update.ignore_packages {
                println!("    - {}", pkg);
            }
        }

        println!("\n{}", "Repositories".green().bold());
        let mut repo_entries: Vec<_> = self.repositories.iter().collect();
        let default_entry = repo_entries
            .iter()
            .position(|(name, _)| *name == "default")
            .map(|i| repo_entries.swap_remove(i));
        repo_entries.sort_by(|a, b| a.0.cmp(b.0));
        if let Some((name, url)) = default_entry {
            println!("  {} -> {}", name, url);
        }
        for (name, url) in repo_entries {
            println!("  {} -> {}", name, url);
        }

        println!("\n{}", "Manual Update Packages".green().bold());
        if self.manual_update_packages.is_empty() {
            println!("  (none)");
        } else {
            for pkg in &self.manual_update_packages {
                println!("  - {}", pkg);
            }
        }

        println!("\n{}", "Skip Install Packages (system update)".green().bold());
        if self.skip_install_packages.is_empty() {
            println!("  (none)");
        } else {
            for pkg in &self.skip_install_packages {
                println!("  - {}", pkg);
            }
        }

        println!("\n{}", "Skip Install After Compilation".green().bold());
        match &self.skip_install_packages_after_compilation {
            None if self.skip_install_packages.is_empty() => println!("  (none)"),
            None => {
                println!("  (unset; using skip_install_packages for backward compatibility)");
                for pkg in &self.skip_install_packages {
                    println!("  - {}", pkg);
                }
            }
            Some(list) if list.is_empty() => println!("  (none)"),
            Some(list) => {
                for pkg in list {
                    println!("  - {}", pkg);
                }
            }
        }

        println!("\n{}", "Compilers".green().bold());
        if self.compilers.is_empty() {
            println!("  (none)");
        } else {
            let mut comp_entries: Vec<_> = self.compilers.iter().collect();
            comp_entries.sort_by(|a, b| a.0.cmp(b.0));
            for (name, cfg) in comp_entries {
                println!("  - {}: cc={} cxx={}", name, cfg.cc, cfg.cxx);
            }
        }

        println!("\n{}", "Self-Updates".green().bold());
        println!("  check_for_update_on_startup: {}", self.check_for_update_on_startup);
        println!("  auto_update_on_startup: {}", self.auto_update_on_startup);
        println!("  self_update_at_updates: {}", self.self_update_at_updates);
        println!("  self_update_raw_url: {}", self.self_update_raw_url);
        println!("  self_update_install_path: {}", self.self_update_install_path);
        println!(
            "  self_update_use_pacman: {}",
            match self.self_update_use_pacman {
                None => "auto".to_string(),
                Some(true) => "true".to_string(),
                Some(false) => "false".to_string(),
            }
        );
        println!(
            "  install_testing_phase_archlinux_packages: {}",
            self.install_testing_phase_archlinux_packages
        );

        println!("\n{}", "Package Profiles".green().bold());
        let mut pkg_entries: Vec<_> = self.packages.iter().collect();
        pkg_entries.sort_by(|a, b| a.0.cmp(b.0));
        for (name, cfg) in pkg_entries {
            println!("  {}", format!("- {}", name).bold());
            let mut profile_line = format!(
                "    source={} build_env={} tests={}",
                cfg.source.as_deref().unwrap_or("-"),
                cfg.build_env.as_deref().unwrap_or("-"),
                cfg.tests
                    .map(|v| if v { "on" } else { "off" })
                    .unwrap_or("-"),
            );
            if let Some(alias) = &cfg.alias {
                profile_line.push_str(&format!(" alias={}", alias));
            }
            println!("{}", profile_line);
            if let Some(cmd) = &cfg.custom_local_build_command {
                println!("    custom_local_build_command: {}", cmd);
            }
            if let Some(cmd) = &cfg.custom_chroot_build_command {
                println!("    custom_chroot_build_command: {}", cmd);
            }
            if let Some(cmd) = &cfg.pre_update_command {
                println!("    pre_update_command: {}", cmd);
            }
            if let Some(cmd) = &cfg.post_update_command {
                println!("    post_update_command: {}", cmd);
            }
            if let Some(comp) = &cfg.compiler {
                println!("    compiler: {}", comp);
            }
            if let Some(repo) = &cfg.upstream_github {
                println!(
                    "    upstream_github: {} (prereleases: {})",
                    repo, cfg.upstream_prereleases
                );
            }
            if let Some(code) = &cfg.ramdisk {
                println!("    ramdisk: {} (w=workdir, c=chroot, p=packages)", code);
            }
            if let Some(n) = cfg.compilation_threads {
                println!("    compilation_threads: {}", n);
            }
            if cfg.compile_alone {
                println!("    compile_alone: true");
            }
            if cfg.compilation_priority != default_compilation_priority() {
                println!("    compilation_priority: {}", cfg.compilation_priority);
            }
            if let Some(v) = cfg.ignore_already_made_packages {
                println!("    ignore_already_made_packages: {}", v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn test_parse_cpu_scheduler_config() {
        let toml_content = r#"
config_version = 1
manual_update_packages = []
skip_install_packages = []

[paths]
packages_path = "/tmp"
chroot_base_path = "/tmp"
ready_made_packages_path = "/tmp"

[build]
default_environment = "local"
global_cpu_threads_mode = "flexible"
global_cpu_threads_cap = 10
maximum_cpu_threads_cap = 15
default_compilation_threads = 4

[system_update]
command_to_update_repositories = "pacman -Su"
command_to_perform_system_update = "pacman -Syu"
command_to_perform_system_update_no_refresh = "pacman -Su"
ignore_flag = "--ignore"
ignore_packages = []

[repositories]
default = "arch"
arch = "https://gitlab.archlinux.org/archlinux/packaging/packages"

[packages.linux-cachyos]
compilation_threads = 8
compile_alone = true
compilation_priority = 10
"#;
        let config: Config = toml::from_str(toml_content).unwrap();
        assert_eq!(config.build.global_cpu_threads_mode, "flexible");
        assert_eq!(config.build.global_cpu_threads_cap, Some(10));
        assert_eq!(config.build.maximum_cpu_threads_cap, Some(15));
        assert_eq!(config.build.default_compilation_threads, Some(4));
        let pkg = config.packages.get("linux-cachyos").unwrap();
        assert_eq!(pkg.compilation_threads, Some(8));
        assert!(pkg.compile_alone);
        assert_eq!(pkg.compilation_priority, 10);
        assert!(!config.build.ignore_already_made_packages);
        assert!(pkg.ignore_already_made_packages.is_none());
    }

    #[test]
    fn test_parse_ignore_already_made_packages() {
        let toml_content = r#"
config_version = 1
manual_update_packages = []
skip_install_packages = []

[paths]
packages_path = "/tmp"
chroot_base_path = "/tmp"
ready_made_packages_path = "/tmp"

[build]
default_environment = "local"
ignore_already_made_packages = true

[system_update]
command_to_update_repositories = "pacman -Su"
command_to_perform_system_update = "pacman -Syu"
ignore_flag = "--ignore"
ignore_packages = []

[repositories]
default = "arch"
arch = "https://gitlab.archlinux.org/archlinux/packaging/packages"

[packages.firefox]
ignore_already_made_packages = false
"#;
        let config: Config = toml::from_str(toml_content).unwrap();
        assert!(config.build.ignore_already_made_packages);
        assert_eq!(
            config
                .packages
                .get("firefox")
                .and_then(|p| p.ignore_already_made_packages),
            Some(false)
        );
    }

    #[test]
    fn test_parse_install_testing_packages_under_build() {
        let toml_content = r#"
config_version = 1
manual_update_packages = []
skip_install_packages = []

[paths]
packages_path = "/tmp"
chroot_base_path = "/tmp"
ready_made_packages_path = "/tmp"

[build]
default_environment = "local"
install_testing_phase_archlinux_packages = true

[system_update]
command_to_update_repositories = "pacman -Su"
command_to_perform_system_update = "pacman -Syu"
command_to_perform_system_update_no_refresh = "pacman -Su"
ignore_flag = "--ignore"
ignore_packages = []

[repositories]
default = "arch"
arch = "https://gitlab.archlinux.org/archlinux/packaging/packages"

[packages]
"#;
        let mut config: Config = toml::from_str(toml_content).unwrap();
        assert!(config.build.system_update_first);
        assert!(!config.install_testing_phase_archlinux_packages);
        if let Some(val) = config.build.install_testing_phase_archlinux_packages {
            config.install_testing_phase_archlinux_packages = val;
        }
        assert!(config.install_testing_phase_archlinux_packages);
    }

    #[test]
    fn test_get_command_to_perform_system_update_no_refresh() {
        let mut sys_update = super::SystemUpdateConfig {
            command_to_update_repositories: "yay -Sy".into(),
            command_to_perform_system_update: "yay -Syu --quiet".into(),
            command_to_perform_system_update_no_refresh: None,
            ignore_flag: "--ignore".into(),
            ignore_packages: vec![],
        };

        // Derives from command_to_perform_system_update (replacing -Syu with -Su)
        assert_eq!(
            sys_update.get_command_to_perform_system_update_no_refresh(),
            "yay -Su --quiet"
        );

        // Derives from command_to_perform_system_update (replacing -Sy with -S)
        sys_update.command_to_perform_system_update = "pacman -Sy".into();
        assert_eq!(
            sys_update.get_command_to_perform_system_update_no_refresh(),
            "pacman -S"
        );

        // Obeys explicit override if present
        sys_update.command_to_perform_system_update_no_refresh = Some("custom_command".into());
        assert_eq!(
            sys_update.get_command_to_perform_system_update_no_refresh(),
            "custom_command"
        );
    }

    #[test]
    fn test_parse_ramdisk_section() {
        let toml_content = r#"
config_version = 1
manual_update_packages = []
skip_install_packages = []

[paths]
packages_path = "/tmp/abs/packages"
chroot_base_path = "/tmp/abs/chroot"
ready_made_packages_path = "/tmp/abs/ready"

[ramdisk]
enabled = true
mount_point = "/run/abs-ram"
size = "8G"
build_workdir = true
chroot = true
packages = false
seed_chroot_from = "/tmp/abs/chroot"
sync_chroot_on_exit = true
min_free_ram_mb = 2048

[build]
default_environment = "local"

[system_update]
command_to_update_repositories = "pacman -Sy"
command_to_perform_system_update = "pacman -Syu"
ignore_flag = "--ignore"
ignore_packages = []

[repositories]
default = "arch"
arch = "https://gitlab.archlinux.org/archlinux/packaging/packages"

[packages]
"#;
        let config: Config = toml::from_str(toml_content).unwrap();
        assert!(config.ramdisk.enabled);
        assert_eq!(config.ramdisk.mount_point, "/run/abs-ram");
        assert_eq!(config.ramdisk.size, "8G");
        assert!(config.ramdisk.build_workdir);
        assert!(config.ramdisk.chroot);
        assert!(!config.ramdisk.packages);
        assert_eq!(
            config.ramdisk.seed_chroot_from.as_deref(),
            Some("/tmp/abs/chroot")
        );
        assert!(config.ramdisk.sync_chroot_on_exit);
        assert_eq!(config.ramdisk.min_free_ram_mb, 2048);
    }

    #[test]
    fn test_ramdisk_defaults_when_section_missing() {
        let toml_content = r#"
config_version = 1
manual_update_packages = []
skip_install_packages = []

[paths]
packages_path = "/tmp"
chroot_base_path = "/tmp"
ready_made_packages_path = "/tmp"

[build]
default_environment = "local"

[system_update]
command_to_update_repositories = "pacman -Sy"
command_to_perform_system_update = "pacman -Syu"
ignore_flag = "--ignore"
ignore_packages = []

[repositories]
default = "arch"
arch = "https://gitlab.archlinux.org/archlinux/packaging/packages"

[packages]
"#;
        let config: Config = toml::from_str(toml_content).unwrap();
        assert!(!config.ramdisk.enabled);
        assert_eq!(config.ramdisk.mount_point, "/run/abs-ram");
        assert!(!config.ramdisk.build_workdir);
        assert!(!config.ramdisk.chroot);
        assert!(!config.ramdisk.packages);
        assert!(config.ramdisk.reclaim_mount_on_startup);
    }

    #[test]
    fn skip_install_after_compilation_fallback() {
        let toml_content = r#"
config_version = 1
manual_update_packages = []
skip_install_packages = ["foo", "bar"]

[paths]
packages_path = "/tmp"
chroot_base_path = "/tmp"
ready_made_packages_path = "/tmp"

[build]
default_environment = "local"

[system_update]
command_to_update_repositories = "pacman -Sy"
command_to_perform_system_update = "pacman -Syu"
ignore_flag = "--ignore"
ignore_packages = []

[repositories]
default = "arch"
arch = "https://gitlab.archlinux.org/archlinux/packaging/packages"

[packages]
"#;
        let config: Config = toml::from_str(toml_content).unwrap();
        assert_eq!(config.skip_install_after_compilation(), &["foo", "bar"]);

        let toml_with_explicit = r#"
config_version = 1
manual_update_packages = []
skip_install_packages = ["foo"]
skip_install_packages_after_compilation = ["bar"]

[paths]
packages_path = "/tmp"
chroot_base_path = "/tmp"
ready_made_packages_path = "/tmp"

[build]
default_environment = "local"

[system_update]
command_to_update_repositories = "pacman -Sy"
command_to_perform_system_update = "pacman -Syu"
ignore_flag = "--ignore"
ignore_packages = []

[repositories]
default = "arch"
arch = "https://gitlab.archlinux.org/archlinux/packaging/packages"

[packages]
"#;
        let config: Config = toml::from_str(toml_with_explicit).unwrap();
        assert_eq!(config.skip_install_after_compilation(), &["bar"]);
    }

    #[test]
    fn test_clean_chroot_after_compilation_defaults_true() {
        let toml_content = r#"
config_version = 1
manual_update_packages = []
skip_install_packages = []

[paths]
packages_path = "/tmp"
chroot_base_path = "/tmp"
ready_made_packages_path = "/tmp"

[build]
default_environment = "local"

[system_update]
command_to_update_repositories = "pacman -Sy"
command_to_perform_system_update = "pacman -Syu"
ignore_flag = "--ignore"
ignore_packages = []

[repositories]
default = "arch"
arch = "https://gitlab.archlinux.org/archlinux/packaging/packages"

[packages]
"#;
        let config: Config = toml::from_str(toml_content).unwrap();
        assert!(config.build.clean_chroot_after_compilation);
    }
}
