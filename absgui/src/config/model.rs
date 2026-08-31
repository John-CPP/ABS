use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Full ABS config document.
///
/// Field ordering matters for TOML output: all scalar/array ("value") fields are
/// declared before any table fields (sub-structs / maps) so the emitted TOML is
/// always valid. Fields the CLI requires (no `#[serde(default)]` on its side) are
/// always serialized; optional ones are skipped when empty to keep the file clean.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    pub self_update_install_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_update_use_pacman: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_absgui: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_testing_phase_archlinux_packages: Option<bool>,

    // Required arrays (CLI has no default for these).
    #[serde(default)]
    pub manual_update_packages: Vec<String>,
    #[serde(default)]
    pub skip_install_packages: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_install_packages_after_compilation: Option<Vec<String>>,

    /// Packages pinned to a fixed pkgver-pkgrel (mirrors CLI `held_packages`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub held_packages: Vec<HeldPackage>,

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

/// A package held at a fixed version with optional auto-recompile triggers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct HeldPackage {
    pub name: String,
    /// `pkgver-pkgrel` (epoch may be embedded in pkgver).
    pub version: String,
    #[serde(default)]
    pub auto_recompile_trigger: AutoRecompileTrigger,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AutoRecompileTrigger {
    /// Trigger package name -> last recorded installed version.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub on_packages_updated: HashMap<String, String>,
}

impl HeldPackage {
    /// Serialize triggers as `name=version` (or bare `name`) entries, comma-separated.
    pub fn triggers_text(&self) -> String {
        let mut pairs: Vec<_> = self
            .auto_recompile_trigger
            .on_packages_updated
            .iter()
            .collect();
        pairs.sort_by(|a, b| a.0.cmp(b.0));
        pairs
            .into_iter()
            .map(|(k, v)| {
                if v.is_empty() {
                    k.clone()
                } else {
                    format!("{k}={v}")
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Parse triggers from commas and/or newlines (`name` or `name=version`).
    pub fn set_triggers_from_text(&mut self, text: &str) {
        let mut map = HashMap::new();
        for part in text.split([',', '\n']) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if let Some((name, ver)) = part.split_once('=') {
                let name = name.trim();
                if !name.is_empty() {
                    map.insert(name.to_string(), ver.trim().to_string());
                }
            } else {
                map.insert(part.to_string(), String::new());
            }
        }
        self.auto_recompile_trigger.on_packages_updated = map;
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompilerSection {
    pub cc: String,
    pub cxx: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
            packages_path: "$XDG_CACHE_HOME/abs/packages".into(),
            chroot_base_path: "$XDG_CACHE_HOME/abs/chroot".into(),
            ready_made_packages_path: "$XDG_CACHE_HOME/abs/ready".into(),
            chroot_makepkg_conf: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildSection {
    #[serde(default = "default_local")]
    pub default_environment: String,
    #[serde(default)]
    pub ignore_compilation_failures: bool,
    #[serde(default)]
    pub compile_first_install_after: bool,
    #[serde(default)]
    pub clean_install_by_default: bool,
    #[serde(default)]
    pub ignore_already_made_packages: bool,
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
            ignore_already_made_packages: false,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemUpdateSection {
    #[serde(default = "default_pacman_sy", alias = "command")]
    pub command_to_update_repositories: String,
    #[serde(default = "default_pacman_syu", alias = "command_with_refresh")]
    pub command_to_perform_system_update: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "command_no_refresh"
    )]
    pub command_to_perform_system_update_no_refresh: Option<String>,
    #[serde(default = "default_ignore")]
    pub ignore_flag: String,
    #[serde(default)]
    pub ignore_packages: Vec<String>,
    /// Minutes between automatic pending-update fetches. `0` = only the Refresh button
    /// (first visit this session still loads the list). Accepts a TOML integer or string.
    #[serde(
        default,
        alias = "system_update_auto_refresh_delay",
        deserialize_with = "deserialize_u32_from_int_or_str"
    )]
    pub auto_refresh_delay: u32,
    /// When true, AbsGui keeps the sudo password in a private runtime file until the app exits.
    #[serde(default, alias = "system_update_remember_sudo")]
    pub remember_sudo: bool,
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
            auto_refresh_delay: 0,
            remember_sudo: false,
        }
    }
}

/// Whether AbsGui should fetch the pending-update list.
///
/// A missing list (first visit this session) always fetches. `delay_minutes == 0`
/// never auto-refreshes after a list is already in hand.
pub fn pending_list_needs_fetch(
    delay_minutes: u32,
    last_refresh: Option<Instant>,
    now: Instant,
    have_list: bool,
) -> bool {
    if !have_list {
        return true;
    }
    if delay_minutes == 0 {
        return false;
    }
    let Some(last) = last_refresh else {
        return true;
    };
    now.saturating_duration_since(last) >= Duration::from_secs(u64::from(delay_minutes) * 60)
}

fn deserialize_u32_from_int_or_str<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};
    use std::fmt;

    struct U32Loose;

    impl<'de> Visitor<'de> for U32Loose {
        type Value = u32;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a non-negative integer or a numeric string")
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<u32, E> {
            u32::try_from(v).map_err(E::custom)
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<u32, E> {
            u32::try_from(v).map_err(E::custom)
        }

        fn visit_u32<E: de::Error>(self, v: u32) -> Result<u32, E> {
            Ok(v)
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<u32, E> {
            v.trim().parse().map_err(E::custom)
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<u32, E> {
            self.visit_str(&v)
        }
    }

    deserializer.deserialize_any(U32Loose)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    #[serde(default = "default_zram")]
    pub zram: String,
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
fn default_zram() -> String {
    "full".into()
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
            zram: default_zram(),
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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
    pub zram: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compilation_threads: Option<usize>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub compile_alone: bool,
    #[serde(
        default = "default_compilation_priority",
        skip_serializing_if = "is_default_priority"
    )]
    pub compilation_priority: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignore_already_made_packages: Option<bool>,
    // Tables last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel: Option<KernelSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pgo: Option<PgoSection>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
    #[serde(
        default,
        rename = "_cc_harder",
        skip_serializing_if = "Option::is_none"
    )]
    pub cc_harder: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PgoSection {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_pgo_preset")]
    pub preset: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profiles_archive_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_kernels_dir: Option<String>,
    #[serde(default = "default_auto")]
    pub profile_scratch_dir: String,
    #[serde(default = "default_true")]
    pub perf_data_on_ram: bool,
    #[serde(default = "default_true")]
    pub propeller_profiles_on_ram: bool,
    #[serde(default = "default_convert_relocate")]
    pub convert_relocate: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark_command: Option<String>,
    #[serde(default = "default_benchmark_preset")]
    pub benchmark_preset: String,
    #[serde(default = "default_compare_preset")]
    pub compare_preset: String,
    #[serde(default = "default_kernel_workload_seconds")]
    pub kernel_workload_seconds: u32,
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
    #[serde(default = "default_true")]
    pub select_boot_kernel: bool,
    #[serde(default)]
    pub auto_restart: bool,
    #[serde(default)]
    pub reboot_before_start: bool,
    #[serde(default)]
    pub shutdown_after_finish: bool,
    #[serde(default)]
    pub reuse_afdo_profile: bool,
    #[serde(default)]
    pub reuse_propeller_profile: bool,
    #[serde(default)]
    pub skip_propeller: bool,
    #[serde(default)]
    pub compare_current: bool,
    #[serde(default)]
    pub compare_debug: bool,
    #[serde(default)]
    pub compare_debug_clean: bool,
    #[serde(default)]
    pub compare_autofdo: bool,
    #[serde(default)]
    pub compare_autofdo_clean: bool,
    #[serde(default)]
    pub compare_final: bool,
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
    "auto".into()
}

fn default_afdo_profile_name() -> String {
    "kernel-compilation.afdo".into()
}

fn default_perf_extra_args() -> String {
    "--mmap-pages 4096 -a -N -b -c 1000003".into()
}

fn default_perf_event_args() -> String {
    "auto".into()
}

fn default_auto() -> String {
    "auto".into()
}

fn default_benchmark_preset() -> String {
    "kernel".into()
}

fn default_compare_preset() -> String {
    "auto".into()
}

fn default_kernel_workload_seconds() -> u32 {
    0
}

fn default_profiling_quality() -> String {
    "sweet".into()
}

fn default_convert_relocate() -> String {
    "force".into()
}

impl Default for PgoSection {
    fn default() -> Self {
        Self {
            enabled: true,
            preset: default_pgo_preset(),
            profiles_archive_dir: None,
            save_kernels_dir: None,
            profile_scratch_dir: default_auto(),
            perf_data_on_ram: true,
            propeller_profiles_on_ram: true,
            convert_relocate: default_convert_relocate(),
            benchmark_command: None,
            benchmark_preset: default_benchmark_preset(),
            compare_preset: default_compare_preset(),
            kernel_workload_seconds: default_kernel_workload_seconds(),
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
            select_boot_kernel: true,
            auto_restart: false,
            reboot_before_start: false,
            shutdown_after_finish: false,
            reuse_afdo_profile: false,
            reuse_propeller_profile: false,
            skip_propeller: false,
            compare_current: false,
            compare_debug: false,
            compare_debug_clean: false,
            compare_autofdo: false,
            compare_autofdo_clean: false,
            compare_final: false,
            state_file: None,
        }
    }
}

/// Built-in kernel pick defaults (CachyOS PKGBUILD-aligned) used when a field is unset.
pub fn default_kernel_section() -> KernelSection {
    KernelSection {
        cpusched: Some("cachyos".into()),
        processor_opt: Some("native".into()),
        use_llvm_lto: Some("thin".into()),
        hz_ticks: Some("1000".into()),
        tickrate: Some("full".into()),
        preempt: Some("full".into()),
        hugepage: Some("always".into()),
        cc_harder: Some("yes".into()),
        ..Default::default()
    }
}

impl KernelSection {
    /// True when the user has never chosen any kernel option (all fields unset).
    pub fn is_unset(&self) -> bool {
        *self == Self::default()
    }
}

/// Built-in default template used when a kernel is configured for the first time.
pub fn default_kernel_template() -> PackageSection {
    PackageSection {
        source: Some("aur".into()),
        build_env: Some("local".into()),
        ramdisk: Some("wr".into()),
        kernel: Some(default_kernel_section()),
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
            self_update_install_path: None,
            self_update_use_pacman: None,
            install_absgui: None,
            lang: None,
            install_testing_phase_archlinux_packages: None,
            manual_update_packages: Vec::new(),
            skip_install_packages: Vec::new(),
            skip_install_packages_after_compilation: None,
            held_packages: Vec::new(),
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
    ///
    /// Packages that already exist but have no kernel options yet (the user never
    /// picked any) also receive the default picks so the kernel page is not empty.
    pub fn ensure_kernel_from_defaults(&mut self, name: &str) {
        if !self.packages.contains_key(name) {
            let template = self.kernel_defaults.clone();
            self.packages.insert(name.to_string(), template);
        }
        let default_kernel = self
            .kernel_defaults
            .kernel
            .clone()
            .filter(|k| !k.is_unset())
            .unwrap_or_else(default_kernel_section);
        let default_pgo = self
            .kernel_defaults
            .pgo
            .clone()
            .unwrap_or_else(PgoSection::default);
        let default_source = self.kernel_defaults.source.clone();
        let default_build_env = self.kernel_defaults.build_env.clone();
        let default_ramdisk = self.kernel_defaults.ramdisk.clone();
        let default_zram = self.kernel_defaults.zram.clone();

        let pkg = self.packages.get_mut(name).expect("just inserted");
        if pkg.kernel.as_ref().is_none_or(KernelSection::is_unset) {
            pkg.kernel = Some(default_kernel);
        }
        if pkg.pgo.is_none() {
            pkg.pgo = Some(default_pgo);
        }
        if pkg.source.is_none() {
            pkg.source = default_source;
        }
        if pkg.build_env.is_none() {
            pkg.build_env = default_build_env;
        }
        if pkg.ramdisk.is_none() {
            pkg.ramdisk = default_ramdisk;
        }
        if pkg.zram.is_none() {
            pkg.zram = default_zram;
        }
    }
}

#[cfg(test)]
mod pending_refresh_tests {
    use super::pending_list_needs_fetch;
    use std::time::{Duration, Instant};

    fn ago(now: Instant, minutes: u64) -> Instant {
        now.checked_sub(Duration::from_secs(minutes * 60))
            .expect("instant")
    }

    #[test]
    fn first_visit_always_fetches() {
        let now = Instant::now();
        assert!(pending_list_needs_fetch(0, None, now, false));
        assert!(pending_list_needs_fetch(15, None, now, false));
    }

    #[test]
    fn zero_delay_never_auto_refreshes_after_a_list_exists() {
        let now = Instant::now();
        assert!(!pending_list_needs_fetch(0, Some(ago(now, 120)), now, true));
    }

    #[test]
    fn positive_delay_refreshes_when_interval_elapsed() {
        let now = Instant::now();
        assert!(!pending_list_needs_fetch(15, Some(ago(now, 14)), now, true));
        assert!(pending_list_needs_fetch(15, Some(ago(now, 15)), now, true));
        assert!(pending_list_needs_fetch(15, None, now, true));
    }
}
