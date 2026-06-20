use crate::config::{Config, PackageConfig, RamdiskConfig};
use crate::package_spec::PackageSpec;
use crate::utils::{
    append_deletable_roots, check_sudo_removal, init_deletable_roots, path_has_prefix,
    resolve_path_for_deletion, run_command, validate_config_path,
};
use crate::{blog, die, ewarn, is_dry_run_mode, vlog};
use colored::Colorize;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

#[cfg(unix)]
use std::os::unix::fs::symlink;

struct RamdiskSession {
    config: RamdiskConfig,
    mount_point: PathBuf,
    /// When true, `shutdown()` runs `umount` on `mount_point` (ABS mounted or reclaimed it this run).
    unmount_on_shutdown: bool,
}

static SESSION: OnceLock<Mutex<Option<RamdiskSession>>> = OnceLock::new();
static SHUTDOWN_DONE: AtomicBool = AtomicBool::new(false);
static EXIT_HANDLERS_INSTALLED: AtomicBool = AtomicBool::new(false);

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
    config.paths.packages_path.clone()
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
}

impl RamdiskTargets {
    #[allow(dead_code)]
    pub fn any(&self) -> bool {
        self.build_workdir || self.chroot || self.packages
    }
}

/// Parse `w` (workdir), `c` (chroot), `p` (packages). Separators are ignored (`wcp`, `w,c,p`, `w-c-p`).
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
            ',' | ' ' | '+' | '/' | '-' | '_' => {}
            _ => {
                return Err(format!(
                    "invalid ramdisk target '{ch}' in {:?} (use w=workdir, c=chroot, p=packages)",
                    code
                ));
            }
        }
    }
    if !code.trim().is_empty() && !saw {
        return Err(format!(
            "ramdisk targets {:?} contain no recognized letters (use w, c, p)",
            code
        ));
    }
    Ok(targets)
}

pub fn resolve_ramdisk_targets(
    config: &Config,
    pkg_config: Option<&PackageConfig>,
    spec: Option<&PackageSpec>,
) -> Result<RamdiskTargets, String> {
    if !config.ramdisk.enabled {
        return Ok(RamdiskTargets::default());
    }

    let mut targets = RamdiskTargets {
        build_workdir: config.ramdisk.build_workdir,
        chroot: config.ramdisk.chroot,
        packages: config.ramdisk.packages,
    };

    let override_code = spec
        .and_then(|s| s.ramdisk.as_deref())
        .or_else(|| pkg_config.and_then(|p| p.ramdisk.as_deref()));

    if let Some(code) = override_code {
        targets = parse_ramdisk_targets(code)?;
    }

    Ok(targets)
}

pub fn resolve_ramdisk_targets_for_pkg(config: &Config, pkg_name: &str) -> RamdiskTargets {
    let pkg_config = config.packages.get(pkg_name);
    resolve_ramdisk_targets(config, pkg_config, None).unwrap_or_default()
}

pub fn packages_path_for(
    config: &Config,
    pkg_name: &str,
    spec: Option<&PackageSpec>,
) -> Result<String, String> {
    let pkg_config = config.packages.get(pkg_name);
    let targets = resolve_ramdisk_targets(config, pkg_config, spec)?;
    Ok(effective_packages_path(config, &targets))
}

pub fn packages_path_for_pkg(config: &Config, pkg_name: &str) -> String {
    let targets = resolve_ramdisk_targets_for_pkg(config, pkg_name);
    effective_packages_path(config, &targets)
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
            let (uid, gid) = invoking_uid_gid();
            let path_str = path.to_string_lossy();
            if is_dry_run_mode() {
                println!("[DRY RUN] sudo mkdir -p {}", path_str);
                println!("[DRY RUN] sudo chown {}:{} {}", uid, gid, path_str);
                return Ok(());
            }
            run_command("sudo", &["mkdir", "-p", path_str.as_ref()], None::<&str>)?;
            run_command(
                "sudo",
                &["chown", &format!("{uid}:{gid}"), path_str.as_ref()],
                None::<&str>,
            )?;
            Ok(())
        }
        Err(e) => Err(format!("failed to create {}: {}", path.display(), e)),
    }
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
    if let Err(e) = ctrlc::set_handler(|| {
        eprintln!("==> WARNING: Interrupted; stopping builds and unmounting ramdisk...");
        crate::utils::terminate_foreground_children();
        crate::pkgbuild::restore_pending_pkgbuilds();
        shutdown();
        std::process::exit(130);
    }) {
        ewarn!("Failed to install ramdisk interrupt handler: {}", e);
    }
}

pub fn initialize(config: &Config) -> Result<(), String> {
    if !config.ramdisk.enabled {
        return Ok(());
    }

    let available = mem_available_mb()?;
    if available < config.ramdisk.min_free_ram_mb {
        die!(
            "Refusing to mount ramdisk: MemAvailable is {} MiB (min_free_ram_mb = {})",
            available, config.ramdisk.min_free_ram_mb
        );
    }

    if config.ramdisk.packages && config.ramdisk.warn_packages_ram {
        ewarn!(
            "[ramdisk] packages = true moves the entire packages_path to tmpfs. \
             This can use many gigabytes of RAM for large git trees and sources."
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

pub fn refresh_deletable_roots(config: &Config) -> Result<(), String> {
    let default_targets = resolve_ramdisk_targets_for_pkg(config, "");
    let chroot_base = effective_chroot_base_path(config, &default_targets);
    let packages_path = effective_packages_path(config, &default_targets);
    let mut extra = Vec::new();
    if let Some(session) = session_ref() {
        extra = ramdisk_mount_subdirs(&session.mount_point);
    }
    init_deletable_roots(
        &packages_path,
        &chroot_base,
        &config.paths.ready_made_packages_path,
        &extra,
    )
}

pub fn is_session_active() -> bool {
    session_ref().is_some()
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
    if chroot_base.join("base").join("root").is_dir() {
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
            &format!("{}/", seed_path.to_string_lossy()),
            &format!("{}/", chroot_base.to_string_lossy()),
        ],
        None::<&str>,
    )
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

pub fn shutdown() {
    if SHUTDOWN_DONE.swap(true, Ordering::SeqCst) {
        return;
    }

    crate::utils::terminate_foreground_children();

    let session = session_lock().lock().unwrap().take();
    let Some(session) = session else {
        return;
    };

    sync_chroot_to_seed(&session);
    unmount_tmpfs(&session.mount_point, session.unmount_on_shutdown);
}

fn migrate_dir_to_workdir(source: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir_all(target.parent().unwrap_or(target))
        .map_err(|e| format!("failed to create {}: {}", target.display(), e))?;

    if source.is_symlink() {
        fs::remove_file(source).map_err(|e| format!("failed to remove symlink {}: {}", source.display(), e))?;
    } else if source.is_dir() {
        if is_dry_run_mode() {
            println!(
                "[DRY RUN] cp -a {}/. {}/",
                source.display(),
                target.display()
            );
        } else {
            run_command(
                "cp",
                &[
                    "-a",
                    &format!("{}/.", source.to_string_lossy()),
                    &format!("{}/.", target.to_string_lossy()),
                ],
                None::<&str>,
            )?;
        }
        check_sudo_removal(source)?;
    } else if source.exists() {
        return Err(format!(
            "{} exists but is neither a directory nor a symlink",
            source.display()
        ));
    } else {
        fs::create_dir_all(target)
            .map_err(|e| format!("failed to create {}: {}", target.display(), e))?;
    }

    if is_dry_run_mode() {
        println!(
            "[DRY RUN] ln -s {} {}",
            target.display(),
            source.display()
        );
        return Ok(());
    }

    #[cfg(unix)]
    symlink(target, source).map_err(|e| {
        format!(
            "failed to symlink {} -> {}: {}",
            source.display(),
            target.display(),
            e
        )
    })?;
    Ok(())
}

pub struct WorkdirGuard {
    repo_dir: PathBuf,
    workdir: PathBuf,
}

impl WorkdirGuard {
    pub fn setup(
        config: &Config,
        repo_dir: &Path,
        targets: &RamdiskTargets,
    ) -> Result<Option<Self>, String> {
        let Some(session) = session_ref() else {
            return Ok(None);
        };
        if !targets.build_workdir {
            return Ok(None);
        }

        let packages_path = effective_packages_path(config, targets);
        let key = workdir_key(repo_dir, &packages_path);
        let workdir = session.mount_point.join("work").join(key);
        let src_target = workdir.join("src");
        let pkg_target = workdir.join("pkg");

        vlog!(
            "Ramdisk build workdir for {}: {}",
            repo_dir.display(),
            workdir.display()
        );

        migrate_dir_to_workdir(&repo_dir.join("src"), &src_target)?;
        migrate_dir_to_workdir(&repo_dir.join("pkg"), &pkg_target)?;

        Ok(Some(Self {
            repo_dir: repo_dir.to_path_buf(),
            workdir,
        }))
    }
}

impl Drop for WorkdirGuard {
    fn drop(&mut self) {
        for name in ["src", "pkg"] {
            let link = self.repo_dir.join(name);
            if link.is_symlink() {
                let _ = fs::remove_file(&link);
            }
        }
        if self.workdir.exists() {
            let _ = check_sudo_removal(&self.workdir);
        }
    }
}

pub fn remove_ramdisk_work(_config: &Config) {
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
    fn parse_ramdisk_targets_letters() {
        let t = parse_ramdisk_targets("wcp").unwrap();
        assert!(t.build_workdir && t.chroot && t.packages);
        let t = parse_ramdisk_targets("w").unwrap();
        assert!(t.build_workdir && !t.chroot && !t.packages);
        let t = parse_ramdisk_targets("w,c-p").unwrap();
        assert!(t.build_workdir && t.chroot && t.packages);
        assert!(parse_ramdisk_targets("").unwrap().build_workdir == false);
        assert!(parse_ramdisk_targets("xyz").is_err());
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

        let global = resolve_ramdisk_targets(&config, None, None).unwrap();
        assert!(!global.build_workdir && !global.chroot && !global.packages);

        let pkg = PackageConfig {
            ramdisk: Some("wcp".to_string()),
            ..Default::default()
        };
        let t = resolve_ramdisk_targets(&config, Some(&pkg), None).unwrap();
        assert!(t.build_workdir && t.chroot && t.packages);

        let spec = PackageSpec {
            ramdisk: Some("c".to_string()),
            ..PackageSpec::plain("mesa")
        };
        let t = resolve_ramdisk_targets(&config, Some(&pkg), Some(&spec)).unwrap();
        assert!(!t.build_workdir && t.chroot && !t.packages);
    }

    #[test]
    fn mount_point_validation_allows_run_subpath() {
        assert!(validate_config_path("ramdisk.mount_point", "/run/abs-ram").is_ok());
        assert!(validate_config_path("ramdisk.mount_point", "/run").is_err());
    }
}
