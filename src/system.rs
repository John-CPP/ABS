use crate::config::Config;
use crate::utils::{parse_command_argv, run_argv_command};
use crate::{die, vlog};
use colored::Colorize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemUpdateMode {
    UpdateRepositories,
    PerformUpdateWithRefresh,
    PerformUpdateNoRefresh,
}

pub(crate) fn is_root() -> bool {
    if let Ok(output) = std::process::Command::new("id").arg("-u").output()
        && let Ok(uid_str) = std::str::from_utf8(&output.stdout)
        && let Ok(uid) = uid_str.trim().parse::<u32>()
    {
        return uid == 0;
    }
    if let Ok(user) = std::env::var("USER") {
        return user == "root";
    }
    false
}

pub(crate) fn transform_system_update_argv(
    mut argv: Vec<String>,
    is_root_user: bool,
) -> Vec<String> {
    let cmd = argv.first().map(String::as_str).unwrap_or("");
    if (cmd == "pacman" || cmd.ends_with("/pacman")) && !is_root_user {
        argv.insert(0, "sudo".into());
    }
    argv
}

fn argv_has_flag(argv: &[String], flag: &str) -> bool {
    let eq = format!("{flag}=");
    argv.iter().any(|a| a == flag || a.starts_with(&eq))
}

fn push_flag(argv: &mut Vec<String>, flag: &str) {
    if !argv_has_flag(argv, flag) {
        argv.push(flag.into());
    }
}

fn push_flag_value(argv: &mut Vec<String>, flag: &str, value: &str) {
    if !argv_has_flag(argv, flag) {
        argv.push(flag.into());
        argv.push(value.into());
    }
}

fn update_helper_bin(argv: &[String]) -> &str {
    let mut i = 0;
    if argv
        .first()
        .is_some_and(|c| c == "sudo" || c.ends_with("/sudo"))
    {
        i = 1;
    }
    argv.get(i)
        .map(|s| s.rsplit('/').next().unwrap_or(s.as_str()))
        .unwrap_or("")
}

/// When launched from absgui, skip pacman/yay/paru/pikaur confirmation menus
/// (e.g. yay's "Packages to exclude"). The GUI reply field is for abs's own
/// prompts (install compiled artifacts), not the helper's upgrade TUI.
pub(crate) fn apply_noninteractive_update_flags(argv: &mut Vec<String>, gui: bool) {
    if !gui {
        return;
    }
    push_flag(argv, "--noconfirm");
    match update_helper_bin(argv) {
        "yay" => {
            // `--noconfirm` still leaves yay's numbered exclude/clean/diff/edit menus.
            push_flag_value(argv, "--answerupgrade", "None");
            push_flag_value(argv, "--answerclean", "None");
            push_flag_value(argv, "--answerdiff", "None");
            push_flag_value(argv, "--answeredit", "None");
        }
        "paru" => {
            push_flag(argv, "--skipreview");
        }
        "pikaur" => {
            push_flag(argv, "--noedit");
        }
        _ => {}
    }
}

pub(crate) fn packages_ignored_during_system_update(config: &Config) -> Vec<String> {
    let held = crate::held::held_names(config);
    let raw: Vec<String> = config
        .system_update
        .ignore_packages
        .iter()
        .chain(config.manual_update_packages.iter())
        .chain(config.skip_install_packages.iter())
        .chain(held.iter())
        .chain(crate::pgo::active_pipeline_hold_packages(config).iter())
        .cloned()
        .collect();
    crate::package_pattern::expand_package_patterns(&raw)
}

/// Always appends `ignore_flag` for each entry in `ignore_packages`, `manual_update_packages`,
/// and `skip_install_packages` (deduped), so repo packages never replace packages you build with ABS.
///
/// Returns `false` when a kernel PGO pipeline is in progress and the update was skipped.
pub fn run_system_update(config: &Config, mode: SystemUpdateMode) -> bool {
    crate::utils::apply_gui_nested_sudo_askpass();
    let active = crate::pgo::active_pipelines(config);
    if !active.is_empty() {
        warn_system_update_blocked_during_pgo(&active, mode);
        return false;
    }

    let cmd_str = match mode {
        SystemUpdateMode::UpdateRepositories => {
            config.system_update.command_to_update_repositories.clone()
        }
        SystemUpdateMode::PerformUpdateWithRefresh => config
            .system_update
            .command_to_perform_system_update
            .clone(),
        SystemUpdateMode::PerformUpdateNoRefresh => config
            .system_update
            .get_command_to_perform_system_update_no_refresh(),
    };

    let mut argv = match parse_command_argv(&cmd_str) {
        Ok(v) => transform_system_update_argv(v, is_root()),
        Err(e) => die!("Invalid system update command: {e}"),
    };

    apply_noninteractive_update_flags(&mut argv, std::env::var_os("ABS_GUI").is_some());

    for pkg in packages_ignored_during_system_update(config) {
        argv.push(config.system_update.ignore_flag.clone());
        argv.push(pkg);
    }

    vlog!("Executing system update: {}", argv.join(" "));

    if zram_for_system_update(mode) {
        crate::zram::require_headroom(
            "system update",
            config.ramdisk.min_free_ram_mb.saturating_mul(1024 * 1024),
            config.zram_mode_for(None).unwrap_or_else(|e| die!("{e}")),
        );
    }

    if let Err(e) = run_argv_command(&argv, None::<&str>) {
        die!("System update failed: {}", e);
    }
    true
}

/// Repo refresh (`pacman -Sy` / pending-list fetch) is not a compile. Zram is for `-U` / `-RU`.
fn zram_for_system_update(mode: SystemUpdateMode) -> bool {
    !matches!(mode, SystemUpdateMode::UpdateRepositories)
}

fn system_update_mode_label(mode: SystemUpdateMode) -> &'static str {
    match mode {
        SystemUpdateMode::UpdateRepositories => "repository refresh",
        SystemUpdateMode::PerformUpdateWithRefresh | SystemUpdateMode::PerformUpdateNoRefresh => {
            "system update"
        }
    }
}

pub(crate) fn warn_system_update_blocked_during_pgo(
    pipelines: &[crate::pgo::ActivePgoPipeline],
    mode: SystemUpdateMode,
) {
    let action = system_update_mode_label(mode);
    eprintln!();
    eprintln!(
        "{} {}",
        "==> PGO IN PROGRESS — SYSTEM UPDATE SKIPPED".red().bold(),
        format!("({action} blocked while kernel PGO pipeline(s) are active)")
            .yellow()
            .bold()
    );
    for pipeline in pipelines {
        eprintln!(
            "    {} {} — {}",
            "•".yellow().bold(),
            pipeline.package.yellow().bold(),
            pipeline.stage_label.yellow()
        );
    }
    eprintln!(
        "    {} Finish with {} or abandon with {} before running system updates.",
        "Hint:".bold(),
        "`abs --pgo-resume PKG`".cyan(),
        "`abs --pgo-abort PKG`".cyan()
    );
    eprintln!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BuildConfig, Config, PathsConfig, SystemUpdateConfig};

    fn minimal_config(skip_install: Vec<&str>, manual: Vec<&str>, ignore: Vec<&str>) -> Config {
        Config {
            config_version: 1,
            paths: PathsConfig {
                packages_path: "/tmp/p".into(),
                chroot_base_path: "/tmp/c".into(),
                ready_made_packages_path: "/tmp/r".into(),
                chroot_makepkg_conf: None,
            },
            build: BuildConfig {
                default_environment: "local".into(),
                ignore_compilation_failures: false,
                compile_first_install_after: false,
                clean_install_by_default: false,
                ignore_already_made_packages: false,
                concurrent_repos_downloads_limit: 1,
                concurrent_compilations_limit: 1,
                fast_aur_rpc_update_checks: false,
                system_update_first: false,
                clean_chroot_after_compilation: true,
                global_cpu_threads_mode: "strict".into(),
                global_cpu_threads_cap: None,
                maximum_cpu_threads_cap: None,
                default_compilation_threads: None,
                default_compiler: None,
                check_for_update_on_startup: None,
                auto_update_on_startup: None,
                self_update_at_updates: None,
                self_update_install_path: None,
                install_testing_phase_archlinux_packages: None,
            },
            system_update: SystemUpdateConfig {
                command_to_update_repositories: "pacman -Sy".into(),
                command_to_perform_system_update: "pacman -Syu".into(),
                command_to_perform_system_update_no_refresh: None,
                ignore_flag: "--ignore".into(),
                ignore_packages: ignore.into_iter().map(String::from).collect(),
                auto_refresh_delay: 0,
                remember_sudo: false,
            },
            repositories: Default::default(),
            manual_update_packages: manual.into_iter().map(String::from).collect(),
            skip_install_packages: skip_install.into_iter().map(String::from).collect(),
            skip_install_packages_after_compilation: None,
            held_packages: Default::default(),
            packages: Default::default(),
            check_for_update_on_startup: false,
            auto_update_on_startup: false,
            self_update_install_path: String::new(),
            self_update_use_pacman: None,
            self_update_at_updates: false,
            install_absgui: None,
            install_testing_phase_archlinux_packages: false,
            compilers: Default::default(),
            ramdisk: Default::default(),
            lang: None,
        }
    }

    #[test]
    fn packages_ignored_includes_held() {
        let mut config = minimal_config(vec![], vec![], vec![]);
        config.held_packages = vec![crate::config::HeldPackage {
            name: "heldpkg".into(),
            version: "1.0.0-1".into(),
            auto_recompile_trigger: Default::default(),
        }];
        assert!(packages_ignored_during_system_update(&config).contains(&"heldpkg".into()));
    }

    #[test]
    fn packages_ignored_includes_skip_install_and_dedupes() {
        let config = minimal_config(vec!["foo"], vec!["foo", "bar"], vec!["baz"]);
        assert_eq!(
            packages_ignored_during_system_update(&config),
            vec!["baz", "foo", "bar"]
        );
    }

    #[test]
    fn system_update_mode_label_names() {
        assert_eq!(
            system_update_mode_label(SystemUpdateMode::UpdateRepositories),
            "repository refresh"
        );
        assert_eq!(
            system_update_mode_label(SystemUpdateMode::PerformUpdateWithRefresh),
            "system update"
        );
        assert_eq!(
            system_update_mode_label(SystemUpdateMode::PerformUpdateNoRefresh),
            "system update"
        );
        assert!(!zram_for_system_update(
            SystemUpdateMode::UpdateRepositories
        ));
        assert!(zram_for_system_update(
            SystemUpdateMode::PerformUpdateWithRefresh
        ));
        assert!(zram_for_system_update(
            SystemUpdateMode::PerformUpdateNoRefresh
        ));
    }

    #[test]
    fn run_system_update_skipped_when_pgo_pipeline_active() {
        use crate::config::{PackageConfig, PgoConfig};
        use crate::pgo::{PgoStageId, PgoState};

        let dir = std::env::temp_dir().join(format!("abs-sys-pgo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let state_path = dir.join("linux-cachyos.json");
        let state = PgoState {
            package: "linux-cachyos".into(),
            repo_dir: "/tmp/repo".into(),
            current_stage: PgoStageId::WaitReboot2,
            started_at: 0,
            updated_at: 0,
            expected_kernel_uname: None,
            expected_package_base: None,
            stage_history: vec![],
            compare_run_dir: None,
        };
        std::fs::write(&state_path, serde_json::to_string_pretty(&state).unwrap()).unwrap();

        let mut config = minimal_config(vec![], vec![], vec![]);
        let pgo = PgoConfig {
            enabled: true,
            preset: "cachyos-kernel".into(),
            profiles_archive_dir: Some(dir.to_string_lossy().into_owned()),
            profile_scratch_dir: "auto".into(),
            perf_data_on_ram: true,
            propeller_profiles_on_ram: true,
            convert_relocate: "force".into(),
            benchmark_command: None,
            benchmark_workdir: None,
            benchmark_preset: "kernel".into(),
            compare_preset: "auto".into(),
            kernel_workload_seconds: 0,
            profiling_quality: "sweet".into(),
            build_user: None,
            perf_event_args: "auto".into(),
            perf_extra_args: crate::config::PERF_EXTRA_ARGS_STANDARD.into(),
            sysctl_command: None,
            vmlinux: "auto".into(),
            afdo_tool: "llvm-profgen".into(),
            propeller_tool: "create_llvm_prof".into(),
            afdo_profile_name: "kernel-compilation.afdo".into(),
            verify_boot: true,
            select_boot_kernel: true,
            auto_restart: false,
            reboot_before_start: false,
            reuse_afdo_profile: false,
            reuse_propeller_profile: false,
            skip_propeller: false,
            compare_current: false,
            compare_debug: false,
            compare_debug_clean: false,
            compare_autofdo: false,
            compare_autofdo_clean: false,
            compare_final: false,
            state_file: Some(state_path.to_string_lossy().into_owned()),
        };
        config.packages.insert(
            "linux-cachyos".into(),
            PackageConfig {
                pgo: Some(pgo),
                ..Default::default()
            },
        );

        assert!(!run_system_update(
            &config,
            SystemUpdateMode::PerformUpdateWithRefresh
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_transform_system_update_command() {
        // Non-root user: pacman commands should get sudo prepended
        assert_eq!(
            transform_system_update_argv(vec!["pacman".into(), "-Su".into()], false),
            vec!["sudo", "pacman", "-Su"]
        );
        assert_eq!(
            transform_system_update_argv(vec!["pacman".into()], false),
            vec!["sudo", "pacman"]
        );

        // Root user: pacman commands should NOT get sudo prepended
        assert_eq!(
            transform_system_update_argv(vec!["pacman".into(), "-Su".into()], true),
            vec!["pacman", "-Su"]
        );

        // Already has sudo: should NOT get sudo prepended for either
        assert_eq!(
            transform_system_update_argv(vec!["sudo".into(), "pacman".into(), "-Su".into()], false),
            vec!["sudo", "pacman", "-Su"]
        );
        assert_eq!(
            transform_system_update_argv(vec!["sudo".into(), "pacman".into(), "-Su".into()], true),
            vec!["sudo", "pacman", "-Su"]
        );

        // Non-pacman command (e.g. yay): should NOT get sudo prepended
        assert_eq!(
            transform_system_update_argv(vec!["yay".into(), "-Su".into()], false),
            vec!["yay", "-Su"]
        );
    }

    #[test]
    fn gui_flags_skipped_when_not_gui() {
        let mut argv = vec!["yay".into(), "-Syu".into()];
        apply_noninteractive_update_flags(&mut argv, false);
        assert_eq!(argv, vec!["yay", "-Syu"]);
    }

    #[test]
    fn gui_yay_gets_noconfirm_and_answer_flags() {
        let mut argv = vec!["yay".into(), "-Syu".into()];
        apply_noninteractive_update_flags(&mut argv, true);
        assert!(argv.iter().any(|a| a == "--noconfirm"));
        assert!(
            argv.windows(2)
                .any(|w| w[0] == "--answerupgrade" && w[1] == "None")
        );
        assert!(
            argv.windows(2)
                .any(|w| w[0] == "--answerclean" && w[1] == "None")
        );
        assert!(
            argv.windows(2)
                .any(|w| w[0] == "--answerdiff" && w[1] == "None")
        );
        assert!(
            argv.windows(2)
                .any(|w| w[0] == "--answeredit" && w[1] == "None")
        );
    }

    #[test]
    fn gui_does_not_duplicate_existing_flags() {
        let mut argv = vec![
            "yay".into(),
            "-Syu".into(),
            "--noconfirm".into(),
            "--answerupgrade".into(),
            "All".into(),
        ];
        apply_noninteractive_update_flags(&mut argv, true);
        assert_eq!(argv.iter().filter(|a| *a == "--noconfirm").count(), 1);
        assert_eq!(argv.iter().filter(|a| *a == "--answerupgrade").count(), 1);
        assert!(
            argv.windows(2)
                .any(|w| w[0] == "--answerupgrade" && w[1] == "All")
        );
    }

    #[test]
    fn gui_paru_gets_skipreview() {
        let mut argv = vec!["paru".into(), "-Syu".into()];
        apply_noninteractive_update_flags(&mut argv, true);
        assert!(argv.contains(&"--noconfirm".into()));
        assert!(argv.contains(&"--skipreview".into()));
    }

    #[test]
    fn gui_pacman_gets_noconfirm_only() {
        let mut argv = vec!["sudo".into(), "pacman".into(), "-Syu".into()];
        apply_noninteractive_update_flags(&mut argv, true);
        assert_eq!(argv, vec!["sudo", "pacman", "-Syu", "--noconfirm"]);
    }
}
