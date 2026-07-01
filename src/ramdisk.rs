use crate::config::{Config, PackageConfig, RamdiskConfig};
use crate::package_spec::PackageSpec;
use crate::utils::{
    append_deletable_roots, check_sudo_removal, path_has_prefix,
    resolve_path_for_deletion, run_command, validate_config_path,
};
use crate::{blog, die, ewarn, is_dry_run_mode, vlog};
use colored::Colorize;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

struct RamdiskSession {
    config: RamdiskConfig,
    mount_point: PathBuf,
    /// When true, `shutdown()` runs `umount` on `mount_point` (ABS mounted or reclaimed it this run).
    unmount_on_shutdown: bool,
}

static SESSION: OnceLock<Mutex<Option<RamdiskSession>>> = OnceLock::new();
static SHUTDOWN_DONE: AtomicBool = AtomicBool::new(false);
static EXIT_HANDLERS_INSTALLED: AtomicBool = AtomicBool::new(false);
static PENDING_WORKDIR_REMOVALS: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();

fn pending_workdir_removals() -> &'static Mutex<Vec<PathBuf>> {
    PENDING_WORKDIR_REMOVALS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Ramdisk work trees queued for removal at [`shutdown`] (after the exit pause).
pub fn pending_workdir_paths() -> Vec<PathBuf> {
    pending_workdir_removals().lock().unwrap().clone()
}

fn defer_workdir_removal(path: PathBuf) {
    if path.as_os_str().is_empty() {
        return;
    }
    pending_workdir_removals().lock().unwrap().push(path);
}

pub fn cleanup_pending_workdirs() {
    let paths: Vec<PathBuf> = pending_workdir_removals().lock().unwrap().drain(..).collect();
    for path in paths {
        if path.exists() {
            let _ = check_sudo_removal(&path);
        }
    }
}

/// True when `<chrootdir>/root` looks like a finished `mkarchroot base-devel` tree.
pub fn is_chroot_rootfs_complete(rootfs: &Path) -> bool {
    rootfs.join("etc/pacman.conf").is_file()
        && rootfs.join("usr/bin/pacman").is_file()
        && rootfs.join("var/lib/pacman/local").is_dir()
}

fn session_lock() -> &'static Mutex<Option<RamdiskSession>> {
    SESSION.get_or_init(|| Mutex::new(None))
}

fn session_ref() -> Option<RamdiskSessionSnapshot> {
    session_lock()
        .lock()
        .unwrap()
        .as_ref()
        .map(RamdiskSessionSnapshot::from_session)
}

#[derive(Clone)]
struct RamdiskSessionSnapshot {
    config: RamdiskConfig,
    mount_point: PathBuf,
}

impl RamdiskSessionSnapshot {
    fn from_session(session: &RamdiskSession) -> Self {
        Self {
            config: session.config.clone(),
            mount_point: session.mount_point.clone(),
        }
    }
}

pub fn effective_packages_path(config: &Config, targets: &RamdiskTargets) -> String {
    if targets.packages
        && let Some(session) = session_ref()
    {
        return session
            .mount_point
            .join("packages")
            .to_string_lossy()
            .into_owned();
    }
    disk_packages_path(config)
}

/// On-disk packages path (never tmpfs). Source tarballs and git repos live here unless `p` is set.
pub fn disk_packages_path(config: &Config) -> String {
    config.paths.packages_path.clone()
}

/// Where git clones / PKGBUILD / makepkg source tarballs are stored for this build.
/// When only compilation (`w`) or profiles (`r`) use the ramdisk, downloads stay on disk.
pub fn download_packages_path(config: &Config, targets: &RamdiskTargets) -> String {
    effective_packages_path(config, targets)
}

/// Persistent makepkg source tarball cache beside the repo (always on disk).
pub fn srcdest_for_repo(repo_dir: &Path) -> PathBuf {
    repo_dir.join(".makepkg-src")
}

pub fn effective_chroot_base_path(config: &Config, targets: &RamdiskTargets) -> String {
    if targets.chroot
        && let Some(session) = session_ref()
    {
        return session
            .mount_point
            .join("chroot")
            .to_string_lossy()
            .into_owned();
    }
    config.paths.chroot_base_path.clone()
}

/// Which ramdisk areas apply for the current package build.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RamdiskTargets {
    pub build_workdir: bool,
    pub chroot: bool,
    pub packages: bool,
    /// PGO profile collection scratch (perf data) on tmpfs before archiving.
    pub profiles: bool,
}

impl RamdiskTargets {
    pub fn any(&self) -> bool {
        self.build_workdir || self.chroot || self.packages || self.profiles
    }
}

/// Per-package / CLI override that disables all ramdisk targets (disk-only build).
pub const RAMDISK_DISABLED: &str = "disabled";

/// True for `disabled` (case-insensitive); also accepts empty string (legacy abs.toml).
pub fn is_ramdisk_disabled(code: &str) -> bool {
    code.trim().is_empty() || code.trim().eq_ignore_ascii_case(RAMDISK_DISABLED)
}

/// Canonical ramdisk code for abs.toml / CLI (e.g. `"wr"`, `"wcp"`).
pub fn format_ramdisk_targets(targets: &RamdiskTargets) -> String {
    let mut s = String::new();
    if targets.build_workdir {
        s.push('w');
    }
    if targets.chroot {
        s.push('c');
    }
    if targets.packages {
        s.push('p');
    }
    if targets.profiles {
        s.push('r');
    }
    s
}

/// Parse `w` (workdir), `c` (chroot), `p` (packages), `r` (PGO profile scratch).
/// Separators are ignored (`wcp`, `w,c,p`, `w-c-p`).
pub fn parse_ramdisk_targets(code: &str) -> Result<RamdiskTargets, String> {
    let mut targets = RamdiskTargets::default();
    let mut saw = false;
    for ch in code.chars() {
        match ch.to_ascii_lowercase() {
            'w' => {
                targets.build_workdir = true;
                saw = true;
            }
            'c' => {
                targets.chroot = true;
                saw = true;
            }
            'p' => {
                targets.packages = true;
                saw = true;
            }
            'r' => {
                targets.profiles = true;
                saw = true;
            }
            ',' | ' ' | '+' | '/' | '-' | '_' => {}
            _ => {
                return Err(format!(
                    "invalid ramdisk target '{ch}' in {:?} (use w=workdir, c=chroot, p=packages, r=profiles)",
                    code
                ));
            }
        }
    }
    if !code.trim().is_empty() && !saw {
        return Err(format!(
            "ramdisk targets {:?} contain no recognized letters (use w, c, p, r)",
            code
        ));
    }
    Ok(targets)
}

pub fn resolve_ramdisk_targets(
    config: &Config,
    pkg_config: Option<&PackageConfig>,
    spec: Option<&PackageSpec>,
    cli_ramdisk: Option<&str>,
) -> Result<RamdiskTargets, String> {
    let override_code = spec
        .and_then(|s| s.ramdisk.as_deref())
        .or(cli_ramdisk)
        .or_else(|| pkg_config.and_then(|p| p.ramdisk.as_deref()));

    // Per-package / CLI ramdisk= (e.g. kernel PGO `wr`) applies even when [ramdisk].enabled = false.
    if let Some(code) = override_code {
        if is_ramdisk_disabled(code) {
            return Ok(RamdiskTargets::default());
        }
        return parse_ramdisk_targets(code);
    }

    if !config.ramdisk.enabled {
        return Ok(RamdiskTargets::default());
    }

    Ok(RamdiskTargets {
        build_workdir: config.ramdisk.build_workdir,
        chroot: config.ramdisk.chroot,
        packages: config.ramdisk.packages,
        profiles: false,
    })
}

pub fn resolve_ramdisk_targets_for_pkg(config: &Config, pkg_name: &str) -> RamdiskTargets {
    let pkg_config = config.packages.get(pkg_name);
    resolve_ramdisk_targets(config, pkg_config, None, None).unwrap_or_default()
}

pub fn packages_path_for_pkg(config: &Config, pkg_name: &str) -> String {
    let targets = resolve_ramdisk_targets_for_pkg(config, pkg_name);
    download_packages_path(config, &targets)
}

pub fn warn_if_packages_on_ram(config: &Config, pkg_name: &str, targets: &RamdiskTargets) {
    if targets.packages && config.ramdisk.warn_packages_ram {
        ewarn!(
            "[ramdisk] packages (p) enabled for {}; entire packages_path uses tmpfs for this build",
            pkg_name
        );
    }
}

fn chroot_seed_source(targets: &RamdiskTargets) -> Option<String> {
    if !targets.chroot {
        return None;
    }
    let session = session_ref()?;
    session.config.seed_chroot_from.clone()
}

fn is_mount_point(path: &Path) -> bool {
    let Ok(file) = fs::File::open("/proc/mounts") else {
        return false;
    };
    let target = path.to_string_lossy();
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .any(|line| {
            let mut parts = line.split_whitespace();
            let mount_path = parts.nth(1).unwrap_or("");
            mount_path == target
        })
}

pub fn mem_available_mb() -> Result<u64, String> {
    let content = fs::read_to_string("/proc/meminfo")
        .map_err(|e| format!("failed to read /proc/meminfo: {}", e))?;
    for line in content.lines() {
        if let Some(kb) = line.strip_prefix("MemAvailable:") {
            let kb = kb
                .trim()
                .strip_suffix(" kB")
                .unwrap_or(kb.trim())
                .trim()
                .parse::<u64>()
                .map_err(|e| format!("failed to parse MemAvailable: {}", e))?;
            return Ok(kb / 1024);
        }
    }
    Err("MemAvailable not found in /proc/meminfo".into())
}

pub fn workdir_key(repo_dir: &Path, packages_path: &str) -> String {
    let repo_abs = resolve_path_for_deletion(repo_dir).unwrap_or_else(|_| repo_dir.to_path_buf());
    let base_abs =
        resolve_path_for_deletion(Path::new(packages_path)).unwrap_or_else(|_| PathBuf::from(packages_path));

    if path_has_prefix(&base_abs, &repo_abs) {
        let rel = repo_abs
            .strip_prefix(&base_abs)
            .unwrap_or(&repo_abs);
        let key = rel
            .components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().replace(['/', '\\'], "_")),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("__");
        if !key.is_empty() {
            return key;
        }
    }

    let fallback = repo_abs
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().replace(['/', '\\'], "_")),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("__");
    if fallback.is_empty() {
        "work".to_string()
    } else {
        fallback
    }
}

fn invoking_uid_gid() -> (u32, u32) {
    if let (Ok(uid), Ok(gid)) = (std::env::var("SUDO_UID"), std::env::var("SUDO_GID"))
        && let (Ok(u), Ok(g)) = (uid.parse::<u32>(), gid.parse::<u32>())
    {
        return (u, g);
    }
    unsafe { (libc::getuid(), libc::getgid()) }
}

fn tmpfs_mount_options(config: &RamdiskConfig) -> String {
    let (uid, gid) = invoking_uid_gid();
    format!(
        "size={},mode={},uid={},gid={}",
        config.size, config.mode, uid, gid
    )
}

fn ensure_mount_point_dir(mount_point: &Path) -> Result<(), String> {
    if mount_point.is_dir() {
        return Ok(());
    }
    if is_dry_run_mode() {
        println!(
            "[DRY RUN] sudo mkdir -p {}",
            mount_point.display()
        );
        return Ok(());
    }
    run_command(
        "sudo",
        &["mkdir", "-p", mount_point.to_string_lossy().as_ref()],
        None::<&str>,
    )
}

fn ensure_ramdisk_subdir(path: &Path) -> Result<(), String> {
    match fs::create_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            ensure_ramdisk_path(path)
        }
        Err(e) => Err(format!("failed to create {}: {}", path.display(), e)),
    }
}

/// Create a directory under the active ramdisk mount with correct ownership.
/// Uses sudo because stale mounts or prior runs may leave `work/` root-owned.
fn ensure_ramdisk_path(path: &Path) -> Result<(), String> {
    if is_dry_run_mode() {
        println!("[DRY RUN] sudo mkdir -p {}", path.display());
        println!(
            "[DRY RUN] sudo chown -R {}:{} {}",
            invoking_uid_gid().0,
            invoking_uid_gid().1,
            path.display()
        );
        return Ok(());
    }
    let (uid, gid) = invoking_uid_gid();
    let path_str = path.to_string_lossy();
    run_command("sudo", &["mkdir", "-p", path_str.as_ref()], None::<&str>)?;
    run_command(
        "sudo",
        &["chown", "-R", &format!("{uid}:{gid}"), path_str.as_ref()],
        None::<&str>,
    )?;
    if !path.is_dir() {
        return Err(format!(
            "ramdisk directory {} was not created (is tmpfs mounted and writable?)",
            path.display()
        ));
    }
    Ok(())
}

/// Large PGO profiling artifacts live on disk (or pgo-scratch), not in the ramdisk workdir.
const PGO_REPO_RSYNC_EXCLUDES: &[&str] = &[
    ".git/",
    "kernel.data",
    "kernel.data.old",
    "propeller.data",
    "propeller.data.old",
];

fn repair_root_owned_pgo_artifacts(repo: &Path) {
    for name in PGO_REPO_RSYNC_EXCLUDES {
        if *name == ".git/" {
            continue;
        }
        let path = repo.join(name);
        if path.is_file() {
            let _ = crate::utils::ensure_build_user_can_read(&path);
        }
    }
}

fn sync_package_tree_to_ramdisk(
    disk_repo: &Path,
    ram_repo: &Path,
    clear_pkg: bool,
) -> Result<(), String> {
    blog!(
        "Ramdisk (w): syncing package tree {} → {}",
        disk_repo.display(),
        ram_repo.display()
    );
    repair_root_owned_pgo_artifacts(disk_repo);
    ensure_ramdisk_path(ram_repo)?;
    if clear_pkg {
        let pkg = ram_repo.join("pkg");
        if pkg.exists() {
            check_sudo_removal(&pkg)?;
        }
    }
    if is_dry_run_mode() {
        println!(
            "[DRY RUN] rsync -a --delete --exclude .git/ {}/ {}/",
            disk_repo.display(),
            ram_repo.display()
        );
        return Ok(());
    }
    let src = format!("{}/", disk_repo.to_string_lossy());
    let dst = format!("{}/", ram_repo.to_string_lossy());
    let mut rsync_args: Vec<String> = vec!["-a".into(), "--delete".into()];
    for exclude in PGO_REPO_RSYNC_EXCLUDES {
        rsync_args.push("--exclude".into());
        rsync_args.push((*exclude).into());
    }
    rsync_args.push(src);
    rsync_args.push(dst);
    let refs: Vec<&str> = rsync_args.iter().map(String::as_str).collect();
    run_command("rsync", &refs, None::<&str>)
}

fn ensure_mount_tree(mount_point: &Path) -> Result<(), String> {
    for sub in ["work", "chroot", "packages"] {
        ensure_ramdisk_subdir(&mount_point.join(sub))?;
    }
    Ok(())
}

fn unmount_mount_point(mount_point: &Path) -> bool {
    if !is_mount_point(mount_point) {
        return true;
    }
    let path = mount_point.to_string_lossy();
    if is_dry_run_mode() {
        println!("[DRY RUN] sudo umount {path}");
        return true;
    }
    if run_command("sudo", &["umount", path.as_ref()], None::<&str>).is_ok() {
        return true;
    }
    vlog!("Mount {} is busy; trying lazy unmount (umount -l)...", mount_point.display());
    run_command("sudo", &["umount", "-l", path.as_ref()], None::<&str>).is_ok()
}

fn force_unmount(mount_point: &Path) -> Result<(), String> {
    if unmount_mount_point(mount_point) {
        Ok(())
    } else {
        Err(format!(
            "failed to unmount {} (still busy after lazy unmount)",
            mount_point.display()
        ))
    }
}

fn mount_tmpfs(mount_point: &Path, config: &RamdiskConfig) -> Result<bool, String> {
    if is_mount_point(mount_point) {
        if config.reclaim_mount_on_startup {
            let cached_chroot = mount_point.join("chroot/base/root");
            if is_chroot_rootfs_complete(&cached_chroot) {
                blog!(
                    "Reusing ramdisk at {} (cached chroot; run `abs --ramdisk-shutdown` to clear)",
                    mount_point.display()
                );
                ensure_mount_tree(mount_point)?;
                return Ok(false);
            }
            vlog!(
                "Reclaiming existing tmpfs mount at {} before mounting a fresh ramdisk...",
                mount_point.display()
            );
            force_unmount(mount_point)?;
        } else {
            vlog!(
                "Ramdisk mount point {} is already mounted; reusing without unmount",
                mount_point.display()
            );
            ensure_mount_tree(mount_point)?;
            return Ok(false);
        }
    }

    if is_dry_run_mode() {
        let opts = tmpfs_mount_options(config);
        println!(
            "[DRY RUN] sudo mount -t tmpfs -o {} tmpfs {}",
            opts,
            mount_point.display()
        );
        ensure_mount_point_dir(mount_point)?;
        ensure_mount_tree(mount_point)?;
        return Ok(true);
    }

    ensure_mount_point_dir(mount_point)?;

    run_command(
        "sudo",
        &[
            "mount",
            "-t",
            "tmpfs",
            "-o",
            &tmpfs_mount_options(config),
            "tmpfs",
            &mount_point.to_string_lossy(),
        ],
        None::<&str>,
    )?;
    ensure_mount_tree(mount_point)?;
    Ok(true)
}

fn unmount_tmpfs(mount_point: &Path, unmount_on_shutdown: bool) {
    if !unmount_on_shutdown {
        return;
    }
    if unmount_mount_point(mount_point) {
        vlog!("Unmounted ramdisk at {}", mount_point.display());
    } else {
        ewarn!(
            "Failed to unmount ramdisk at {} (still busy). Try: sudo umount -l {}",
            mount_point.display(),
            mount_point.display()
        );
    }
}

fn ramdisk_mount_subdirs(mount_point: &Path) -> Vec<PathBuf> {
    ["work", "chroot", "packages"]
        .iter()
        .map(|sub| mount_point.join(sub))
        .collect()
}

/// Install Ctrl+C / SIGTERM handlers that unmount the ramdisk before exiting.
pub fn install_exit_handlers() {
    if EXIT_HANDLERS_INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let on_interrupt = || {
        eprintln!(
            "==> WARNING: Interrupted (abs {}); stopping builds (ramdisk kept mounted for retry)...",
            env!("CARGO_PKG_VERSION")
        );
        crate::utils::terminate_foreground_children();
        if let Some(session) = session_ref() {
            crate::utils::kill_processes_with_cwd_under(&session.mount_point, "ramdisk");
        }
        crate::pkgbuild::restore_pending_pkgbuilds();
        crate::utils::restore_terminal();
        shutdown_on_interrupt();
        std::process::exit(130);
    };
    if let Err(e) = ctrlc::set_handler(on_interrupt) {
        ewarn!("Failed to install ramdisk interrupt handler: {}", e);
    }
    unsafe {
        extern "C" fn on_sigterm(_: libc::c_int) {
            eprintln!(
                "==> WARNING: Terminated (abs {}); stopping builds (ramdisk kept mounted for retry)...",
                env!("CARGO_PKG_VERSION")
            );
            crate::utils::terminate_foreground_children();
            if let Some(session) = session_ref() {
                crate::utils::kill_processes_with_cwd_under(&session.mount_point, "ramdisk");
            }
            crate::pkgbuild::restore_pending_pkgbuilds();
            crate::utils::restore_terminal();
            shutdown_on_interrupt();
            unsafe {
                libc::_exit(143);
            }
        }
        libc::signal(libc::SIGTERM, on_sigterm as *const () as libc::sighandler_t);
    }
}

/// Mount tmpfs when a package task needs ramdisk targets (`w`/`c`/`p`/`r`). No-op when no target
/// flags apply for this package, or a session is already active. Per-package `ramdisk = "wr"`
/// mounts tmpfs even if `[ramdisk].enabled = false` (global `enabled` only gates default targets).
pub fn ensure_for_targets(config: &Config, targets: &RamdiskTargets) -> Result<(), String> {
    if !targets.any() {
        return Ok(());
    }
    if session_ref().is_some() {
        return Ok(());
    }

    if targets.packages && config.ramdisk.warn_packages_ram {
        ewarn!(
            "[ramdisk] packages (p) enabled for this build; packages_path uses tmpfs \
             (high RAM use for large git trees)"
        );
    }

    mount_session(config)
}

fn mount_session(config: &Config) -> Result<(), String> {
    let available = mem_available_mb()?;
    if available < config.ramdisk.min_free_ram_mb {
        die!(
            "Refusing to mount ramdisk: MemAvailable is {} MiB (min_free_ram_mb = {})",
            available, config.ramdisk.min_free_ram_mb
        );
    }

    let mount_point = PathBuf::from(&config.ramdisk.mount_point);
    let unmount_on_shutdown = mount_tmpfs(&mount_point, &config.ramdisk)?;

    if unmount_on_shutdown {
        blog!(
            "Mounted tmpfs ramdisk at {} (size={})",
            mount_point.display(),
            config.ramdisk.size
        );
    } else {
        blog!(
            "Using existing tmpfs ramdisk at {} (reclaim_mount_on_startup = false)",
            mount_point.display()
        );
    }

    install_exit_handlers();

    let extra = ramdisk_mount_subdirs(&mount_point);
    append_deletable_roots(&extra)?;

    let session = RamdiskSession {
        config: config.ramdisk.clone(),
        mount_point,
        unmount_on_shutdown,
    };

    *session_lock().lock().unwrap() = Some(session);
    Ok(())
}

pub fn is_session_active() -> bool {
    session_ref().is_some()
}

/// Tmpfs mount point when a ramdisk session is active.
pub fn session_mount_point() -> Option<PathBuf> {
    session_ref().map(|s| s.mount_point)
}

/// Path for PGO perf/profile scratch on the active ramdisk session.
pub fn pgo_scratch_path(mount: &Path, package: &str) -> PathBuf {
    mount.join("pgo-scratch").join(package)
}

/// Mount tmpfs (when needed) and create `pgo-scratch/<package>` for PGO profiling.
pub fn ensure_pgo_scratch_dir(
    config: &Config,
    package: &str,
    targets: &RamdiskTargets,
) -> Result<Option<PathBuf>, String> {
    if !targets.profiles && !targets.build_workdir {
        return Ok(None);
    }
    ensure_for_targets(config, targets)?;
    let Some(mount) = session_mount_point() else {
        return Ok(None);
    };
    let scratch = pgo_scratch_path(&mount, package);
    ensure_ramdisk_path(&scratch)?;
    Ok(Some(scratch))
}

pub fn seed_chroot_if_needed(
    _config: &Config,
    chroot_base: &Path,
    targets: &RamdiskTargets,
) -> Result<(), String> {
    let Some(seed) = chroot_seed_source(targets) else {
        return Ok(());
    };
    let seed_path = Path::new(&seed);
    if !seed_path.is_dir() {
        vlog!(
            "Chroot seed path {} does not exist; skipping seed",
            seed_path.display()
        );
        return Ok(());
    }

    let chroot_base = chroot_base.to_path_buf();
    let rootfs = chroot_base.join("base").join("root");
    if is_chroot_rootfs_complete(&rootfs) {
        vlog!(
            "Ramdisk chroot already present at {}; skipping seed copy",
            chroot_base.display()
        );
        return Ok(());
    }

    blog!(
        "Seeding ramdisk chroot from {} -> {} (full copy; omit seed_chroot_from for a fresh mkarchroot instead)...",
        seed_path.display(),
        chroot_base.display()
    );

    if is_dry_run_mode() {
        println!(
            "[DRY RUN] rsync -a {}/ {}/",
            seed_path.display(),
            chroot_base.display()
        );
        return Ok(());
    }

    fs::create_dir_all(&chroot_base)
        .map_err(|e| format!("failed to create {}: {}", chroot_base.display(), e))?;

    run_command(
        "rsync",
        &[
            "-a",
            "--info=progress2",
            &format!("{}/", seed_path.to_string_lossy()),
            &format!("{}/", chroot_base.to_string_lossy()),
        ],
        None::<&str>,
    )?;
    blog!("Chroot seed copy finished.");
    Ok(())
}

fn sync_chroot_to_seed(session: &RamdiskSession) {
    if !session.config.sync_chroot_on_exit || !session.config.chroot {
        return;
    }
    let Some(seed) = session.config.seed_chroot_from.clone() else {
        vlog!("sync_chroot_on_exit is set but seed_chroot_from is unset; skipping chroot sync");
        return;
    };

    let ram_chroot = session.mount_point.join("chroot");
    if !ram_chroot.is_dir() {
        return;
    }

    blog!("Syncing ramdisk chroot back to {}...", seed);

    if is_dry_run_mode() {
        println!(
            "[DRY RUN] rsync -a {}/ {}/",
            ram_chroot.display(),
            seed
        );
        return;
    }

    if let Err(e) = validate_config_path("ramdisk.seed_chroot_from", seed.as_str()) {
        ewarn!("Skipping chroot sync: {}", e);
        return;
    }

    if let Err(e) = fs::create_dir_all(&seed) {
        ewarn!("Failed to create chroot seed directory {}: {}", seed, e);
        return;
    }

    if let Err(e) = run_command(
        "rsync",
        &[
            "-a",
            "--delete",
            &format!("{}/", ram_chroot.to_string_lossy()),
            &format!("{}/", seed),
        ],
        None::<&str>,
    ) {
        ewarn!("Failed to sync ramdisk chroot to seed: {}", e);
    }
}

pub fn shutdown_on_interrupt() {
    if SHUTDOWN_DONE.swap(true, Ordering::SeqCst) {
        return;
    }

    crate::utils::terminate_foreground_children();

    let session = session_lock().lock().unwrap().take();
    let Some(session) = session else {
        return;
    };

    sync_chroot_to_seed(&session);
    if session.unmount_on_shutdown {
        crate::utils::phase_banner(format!(
            "Ramdisk left mounted at {} — retry the build to skip mkarchroot; run `abs --ramdisk-shutdown` to unmount",
            session.mount_point.display()
        ));
    }
}

pub fn shutdown() {
    if SHUTDOWN_DONE.swap(true, Ordering::SeqCst) {
        return;
    }

    crate::utils::terminate_foreground_children();

    let session = session_lock().lock().unwrap().take();
    let Some(session) = session else {
        cleanup_pending_workdirs();
        return;
    };

    sync_chroot_to_seed(&session);
    cleanup_pending_workdirs();
    unmount_tmpfs(&session.mount_point, session.unmount_on_shutdown);
}

/// Unmount the configured ramdisk when the abs process no longer has session state
/// (e.g. GUI abort killed abs, or user ran `--ramdisk-shutdown`).
pub fn force_unmount_configured(config: &Config) -> Result<(), String> {
    let mount_point = PathBuf::from(config.ramdisk.mount_point.trim());
    if mount_point.as_os_str().is_empty() {
        return Ok(());
    }
    if !is_mount_point(&mount_point) {
        vlog!(
            "Ramdisk mount point {} is not mounted; nothing to unmount",
            mount_point.display()
        );
        return Ok(());
    }

    if config.ramdisk.sync_chroot_on_exit && config.ramdisk.chroot {
        let session = RamdiskSession {
            config: config.ramdisk.clone(),
            mount_point: mount_point.clone(),
            unmount_on_shutdown: true,
        };
        sync_chroot_to_seed(&session);
    }

    blog!("Unmounting ramdisk at {}...", mount_point.display());
    cleanup_pending_workdirs();
    force_unmount(&mount_point)
}

pub struct WorkdirGuard {
    disk_repo: PathBuf,
    ram_repo: Option<PathBuf>,
}

impl WorkdirGuard {
    /// Directory where makepkg should run (ramdisk copy when `w` is active).
    pub fn build_dir(&self) -> &Path {
        self.ram_repo
            .as_deref()
            .unwrap_or(self.disk_repo.as_path())
    }

    pub fn uses_ramdisk(&self) -> bool {
        self.ram_repo.is_some()
    }

    pub fn setup(
        config: &Config,
        disk_repo: &Path,
        targets: &RamdiskTargets,
        clear_pkg: bool,
    ) -> Result<Option<Self>, String> {
        if !targets.build_workdir {
            return Ok(None);
        }
        let Some(session) = session_ref() else {
            return Ok(None);
        };

        let packages_path = download_packages_path(config, targets);
        let key = workdir_key(disk_repo, &packages_path);
        let ram_repo = session.mount_point.join("work").join(key);

        sync_package_tree_to_ramdisk(disk_repo, &ram_repo, clear_pkg)?;

        Ok(Some(Self {
            disk_repo: disk_repo.to_path_buf(),
            ram_repo: Some(ram_repo),
        }))
    }
}

impl Drop for WorkdirGuard {
    fn drop(&mut self) {
        if let Some(ram) = self.ram_repo.take()
            && ram.exists()
        {
            // Keep the tree until after the interactive exit pause (see `shutdown`).
            defer_workdir_removal(ram);
        }
    }
}

pub fn remove_ramdisk_work(_config: &Config) {
    cleanup_pending_workdirs();
    if let Some(session) = session_ref() {
        let work = session.mount_point.join("work");
        if work.exists() {
            let _ = check_sudo_removal(&work);
        }
    }
}

pub fn full_clean_packages_target(config: &Config) -> PathBuf {
    if let Some(session) = session_ref() {
        return session.mount_point.join("packages");
    }
    PathBuf::from(&config.paths.packages_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workdir_key_uses_relative_path() {
        let base = PathBuf::from("/data/abs/packages");
        let repo = base.join("aur").join("xray");
        let key = workdir_key(&repo, base.to_str().unwrap());
        assert_eq!(key, "aur__xray");
    }

    #[test]
    fn workdir_key_sanitizes_slashes() {
        let key = workdir_key(
            Path::new("/data/abs/packages/cachyos/mesa"),
            "/data/abs/packages",
        );
        assert_eq!(key, "cachyos__mesa");
    }

    #[test]
    fn pgo_scratch_path_under_mount() {
        let p = pgo_scratch_path(Path::new("/run/abs-ram"), "linux-cachyos");
        assert_eq!(p, PathBuf::from("/run/abs-ram/pgo-scratch/linux-cachyos"));
    }

    #[test]
    fn pending_workdir_paths_tracks_deferred_removals() {
        let dir = std::env::temp_dir().join(format!(
            "abs-pending-workdir-{}",
            std::process::id()
        ));
        let _ = fs::create_dir_all(&dir);
        defer_workdir_removal(dir.clone());
        assert_eq!(pending_workdir_paths(), vec![dir.clone()]);
        cleanup_pending_workdirs();
        assert!(pending_workdir_paths().is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn parse_ramdisk_targets_letters() {
        let t = parse_ramdisk_targets("wcp").unwrap();
        assert!(t.build_workdir && t.chroot && t.packages && !t.profiles);
        let t = parse_ramdisk_targets("wr").unwrap();
        assert!(t.build_workdir && t.profiles && !t.chroot && !t.packages);
        assert_eq!(format_ramdisk_targets(&t), "wr");
        let t = parse_ramdisk_targets("w").unwrap();
        assert!(t.build_workdir && !t.chroot && !t.packages);
        let t = parse_ramdisk_targets("w,c-p").unwrap();
        assert!(t.build_workdir && t.chroot && t.packages);
        assert!(!parse_ramdisk_targets("").unwrap().build_workdir);
        assert!(parse_ramdisk_targets("xyz").is_err());
        assert!(parse_ramdisk_targets("disabled").is_err());
    }

    #[test]
    fn is_ramdisk_disabled_values() {
        assert!(is_ramdisk_disabled(""));
        assert!(is_ramdisk_disabled("disabled"));
        assert!(is_ramdisk_disabled("DISABLED"));
        assert!(!is_ramdisk_disabled("w"));
    }

    #[test]
    fn resolve_ramdisk_disabled_overrides_global() {
        use crate::config::Config;

        let toml_content = r#"
config_version = 1
manual_update_packages = []
skip_install_packages = []

[paths]
packages_path = "/tmp/p"
chroot_base_path = "/tmp/c"
ready_made_packages_path = "/tmp/r"

[build]
default_environment = "local"

[system_update]
command_to_update_repositories = "pacman -Sy"
command_to_perform_system_update = "pacman -Syu"
ignore_flag = "--ignore"
ignore_packages = []

[repositories]
default = "arch"
arch = "https://gitlab.archlinux.org/archlinux/packaging/packages"

[ramdisk]
enabled = true
build_workdir = true
chroot = true
packages = true

[packages]
"#;
        let config: Config = toml::from_str(toml_content).unwrap();
        let t = resolve_ramdisk_targets(&config, None, None, Some("disabled")).unwrap();
        assert!(!t.any());
        let t = resolve_ramdisk_targets(&config, None, None, Some("wcp")).unwrap();
        assert!(t.build_workdir && t.chroot && t.packages);
    }

    #[test]
    fn resolve_ramdisk_targets_override() {
        use crate::config::{Config, PackageConfig};
        use crate::package_spec::PackageSpec;

        let toml_content = r#"
config_version = 1
manual_update_packages = []
skip_install_packages = []

[paths]
packages_path = "/tmp"
chroot_base_path = "/tmp"
ready_made_packages_path = "/tmp"

[build]
default_environment = "local"

[system_update]
command_to_update_repositories = "pacman -Sy"
command_to_perform_system_update = "pacman -Syu"
ignore_flag = "--ignore"
ignore_packages = []

[repositories]
default = "arch"
arch = "https://gitlab.archlinux.org/archlinux/packaging/packages"

[ramdisk]
enabled = true
build_workdir = false
chroot = false
packages = false

[packages]
"#;
        let config: Config = toml::from_str(toml_content).unwrap();

        let global = resolve_ramdisk_targets(&config, None, None, None).unwrap();
        assert!(!global.build_workdir && !global.chroot && !global.packages);

        let pkg = PackageConfig {
            ramdisk: Some("wcp".to_string()),
            ..Default::default()
        };
        let t = resolve_ramdisk_targets(&config, Some(&pkg), None, None).unwrap();
        assert!(t.build_workdir && t.chroot && t.packages);

        let t = resolve_ramdisk_targets(&config, Some(&pkg), None, Some("w")).unwrap();
        assert!(t.build_workdir && !t.chroot && !t.packages);

        let spec = PackageSpec {
            ramdisk: Some("c".to_string()),
            ..PackageSpec::plain("mesa")
        };
        let t = resolve_ramdisk_targets(&config, Some(&pkg), Some(&spec), Some("w")).unwrap();
        assert!(!t.build_workdir && t.chroot && !t.packages);
    }

    #[test]
    fn resolve_ramdisk_per_package_wr_without_global_enabled() {
        use crate::config::Config;

        let toml_content = r#"
config_version = 1
manual_update_packages = []
skip_install_packages = []

[paths]
packages_path = "/tmp/p"
chroot_base_path = "/tmp/c"
ready_made_packages_path = "/tmp/r"

[build]
default_environment = "local"

[system_update]
command_to_update_repositories = "pacman -Sy"
command_to_perform_system_update = "pacman -Syu"
ignore_flag = "--ignore"
ignore_packages = []

[repositories]
default = "arch"
arch = "https://gitlab.archlinux.org/archlinux/packaging/packages"

[ramdisk]
enabled = false

[packages.linux-cachyos]
ramdisk = "wr"
"#;
        let config: Config = toml::from_str(toml_content).unwrap();
        let pkg = config.packages.get("linux-cachyos").unwrap();
        let t = resolve_ramdisk_targets(&config, Some(pkg), None, None).unwrap();
        assert!(t.build_workdir && t.profiles && !t.packages && !t.chroot);
    }

    #[test]
    fn ensure_for_targets_noop_without_target_flags() {
        use crate::config::Config;

        let toml_content = r#"
config_version = 1
manual_update_packages = []
skip_install_packages = []

[paths]
packages_path = "/tmp/p"
chroot_base_path = "/tmp/c"
ready_made_packages_path = "/tmp/r"

[build]
default_environment = "local"

[system_update]
command_to_update_repositories = "pacman -Sy"
command_to_perform_system_update = "pacman -Syu"
ignore_flag = "--ignore"
ignore_packages = []

[repositories]
default = "arch"
arch = "https://gitlab.archlinux.org/archlinux/packaging/packages"

[ramdisk]
enabled = true
build_workdir = false
chroot = false
packages = false

[packages]
"#;
        let config: Config = toml::from_str(toml_content).unwrap();
        let targets = RamdiskTargets::default();
        ensure_for_targets(&config, &targets).unwrap();
        assert!(!is_session_active());
    }

    #[test]
    fn mount_point_validation_allows_run_subpath() {
        assert!(validate_config_path("ramdisk.mount_point", "/run/abs-ram").is_ok());
        assert!(validate_config_path("ramdisk.mount_point", "/run").is_err());
    }
}
