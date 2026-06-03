use crate::cli::Cli;
use crate::config::Config;
use crate::git::{is_per_package_repo, prepare_repo, PkgbuildDirCache};
use crate::package_spec::PackageSpec;
use crate::pkgbuild::{
    apply_pkgbuild_overrides, backup_pkgbuild, bump_pkgrel, restore_pkgbuild, update_pkgsums,
    inject_compiler_env,
};
use crate::utils::{
    pacman_query_version, pacman_sync_version, read_pkg_full_version_from_dir,
    remove_src_pkg_workdirs, remove_stale_pkgs_in_pkgdest, run_command,
    run_shell_in_dir_with_tee, vercmp,
};
use crate::{blog, die, ewarn, vlog};
use colored::Colorize;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

fn normalize_repo_name(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// Built-in repository URLs used when `[repositories]` has no entry (e.g. `abs --repo=aur`).
fn known_repository_url(repo_name: &str) -> Option<String> {
    match normalize_repo_name(repo_name).as_str() {
        "aur" => Some("https://aur.archlinux.org".to_string()),
        _ => None,
    }
}

/// Resolve a `[repositories]` entry to a clone URL. Values may be a URL, or another key
/// (e.g. `default = "arch"` then `arch = "https://..."`).
fn repository_url(repos: &HashMap<String, String>, start: &str) -> Option<String> {
    let mut key = start.to_string();
    for _ in 0..8 {
        let v = repos.get(&key)?;
        if v.contains("://") || v.starts_with("git@") {
            return Some(v.clone());
        }
        key = v.clone();
    }
    None
}

pub struct PkgbuildGuard<'a> {
    pub repo_dir: &'a Path,
}

impl<'a> Drop for PkgbuildGuard<'a> {
    fn drop(&mut self) {
        restore_pkgbuild(self.repo_dir);
    }
}

/// Ensure `<chrootdir>/root` exists for `makechrootpkg -r <chrootdir>` (see makechrootpkg(1)).
fn ensure_devtools_chroot(chrootdir: &Path) -> Result<(), String> {
    let rootfs = chrootdir.join("root");

    if rootfs.is_dir() {
        return Ok(());
    }
    if rootfs.exists() {
        return Err(format!(
            "{} exists but is not a directory; remove it or change chroot_base_path.",
            rootfs.display()
        ));
    }

    // Older ABS called `mkarchroot` on `.../base` instead of `.../base/root`, which breaks
    // makechrootpkg (it syncs `root` -> `$USER` and expects `root/etc/makepkg.conf`).
    if chrootdir.is_dir() && chrootdir.join("etc").is_dir() && !rootfs.is_dir() {
        return Err(format!(
            "Incompatible chroot layout at {} (rootfs was created at 'base/' instead of 'base/root/'). \
             Remove it and retry, for example: sudo rm -rf {}",
            chrootdir.display(),
            chrootdir.display(),
        ));
    }

    blog!(
        "Chroot rootfs missing at {}; creating with mkarchroot (first run may take a while)...",
        rootfs.display()
    );

    run_command(
        "sudo",
        &["mkdir", "-p", &chrootdir.to_string_lossy()],
        None::<&str>,
    )?;

    let dest = rootfs.to_string_lossy();
    run_command(
        "sudo",
        &["mkarchroot", dest.as_ref(), "base-devel"],
        None::<&str>,
    )?;

    if !rootfs.is_dir() {
        return Err(format!(
            "mkarchroot finished but {} is not a usable directory",
            rootfs.display()
        ));
    }

    Ok(())
}

fn run_build_with_key_retry(build_cmd: &str, repo_dir: &Path) -> Result<(), String> {
    let key_re = Regex::new(r"(?i)unknown public key ([0-9A-F]+)")
        .map_err(|e| format!("Failed to compile missing-key regex: {}", e))?;
    // Large logs (e.g. Firefox) can mention "unknown public key" long before the real failure in
    // `prepare()` / `build()` / `check()`. Retrying the whole makepkg then re-runs those phases for no benefit.
    let pkgbuild_phase_failed_re = Regex::new(r"(?i)A failure occurred in (prepare|build|check)\(\)")
        .map_err(|e| format!("Failed to compile phase-failure regex: {}", e))?;
    let mut seen_keys: HashSet<String> = HashSet::new();

    loop {
        match run_shell_in_dir_with_tee(repo_dir, build_cmd) {
            Ok(()) => return Ok(()),
            Err(err) => {
                if pkgbuild_phase_failed_re.is_match(&err) {
                    return Err(err);
                }
                let mut newly_found = Vec::new();
                for caps in key_re.captures_iter(&err) {
                    let key = caps[1].to_uppercase();
                    if seen_keys.insert(key.clone()) {
                        newly_found.push(key);
                    }
                }
                if newly_found.is_empty() {
                    return Err(err);
                }

                for key in newly_found {
                    crate::vlog!("Importing missing key: {}", key);
                    if let Err(gpg_err) = run_command(
                        "gpg",
                        &[
                            "--keyserver",
                            "hkps://keyserver.ubuntu.com",
                            "--recv-keys",
                            &key,
                        ],
                        None::<&str>,
                    ) {
                        return Err(format!(
                            "Build failed and key import also failed for {}: {}\nOriginal build error:\n{}",
                            key, gpg_err, err
                        ));
                    }
                }
                crate::vlog!("Retrying build after importing keys...");
            }
        }
    }
}

fn resolve_pkg_repo(
    pkg: &str,
    cli: &Cli,
    config: &Config,
    spec: Option<&PackageSpec>,
) -> (String, String, String) {
    let pkg_config = config.packages.get(pkg);

    let mut repo_name = config
        .repositories
        .get("default")
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            die!("Missing [repositories] entry: default = \"<repo-key>\" (see abs.toml.example)")
        });
    if let Some(r) = spec.and_then(|s| s.repo.as_deref()).or(cli.repo.as_deref()) {
        repo_name = normalize_repo_name(r);
    } else if let Some(pc) = pkg_config
        && let Some(src) = &pc.source
    {
        repo_name = normalize_repo_name(src);
    }

    let repo_url_string = repository_url(&config.repositories, &repo_name)
        .or_else(|| known_repository_url(&repo_name))
        .unwrap_or_else(|| {
            ewarn!(
                "Repository '{}' not found in config. Using default.",
                repo_name
            );
            let default_key = config
                .repositories
                .get("default")
                .map(|s| s.as_str())
                .unwrap_or("arch");
            repository_url(&config.repositories, default_key).unwrap_or_else(|| {
                die!(
                    "Could not resolve a repository URL (check [repositories] for '{}' and 'default')",
                    repo_name
                )
            })
        });

    let base_pkg = pkg_config
        .and_then(|pc| pc.alias.as_deref())
        .unwrap_or(pkg)
        .to_string();

    (repo_name, repo_url_string, base_pkg)
}

/// Public wrapper for manual-update paths (`-R`/`-RU`) without a CLI [`PackageSpec`].
pub fn resolve_pkg_repo_for_manual(
    pkg: &str,
    cli: &Cli,
    config: &Config,
) -> (String, String, String) {
    resolve_pkg_repo(pkg, cli, config, None)
}

use std::sync::OnceLock;
use std::sync::Mutex;

static AUR_UP_TO_DATE_PACKAGES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn aur_up_to_date_cache() -> &'static Mutex<HashSet<String>> {
    AUR_UP_TO_DATE_PACKAGES.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn mark_aur_package_up_to_date(pkg: &str) {
    if let Ok(mut cache) = aur_up_to_date_cache().lock() {
        cache.insert(pkg.to_string());
    }
}

pub fn is_aur_package_up_to_date(pkg: &str) -> bool {
    if let Ok(cache) = aur_up_to_date_cache().lock() {
        cache.contains(pkg)
    } else {
        false
    }
}

pub fn unmark_aur_package_up_to_date(pkg: &str) {
    if let Ok(mut cache) = aur_up_to_date_cache().lock() {
        cache.remove(pkg);
    }
}

static STABLE_UP_TO_DATE_PACKAGES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn stable_up_to_date_cache() -> &'static Mutex<HashSet<String>> {
    STABLE_UP_TO_DATE_PACKAGES.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn mark_stable_package_up_to_date(pkg: &str) {
    if let Ok(mut cache) = stable_up_to_date_cache().lock() {
        cache.insert(pkg.to_string());
    }
}

pub fn is_stable_package_up_to_date(pkg: &str) -> bool {
    if let Ok(cache) = stable_up_to_date_cache().lock() {
        cache.contains(pkg)
    } else {
        false
    }
}

/// After `git pull` on a shared repo (`-R`), decide if PKGBUILD versions are newer than installed.
fn manual_src_newer_than_installed(pkg: &str, cli: &Cli, config: &Config) -> Result<bool, String> {
    if is_aur_package_up_to_date(pkg) || is_stable_package_up_to_date(pkg) {
        return Ok(false);
    }
    let (repo_name, repo_url_string, base_pkg) = resolve_pkg_repo(pkg, cli, config, None);
    if !config.install_testing_phase_archlinux_packages
        && (repo_name == "arch" || repo_name == "cachyos")
        && let Ok(Some(sync_ver)) = pacman_sync_version(&base_pkg) {
            let Some(inst_ver) = pacman_query_version(&base_pkg)? else {
                return Ok(true);
            };
            return Ok(vercmp(&sync_ver, &inst_ver)? > 0);
        }
    let repo_url = repo_url_string.as_str();
    // Callers that pass `-R` with `-U` run `sync_manual_repo_remotes` first; only read the tree here.
    let pkg_dir = prepare_repo(
        pkg,
        &base_pkg,
        &repo_name,
        repo_url,
        &config.paths.packages_path,
        false,
        false,
        None,
    );
    let src_ver = read_pkg_full_version_from_dir(pkg_dir.as_path())?;
    let Some(inst_ver) = pacman_query_version(&base_pkg)? else {
        return Ok(true);
    };
    Ok(vercmp(&src_ver, &inst_ver)? > 0)
}

/// `git pull` (or clone) for each distinct remote: **arch** / **aur** use one clone per package
/// (`arch:<base_pkg>`, `aur:<base_pkg>`); **other repositories** run at most once per `repo_name` no matter how many
/// `manual_update_packages` share it. [`crate::git::prepare_repo`] also skips a second `git pull`
/// on the same clone path in one process. Does not compile; callers run report / builds / update.
pub fn sync_manual_repo_remotes(config: &Config, cli: &Cli) {
    vlog!("Syncing git remotes for manual_update_packages...");
    if config.manual_update_packages.is_empty() {
        vlog!("manual_update_packages is empty; nothing to sync.");
        return;
    }

    if config.build.fast_aur_rpc_update_checks {
        let mut aur_packages = Vec::new();
        for pkg in &config.manual_update_packages {
            let (repo_name, _, _) = resolve_pkg_repo(pkg, cli, config, None);
            if repo_name == "aur" {
                aur_packages.push(pkg.clone());
            }
        }

        if !aur_packages.is_empty() {
            vlog!("AUR RPC: checking update status for: {:?}", aur_packages);
            match crate::aur_rpc::fetch_aur_packages_info(&aur_packages) {
                Ok(versions) => {
                    for pkg in &aur_packages {
                        let (_, _, base_pkg) = resolve_pkg_repo(pkg, cli, config, None);
                        if let Some(remote_ver) = versions.get(pkg)
                            && let Ok(Some(inst_ver)) = pacman_query_version(&base_pkg)
                                && let Ok(c) = vercmp(remote_ver, &inst_ver) {
                                    if c <= 0 {
                                        vlog!("AUR RPC: {} is up-to-date (remote: {}, installed: {}). Skipping git pull.", pkg, remote_ver, inst_ver);
                                        mark_aur_package_up_to_date(pkg);
                                    } else {
                                        vlog!("AUR RPC: {} requires update (remote: {}, installed: {}).", pkg, remote_ver, inst_ver);
                                    }
                                }
                    }
                }
                Err(e) => {
                    ewarn!("AUR RPC check failed: {}; falling back to standard Git update checks", e);
                }
            }
        }
    }

    if !config.install_testing_phase_archlinux_packages {
        for pkg in &config.manual_update_packages {
            let (repo_name, _, base_pkg) = resolve_pkg_repo(pkg, cli, config, None);
            if (repo_name == "arch" || repo_name == "cachyos")
                && let Ok(Some(sync_ver)) = pacman_sync_version(&base_pkg)
                    && let Ok(Some(inst_ver)) = pacman_query_version(&base_pkg)
                        && let Ok(c) = vercmp(&sync_ver, &inst_ver) {
                            if c <= 0 {
                                vlog!(
                                    "Stable Repo Check: {} is up-to-date in stable (sync: {}, installed: {}). Skipping git pull.",
                                    pkg,
                                    sync_ver,
                                    inst_ver
                                );
                                mark_stable_package_up_to_date(pkg);
                            } else {
                                vlog!(
                                    "Stable Repo Check: {} requires update in stable (sync: {}, installed: {}).",
                                    pkg,
                                    sync_ver,
                                    inst_ver
                                );
                            }
                        }
        }
    }

    struct SyncTask {
        pkg: String,
        base_pkg: String,
        repo_name: String,
        repo_url_string: String,
    }

    let mut seen = HashSet::new();
    let mut tasks = Vec::new();

    for pkg in &config.manual_update_packages {
        if is_aur_package_up_to_date(pkg) || is_stable_package_up_to_date(pkg) {
            continue;
        }
        let (repo_name, repo_url_string, base_pkg) = resolve_pkg_repo(pkg, cli, config, None);
        let key = if is_per_package_repo(&repo_name) {
            format!("{}:{base_pkg}", normalize_repo_name(&repo_name))
        } else {
            repo_name.clone()
        };
        if !seen.insert(key) {
            continue;
        }
        tasks.push(SyncTask {
            pkg: pkg.clone(),
            base_pkg,
            repo_name,
            repo_url_string,
        });
    }

    if tasks.is_empty() {
        return;
    }

    tasks.reverse();

    let tasks_mutex = std::sync::Mutex::new(tasks);
    let concurrency_limit = config.build.concurrent_repos_downloads_limit.max(1);

    std::thread::scope(|s| {
        for _ in 0..concurrency_limit {
            s.spawn(|| {
                loop {
                    let task = {
                        let mut guard = tasks_mutex.lock().unwrap();
                        guard.pop()
                    };
                    let Some(task) = task else {
                        break;
                    };

                    let start = std::time::Instant::now();
                    let _ = prepare_repo(
                        &task.pkg,
                        &task.base_pkg,
                        &task.repo_name,
                        task.repo_url_string.as_str(),
                        &config.paths.packages_path,
                        false,
                        true,
                        None,
                    );
                    vlog!("Synced {} (repo {}) in {:?}", task.pkg, task.repo_name, start.elapsed());
                }
            });
        }
    });
}

enum ManualPkgVersionLine {
    UpToDate { current: String },
    Upgrade { current: String, new: String },
}

fn classify_manual_pkg_version(
    pkg: &str,
    cli: &Cli,
    config: &Config,
    pkgbuild_cache: &mut PkgbuildDirCache,
) -> Result<ManualPkgVersionLine, String> {
    let (repo_name, repo_url_string, base_pkg) = resolve_pkg_repo(pkg, cli, config, None);
    if (is_aur_package_up_to_date(pkg) || is_stable_package_up_to_date(pkg))
        && let Ok(Some(inst)) = pacman_query_version(&base_pkg) {
            return Ok(ManualPkgVersionLine::UpToDate { current: inst });
        }
    if !config.install_testing_phase_archlinux_packages
        && (repo_name == "arch" || repo_name == "cachyos")
        && let Ok(Some(sync_ver)) = pacman_sync_version(&base_pkg) {
            let inst = pacman_query_version(&base_pkg)?;
            let Some(inst_ver) = inst else {
                return Ok(ManualPkgVersionLine::Upgrade {
                    current: "not installed".to_string(),
                    new: sync_ver,
                });
            };
            if vercmp(&sync_ver, &inst_ver)? > 0 {
                return Ok(ManualPkgVersionLine::Upgrade {
                    current: inst_ver,
                    new: sync_ver,
                });
            } else {
                return Ok(ManualPkgVersionLine::UpToDate { current: inst_ver });
            }
        }
    let pkg_dir = prepare_repo(
        pkg,
        &base_pkg,
        &repo_name,
        repo_url_string.as_str(),
        &config.paths.packages_path,
        false,
        false,
        Some(pkgbuild_cache),
    );
    let src = read_pkg_full_version_from_dir(pkg_dir.as_path())?;
    let inst = pacman_query_version(&base_pkg)?;
    let Some(inst) = inst else {
        return Ok(ManualPkgVersionLine::Upgrade {
            current: "not installed".to_string(),
            new: src,
        });
    };
    match vercmp(&src, &inst)? {
        x if x > 0 => Ok(ManualPkgVersionLine::Upgrade {
            current: inst,
            new: src,
        }),
        _ => Ok(ManualPkgVersionLine::UpToDate { current: inst }),
    }
}

fn print_manual_version_line(pkg: &str, line: ManualPkgVersionLine) {
    if crate::is_silent_mode() {
        return;
    }

    print!("{} ", "==>".blue());
    print!("{}: ", pkg);
    match line {
        ManualPkgVersionLine::UpToDate { current } => {
            print!("{}", "Up-to-date".green().bold());
            println!(" (current version: {})", current.green());
        }
        ManualPkgVersionLine::Upgrade { current, new } => {
            print!("{}", "Has an upgrade".red().bold());
            println!(" ({} vs {})", current.red(), new.green());
        }
    }
}

/// After `sync_manual_repo_remotes`, compare each manual package's PKGBUILD to `pacman -Q`.
pub fn report_manual_update_versions(config: &Config, cli: &Cli) {
    vlog!("PKGBUILD vs installed (manual_update_packages):");
    let mut pkgbuild_cache = PkgbuildDirCache::new();
    for pkg in &config.manual_update_packages {
        match classify_manual_pkg_version(pkg, cli, config, &mut pkgbuild_cache) {
            Ok(line) => print_manual_version_line(pkg, line),
            Err(e) => {
                ewarn!("{}: {}", pkg, e);
            }
        }
    }
}

pub fn should_run_manual_prebuild(
    pkg: &str,
    cli: &Cli,
    config: &Config,
) -> bool {
    if cli.force_build {
        return true;
    }
    if cli.force_repo_update {
        match manual_src_newer_than_installed(pkg, cli, config) {
            Ok(v) => v,
            Err(e) => {
                ewarn!(
                    "{}: could not compare PKGBUILD to installed ({}); skipping",
                    pkg,
                    e
                );
                false
            }
        }
    } else {
        false
    }
}

struct EffectiveConfig {
    build_env: String,
    skip_tests: bool,
    compiler: Option<String>,
}

fn resolve_effective_config(
    spec: &PackageSpec,
    cli: &Cli,
    config: &Config,
    pkg_config: Option<&crate::config::PackageConfig>,
) -> EffectiveConfig {
    let build_env = if spec.chroot_build == Some(true) {
        "chroot".to_string()
    } else if spec.local_build == Some(true) || cli.local_build {
        "local".to_string()
    } else if cli.chroot_build {
        "chroot".to_string()
    } else if let Some(pc) = pkg_config
        && let Some(env) = &pc.build_env
    {
        env.clone()
    } else {
        config.build.default_environment.clone()
    };

    let skip_tests = spec.no_check == Some(true)
        || cli.no_check
        || pkg_config.is_some_and(|pc| pc.tests.is_some_and(|t| !t));

    let compiler = spec.compiler.clone()
        .or_else(|| pkg_config.and_then(|pc| pc.compiler.clone()))
        .or_else(|| config.build.default_compiler.clone());

    EffectiveConfig {
        build_env,
        skip_tests,
        compiler,
    }
}

/// Install prompts and `pacman -U` for `spec`, using `makepkg --packagelist` from the prepared repo.
/// Used after [`process_package`] when **`compile_first_install_after`** deferred the install pass.
pub fn install_package_phase(spec: &PackageSpec, cli: &Cli, config: &Config) {
    if cli.compile_only || cli.install_only || cli.download_only {
        return;
    }

    let pkg = spec.name.as_str();
    let pkg_config = config.packages.get(pkg);
    let (repo_name, repo_url_string, base_pkg) = resolve_pkg_repo(pkg, cli, config, Some(spec));
    let repo_dir_path = prepare_repo(
        pkg,
        base_pkg.as_str(),
        &repo_name,
        repo_url_string.as_str(),
        &config.paths.packages_path,
        false,
        false,
        None,
    );
    let repo_dir = repo_dir_path.as_path();

    crate::install::install_artifacts(
        pkg,
        base_pkg.as_str(),
        Some(repo_dir),
        config,
    );

    if let Some(pc) = pkg_config
        && let Some(cmd) = &pc.post_update_command
    {
        blog!("Running post-update command...");
        if let Err(e) = run_command("sh", &["-c", cmd], Some(repo_dir)) {
            ewarn!("Post-update command failed: {}", e);
        }
    }
}

fn inject_chroot_makepkg_conf(chrootdir: &Path, config: &Config) -> Result<(), String> {
    if let Some(custom_conf) = &config.paths.chroot_makepkg_conf {
        let custom_conf_path = Path::new(custom_conf);
        if !custom_conf_path.exists() {
            return Err(format!(
                "Custom chroot makepkg.conf path does not exist: {}",
                custom_conf
            ));
        }

        let target_conf = chrootdir.join("root").join("etc").join("makepkg.conf");
        vlog!(
            "Injecting custom makepkg.conf '{}' into chroot '{}'...",
            custom_conf,
            target_conf.display()
        );

        run_command(
            "sudo",
            &[
                "cp",
                custom_conf_path.to_string_lossy().as_ref(),
                target_conf.to_string_lossy().as_ref(),
            ],
            None::<&str>,
        )?;
    }
    Ok(())
}

/// `defer_install`: when true (compile-first mode), build only; caller runs [`install_package_phase`] later.
///
/// `chroot_copy`: when set, names the per-build `makechrootpkg` working copy (`-l`). Parallel
/// compilations **must** pass a unique name per worker, otherwise concurrent builds race on the
/// default `<chrootdir>/$USER` copy and corrupt each other.
///
/// Returns **`false`** if the build failed and **`ignore_compilation_failures`** is set (caller continues).
pub fn process_package(
    spec: &PackageSpec,
    cli: &Cli,
    config: &Config,
    defer_install: bool,
    chroot_copy: Option<&str>,
) -> bool {
    let pkg = spec.name.as_str();
    let pkg_config = config.packages.get(pkg);
    let (repo_name, repo_url_string, base_pkg) = resolve_pkg_repo(pkg, cli, config, Some(spec));
    let repo_url = repo_url_string.as_str();
    let base_pkg_name = base_pkg.as_str();

    if cli.install_only {
        blog!("Install-only mode, searching for existing artifacts...");
        let repo_dir_path = prepare_repo(
            pkg,
            base_pkg_name,
            &repo_name,
            repo_url,
            &config.paths.packages_path,
            false,
            false,
            None,
        );
        crate::install::install_artifacts(pkg, base_pkg_name, Some(&repo_dir_path), config);
        return true;
    }

    let install_deferred_this_run = defer_install && !cli.compile_only;

    if cli.download_only {
        blog!("Downloading sources for {}...", pkg);
        let _ = prepare_repo(
            pkg,
            base_pkg_name,
            &repo_name,
            repo_url,
            &config.paths.packages_path,
            cli.clean,
            true,
            None,
        );
        return true;
    }

    // With `-RU`, git remotes are refreshed once in `main` before manual builds — avoid a second pull per package.
    let refresh_remote = cli.force_repo_update && !cli.system_update;
    // Actual build flow
    let repo_dir_path = prepare_repo(
        pkg,
        base_pkg_name,
        &repo_name,
        repo_url,
        &config.paths.packages_path,
        cli.clean,
        refresh_remote,
        None,
    );
    let repo_dir = repo_dir_path.as_path();

    // Bash `process_package` order: `prepare_repo` → `PRE_UPDATE_COMMANDS` → `prepare_sums_pkgrel` → build …
    // Rust mirrors that **except** we snapshot `PKGBUILD` here first (Bash has no separate backup file).
    // This **must** run before `pre_update_command` (TOML `pre_update_command` / Bash `PRE_UPDATE_COMMANDS`)
    // so those hooks can edit `PKGBUILD` and we can still restore the pre-hook tree on exit.
    // If `.PKGBUILD.emerge_backup` already exists (e.g. last run stopped before restore), we do not
    // overwrite it — keep the upstream baseline for bump logic.
    backup_pkgbuild(repo_dir);
    let _guard = PkgbuildGuard { repo_dir };

    let effective_cfg = resolve_effective_config(spec, cli, config, pkg_config);

    // Resolve and inject custom compiler if specified
    if let Some(comp_key) = &effective_cfg.compiler {
        if let Some(comp_cfg) = config.compilers.get(comp_key) {
            blog!("Compiling with custom compiler '{}': cc={} cxx={}", comp_key, comp_cfg.cc, comp_cfg.cxx);
            if let Err(e) = inject_compiler_env(repo_dir, &comp_cfg.cc, &comp_cfg.cxx) {
                die!("Failed to configure custom compiler: {}", e);
            }
        } else {
            die!("Custom compiler '{}' is not defined in the [compilers] configuration section", comp_key);
        }
    }

    if cli.clean_install || config.build.clean_install_by_default {
        blog!("Clean install: removing src/ and pkg/...");
        if let Err(e) = remove_src_pkg_workdirs(repo_dir) {
            die!("Failed to remove src/ or pkg/: {}", e);
        }
    }

    if let Some(pc) = pkg_config
        && let Some(cmd) = &pc.pre_update_command
    {
        blog!("Running pre-update command...");
        if let Err(e) = run_command("sh", &["-c", cmd], Some(repo_dir)) {
            die!("Pre-update command failed: {}", e);
        }
    }

    if !spec.pkgbuild_overrides.is_empty() {
        blog!("Applying PKGBUILD overrides for {}...", pkg);
        apply_pkgbuild_overrides(repo_dir, &spec.pkgbuild_overrides);
    }

    let is_upgrade = if let Ok(src_ver) = read_pkg_full_version_from_dir(repo_dir) {
        if let Ok(Some(inst_ver)) = pacman_query_version(base_pkg_name) {
            vercmp(&src_ver, &inst_ver).ok().is_some_and(|c| c > 0)
        } else {
            true // Not installed
        }
    } else {
        false
    };

    // Run updpkgsums when `-u` is set, when CLI overrides changed PKGBUILD fields (e.g. pkgver), or when it's an upgrade.
    if (cli.update_sums || !spec.pkgbuild_overrides.is_empty() || is_upgrade) && !update_pkgsums(repo_dir) {
        ewarn!("updpkgsums failed, continuing...");
    }
    if !spec.pkgbuild_overrides.contains_key("pkgrel") {
        bump_pkgrel(repo_dir);
    }

    // Drop older PKGDEST artifacts for this base name so install prompts do not list stale builds.
    remove_stale_pkgs_in_pkgdest(
        &config.paths.ready_made_packages_path,
        base_pkg_name,
    );

    let build_env = effective_cfg.build_env.clone();
    let skip_tests = effective_cfg.skip_tests;

    let mut custom_cmd = None;
    if let Some(pc) = pkg_config {
        if build_env == "local" {
            custom_cmd = pc.custom_local_build_command.clone();
        } else {
            custom_cmd = pc.custom_chroot_build_command.clone();
        }
    }

    if let Some(cmd) = custom_cmd {
        blog!("Executing custom build command...");
        if let Err(e) = run_build_with_key_retry(&cmd, repo_dir) {
            if config.build.ignore_compilation_failures {
                ewarn!("Custom build command failed for {}: {}", pkg, e);
                restore_pkgbuild(repo_dir);
                return false;
            }
            die!("Custom build command failed: {}", e);
        }
    } else if build_env == "local" {
        blog!("Building locally with makepkg...");

        let mut build_cmd = format!(
            "PKGDEST=\"{}\" makepkg --syncdeps --noconfirm --needed -f",
            config.paths.ready_made_packages_path
        );
        if cli.clean {
            build_cmd.push_str(" -c");
        }

        if skip_tests {
            build_cmd.push_str(" --nocheck");
        }

        if let Err(e) = run_build_with_key_retry(&build_cmd, repo_dir) {
            if config.build.ignore_compilation_failures {
                ewarn!("makepkg failed for {}: {}", pkg, e);
                restore_pkgbuild(repo_dir);
                return false;
            }
            die!("makepkg failed for {}: {}", pkg, e);
        }
    } else {
        blog!("Building in chroot with makechrootpkg...");
        // `makechrootpkg -r <dir>` expects `<dir>/root` (see mkarchroot / makechrootpkg man pages).
        let chrootdir = PathBuf::from(&config.paths.chroot_base_path).join("base");
        if let Err(e) = ensure_devtools_chroot(&chrootdir).and_then(|_| inject_chroot_makepkg_conf(&chrootdir, config)) {
            if config.build.ignore_compilation_failures {
                ewarn!("Chroot setup failed for {}: {}", pkg, e);
                restore_pkgbuild(repo_dir);
                return false;
            }
            die!("Chroot setup failed for {}: {}", pkg, e);
        }
        let mut build_cmd = format!(
            "PKGDEST=\"{}\" makechrootpkg -c -r \"{}\" -d \"{}\"",
            config.paths.ready_made_packages_path,
            chrootdir.to_string_lossy(),
            repo_dir.to_string_lossy()
        );
        // Give each concurrent build its own chroot working copy so parallel `makechrootpkg`
        // invocations do not clobber the shared default `<chrootdir>/$USER` copy.
        if let Some(copy) = chroot_copy {
            build_cmd.push_str(&format!(" -l \"{}\"", copy));
        }
        if skip_tests {
            build_cmd.push_str(" -- --nocheck");
        }
        if let Err(e) = run_build_with_key_retry(&build_cmd, repo_dir) {
            if config.build.ignore_compilation_failures {
                ewarn!("makechrootpkg failed for {}: {}", pkg, e);
                restore_pkgbuild(repo_dir);
                return false;
            }
            die!("makechrootpkg failed for {}: {}", pkg, e);
        }
    }

    // Bash: install then post-update (both only if not `-o` and not deferred). Hooks still see the bumped PKGBUILD.
    if !cli.compile_only && !install_deferred_this_run {
        crate::install::install_artifacts(pkg, base_pkg_name, Some(repo_dir), config);

        if let Some(pc) = pkg_config
            && let Some(cmd) = &pc.post_update_command
        {
            blog!("Running post-update command...");
            if let Err(e) = run_command("sh", &["-c", cmd], Some(repo_dir)) {
                ewarn!("Post-update command failed: {}", e);
            }
        }
    }

    // Build (and optional install) are done — no more compilation. Restore upstream PKGBUILD now
    // instead of only at scope end; `Drop` becomes a no-op once backup is consumed.
    restore_pkgbuild(repo_dir);

    true
}

#[cfg(test)]
mod tests {
    use super::{known_repository_url, repository_url};
    use std::collections::HashMap;

    fn sample_repos() -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("default".into(), "arch".into());
        m.insert("arch".into(), "https://gitlab.example/pkg".into());
        m.insert(
            "cachyos".into(),
            "https://github.com/example/cachy.git".into(),
        );
        m
    }

    #[test]
    fn repository_url_direct_https() {
        let m = sample_repos();
        assert_eq!(
            repository_url(&m, "cachyos").as_deref(),
            Some("https://github.com/example/cachy.git")
        );
    }

    #[test]
    fn repository_url_follows_default_chain() {
        let m = sample_repos();
        assert_eq!(
            repository_url(&m, "default").as_deref(),
            Some("https://gitlab.example/pkg")
        );
    }

    #[test]
    fn repository_url_git_ssh() {
        let mut m = HashMap::new();
        m.insert("priv".into(), "git@github.com:org/repo.git".into());
        assert_eq!(
            repository_url(&m, "priv").as_deref(),
            Some("git@github.com:org/repo.git")
        );
    }

    #[test]
    fn repository_url_unknown_returns_none() {
        let m = sample_repos();
        assert!(repository_url(&m, "missing").is_none());
    }

    #[test]
    fn known_repository_url_aur() {
        assert_eq!(
            known_repository_url("aur").as_deref(),
            Some("https://aur.archlinux.org")
        );
        assert_eq!(
            known_repository_url("AUR").as_deref(),
            Some("https://aur.archlinux.org")
        );
    }

    #[test]
    fn test_aur_up_to_date_cache() {
        let pkg = "test_pkg_cache";
        // Ensure starting state is false
        super::unmark_aur_package_up_to_date(pkg);
        assert!(!super::is_aur_package_up_to_date(pkg));

        // Mark it as up to date and check
        super::mark_aur_package_up_to_date(pkg);
        assert!(super::is_aur_package_up_to_date(pkg));

        // Unmark it and check
        super::unmark_aur_package_up_to_date(pkg);
        assert!(!super::is_aur_package_up_to_date(pkg));
    }
}
