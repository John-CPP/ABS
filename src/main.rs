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
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use utils::{check_sudo_removal, run_command};

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
        && (cli.install_keys || cli.remove_chroot || cli.clean_all)
    {
        return;
    }

    if cli.system_update {
        blog!("Starting system update mode...");

        let updates = system::check_updates();
        if !updates.is_empty() {
            println!("Updates available:\n{}", updates);
        }
        let mut manually_compiled_packages: Vec<String> = Vec::new();

        // We should also check manual updates
        for pkg in &config.manual_update_packages {
            if !cli.packages.contains(pkg) {
                // Check if this package is actually in the updates list
                let needs_update = updates
                    .lines()
                    .any(|line| line.starts_with(&format!("{} ", pkg)));

                if needs_update || cli.force_build {
                    blog!("Checking custom repository updates for {}", pkg);
                    build::process_package(pkg, &cli, &config);
                    manually_compiled_packages.push(pkg.clone());
                } else {
                    blog!(
                        "No system updates pending for custom package '{}'. Skipping...",
                        pkg
                    );
                }
            }
        }

        system::run_system_update(&config, cli.force_repo_update, &manually_compiled_packages);
    } else {
        if cli.packages.is_empty() {
            die!("No packages specified.");
        }

        for pkg in &cli.packages {
            blog!("Processing package: {}", pkg);
            build::process_package(pkg, &cli, &config);
        }
    }
}
