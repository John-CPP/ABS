mod build;
mod cli;
mod config;
mod git;
mod install;
mod package_spec;
mod pkgbuild;
mod self_update;
mod system;
mod upstream;
mod utils;

use std::sync::{Arc, Mutex};

use clap::Parser;
use cli::Cli;
use colored::Colorize;
use package_spec::parse_package_specs;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use utils::{check_sudo_removal, prime_sudo_for_session, run_command, spawn_sudo_keepalive};

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verbosity {
    Silent = 0,
    Normal = 1,
    Verbose = 2,
}

static VERBOSITY: AtomicU8 = AtomicU8::new(Verbosity::Normal as u8);
static DRY_RUN_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_verbosity(v: Verbosity) {
    VERBOSITY.store(v as u8, Ordering::Relaxed);
}

pub fn verbosity() -> Verbosity {
    match VERBOSITY.load(Ordering::Relaxed) {
        0 => Verbosity::Silent,
        1 => Verbosity::Normal,
        2 => Verbosity::Verbose,
        _ => Verbosity::Normal,
    }
}

pub fn is_silent_mode() -> bool {
    verbosity() == Verbosity::Silent
}

pub fn is_verbose_mode() -> bool {
    verbosity() == Verbosity::Verbose
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
        if $crate::verbosity() >= $crate::Verbosity::Normal {
            println!("{} {}", "==>".blue(), format!($($arg)*));
        }
    };
}

#[macro_export]
macro_rules! vlog {
    ($($arg:tt)*) => {
        if $crate::verbosity() >= $crate::Verbosity::Verbose {
            println!("==> {}", format!($($arg)*));
        }
    };
}

fn main() {
    let cli = Cli::parse();

    let v = match (cli.verbose, cli.silent) {
        (true, true) => Verbosity::Normal,
        (true, false) => Verbosity::Verbose,
        (false, true) => Verbosity::Silent,
        (false, false) => Verbosity::Normal,
    };
    set_verbosity(v);
    set_dry_run_mode(cli.dry_run);

    if cli.configure.is_some() {
        config::Config::open_in_editor(cli.configure.as_deref().filter(|s| !s.is_empty()));
        return;
    }

    let config = config::Config::load_config();

    if cli.check_update {
        match self_update::check_for_update(&config.self_update_raw_url) {
            Ok((true, latest)) => {
                println!(
                    "{} A new version of ABS is available: {} (current: {}). Run 'abs --self-update' to update!",
                    "==>".yellow(),
                    latest.green(),
                    env!("CARGO_PKG_VERSION").yellow()
                );
            }
            Ok((false, _)) => {
                blog!("ABS is up-to-date (current version: {}).", env!("CARGO_PKG_VERSION").green());
            }
            Err(e) => {
                die!("Update check failed: {}", e);
            }
        }
        return;
    }

    if cli.self_update {
        if let Err(e) = self_update::run_self_update(&config, false) {
            die!("{}", e);
        }
        return;
    }

    // Handle auto-update on startup
    if config.auto_update_on_startup && !cli.dry_run {
        let _ = self_update::run_self_update(&config, true);
    }

    // Handle background update check on startup
    let update_notifier = Arc::new(Mutex::new(None));
    let update_notifier_clone = Arc::clone(&update_notifier);
    let raw_url = config.self_update_raw_url.clone();
    // Skip redundant background update checks if we are running synchronous auto-updates
    let skip_background = config.auto_update_on_startup || (config.self_update_at_updates && cli.system_update);
    if config.check_for_update_on_startup && !skip_background {
        std::thread::spawn(move || {
            if let Ok((true, latest)) = self_update::check_for_update(&raw_url) {
                if let Ok(mut guard) = update_notifier_clone.lock() {
                    *guard = Some(latest);
                }
            }
        });
    }

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
        && (cli.install_keys || cli.remove_chroot || cli.clean_all || cli.configure.is_some())
    {
        return;
    }

    // `-R` without `-U`: sync all manual repos, report PKGBUILD vs installed, then `command` (not refresh).
        if cli.force_repo_update && !cli.system_update && cli.packages.is_empty() {
            blog!("Repository refresh (manual_update_packages) and system update...");
            build::sync_manual_repo_remotes(&config, &cli);
            upstream::sync_upstream_pkgbuilds(&config, &cli);
            if !is_silent_mode() {
                println!();
            }
            build::report_manual_update_versions(&config, &cli);
            system::run_system_update(&config, false);
            return;
        }

    let defer_install_pass = config.build.compile_first_install_after
        && !cli.compile_only
        && !cli.install_only
        && !cli.download_only;

    if cli.system_update {
        if config.self_update_at_updates && !cli.dry_run {
            vlog!("self_update_at_updates is enabled, checking for update before system update...");
            match self_update::run_self_update(&config, true) {
                Ok(true) => {
                    blog!("ABS successfully updated. Please re-run the command.");
                    return;
                }
                Ok(false) => {}
                Err(e) => {
                    ewarn!("Self update check failed before system update: {}", e);
                }
            }
        }

        blog!("Starting system update mode...");
        let cli_package_names: HashSet<String> = parse_package_specs(&cli.packages)
            .into_iter()
            .map(|s| s.name)
            .collect();

        if cli.force_repo_update {
            blog!("Refreshing git remotes for manual_update_packages (-R)...");
            build::sync_manual_repo_remotes(&config, &cli);
            upstream::sync_upstream_pkgbuilds(&config, &cli);
            if !is_silent_mode() {
                println!();
            }
            build::report_manual_update_versions(&config, &cli);
        }

        let mut skipped_install_after_compile_fail = HashSet::<String>::new();

        for pkg in &config.manual_update_packages {
            if cli_package_names.contains(pkg) {
                continue;
            }

            if build::should_run_manual_prebuild(pkg, &cli, &config) {
                vlog!("Manual update package: {}", pkg);
                if !build::process_package(&package_spec::PackageSpec::plain(pkg), &cli, &config, defer_install_pass) {
                    skipped_install_after_compile_fail.insert(pkg.clone());
                }
            }
        }

        if defer_install_pass {
            vlog!("Install phase (compile-first: all scheduled builds finished)...");
            for pkg in &config.manual_update_packages {
                if cli_package_names.contains(pkg) {
                    continue;
                }
                if skipped_install_after_compile_fail.contains(pkg) {
                    continue;
                }
                if build::should_run_manual_prebuild(pkg, &cli, &config) {
                    build::install_package_phase(
                        &package_spec::PackageSpec::plain(pkg),
                        &cli,
                        &config,
                    );
                }
            }
        }

        let use_refresh = cli.force_repo_update;
        system::run_system_update(&config, use_refresh);
    } else {
        let package_specs = parse_package_specs(&cli.packages);

        if package_specs.is_empty() {
            die!("No packages specified.");
        }

        let mut skipped_install_after_compile_fail = HashSet::<String>::new();

        for spec in &package_specs {
            blog!("Processing package: {}", spec.name);
            if !build::process_package(spec, &cli, &config, defer_install_pass) {
                skipped_install_after_compile_fail.insert(spec.name.clone());
            }
        }

        if defer_install_pass {
            vlog!("Install phase (compile-first: all scheduled builds finished)...");
            for spec in &package_specs {
                if skipped_install_after_compile_fail.contains(&spec.name) {
                    continue;
                }
                build::install_package_phase(spec, &cli, &config);
            }
        }
    }

    if let Ok(guard) = update_notifier.lock() {
        if let Some(latest) = &*guard {
            println!(
                "{} A new version of ABS is available: {} (current: {}). Run 'abs --self-update' to update!",
                "==>".yellow(),
                latest.green(),
                env!("CARGO_PKG_VERSION").yellow()
            );
        }
    }
}
