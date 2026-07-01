use crate::config::Config;
use crate::utils::{run_command, sh_single_quote};
use crate::{die, vlog};
use colored::Colorize;
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemUpdateMode {
    UpdateRepositories,
    PerformUpdateWithRefresh,
    PerformUpdateNoRefresh,
}

fn is_root() -> bool {
    if let Ok(output) = std::process::Command::new("id").arg("-u").output()
        && let Ok(uid_str) = std::str::from_utf8(&output.stdout)
            && let Ok(uid) = uid_str.trim().parse::<u32>() {
                return uid == 0;
            }
    if let Ok(user) = std::env::var("USER") {
        return user == "root";
    }
    false
}

fn transform_system_update_command(mut cmd_str: String, is_root_user: bool) -> String {
    let trimmed = cmd_str.trim();
    if (trimmed.starts_with("pacman ") || trimmed == "pacman")
        && !trimmed.starts_with("sudo ")
        && !is_root_user
    {
        cmd_str = format!("sudo {}", cmd_str);
    }
    cmd_str
}

fn packages_ignored_during_system_update(config: &Config) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for pkg in config
        .system_update
        .ignore_packages
        .iter()
        .chain(config.manual_update_packages.iter())
        .chain(config.skip_install_packages.iter())
        .chain(crate::pgo::active_pipeline_hold_packages(config).iter())
    {
        if seen.insert(pkg.clone()) {
            out.push(pkg.clone());
        }
    }
    out
}

/// Always appends `ignore_flag` for each entry in `ignore_packages`, `manual_update_packages`,
/// and `skip_install_packages` (deduped), so repo packages never replace packages you build with ABS.
pub fn run_system_update(config: &Config, mode: SystemUpdateMode) {
    let mut cmd_str = match mode {
        SystemUpdateMode::UpdateRepositories => {
            config.system_update.command_to_update_repositories.clone()
        }
        SystemUpdateMode::PerformUpdateWithRefresh => {
            config.system_update.command_to_perform_system_update.clone()
        }
        SystemUpdateMode::PerformUpdateNoRefresh => {
            config.system_update.get_command_to_perform_system_update_no_refresh()
        }
    };

    cmd_str = transform_system_update_command(cmd_str, is_root());

    for pkg in packages_ignored_during_system_update(config) {
        cmd_str.push_str(&format!(" {} {}", config.system_update.ignore_flag, sh_single_quote(&pkg)));
    }

    vlog!("Executing system update: {}", cmd_str);

    // We run it via sh -c to allow complex yay commands from config
    if let Err(e) = run_command("sh", &["-c", &cmd_str], None::<&str>) {
        die!("System update failed: {}", e);
    }
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
                concurrent_repos_downloads_limit: 1,
                concurrent_compilations_limit: 1,
                fast_aur_rpc_update_checks: false,
                system_update_first: false,
                clean_chroot_after_compilation: true,
                default_compiler: None,
                check_for_update_on_startup: None,
                auto_update_on_startup: None,
                self_update_at_updates: None,
                self_update_github_url: None,
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
            packages: Default::default(),
            check_for_update_on_startup: false,
            auto_update_on_startup: false,
            self_update_github_url: String::new(),
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
    fn packages_ignored_includes_skip_install_and_dedupes() {
        let config = minimal_config(vec!["foo"], vec!["foo", "bar"], vec!["baz"]);
        assert_eq!(
            packages_ignored_during_system_update(&config),
            vec!["baz", "foo", "bar"]
        );
    }

    #[test]
    fn test_transform_system_update_command() {
        // Non-root user: pacman commands should get sudo prepended
        assert_eq!(
            transform_system_update_command("pacman -Su".to_string(), false),
            "sudo pacman -Su"
        );
        assert_eq!(
            transform_system_update_command("pacman".to_string(), false),
            "sudo pacman"
        );

        // Root user: pacman commands should NOT get sudo prepended
        assert_eq!(
            transform_system_update_command("pacman -Su".to_string(), true),
            "pacman -Su"
        );

        // Already has sudo: should NOT get sudo prepended for either
        assert_eq!(
            transform_system_update_command("sudo pacman -Su".to_string(), false),
            "sudo pacman -Su"
        );
        assert_eq!(
            transform_system_update_command("sudo pacman -Su".to_string(), true),
            "sudo pacman -Su"
        );

        // Non-pacman command (e.g. yay): should NOT get sudo prepended
        assert_eq!(
            transform_system_update_command("yay -Su".to_string(), false),
            "yay -Su"
        );
    }
}
