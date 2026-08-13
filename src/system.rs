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

fn is_root() -> bool {
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

fn transform_system_update_argv(mut argv: Vec<String>, is_root_user: bool) -> Vec<String> {
    let cmd = argv.first().map(String::as_str).unwrap_or("");
    if (cmd == "pacman" || cmd.ends_with("/pacman")) && !is_root_user {
        argv.insert(0, "sudo".into());
    }
    argv
}

fn packages_ignored_during_system_update(config: &Config) -> Vec<String> {
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

    for pkg in packages_ignored_during_system_update(config) {
        argv.push(config.system_update.ignore_flag.clone());
        argv.push(pkg);
    }

    vlog!("Executing system update: {}", argv.join(" "));

    if let Err(e) = run_argv_command(&argv, None::<&str>) {
        die!("System update failed: {}", e);
    }
    true
}

fn system_update_mode_label(mode: SystemUpdateMode) -> &'static str {
    match mode {
        SystemUpdateMode::UpdateRepositories => "repository refresh",
        SystemUpdateMode::PerformUpdateWithRefresh | SystemUpdateMode::PerformUpdateNoRefresh => {
            "system update"
        }
    }
}

fn warn_system_update_blocked_during_pgo(
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
                self_update_raw_url: None,
                self_update_install_path: None,
                install_testing_phase_archlinux_packages: None,
            },
            system_update: SystemUpdateConfig {
                command_to_update_repositories: "pacman -Sy".into(),
                command_to_perform_system_update: "pacman -Syu".into(),
                command_to_perform_system_update_no_refresh: None,
                ignore_flag: "--ignore".into(),
                ignore_packages: ignore.into_iter().map(String::from).collect(),
            },
            repositories: Default::default(),
            manual_update_packages: manual.into_iter().map(String::from).collect(),
            skip_install_packages: skip_install.into_iter().map(String::from).collect(),
            skip_install_packages_after_compilation: None,
            held_packages: Default::default(),
            packages: Default::default(),
            check_for_update_on_startup: false,
            auto_update_on_startup: false,
            self_update_raw_url: String::new(),
            self_update_install_path: String::new(),
            self_update_use_pacman: None,
            self_update_at_updates: false,
            install_testing_phase_archlinux_packages: false,
            compilers: Default::default(),
            ramdisk: Default::default(),
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
        };
        std::fs::write(&state_path, serde_json::to_string_pretty(&state).unwrap()).unwrap();

        let mut config = minimal_config(vec![], vec![], vec![]);
        let pgo = PgoConfig {
            enabled: true,
            preset: "cachyos-kernel".into(),
            profiles_archive_dir: Some(dir.to_string_lossy().into_owned()),
            profile_scratch_dir: "auto".into(),
            perf_data_on_ram: true,
            benchmark_command: None,
            benchmark_workdir: None,
            benchmark_preset: "fast".into(),
            profiling_quality: "maximum".into(),
            build_user: None,
            perf_event_args: "auto".into(),
            perf_extra_args: crate::config::PERF_EXTRA_ARGS_STANDARD.into(),
            sysctl_command: None,
            vmlinux: "auto".into(),
            afdo_tool: "llvm-profgen".into(),
            propeller_tool: "create_llvm_prof".into(),
            afdo_profile_name: "kernel-compilation.afdo".into(),
            verify_boot: true,
            auto_restart: false,
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
}
