mod build;
mod cli;
mod config;
mod git;
mod install;
mod pkgbuild;
mod system;
mod utils;

use clap::Parser;
use cli::Cli;
use colored::Colorize;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use utils::{check_sudo_removal, prime_sudo_for_session, run_command, spawn_sudo_keepalive};

static SILENT_MODE: AtomicBool = AtomicBool::new(false);
static DRY_RUN_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_silent_mode(value: bool) {
    SILENT_MODE.store(value, Ordering::Relaxed);
}

pub fn is_silent_mode() -> bool {
    SILENT_MODE.load(Ordering::Relaxed)
}

pub fn set_dry_run_mode(value: bool) {
    DRY_RUN_MODE.store(value, Ordering::Relaxed);
}

pub fn is_dry_run_mode() -> bool {
    DRY_RUN_MODE.load(Ordering::Relaxed)
}

fn install_all_keys() {
    blog!("Installing Arch Linux and CachyOS keyrings...");
    if let Err(e) = run_command(
        "sudo",
        &[
            "pacman",
            "-Sy",
            "--noconfirm",
            "archlinux-keyring",
            "cachyos-keyring",
        ],
        None::<&str>,
    ) {
        ewarn!("Keyring package install failed: {}", e);
    }

    blog!("Populating keyrings...");
    if let Err(e) = run_command(
        "sudo",
        &["pacman-key", "--populate", "archlinux"],
        None::<&str>,
    ) {
        ewarn!("Failed to populate archlinux keys: {}", e);
    }
    if let Err(e) = run_command(
        "sudo",
        &["pacman-key", "--populate", "cachyos"],
        None::<&str>,
    ) {
        ewarn!("Failed to populate cachyos keys: {}", e);
    }

    if let Err(e) = run_command(
        "sudo",
        &[
            "pacman-key",
            "--keyserver",
            "hkps://keyserver.ubuntu.com",
            "--refresh-keys",
        ],
        None::<&str>,
    ) {
        ewarn!("Failed to refresh keys: {}", e);
    }
}

fn remove_chroot(config: &config::Config) {
    let master_chroot = PathBuf::from(&config.paths.chroot_base_path).join("base");
    if let Err(e) = check_sudo_removal(&master_chroot) {
        ewarn!(
            "Failed to remove chroot '{}': {}",
            master_chroot.display(),
            e
        );
    } else {
        blog!("Removed chroot at {}", master_chroot.display());
    }
}

fn run_full_cleaning(config: &config::Config) {
    remove_chroot(config);
    if let Err(e) = check_sudo_removal(&config.paths.packages_path) {
        ewarn!("Failed to remove packages path: {}", e);
    }
    if let Err(e) = check_sudo_removal(&config.paths.ready_made_packages_path) {
        ewarn!("Failed to remove ready packages path: {}", e);
    }

    if is_dry_run_mode() {
        println!("[DRY RUN] mkdir -p {}", config.paths.packages_path);
        println!(
            "[DRY RUN] mkdir -p {}",
            config.paths.ready_made_packages_path
        );
    } else {
        if let Err(e) = fs::create_dir_all(&config.paths.packages_path) {
            ewarn!("Failed to recreate packages path: {}", e);
        }
        if let Err(e) = fs::create_dir_all(&config.paths.ready_made_packages_path) {
            ewarn!("Failed to recreate ready packages path: {}", e);
        }
    }
    blog!("Full cleaning completed.");
}

#[macro_export]
macro_rules! die {
    ($($arg:tt)*) => {{
        eprintln!("{} {}", "==> ERROR:".red(), format!($($arg)*));
        std::process::exit(1);
    }};
}

#[macro_export]
macro_rules! ewarn {
    ($($arg:tt)*) => {
        eprintln!("{} {}", "==> WARNING:".yellow(), format!($($arg)*));
    };
}

#[macro_export]
macro_rules! blog {
    ($($arg:tt)*) => {
        if !$crate::is_silent_mode() {
            println!("{} {}", "==>".blue(), format!($($arg)*));
        }
    };
}

#[macro_export]
macro_rules! vlog {
    ($verbose:expr, $($arg:tt)*) => {
        if $verbose {
            println!("==> {}", format!($($arg)*));
        }
    };
}

fn main() {
    let cli = Cli::parse();
    set_silent_mode(cli.silent);
    set_dry_run_mode(cli.dry_run);

    let config = config::Config::load_config();

    if cli.list {
        config.print_human_readable();
        return;
    }

    if !cli.dry_run {
        if let Err(e) = prime_sudo_for_session() {
            ewarn!(
                "sudo -v failed (later sudo steps may ask for a password again): {}",
                e
            );
        }
        spawn_sudo_keepalive();
    }

    if cli.system_update && cli.install_only {
        die!("--install-only cannot be used with -U");
    }

    if cli.install_keys {
        install_all_keys();
    }
    if cli.remove_chroot {
        remove_chroot(&config);
    }
    if cli.clean_all {
        run_full_cleaning(&config);
    }

    if cli.packages.is_empty()
        && !cli.system_update
        && !cli.force_repo_update
        && (cli.install_keys || cli.remove_chroot || cli.clean_all)
    {
        return;
    }

    // `-R` without `-U`: sync all manual repos, report PKGBUILD vs installed, then `command` (not refresh).
    if cli.force_repo_update && !cli.system_update && cli.packages.is_empty() {
        blog!("Repository refresh (manual_update_packages) and system update...");
        build::sync_manual_repo_remotes(&config, &cli);
        build::report_manual_update_versions(&config, &cli);
        system::run_system_update(&config, false, cli.verbose);
        return;
    }

    let defer_install_pass = config.build.compile_first_install_after
        && !cli.compile_only
        && !cli.install_only
        && !cli.download_only;

    if cli.system_update {
        blog!("Starting system update mode...");

        let updates = system::check_updates();
        if !updates.is_empty() {
            println!("Updates available:\n{}", updates);
        }

        if cli.force_repo_update {
            blog!("Refreshing git remotes for manual_update_packages (-R)...");
            build::sync_manual_repo_remotes(&config, &cli);
            build::report_manual_update_versions(&config, &cli);
        }

        let helper_line_matches = |pkg: &str| {
            updates
                .lines()
                .any(|line| line.starts_with(&format!("{} ", pkg)))
        };

        let mut skipped_install_after_compile_fail = HashSet::<String>::new();

        for pkg in &config.manual_update_packages {
            if cli.packages.contains(pkg) {
                continue;
            }

            if build::should_run_manual_prebuild(pkg, &cli, &config, helper_line_matches(pkg)) {
                blog!("Manual update package: {}", pkg);
                if !build::process_package(pkg, &cli, &config, defer_install_pass) {
                    skipped_install_after_compile_fail.insert(pkg.clone());
                }
            }
        }

        if defer_install_pass {
            blog!("Install phase (compile-first: all scheduled builds finished)...");
            for pkg in &config.manual_update_packages {
                if cli.packages.contains(pkg) {
                    continue;
                }
                if skipped_install_after_compile_fail.contains(pkg) {
                    continue;
                }
                if build::should_run_manual_prebuild(pkg, &cli, &config, helper_line_matches(pkg)) {
                    build::install_package_phase(pkg, &cli, &config);
                }
            }
        }

        let use_refresh = cli.force_repo_update;
        system::run_system_update(&config, use_refresh, cli.verbose);
    } else {
        if cli.packages.is_empty() {
            die!("No packages specified.");
        }

        let mut skipped_install_after_compile_fail = HashSet::<String>::new();

        for pkg in &cli.packages {
            blog!("Processing package: {}", pkg);
            if !build::process_package(pkg, &cli, &config, defer_install_pass) {
                skipped_install_after_compile_fail.insert(pkg.clone());
            }
        }

        if defer_install_pass {
            blog!("Install phase (compile-first: all scheduled builds finished)...");
            for pkg in &cli.packages {
                if skipped_install_after_compile_fail.contains(pkg) {
                    continue;
                }
                build::install_package_phase(pkg, &cli, &config);
            }
        }
    }
}
