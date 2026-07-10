use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Full ABS config document.
///
/// Field ordering matters for TOML output: all scalar/array ("value") fields are
/// declared before any table fields (sub-structs / maps) so the emitted TOML is
/// always valid. Fields the CLI requires (no `#[serde(default)]` on its side) are
/// always serialized; optional ones are skipped when empty to keep the file clean.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigDocument {
    #[serde(default = "default_config_version")]
    pub config_version: u32,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_for_update_on_startup: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_update_on_startup: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_update_at_updates: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_update_raw_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_update_install_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_update_use_pacman: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_testing_phase_archlinux_packages: Option<bool>,

    // Required arrays (CLI has no default for these).
    #[serde(default)]
    pub manual_update_packages: Vec<String>,
    #[serde(default)]
    pub skip_install_packages: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_install_packages_after_compilation: Option<Vec<String>>,

    // Tables (declared last).
    pub paths: PathsSection,
    #[serde(default)]
    pub ramdisk: RamdiskSection,
    pub build: BuildSection,
    pub system_update: SystemUpdateSection,
    #[serde(default)]
    pub repositories: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub compilers: HashMap<String, CompilerSection>,
    #[serde(default)]
    pub packages: HashMap<String, PackageSection>,
    /// Template applied to a kernel the first time it is configured. Ignored by the CLI.
    #[serde(default = "default_kernel_template")]
    pub kernel_defaults: PackageSection,
}

fn default_config_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompilerSection {
    pub cc: String,
    pub cxx: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathsSection {
    pub packages_path: String,
    pub chroot_base_path: String,
    pub ready_made_packages_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chroot_makepkg_conf: Option<String>,
}

impl Default for PathsSection {
    fn default() -> Self {
        Self {
            packages_path: "$XDG_CONFIG_HOME/.cache/abs/packages".into(),
            chroot_base_path: "$XDG_CONFIG_HOME/.cache/abs/chroot".into(),
            ready_made_packages_path: "$XDG_CONFIG_HOME/.cache/abs/ready".into(),
            chroot_makepkg_conf: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildSection {
    #[serde(default = "default_local")]
    pub default_environment: String,
    #[serde(default)]
    pub ignore_compilation_failures: bool,
    #[serde(default)]
    pub compile_first_install_after: bool,
    #[serde(default)]
    pub clean_install_by_default: bool,
    #[serde(default = "default_ten")]
    pub concurrent_repos_downloads_limit: usize,
    #[serde(default = "default_one")]
    pub concurrent_compilations_limit: usize,
    #[serde(default = "default_true")]
    pub fast_aur_rpc_update_checks: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_compiler: Option<String>,
    #[serde(default = "default_true")]
    pub system_update_first: bool,
    #[serde(default = "default_true")]
    pub clean_chroot_after_compilation: bool,
    #[serde(default = "default_cpu_mode")]
    pub global_cpu_threads_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_cpu_threads_cap: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_cpu_threads_cap: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_compilation_threads: Option<usize>,
}

fn default_local() -> String {
    "local".into()
}
fn default_one() -> usize {
    1
}
fn default_ten() -> usize {
    10
}
fn default_true() -> bool {
    true
}

fn default_cpu_mode() -> String {
    "strict".into()
}

impl Default for BuildSection {
    fn default() -> Self {
        Self {
            default_environment: default_local(),
            ignore_compilation_failures: false,
            compile_first_install_after: false,
            clean_install_by_default: false,
            concurrent_repos_downloads_limit: default_ten(),
            concurrent_compilations_limit: 1,
            fast_aur_rpc_update_checks: true,
            default_compiler: None,
            system_update_first: true,
            clean_chroot_after_compilation: true,
            global_cpu_threads_mode: default_cpu_mode(),
            global_cpu_threads_cap: None,
            maximum_cpu_threads_cap: None,
            default_compilation_threads: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemUpdateSection {
    #[serde(default = "default_pacman_sy")]
    pub command_to_update_repositories: String,
    #[serde(default = "default_pacman_syu")]
    pub command_to_perform_system_update: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_to_perform_system_update_no_refresh: Option<String>,
    #[serde(default = "default_ignore")]
    pub ignore_flag: String,
    #[serde(default)]
    pub ignore_packages: Vec<String>,
}

fn default_pacman_sy() -> String {
    "sudo pacman -Sy".into()
}
fn default_pacman_syu() -> String {
    "sudo pacman -Syu".into()
}
fn default_ignore() -> String {
    "--ignore".into()
}

impl Default for SystemUpdateSection {
    fn default() -> Self {
        Self {
            command_to_update_repositories: default_pacman_sy(),
            command_to_perform_system_update: default_pacman_syu(),
            command_to_perform_system_update_no_refresh: None,
            ignore_flag: default_ignore(),
            ignore_packages: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RamdiskSection {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_mount")]
    pub mount_point: String,
    #[serde(default = "default_size")]
    pub size: String,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub build_workdir: bool,
    #[serde(default)]
    pub chroot: bool,
    #[serde(default)]
    pub packages: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed_chroot_from: Option<String>,
    #[serde(default)]
    pub sync_chroot_on_exit: bool,
    #[serde(default = "default_min_free_ram")]
    pub min_free_ram_mb: u64,
    #[serde(default = "default_true")]
    pub warn_packages_ram: bool,
    #[serde(default = "default_true")]
    pub reclaim_mount_on_startup: bool,
}

fn default_mount() -> String {
    "/run/abs-ram".into()
}
fn default_size() -> String {
    "16G".into()
}
fn default_mode() -> String {
    "0755".into()
}
fn default_min_free_ram() -> u64 {
    4096
}

impl Default for RamdiskSection {
    fn default() -> Self {
        Self {
            enabled: false,
            mount_point: default_mount(),
            size: default_size(),
            mode: default_mode(),
            build_workdir: false,
            chroot: false,
            packages: false,
            seed_chroot_from: None,
            sync_chroot_on_exit: false,
            min_free_ram_mb: default_min_free_ram(),
            warn_packages_ram: true,
            reclaim_mount_on_startup: true,
        }
    }
}

fn default_compilation_priority() -> usize {
    1
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn is_default_priority(v: &usize) -> bool {
    *v == default_compilation_priority()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PackageSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_local_build_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_chroot_build_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_github: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_prereleases: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compiler: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_update_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_update_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ramdisk: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compilation_threads: Option<usize>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub compile_alone: bool,
    #[serde(default = "default_compilation_priority", skip_serializing_if = "is_default_priority")]
    pub compilation_priority: usize,
    // Tables last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel: Option<KernelSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pgo: Option<PgoSection>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KernelSection {
    #[serde(default, rename = "_cpusched", skip_serializing_if = "Option::is_none")]
    pub cpusched: Option<String>,
    #[serde(
        default,
        rename = "_processor_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub processor_opt: Option<String>,
    #[serde(
        default,
        rename = "_use_llvm_lto",
        skip_serializing_if = "Option::is_none"
    )]
    pub use_llvm_lto: Option<String>,
    #[serde(
        default,
        rename = "_use_lto_suffix",
        skip_serializing_if = "Option::is_none"
    )]
    pub use_lto_suffix: Option<String>,
    #[serde(
        default,
        rename = "_use_gcc_suffix",
        skip_serializing_if = "Option::is_none"
    )]
    pub use_gcc_suffix: Option<String>,
    #[serde(default, rename = "_use_kcfi", skip_serializing_if = "Option::is_none")]
    pub use_kcfi: Option<String>,
    #[serde(default, rename = "_HZ_ticks", skip_serializing_if = "Option::is_none")]
    pub hz_ticks: Option<String>,
    #[serde(default, rename = "_tickrate", skip_serializing_if = "Option::is_none")]
    pub tickrate: Option<String>,
    #[serde(default, rename = "_preempt", skip_serializing_if = "Option::is_none")]
    pub preempt: Option<String>,
    #[serde(default, rename = "_hugepage", skip_serializing_if = "Option::is_none")]
    pub hugepage: Option<String>,
    #[serde(default, rename = "_cc_harder", skip_serializing_if = "Option::is_none")]
    pub cc_harder: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PgoSection {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_pgo_preset")]
    pub preset: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profiles_archive_dir: Option<String>,
    #[serde(default = "default_auto")]
    pub profile_scratch_dir: String,
    #[serde(default = "default_true")]
    pub perf_data_on_ram: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark_command: Option<String>,
    #[serde(default = "default_benchmark_preset")]
    pub benchmark_preset: String,
    #[serde(default = "default_profiling_quality")]
    pub profiling_quality: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark_workdir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_user: Option<String>,
    #[serde(default = "default_auto")]
    pub perf_event_args: String,
    #[serde(default = "default_perf_extra_args")]
    pub perf_extra_args: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sysctl_command: Option<String>,
    #[serde(default = "default_auto")]
    pub vmlinux: String,
    #[serde(default = "default_afdo_tool")]
    pub afdo_tool: String,
    #[serde(default = "default_propeller_tool")]
    pub propeller_tool: String,
    #[serde(default = "default_afdo_profile_name")]
    pub afdo_profile_name: String,
    #[serde(default = "default_true")]
    pub verify_boot: bool,
    #[serde(default)]
    pub auto_restart: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_file: Option<String>,
}

fn default_pgo_preset() -> String {
    "cachyos-kernel".into()
}

fn default_afdo_tool() -> String {
    "llvm-profgen".into()
}

fn default_propeller_tool() -> String {
    "create_llvm_prof".into()
}

fn default_afdo_profile_name() -> String {
    "kernel-compilation.afdo".into()
}

fn default_perf_extra_args() -> String {
    "--mmap-pages 131072 -a -N -b -c 56000".into()
}

fn default_perf_event_args() -> String {
    "auto".into()
}

fn default_auto() -> String {
    "auto".into()
}

fn default_benchmark_preset() -> String {
    "fast".into()
}

fn default_profiling_quality() -> String {
    "maximum".into()
}

impl Default for PgoSection {
    fn default() -> Self {
        Self {
            enabled: true,
            preset: default_pgo_preset(),
            profiles_archive_dir: None,
            profile_scratch_dir: default_auto(),
            perf_data_on_ram: true,
            benchmark_command: None,
            benchmark_preset: default_benchmark_preset(),
            profiling_quality: default_profiling_quality(),
            benchmark_workdir: None,
            build_user: None,
            perf_event_args: default_perf_event_args(),
            perf_extra_args: default_perf_extra_args(),
            sysctl_command: None,
            vmlinux: default_auto(),
            afdo_tool: default_afdo_tool(),
            propeller_tool: default_propeller_tool(),
            afdo_profile_name: default_afdo_profile_name(),
            verify_boot: true,
            auto_restart: false,
            state_file: None,
        }
    }
}

/// Built-in default template used when a kernel is configured for the first time.
pub fn default_kernel_template() -> PackageSection {
    PackageSection {
        source: Some("aur".into()),
        build_env: Some("local".into()),
        ramdisk: Some("wr".into()),
        kernel: Some(KernelSection {
            cpusched: Some("cachyos".into()),
            processor_opt: Some("native".into()),
            use_llvm_lto: Some("thin".into()),
            hz_ticks: Some("1000".into()),
            tickrate: Some("full".into()),
            preempt: Some("full".into()),
            ..Default::default()
        }),
        pgo: Some(PgoSection::default()),
        ..Default::default()
    }
}

impl Default for ConfigDocument {
    fn default() -> Self {
        Self {
            config_version: 1,
            check_for_update_on_startup: None,
            auto_update_on_startup: None,
            self_update_at_updates: None,
            self_update_raw_url: None,
            self_update_install_path: None,
            self_update_use_pacman: None,
            install_testing_phase_archlinux_packages: None,
            manual_update_packages: Vec::new(),
            skip_install_packages: Vec::new(),
            skip_install_packages_after_compilation: None,
            paths: PathsSection::default(),
            ramdisk: RamdiskSection::default(),
            build: BuildSection::default(),
            system_update: SystemUpdateSection::default(),
            repositories: default_repositories(),
            compilers: HashMap::new(),
            packages: HashMap::new(),
            kernel_defaults: default_kernel_template(),
        }
    }
}

fn default_repositories() -> HashMap<String, String> {
    HashMap::from([
        (
            "arch".into(),
            "https://gitlab.archlinux.org/archlinux/packaging/packages".into(),
        ),
        ("aur".into(), "https://aur.archlinux.org".into()),
        (
            "cachyos".into(),
            "https://github.com/CachyOS/CachyOS-PKGBUILDS.git".into(),
        ),
        ("default".into(), "arch".into()),
    ])
}

impl ConfigDocument {
    /// Ensure a kernel package exists, seeding it from `kernel_defaults` on first use.
    pub fn ensure_kernel_from_defaults(&mut self, name: &str) {
        if !self.packages.contains_key(name) {
            let template = self.kernel_defaults.clone();
            self.packages.insert(name.to_string(), template);
        }
        let pkg = self.packages.get_mut(name).expect("just inserted");
        if pkg.kernel.is_none() {
            pkg.kernel = Some(KernelSection::default());
        }
        if pkg.pgo.is_none() {
            pkg.pgo = Some(PgoSection::default());
        }
    }
}
