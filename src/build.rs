use crate::cli::Cli;
use crate::config::Config;
use crate::git::{is_per_package_repo, prepare_repo, PkgbuildDirCache};
use crate::package_spec::PackageSpec;
use crate::pkgbuild::{
    apply_pkgbuild_overrides, backup_pkgbuild, bump_pkgrel, parse_validpgpkeys, restore_pkgbuild,
    update_pkgsums, inject_compiler_env,
};
use crate::utils::{
    check_sudo_removal, gpg_has_public_key, gpg_key_short_id, import_gpg_key_for_build,
    pacman_query_version, pacman_sync_version, read_pkg_full_version_from_dir,
    remove_src_pkg_workdirs, remove_stale_pkgs_in_pkgdest, run_command, run_shell_in_dir_with_tee,
    vercmp, ShellRunOpts,
};
use crate::ramdisk::{self, WorkdirGuard};
use std::sync::atomic::{AtomicUsize, Ordering};
use crate::{blog, die, ewarn, vlog};
use colored::Colorize;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// When compilation uses tmpfs (`w`) but the git repo stays on disk, keep makepkg source
/// tarballs on disk so PGO stages do not re-download the kernel archive.
fn ramdisk_srcdest_env(repo_dir: &Path, targets: &ramdisk::RamdiskTargets) -> String {
    if !targets.build_workdir || targets.packages {
        return String::new();
    }
    let srcdest = ramdisk::srcdest_for_repo(repo_dir);
    if let Err(e) = std::fs::create_dir_all(&srcdest) {
        crate::ewarn!("Failed to create SRCDEST {}: {e}", srcdest.display());
    }
    format!(
        "SRCDEST={} ",
        crate::utils::sh_single_quote(&srcdest.to_string_lossy())
    )
}

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
    pub skip_restore: bool,
}

impl<'a> Drop for PkgbuildGuard<'a> {
    fn drop(&mut self) {
        if !self.skip_restore {
            restore_pkgbuild(self.repo_dir);
        }
    }
}

/// Overrides used by the kernel PGO pipeline when calling makepkg.
#[derive(Debug, Clone)]
pub struct PgoBuildContext {
    pub env_vars: HashMap<String, String>,
    pub makepkg_flags: String,
    pub clean_src: bool,
    pub clean_pkg: bool,
    pub defer_pkgbuild_restore: bool,
    pub skip_abs_install: bool,
}

/// Mirror the CachyOS kernel PKGBUILD `pkgbase=linux-$_pkgsuffix` naming from the PGO env vars,
/// starting from the package being built so kernel variants (e.g. `linux-cachyos-bore` →
/// `linux-cachyos-bore-lto`) resolve correctly instead of assuming `linux-cachyos`.
pub fn pgo_pkgbase_from_env(package: &str, env: &HashMap<String, String>) -> String {
    // `linux-cachyos-lto` / `-gcc` are suffix builds of the plain PKGBUILD, so strip the
    // suffix first and re-apply it per the stage env (stage 1 builds the plain kernel).
    let base = package
        .strip_suffix("-lto")
        .or_else(|| package.strip_suffix("-gcc"))
        .unwrap_or(package)
        .trim();
    let base = if base.is_empty() { "linux-cachyos" } else { base };
    let llvm_lto = env.get("_use_llvm_lto").map(String::as_str).unwrap_or("thin");
    let is_lto = matches!(llvm_lto, "thin" | "full" | "thin-dist");
    let lto_suffix = env.get("_use_lto_suffix").map(String::as_str).unwrap_or("no");
    let gcc_suffix = env.get("_use_gcc_suffix").map(String::as_str).unwrap_or("yes");
    if is_lto && lto_suffix == "yes" {
        format!("{base}-lto")
    } else if !is_lto && gcc_suffix == "yes" {
        format!("{base}-gcc")
    } else {
        base.to_string()
    }
}

fn chroot_rootfs_is_complete(rootfs: &Path) -> bool {
    crate::ramdisk::is_chroot_rootfs_complete(rootfs)
}

/// Ensure `<chrootdir>/root` exists for `makechrootpkg -r <chrootdir>` (see makechrootpkg(1)).
fn ensure_devtools_chroot(chrootdir: &Path) -> Result<(), String> {
    let rootfs = chrootdir.join("root");

    if rootfs.is_dir() {
        if chroot_rootfs_is_complete(&rootfs) {
            vlog!("Using existing chroot rootfs at {}.", rootfs.display());
            return Ok(());
        }
        blog!(
            "Incomplete chroot rootfs at {}; removing and recreating with mkarchroot...",
            rootfs.display()
        );
        check_sudo_removal(&rootfs)?;
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
    crate::utils::phase_banner(
        "mkarchroot: installing base-devel into chroot (locale generation is the last step)",
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

    crate::utils::restore_terminal();

    if !rootfs.is_dir() {
        return Err(format!(
            "mkarchroot finished but {} is not a usable directory",
            rootfs.display()
        ));
    }

    blog!("Chroot rootfs ready at {}.", rootfs.display());
    crate::utils::phase_banner("mkarchroot finished — syncing chroot working copy next");
    Ok(())
}

fn chroot_worker_copy_name(chroot_copy: Option<&str>) -> String {
    if let Some(name) = chroot_copy {
        return name.to_string();
    }
    if let Ok(user) = std::env::var("SUDO_USER")
        && !user.is_empty()
        && user != "root"
    {
        return user;
    }
    match std::env::var("USER") {
        Ok(user) if !user.is_empty() && user != "root" => user,
        _ => "copy".to_string(),
    }
}

fn chroot_copy_lock_path(chrootdir: &Path, copy: &str) -> PathBuf {
    chrootdir.join(format!("{copy}.lock"))
}

/// Mirror `makechrootpkg` `sync_chroot` with visible rsync progress (devtools uses `rsync -q`).
fn sync_chroot_working_copy(
    chrootdir: &Path,
    copy: &str,
    refresh: bool,
) -> Result<(), String> {
    let root = chrootdir.join("root");
    let copydir = chrootdir.join(copy);
    let lock_path = chroot_copy_lock_path(chrootdir, copy);

    if lock_path.is_file() {
        vlog!("Removing stale chroot lock {}", lock_path.display());
        let _ = check_sudo_removal(&lock_path);
    }

    if copydir.is_dir() {
        if refresh || !chroot_rootfs_is_complete(&copydir) {
            blog!(
                "Refreshing chroot working copy at {}...",
                copydir.display()
            );
            check_sudo_removal(&copydir)?;
        } else {
            vlog!("Using existing chroot working copy at {}", copydir.display());
            return Ok(());
        }
    }

    if !chroot_rootfs_is_complete(&root) {
        return Err(format!(
            "chroot root {} is missing or incomplete; cannot sync working copy",
            root.display()
        ));
    }

    crate::utils::phase_banner(format!(
        "rsync: {} → {} (makechrootpkg skips this when the copy already exists)",
        root.display(),
        copydir.display()
    ));

    // The chroot root holds root-owned 0600/0700 files (shadow, gnupg keys, sudoers).
    // devtools' makechrootpkg syncs as root; do the same so rsync can read them.
    run_command(
        "sudo",
        &["mkdir", "-p", &copydir.to_string_lossy()],
        None::<&str>,
    )?;

    let src = format!("{}/", root.to_string_lossy());
    let dst = format!("{}/", copydir.to_string_lossy());
    run_command(
        "sudo",
        &[
            "rsync", "-a", "--delete", "--info=progress2", "-W", "-x", &src, &dst,
        ],
        None::<&str>,
    )?;
    run_command(
        "sudo",
        &["touch", copydir.to_string_lossy().as_ref()],
        None::<&str>,
    )?;
    blog!("Chroot working copy ready at {}.", copydir.display());
    Ok(())
}

static ACTIVE_CHROOT_BUILDS: AtomicUsize = AtomicUsize::new(0);

/// Tracks an in-flight chroot build and resets the devtools chroot on drop when configured.
struct ChrootBuildGuard {
    chrootdir: PathBuf,
    worker_copy: Option<String>,
    clean: bool,
}

impl ChrootBuildGuard {
    fn new(config: &Config, chrootdir: PathBuf, chroot_copy: Option<&str>) -> Self {
        let clean = config.build.clean_chroot_after_compilation;
        if clean {
            ACTIVE_CHROOT_BUILDS.fetch_add(1, Ordering::SeqCst);
        }
        Self {
            chrootdir,
            worker_copy: chroot_copy.map(str::to_string),
            clean,
        }
    }

    fn worker_copy_path(&self) -> Option<PathBuf> {
        if let Some(name) = &self.worker_copy {
            return Some(self.chrootdir.join(name));
        }
        std::env::var("USER")
            .ok()
            .map(|user| self.chrootdir.join(user))
    }
}

impl Drop for ChrootBuildGuard {
    fn drop(&mut self) {
        if !self.clean {
            return;
        }

        let remaining = ACTIVE_CHROOT_BUILDS.fetch_sub(1, Ordering::SeqCst);
        if remaining == 1 {
            vlog!(
                "Cleaning devtools chroot at {} after build...",
                self.chrootdir.display()
            );
            if let Err(e) = check_sudo_removal(&self.chrootdir) {
                ewarn!(
                    "Failed to reset devtools chroot at {}: {}",
                    self.chrootdir.display(),
                    e
                );
            }
            return;
        }

        if remaining > 1
            && let Some(copy_path) = self.worker_copy_path()
        {
            vlog!(
                "Removing chroot working copy {} (parallel builds still running)...",
                copy_path.display()
            );
            if let Err(e) = check_sudo_removal(&copy_path) {
                ewarn!(
                    "Failed to remove chroot working copy {}: {}",
                    copy_path.display(),
                    e
                );
            }
        }
    }
}

fn ensure_pkgsource_pgp_keys(build_dir: &Path) {
    let pkgbuild_path = build_dir.join("PKGBUILD");
    let Ok(text) = std::fs::read_to_string(&pkgbuild_path) else {
        return;
    };
    for key in parse_validpgpkeys(&text) {
        if gpg_has_public_key(&key) {
            continue;
        }
        let short = gpg_key_short_id(&key);
        crate::blog!("Importing PKGBUILD signing key {}...", short);
        if let Err(e) = import_gpg_key_for_build(&key) {
            crate::ewarn!("Could not import PGP key {}: {}", short, e);
        }
    }
}

fn run_build_with_key_retry(
    build_cmd: &str,
    repo_dir: &Path,
    opts: ShellRunOpts,
) -> Result<(), String> {
    let key_re = Regex::new(r"(?i)unknown public key ([0-9A-F]+)")
        .map_err(|e| format!("Failed to compile missing-key regex: {}", e))?;
    // Large logs (e.g. Firefox) can mention "unknown public key" long before the real failure in
    // `prepare()` / `build()` / `check()`. Retrying the whole makepkg then re-runs those phases for no benefit.
    let pkgbuild_phase_failed_re = Regex::new(r"(?i)A failure occurred in (prepare|build|check)\(\)")
        .map_err(|e| format!("Failed to compile phase-failure regex: {}", e))?;
    let mut seen_keys: HashSet<String> = HashSet::new();

    loop {
        match run_shell_in_dir_with_tee(repo_dir, build_cmd, opts) {
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
                if newly_found.is_empty()
                    && err.to_ascii_lowercase().contains("pgp")
                    && let Ok(text) = std::fs::read_to_string(repo_dir.join("PKGBUILD"))
                {
                    for key in parse_validpgpkeys(&text) {
                        let short = gpg_key_short_id(&key).to_string();
                        if seen_keys.insert(short.clone()) {
                            newly_found.push(short);
                        }
                    }
                }
                if newly_found.is_empty() {
                    return Err(err);
                }

                for key in newly_found {
                    crate::blog!("Importing missing PGP key {}...", key);
                    if let Err(gpg_err) = import_gpg_key_for_build(&key) {
                        return Err(format!(
                            "Build failed and key import also failed for {}: {}\nOriginal build error:\n{}",
                            key, gpg_err, err
                        ));
                    }
                }
                crate::blog!("Retrying build after importing PGP keys...");
            }
        }
    }
}

pub fn resolve_pkg_repo(
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
                vlog!("{}: not installed; skipping manual update build", pkg);
                return Ok(false);
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
        &ramdisk::packages_path_for_pkg(config, pkg),
        false,
        false,
        None,
    )
    .pkg_dir;
    let src_ver = read_pkg_full_version_from_dir(pkg_dir.as_path())?;
    let Some(inst_ver) = pacman_query_version(&base_pkg)? else {
        vlog!("{}: not installed; skipping manual update build", pkg);
        return Ok(false);
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
                    let packages_path = ramdisk::packages_path_for_pkg(config, &task.pkg);
                    let _ = prepare_repo(
                        &task.pkg,
                        &task.base_pkg,
                        &task.repo_name,
                        task.repo_url_string.as_str(),
                        &packages_path,
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
    NotInstalled,
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
                return Ok(ManualPkgVersionLine::NotInstalled);
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
        &ramdisk::packages_path_for_pkg(config, pkg),
        false,
        false,
        Some(pkgbuild_cache),
    )
    .pkg_dir;
    let src = read_pkg_full_version_from_dir(pkg_dir.as_path())?;
    let inst = pacman_query_version(&base_pkg)?;
    let Some(inst) = inst else {
        return Ok(ManualPkgVersionLine::NotInstalled);
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
        ManualPkgVersionLine::NotInstalled => {
            print!("{}", "Not installed".yellow().bold());
            println!(" (skipped)");
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

/// Whether to force a rebuild even when PKGDEST already has matching artifacts.
/// Precedence: CLI `-n` > per-package config > `[build].ignore_already_made_packages`.
fn should_ignore_already_made(pkg: &str, cli: &Cli, config: &Config) -> bool {
    if cli.force_build {
        return true;
    }
    config
        .packages
        .get(pkg)
        .and_then(|pc| pc.ignore_already_made_packages)
        .unwrap_or(config.build.ignore_already_made_packages)
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
    let ramdisk_targets = match ramdisk::resolve_ramdisk_targets(
        config,
        pkg_config,
        Some(spec),
        cli.ramdisk.as_deref(),
    ) {
        Ok(t) => t,
        Err(e) => die!("Invalid ramdisk targets for {}: {}", pkg, e),
    };
    if let Err(e) = ramdisk::ensure_for_targets(config, &ramdisk_targets) {
        die!("Ramdisk setup failed for {}: {}", pkg, e);
    }
    let packages_path = ramdisk::download_packages_path(config, &ramdisk_targets);
    let (repo_name, repo_url_string, base_pkg) = resolve_pkg_repo(pkg, cli, config, Some(spec));
    let repo_dir_path = prepare_repo(
        pkg,
        base_pkg.as_str(),
        &repo_name,
        repo_url_string.as_str(),
        &packages_path,
        false,
        false,
        None,
    )
    .pkg_dir;
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
    compilation_threads: Option<usize>,
) -> bool {
    let pkg = spec.name.as_str();
    let pkg_config = config.packages.get(pkg);
    let ramdisk_targets = match ramdisk::resolve_ramdisk_targets(
        config,
        pkg_config,
        Some(spec),
        cli.ramdisk.as_deref(),
    ) {
        Ok(t) => t,
        Err(e) => die!("Invalid ramdisk targets for {}: {}", pkg, e),
    };
    if let Err(e) = ramdisk::ensure_for_targets(config, &ramdisk_targets) {
        die!("Ramdisk setup failed for {}: {}", pkg, e);
    }
    ramdisk::warn_if_packages_on_ram(config, pkg, &ramdisk_targets);
    let packages_path = ramdisk::download_packages_path(config, &ramdisk_targets);
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
            &packages_path,
            false,
            false,
            None,
        )
        .pkg_dir;
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
            &packages_path,
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
        &packages_path,
        cli.clean,
        refresh_remote,
        None,
    )
    .pkg_dir;
    let repo_dir = repo_dir_path.as_path();

    // Reuse PKGDEST artifacts when present unless -n / config forces a rebuild.
    // Require artifact version >= PKGBUILD so a stale ready package (e.g. 5.2.3-1.2) does not
    // skip rebuilding after upstream bumps pkgrel (5.2.3-2).
    let src_ver = read_pkg_full_version_from_dir(repo_dir).ok();
    if !should_ignore_already_made(pkg, cli, config)
        && crate::install::has_ready_made_artifacts(
            pkg,
            base_pkg_name,
            &config.paths.ready_made_packages_path,
            src_ver.as_deref(),
        )
    {
        blog!(
            "Already-made packages found for {}; skipping compilation (use -n to rebuild)",
            pkg
        );
        if !cli.compile_only && !install_deferred_this_run {
            crate::install::install_artifacts(pkg, base_pkg_name, Some(repo_dir), config);
        }
        return true;
    }

    // Bash `process_package` order: `prepare_repo` → `PRE_UPDATE_COMMANDS` → `prepare_sums_pkgrel` → build …
    // Rust mirrors that **except** we snapshot `PKGBUILD` here first (Bash has no separate backup file).
    // This **must** run before `pre_update_command` (TOML `pre_update_command` / Bash `PRE_UPDATE_COMMANDS`)
    // so those hooks can edit `PKGBUILD` and we can still restore the pre-hook tree on exit.
    // If `.PKGBUILD.emerge_backup` already exists (e.g. last run stopped before restore), we do not
    // overwrite it — keep the upstream baseline for bump logic.
    backup_pkgbuild(repo_dir);
    let _guard = PkgbuildGuard {
        repo_dir,
        skip_restore: false,
    };

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

    // Kernel build variables are baked into the PKGBUILD so they apply in both local and chroot
    // builds (env-var prefixes do not propagate into makechrootpkg's nspawn environment).
    if !spec.kernel_vars.is_empty() {
        blog!("Applying kernel build options for {}...", pkg);
        apply_pkgbuild_overrides(repo_dir, &spec.kernel_vars);
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
    let threads = compilation_threads
        .or_else(|| crate::build_env::resolve_package_threads(pkg, config, cli));

    let workdir_guard = match WorkdirGuard::setup(config, repo_dir, &ramdisk_targets, false) {
        Ok(guard) => guard,
        Err(e) => die!("Ramdisk build workdir setup failed: {}", e),
    };
    let build_dir = workdir_guard
        .as_ref()
        .map(WorkdirGuard::build_dir)
        .unwrap_or(repo_dir);

    let mut custom_cmd = None;
    if let Some(pc) = pkg_config {
        if build_env == "local" {
            custom_cmd = pc.custom_local_build_command.clone();
        } else {
            custom_cmd = pc.custom_chroot_build_command.clone();
        }
    }

    if let Some(cmd) = custom_cmd {
        ensure_pkgsource_pgp_keys(build_dir);
        blog!("Executing custom build command...");
        if let Err(e) = run_build_with_key_retry(&cmd, build_dir, ShellRunOpts::default()) {
            if config.build.ignore_compilation_failures {
                ewarn!("Custom build command failed for {}: {}", pkg, e);
                restore_pkgbuild(repo_dir);
                return false;
            }
            die!("Custom build command failed: {}", e);
        }
    } else if build_env == "local" {
        ensure_pkgsource_pgp_keys(build_dir);
        blog!("Building locally with makepkg...");

        let mut env_prefix = ramdisk_srcdest_env(repo_dir, &ramdisk_targets);
        // Keeps the wrapper makepkg.conf alive until the build command finished.
        let mut _limiter_guard = None;
        if let Some(n) = threads {
            match crate::build_env::write_local_limiter_conf(pkg, n) {
                Ok(guard) => {
                    env_prefix = format!("{} {env_prefix}", guard.env_assignment());
                    _limiter_guard = Some(guard);
                }
                Err(e) => {
                    if config.build.ignore_compilation_failures {
                        ewarn!("Parallel limiter setup failed for {}: {}", pkg, e);
                        restore_pkgbuild(repo_dir);
                        return false;
                    }
                    die!("Parallel limiter setup failed for {}: {}", pkg, e);
                }
            }
        }

        let mut build_cmd = format!(
            "{}PKGDEST=\"{}\" makepkg --syncdeps --noconfirm --needed -f",
            env_prefix,
            config.paths.ready_made_packages_path
        );
        if cli.clean && !ramdisk_targets.build_workdir {
            build_cmd.push_str(" -c");
        }

        if skip_tests {
            build_cmd.push_str(" --nocheck");
        }

        build_cmd = strip_makepkg_cleanbuild_in_shell(&build_cmd);

        if let Err(e) = run_build_with_key_retry(&build_cmd, build_dir, ShellRunOpts::default()) {
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
        let chroot_base = ramdisk::effective_chroot_base_path(config, &ramdisk_targets);
        let chrootdir = PathBuf::from(&chroot_base).join("base");
        let _chroot_guard = ChrootBuildGuard::new(config, chrootdir.clone(), chroot_copy);
        blog!(
            "Preparing chroot at {} (seed copy or mkarchroot if needed)...",
            chrootdir.display()
        );
        if let Err(e) = ramdisk::seed_chroot_if_needed(config, Path::new(&chroot_base), &ramdisk_targets)
            .and_then(|_| ensure_devtools_chroot(&chrootdir))
            .and_then(|_| inject_chroot_makepkg_conf(&chrootdir, config))
        {
            if config.build.ignore_compilation_failures {
                ewarn!("Chroot setup failed for {}: {}", pkg, e);
                restore_pkgbuild(repo_dir);
                return false;
            }
            die!("Chroot setup failed for {}: {}", pkg, e);
        }
        let copy_name = chroot_worker_copy_name(chroot_copy);
        if let Err(e) = sync_chroot_working_copy(&chrootdir, &copy_name, cli.clean) {
            if config.build.ignore_compilation_failures {
                ewarn!("Chroot sync failed for {}: {}", pkg, e);
                restore_pkgbuild(repo_dir);
                return false;
            }
            die!("Chroot sync failed for {}: {}", pkg, e);
        }
        if let Err(e) =
            crate::build_env::apply_chroot_parallel_dropin(&chrootdir, &copy_name, threads)
        {
            if config.build.ignore_compilation_failures {
                ewarn!("Chroot parallel limiter setup failed for {}: {}", pkg, e);
                restore_pkgbuild(repo_dir);
                return false;
            }
            die!("Chroot parallel limiter setup failed for {}: {}", pkg, e);
        }
        ensure_pkgsource_pgp_keys(build_dir);
        let mut build_cmd = format!(
            "PKGDEST=\"{}\" makechrootpkg -r \"{}\" -d \"{}\"",
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
        blog!("Starting makechrootpkg for {}...", pkg);
        crate::utils::phase_banner(format!(
            "makechrootpkg: installing dependencies and building {pkg} (large packages can take 30+ minutes)"
        ));
        let chroot_opts = ShellRunOpts {
            live_output: true,
            heartbeat_label: Some("makechrootpkg"),
        };
        let chroot_result = run_build_with_key_retry(&build_cmd, repo_dir, chroot_opts);
        if let Err(e) = crate::build_env::apply_chroot_parallel_dropin(&chrootdir, &copy_name, None)
        {
            vlog!("Failed to remove chroot parallel drop-in: {}", e);
        }
        if let Err(e) = chroot_result {
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

fn remove_dir_if_exists(path: &Path) {
    if path.exists()
        && let Err(e) = check_sudo_removal(path)
    {
        die!("Failed to remove {}: {}", path.display(), e);
    }
}

fn format_pgo_makepkg_cmd(
    config: &Config,
    pgo: &PgoBuildContext,
    repo_dir: &Path,
    targets: &ramdisk::RamdiskTargets,
    limiter_env: Option<String>,
) -> String {
    let env_prefix: String = pgo
        .env_vars
        .iter()
        .map(|(k, v)| format!("{}={}", k, crate::utils::sh_single_quote(v)))
        .collect::<Vec<_>>()
        .join(" ");
    let srcdest = ramdisk_srcdest_env(repo_dir, targets);
    let pkgdest = format!(
        "PKGDEST={}",
        crate::utils::sh_single_quote(&config.paths.ready_made_packages_path)
    );
    let mut parts = Vec::new();
    if !srcdest.is_empty() {
        parts.push(srcdest.trim().to_string());
    }
    if let Some(env) = limiter_env {
        parts.push(env);
    }
    if !env_prefix.is_empty() {
        parts.push(env_prefix);
    }
    parts.push(pkgdest);
    let makepkg_flags = sanitize_makepkg_flags_for_ramdisk(&pgo.makepkg_flags, targets);
    format!(
        "{} makepkg --syncdeps --noconfirm {}",
        parts.join(" "),
        makepkg_flags
    )
}

/// makepkg `-c` / `--cleanbuild` removes `src/` and recreates it on disk, breaking ramdisk
/// symlinks. Strip those flags when compilation uses tmpfs (`w`).
fn sanitize_makepkg_flags_for_ramdisk(flags: &str, targets: &ramdisk::RamdiskTargets) -> String {
    if !targets.build_workdir {
        return flags.to_string();
    }
    strip_makepkg_cleanbuild_tokens(flags)
}

/// Remove makepkg clean flags from a token list or full shell command (flags after `makepkg` only).
pub fn strip_makepkg_cleanbuild_in_shell(cmd: &str) -> String {
    let mut out = Vec::new();
    let mut past_makepkg = false;
    let mut stripped = false;
    for tok in cmd.split_whitespace() {
        if tok == "makepkg" {
            past_makepkg = true;
            out.push(tok);
            continue;
        }
        if past_makepkg && (tok == "--cleanbuild" || tok == "-c") {
            stripped = true;
            continue;
        }
        out.push(tok);
    }
    if stripped {
        crate::blog!(
            "Omitting makepkg --cleanbuild/-c so src/ and pkg/ stay on ramdisk tmpfs"
        );
    }
    out.join(" ")
}

fn strip_makepkg_cleanbuild_tokens(flags: &str) -> String {
    let tokens: Vec<&str> = flags.split_whitespace().collect();
    let sanitized: Vec<&str> = tokens
        .iter()
        .copied()
        .filter(|tok| *tok != "--cleanbuild" && *tok != "-c")
        .collect();
    sanitized.join(" ")
}

/// Build a package under PGO pipeline overrides (env-injected makepkg, optional src/pkg cleanup).
pub fn process_package_pgo(
    spec: &PackageSpec,
    cli: &Cli,
    config: &Config,
    pgo: &PgoBuildContext,
    events: &crate::pgo::EventLog,
) -> bool {
    let pkg = spec.name.as_str();
    let pkg_config = config.packages.get(pkg);
    let ramdisk_targets = match ramdisk::resolve_ramdisk_targets(
        config,
        pkg_config,
        Some(spec),
        cli.ramdisk.as_deref(),
    ) {
        Ok(t) => t,
        Err(e) => die!("Invalid ramdisk targets for {}: {}", pkg, e),
    };
    if let Err(e) = ramdisk::ensure_for_targets(config, &ramdisk_targets) {
        die!("Ramdisk setup failed for {}: {}", pkg, e);
    }
    ramdisk::warn_if_packages_on_ram(config, pkg, &ramdisk_targets);
    let packages_path = ramdisk::download_packages_path(config, &ramdisk_targets);
    let (repo_name, repo_url_string, base_pkg) = resolve_pkg_repo(pkg, cli, config, Some(spec));
    let repo_dir_path = prepare_repo(
        pkg,
        base_pkg.as_str(),
        &repo_name,
        repo_url_string.as_str(),
        &packages_path,
        false,
        false,
        None,
    )
    .pkg_dir;
    let repo_dir = repo_dir_path.as_path();

    backup_pkgbuild(repo_dir);
    let _guard = PkgbuildGuard {
        repo_dir,
        skip_restore: pgo.defer_pkgbuild_restore,
    };

    if pgo.clean_src && !ramdisk_targets.build_workdir {
        remove_dir_if_exists(&repo_dir.join("src"));
    }
    if pgo.clean_pkg && !ramdisk_targets.build_workdir {
        remove_dir_if_exists(&repo_dir.join("pkg"));
    }

    if !spec.pkgbuild_overrides.is_empty() {
        apply_pkgbuild_overrides(repo_dir, &spec.pkgbuild_overrides);
    }

    if !spec.pkgbuild_overrides.contains_key("pkgrel") {
        bump_pkgrel(repo_dir);
    }

    let workdir_guard = match WorkdirGuard::setup(config, repo_dir, &ramdisk_targets, pgo.clean_pkg) {
        Ok(guard) => guard,
        Err(e) => die!("Ramdisk build workdir setup failed: {}", e),
    };
    let build_dir = workdir_guard
        .as_ref()
        .map(WorkdirGuard::build_dir)
        .unwrap_or(repo_dir);

    let threads = crate::build_env::resolve_package_threads(pkg, config, cli);
    // Keeps the wrapper makepkg.conf alive until the build command finished.
    let limiter_guard = match threads.map(|n| crate::build_env::write_local_limiter_conf(pkg, n)) {
        Some(Ok(guard)) => Some(guard),
        Some(Err(e)) => die!("Parallel limiter setup failed for {}: {}", pkg, e),
        None => None,
    };

    let mut build_cmd = format_pgo_makepkg_cmd(
        config,
        pgo,
        repo_dir,
        &ramdisk_targets,
        limiter_guard.as_ref().map(|g| g.env_assignment()),
    );
    if workdir_guard.as_ref().is_some_and(WorkdirGuard::uses_ramdisk) {
        build_cmd = strip_makepkg_cleanbuild_in_shell(&build_cmd);
    }
    events.log_line(
        "stdout",
        format!("$ (cd {} && {build_cmd})", build_dir.display()),
    );

    if let Err(e) = run_build_with_key_retry(&build_cmd, build_dir, ShellRunOpts::default()) {
        if !pgo.defer_pkgbuild_restore {
            restore_pkgbuild(repo_dir);
        }
        die!("PGO makepkg failed for {}: {}", pkg, e);
    }

    if !pgo.skip_abs_install && !cli.compile_only {
        let pkgbase = pgo_pkgbase_from_env(pkg, &pgo.env_vars);
        blog!("PGO stage installs packages for pkgbase {pkgbase}");
        crate::install::install_pgo_artifacts(
            pkg,
            &pkgbase,
            Some(repo_dir),
            config,
            &pgo.env_vars,
        );
    }

    if !pgo.defer_pkgbuild_restore {
        restore_pkgbuild(repo_dir);
    }

    true
}

/// One-shot (non-PGO) kernel build: applies the user's full `[packages.PKG.kernel]` options
/// (scheduler, compiler/LTO, tick rate, etc.) to the PKGBUILD, then builds and installs in a
/// single pass via the normal build path. This honors the configured `build_env` (local or
/// chroot), custom build commands, and ramdisk targets, with no profiling.
pub fn process_kernel_oneshot(package: &str, cli: &Cli, config: &Config) -> bool {
    let kernel = config
        .packages
        .get(package)
        .and_then(|p| p.kernel.clone())
        .unwrap_or_default();

    let mut spec = PackageSpec::plain(package);
    for (key, val) in crate::config::kernel_override_pairs(&kernel) {
        if let Some(v) = val {
            spec.kernel_vars.insert(key.to_string(), v.clone());
        }
    }

    blog!("One-shot kernel build for {} (no PGO)", package);
    process_package(&spec, cli, config, false, None, None)
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
    fn strip_makepkg_cleanbuild_in_shell_command() {
        use super::strip_makepkg_cleanbuild_in_shell;

        let cmd = "SRCDEST='/tmp/x' PKGDEST='/tmp/r' makepkg --syncdeps --noconfirm --cleanbuild -sfi";
        assert_eq!(
            strip_makepkg_cleanbuild_in_shell(cmd),
            "SRCDEST='/tmp/x' PKGDEST='/tmp/r' makepkg --syncdeps --noconfirm -sfi"
        );
    }

    #[test]
    fn sanitize_makepkg_flags_strips_cleanbuild_for_ramdisk_w() {
        use super::sanitize_makepkg_flags_for_ramdisk;
        use crate::ramdisk::RamdiskTargets;

        let w = RamdiskTargets {
            build_workdir: true,
            ..Default::default()
        };
        assert_eq!(
            sanitize_makepkg_flags_for_ramdisk("--cleanbuild -sfi --noconfirm", &w),
            "-sfi --noconfirm"
        );
        assert_eq!(
            sanitize_makepkg_flags_for_ramdisk("-sfi -c", &w),
            "-sfi"
        );
        let no_w = RamdiskTargets::default();
        assert_eq!(
            sanitize_makepkg_flags_for_ramdisk("--cleanbuild -sfi", &no_w),
            "--cleanbuild -sfi"
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

    #[test]
    fn pgo_pkgbase_from_env_matches_cachyos_pkgnaming() {
        use std::collections::HashMap;

        let stage1 = HashMap::from([
            ("_use_llvm_lto".into(), "none".into()),
            ("_use_lto_suffix".into(), "no".into()),
            ("_use_gcc_suffix".into(), "no".into()),
        ]);
        assert_eq!(
            super::pgo_pkgbase_from_env("linux-cachyos", &stage1),
            "linux-cachyos"
        );

        let stage2 = HashMap::from([
            ("_use_llvm_lto".into(), "thin".into()),
            ("_use_lto_suffix".into(), "yes".into()),
            ("_use_gcc_suffix".into(), "no".into()),
        ]);
        assert_eq!(
            super::pgo_pkgbase_from_env("linux-cachyos", &stage2),
            "linux-cachyos-lto"
        );

        // Kernel variants keep their own suffix instead of collapsing to linux-cachyos.
        assert_eq!(
            super::pgo_pkgbase_from_env("linux-cachyos-bore", &stage1),
            "linux-cachyos-bore"
        );
        assert_eq!(
            super::pgo_pkgbase_from_env("linux-cachyos-bore", &stage2),
            "linux-cachyos-bore-lto"
        );
        // A package already named with the -lto suffix builds the plain kernel at stage 1
        // and does not double the suffix at stage 2.
        assert_eq!(
            super::pgo_pkgbase_from_env("linux-cachyos-lto", &stage1),
            "linux-cachyos"
        );
        assert_eq!(
            super::pgo_pkgbase_from_env("linux-cachyos-lto", &stage2),
            "linux-cachyos-lto"
        );
    }

    #[test]
    fn chroot_rootfs_complete_requires_pacman_tree() {
        let tmp = std::env::temp_dir().join(format!(
            "abs-chroot-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("root")).unwrap();
        assert!(!super::chroot_rootfs_is_complete(&tmp.join("root")));
        std::fs::create_dir_all(tmp.join("root/etc")).unwrap();
        std::fs::write(tmp.join("root/etc/pacman.conf"), "[options]\n").unwrap();
        std::fs::create_dir_all(tmp.join("root/usr/bin")).unwrap();
        std::fs::write(tmp.join("root/usr/bin/pacman"), "").unwrap();
        std::fs::create_dir_all(tmp.join("root/var/lib/pacman/local")).unwrap();
        assert!(super::chroot_rootfs_is_complete(&tmp.join("root")));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    fn minimal_cli(force_build: bool) -> crate::cli::Cli {
        crate::cli::Cli {
            download_only: false,
            local_build: false,
            chroot_build: false,
            compile_only: false,
            no_check: false,
            force_build,
            clean: false,
            clean_all: false,
            use_sudo_clean: false,
            remove_chroot: false,
            install_keys: false,
            update_sums: false,
            verbose: false,
            silent: false,
            force_repo_update: false,
            system_update: false,
            repo: None,
            jobs: None,
            install_only: false,
            clean_install: false,
            dry_run: true,
            list: false,
            configure: None,
            check_update: false,
            self_update: false,
            help: None,
            ramdisk: None,
            packages: vec![],
            pgo: None,
            pgo_resume: None,
            pgo_status: None,
            pgo_abort: None,
            pgo_keep_stage: false,
            pgo_restart: None,
            pgo_stage: None,
            pgo_once: false,
            pgo_goto: false,
            pgo_auto: false,
            kernel_build: None,
            ramdisk_shutdown: false,
            json: false,
            event_log: None,
            purge: false,
            yes: false,
            no_wait: false,
        }
    }

    fn config_with_ignore(
        global: bool,
        per_pkg: Option<bool>,
    ) -> crate::config::Config {
        let per_pkg_toml = match per_pkg {
            Some(true) => "\n[packages.firefox]\nignore_already_made_packages = true\n",
            Some(false) => "\n[packages.firefox]\nignore_already_made_packages = false\n",
            None => "\n[packages]\n",
        };
        let toml_content = format!(
            r#"
config_version = 1
manual_update_packages = []
skip_install_packages = []

[paths]
packages_path = "/tmp"
chroot_base_path = "/tmp"
ready_made_packages_path = "/tmp"

[build]
default_environment = "local"
ignore_already_made_packages = {global}

[system_update]
command_to_update_repositories = "pacman -Su"
command_to_perform_system_update = "pacman -Syu"
ignore_flag = "--ignore"
ignore_packages = []

[repositories]
default = "arch"
arch = "https://gitlab.archlinux.org/archlinux/packaging/packages"
{per_pkg_toml}
"#
        );
        toml::from_str(&toml_content).unwrap()
    }

    #[test]
    fn should_ignore_already_made_precedence() {
        // Default: respect artifacts (do not ignore).
        let config = config_with_ignore(false, None);
        assert!(!super::should_ignore_already_made(
            "firefox",
            &minimal_cli(false),
            &config
        ));

        // Global true.
        let config = config_with_ignore(true, None);
        assert!(super::should_ignore_already_made(
            "firefox",
            &minimal_cli(false),
            &config
        ));

        // Per-package false overrides global true.
        let config = config_with_ignore(true, Some(false));
        assert!(!super::should_ignore_already_made(
            "firefox",
            &minimal_cli(false),
            &config
        ));

        // Per-package true overrides global false.
        let config = config_with_ignore(false, Some(true));
        assert!(super::should_ignore_already_made(
            "firefox",
            &minimal_cli(false),
            &config
        ));

        // CLI -n wins over per-package false.
        let config = config_with_ignore(false, Some(false));
        assert!(super::should_ignore_already_made(
            "firefox",
            &minimal_cli(true),
            &config
        ));
    }
}
