mod aur_rpc;
mod build;
mod cli;
mod config;
mod dep_graph;
mod git;
mod install;
mod package_spec;
mod pkgbuild;
mod ramdisk;
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
static FORCE_SUDO_CLEAN: AtomicBool = AtomicBool::new(false);

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

pub fn set_force_sudo_clean(value: bool) {
    FORCE_SUDO_CLEAN.store(value, Ordering::Relaxed);
}

pub fn force_sudo_clean() -> bool {
    FORCE_SUDO_CLEAN.load(Ordering::Relaxed)
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
    if ramdisk::is_session_active() {
        let ram_targets = ramdisk::RamdiskTargets {
            chroot: true,
            ..Default::default()
        };
        let chroot_base = ramdisk::effective_chroot_base_path(config, &ram_targets);
        remove_chroot_at(&chroot_base);
    }
    remove_chroot_at(&config.paths.chroot_base_path);
}

fn remove_chroot_at(chroot_base: &str) {
    let master_chroot = PathBuf::from(chroot_base).join("base");
    if !master_chroot.exists() {
        return;
    }
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
    ramdisk::remove_ramdisk_work(config);

    let packages_target = ramdisk::full_clean_packages_target(config);
    if let Err(e) = check_sudo_removal(&packages_target) {
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

struct RamdiskShutdown;

impl Drop for RamdiskShutdown {
    fn drop(&mut self) {
        ramdisk::shutdown();
    }
}

fn refresh_repositories(config: &config::Config, cli: &cli::Cli, run_system_repo_update: bool) {
    if run_system_repo_update {
        system::run_system_update(config, system::SystemUpdateMode::UpdateRepositories);
    }
    build::sync_manual_repo_remotes(config, cli);
    upstream::sync_upstream_pkgbuilds(config, cli);
    if !is_silent_mode() {
        println!();
    }
    build::report_manual_update_versions(config, cli);
}

fn run_deferred_install_phase(
    specs: &[package_spec::PackageSpec],
    skipped_installs: &HashSet<String>,
    cli: &cli::Cli,
    config: &config::Config,
    sort_topologically: bool,
) {
    vlog!("Install phase (compile-first: all scheduled builds finished)...");
    if sort_topologically
        && let Ok(sorted) = dep_graph::sort_packages_topologically(specs, cli, config) {
            for spec in &sorted {
                if skipped_installs.contains(&spec.name) {
                    continue;
                }
                build::install_package_phase(spec, cli, config);
            }
            return;
        }
    for spec in specs {
        if skipped_installs.contains(&spec.name) {
            continue;
        }
        build::install_package_phase(spec, cli, config);
    }
}

#[macro_export]
macro_rules! die {
    ($($arg:tt)*) => {{
        eprintln!("{} {}", "==> ERROR:".red(), format!($($arg)*));
        $crate::pkgbuild::restore_pending_pkgbuilds();
        $crate::ramdisk::shutdown();
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
    set_force_sudo_clean(cli.use_sudo_clean);

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
            if let Ok((true, latest)) = self_update::check_for_update(&raw_url)
                && let Ok(mut guard) = update_notifier_clone.lock() {
                    *guard = Some(latest);
                }
        });
    }

    if cli.list {
        config.print_human_readable();
        return;
    }

    if let Err(e) = ramdisk::initialize(&config) {
        die!("Ramdisk setup failed: {}", e);
    }
    if let Err(e) = ramdisk::refresh_deletable_roots(&config) {
        die!("{}", e);
    }
    let _ramdisk_shutdown = RamdiskShutdown;

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
            refresh_repositories(&config, &cli, true);
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

        if config.build.system_update_first {
            blog!("Performing system update before compilation...");
            system::run_system_update(&config, system::SystemUpdateMode::PerformUpdateWithRefresh);
        }

        if cli.force_repo_update {
            blog!("Refreshing git remotes for manual_update_packages (-R)...");
            refresh_repositories(&config, &cli, !config.build.system_update_first);
        }

        let mut system_specs = Vec::new();
        for pkg in &config.manual_update_packages {
            if cli_package_names.contains(pkg) {
                continue;
            }
            if build::should_run_manual_prebuild(pkg, &cli, &config) {
                system_specs.push(package_spec::PackageSpec::plain(pkg));
            }
        }

        let skipped_install_after_compile_fail = run_compilations(system_specs.clone(), &cli, &config, defer_install_pass);

        if defer_install_pass {
            run_deferred_install_phase(&system_specs, &skipped_install_after_compile_fail, &cli, &config, false);
        }

        if !config.build.system_update_first {
            let mode = if cli.force_repo_update {
                system::SystemUpdateMode::PerformUpdateNoRefresh
            } else {
                system::SystemUpdateMode::PerformUpdateWithRefresh
            };
            system::run_system_update(&config, mode);
        }
    } else {
        let package_specs = parse_package_specs(&cli.packages);

        if package_specs.is_empty() {
            die!("No packages specified.");
        }

        let skipped_install_after_compile_fail = run_compilations(package_specs.clone(), &cli, &config, defer_install_pass);

        if defer_install_pass {
            run_deferred_install_phase(&package_specs, &skipped_install_after_compile_fail, &cli, &config, true);
        }
    }

    if let Ok(guard) = update_notifier.lock()
        && let Some(latest) = &*guard {
            println!(
                "{} A new version of ABS is available: {} (current: {}). Run 'abs --self-update' to update!",
                "==>".yellow(),
                latest.green(),
                env!("CARGO_PKG_VERSION").yellow()
            );
        }
}

fn run_compilations(
    specs: Vec<package_spec::PackageSpec>,
    cli: &Cli,
    config: &config::Config,
    defer_install_pass: bool,
) -> HashSet<String> {
    use std::collections::{HashMap, HashSet};
    use std::sync::{Mutex, Condvar};

    let mut skipped_install = HashSet::new();
    if specs.is_empty() {
        return skipped_install;
    }

    let sorted_specs = match dep_graph::sort_packages_topologically(&specs, cli, config) {
        Ok(sorted) => sorted,
        Err(e) => {
            ewarn!("Dependency sort failed: {}. Falling back to default order.", e);
            specs.clone()
        }
    };

    let concurrency_limit = config.build.concurrent_compilations_limit.max(1);

    if concurrency_limit <= 1 || !defer_install_pass {
        for spec in &sorted_specs {
            blog!("Processing package: {}", spec.name);
            if !build::process_package(spec, cli, config, defer_install_pass, None) {
                skipped_install.insert(spec.name.clone());
            }
        }
        return skipped_install;
    }

    vlog!("Starting parallel compilations (concurrency limit: {})...", concurrency_limit);

    let mut spec_map = HashMap::new();
    let mut base_to_name = HashMap::new();
    let mut name_to_base = HashMap::new();
    let mut deps_map = HashMap::new();

    for spec in &sorted_specs {
        let (_, _, base) = build::resolve_pkg_repo_for_manual(&spec.name, cli, config);
        spec_map.insert(base.clone(), spec.clone());
        base_to_name.insert(base.clone(), spec.name.clone());
        name_to_base.insert(spec.name.clone(), base.clone());
    }

    for (base, spec) in &spec_map {
        let (repo_name, repo_url_string, base_pkg) = build::resolve_pkg_repo_for_manual(&spec.name, cli, config);
        let pkg_config = config.packages.get(&spec.name);
        let targets = ramdisk::resolve_ramdisk_targets(config, pkg_config, Some(spec))
            .unwrap_or_default();
        let pkg_dir = git::prepare_repo(
            &spec.name,
            &base_pkg,
            &repo_name,
            &repo_url_string,
            &ramdisk::effective_packages_path(config, &targets),
            false,
            false,
            None,
        );
        let all_deps = pkgbuild::parse_pkg_dependencies(pkg_dir.as_path());
        let mut filtered_deps = HashSet::new();
        for dep in all_deps {
            if spec_map.contains_key(&dep) && dep != *base {
                filtered_deps.insert(dep);
            }
        }
        deps_map.insert(base.clone(), filtered_deps);
    }

    use std::sync::Arc;

    let deps_map = Arc::new(deps_map);
    let spec_map = Arc::new(spec_map);
    let state = Arc::new(Mutex::new((
        sorted_specs.iter().map(|s| name_to_base.get(&s.name).unwrap().clone()).collect::<HashSet<String>>(),
        HashSet::<String>::new(),
        HashSet::<String>::new(),
        HashSet::<String>::new(),
    )));
    let cvar = Arc::new(Condvar::new());

    std::thread::scope(|scope| {
        for worker_id in 0..concurrency_limit {
            let state = Arc::clone(&state);
            let cvar = Arc::clone(&cvar);
            let deps_map = Arc::clone(&deps_map);
            let spec_map = Arc::clone(&spec_map);
            scope.spawn(move || {
                loop {
                    let mut task_to_run = None;

                    {
                        let mut guard = state.lock().unwrap();
                        loop {
                            let (ref mut pending, ref mut compiling, ref mut finished, ref mut failed) = *guard;

                            if pending.is_empty() && compiling.is_empty() {
                                return;
                            }

                            let mut found_task = None;
                            for pending_task in pending.iter() {
                                let deps = deps_map.get(pending_task).unwrap();
                                let all_finished = deps.iter().all(|d| finished.contains(d));
                                let any_failed = deps.iter().any(|d| failed.contains(d));

                                if any_failed {
                                    found_task = Some((pending_task.clone(), true));
                                    break;
                                } else if all_finished {
                                    found_task = Some((pending_task.clone(), false));
                                    break;
                                }
                            }

                            if let Some((task, is_blocked)) = found_task {
                                pending.remove(&task);
                                if is_blocked {
                                    vlog!("Parallel compile: Skipping {} because its dependency failed.", task);
                                    failed.insert(task);
                                } else {
                                    compiling.insert(task.clone());
                                    task_to_run = Some(task);
                                }
                                break;
                            }

                            if !compiling.is_empty() {
                                guard = cvar.wait(guard).unwrap();
                            } else {
                                return;
                            }
                        }
                    }

                    if let Some(base_name) = task_to_run {
                        let spec = spec_map.get(&base_name).unwrap();
                        blog!("Processing package [Worker {}]: {}", worker_id, spec.name);

                        let chroot_copy = format!("abs-worker-{}", worker_id);
                        let success = build::process_package(spec, cli, config, true, Some(&chroot_copy));

                        {
                            let mut guard = state.lock().unwrap();
                            let (_, ref mut compiling, ref mut finished, ref mut failed) = *guard;
                            compiling.remove(&base_name);
                            if success {
                                finished.insert(base_name.clone());
                            } else {
                                failed.insert(base_name.clone());
                            }
                            cvar.notify_all();
                        }
                    }
                }
            });
        }
    });

    {
        let guard = state.lock().unwrap();
        let (_, _, _, ref failed) = *guard;
        for base in failed {
            if let Some(name) = base_to_name.get(base) {
                skipped_install.insert(name.clone());
            }
        }
    }

    skipped_install
}
