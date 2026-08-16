//! Native file/folder pickers.
//!
//! On Linux the XDG FileChooser portal is used when a desktop implements it
//! (GNOME, KDE, COSMIC, GTK portal, LXQt, …). Sessions without that interface
//! (wlroots-only, missing portal package) fall back to a DE-ordered CLI chain:
//! kdialog, zenity, matedialog, yad, qarma. User cancel stops the chain so a
//! second dialog is not opened. Override with
//! `ABS_FILE_DIALOG=portal|kdialog|zenity|matedialog|yad|qarma`.

use crate::log_save::{dialog_directory_hint, ExpandCtx, LogSaveFormat, LogSaveTarget};
use crate::messages::{PathField, PathKind};
use crate::system_theme::{desktop_has, desktop_tokens};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn pick_path(field: PathField, kind: PathKind, current: &str) -> Option<String> {
    let title = match kind {
        PathKind::Folder => folder_dialog_title(field),
        PathKind::File => file_dialog_title(field),
    };
    pick(Request {
        title,
        start: start_dir(
            expand_for_dialog(current).as_deref(),
            kind == PathKind::File,
        ),
        file_name: None,
        filters: Vec::new(),
        kind,
        save: false,
    })
}

/// Native save-file dialog. `suggested` is an already-expanded full file path.
pub fn save_file(title: &str, suggested: &str, format: LogSaveFormat) -> Option<String> {
    let path = PathBuf::from(suggested.trim());
    let default_name = format!("log.{}", format.ext());
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&default_name)
        .to_string();
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_string_lossy().into_owned());
    pick(Request {
        title,
        start: start_dir(parent.as_deref(), true),
        file_name: Some(file_name),
        filters: log_save_filters(format),
        kind: PathKind::File,
        save: true,
    })
}

pub fn pick_log_folder(target: LogSaveTarget, current: &str) -> Option<String> {
    let ctx = ExpandCtx::now(target, LogSaveFormat::Txt);
    let dir = dialog_directory_hint(current, &ctx);
    let dir_s = dir.to_string_lossy().into_owned();
    pick(Request {
        title: abs_i18n::t("gui.dialog.log_folder"),
        start: start_dir(Some(&dir_s), false),
        file_name: None,
        filters: Vec::new(),
        kind: PathKind::Folder,
        save: false,
    })
}

pub fn pick_path_generic(kind: PathKind, current: &str) -> Option<String> {
    let title = match kind {
        PathKind::Folder => abs_i18n::t("gui.dialog.folder"),
        PathKind::File => abs_i18n::t("gui.dialog.file"),
    };
    pick(Request {
        title,
        start: start_dir(
            expand_for_dialog(current).as_deref(),
            kind == PathKind::File,
        ),
        file_name: None,
        filters: Vec::new(),
        kind,
        save: false,
    })
}

struct Request<'a> {
    title: &'a str,
    start: PathBuf,
    file_name: Option<String>,
    filters: Vec<Filter>,
    kind: PathKind,
    save: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Filter {
    name: String,
    exts: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    Portal,
    Kdialog,
    Zenity,
    Matedialog,
    Yad,
    Qarma,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PortalProbe {
    Available,
    Missing,
    Unknown,
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Picked(PathBuf),
    Cancelled,
    Unavailable,
}

fn pick(req: Request<'_>) -> Option<String> {
    if let Some(forced) = forced_backend() {
        let portal_ok = matches!(forced, Backend::Portal);
        return match run_backend(forced, &req, portal_ok) {
            Outcome::Picked(path) => Some(path.display().to_string()),
            Outcome::Cancelled | Outcome::Unavailable => None,
        };
    }
    let probe = probe_file_chooser_portal();
    let portal_ok = matches!(probe, PortalProbe::Available);
    for backend in backends_for(probe, desktop_tokens()) {
        match run_backend(backend, &req, portal_ok) {
            Outcome::Picked(path) => return Some(path.display().to_string()),
            Outcome::Cancelled => return None,
            Outcome::Unavailable => {}
        }
    }
    None
}

fn backends_for(probe: PortalProbe, tokens: Vec<String>) -> Vec<Backend> {
    match probe {
        PortalProbe::Available => vec![Backend::Portal],
        PortalProbe::Missing => cli_backends(tokens),
        PortalProbe::Unknown => {
            let mut order = cli_backends(tokens);
            order.push(Backend::Portal);
            order
        }
    }
}

fn forced_backend() -> Option<Backend> {
    parse_forced_backend(std::env::var("ABS_FILE_DIALOG").ok().as_deref())
}

fn parse_forced_backend(raw: Option<&str>) -> Option<Backend> {
    match raw?.trim().to_ascii_lowercase().as_str() {
        "portal" | "rfd" | "xdg" => Some(Backend::Portal),
        "kdialog" => Some(Backend::Kdialog),
        "zenity" => Some(Backend::Zenity),
        "matedialog" => Some(Backend::Matedialog),
        "yad" => Some(Backend::Yad),
        "qarma" => Some(Backend::Qarma),
        _ => None,
    }
}

fn cli_backends(tokens: Vec<String>) -> Vec<Backend> {
    if desktop_has(&tokens, &["kde", "plasma", "lxqt", "ukui", "deepin", "dde"]) {
        vec![
            Backend::Kdialog,
            Backend::Qarma,
            Backend::Yad,
            Backend::Zenity,
            Backend::Matedialog,
        ]
    } else if desktop_has(&tokens, &["mate"]) {
        vec![
            Backend::Matedialog,
            Backend::Zenity,
            Backend::Yad,
            Backend::Qarma,
            Backend::Kdialog,
        ]
    } else {
        vec![
            Backend::Zenity,
            Backend::Matedialog,
            Backend::Yad,
            Backend::Qarma,
            Backend::Kdialog,
        ]
    }
}

fn run_backend(backend: Backend, req: &Request<'_>, portal_ok: bool) -> Outcome {
    match backend {
        Backend::Portal => match rfd_pick(req) {
            Some(path) => Outcome::Picked(path),
            None => {
                if portal_ok {
                    Outcome::Cancelled
                } else {
                    Outcome::Unavailable
                }
            }
        },
        Backend::Kdialog => run_kdialog(req),
        Backend::Zenity => run_zenity_like(req, "zenity"),
        Backend::Matedialog => run_zenity_like(req, "matedialog"),
        Backend::Yad => run_yad(req),
        Backend::Qarma => run_zenity_like(req, "qarma"),
    }
}

fn rfd_pick(req: &Request<'_>) -> Option<PathBuf> {
    let mut dlg = rfd::FileDialog::new()
        .set_title(req.title)
        .set_directory(&req.start);
    if let Some(name) = req.file_name.as_deref() {
        dlg = dlg.set_file_name(name);
    }
    for filter in &req.filters {
        let exts: Vec<&str> = filter.exts.iter().map(String::as_str).collect();
        dlg = dlg.add_filter(&filter.name, &exts);
    }
    if req.save {
        dlg.save_file()
    } else {
        match req.kind {
            PathKind::Folder => dlg.pick_folder(),
            PathKind::File => dlg.pick_file(),
        }
    }
}

fn run_kdialog(req: &Request<'_>) -> Outcome {
    let Some(bin) = trusted_bin("kdialog") else {
        return Outcome::Unavailable;
    };
    let start = kdialog_start(req);
    let mut args = vec!["--title".into(), req.title.to_string()];
    if req.save {
        args.push("--getsavefilename".into());
        args.push(start);
        args.push(kdialog_filter(&req.filters));
    } else if req.kind == PathKind::Folder {
        args.push("--getexistingdirectory".into());
        args.push(start);
    } else {
        args.push("--getopenfilename".into());
        args.push(start);
        if !req.filters.is_empty() {
            args.push(kdialog_filter(&req.filters));
        }
    }
    run_picker(&bin, &args)
}

fn run_zenity_like(req: &Request<'_>, name: &str) -> Outcome {
    let Some(bin) = trusted_bin(name) else {
        return Outcome::Unavailable;
    };
    let mut args = vec![
        "--file-selection".into(),
        "--title".into(),
        req.title.to_string(),
        format!("--filename={}", zenity_filename(req)),
    ];
    if req.kind == PathKind::Folder {
        args.push("--directory".into());
    }
    if req.save {
        args.push("--save".into());
        args.push("--confirm-overwrite".into());
    }
    for filter in &req.filters {
        args.push("--file-filter".into());
        args.push(zenity_filter(filter));
    }
    run_picker(&bin, &args)
}

fn run_yad(req: &Request<'_>) -> Outcome {
    let Some(bin) = trusted_bin("yad") else {
        return Outcome::Unavailable;
    };
    let mut args = vec![
        "--file".into(),
        "--title".into(),
        req.title.to_string(),
        format!("--filename={}", zenity_filename(req)),
    ];
    if req.kind == PathKind::Folder {
        args.push("--directory".into());
    }
    if req.save {
        args.push("--save".into());
        args.push("--confirm-overwrite".into());
    }
    for filter in &req.filters {
        args.push(format!("--file-filter={}", zenity_filter(filter)));
    }
    run_picker(&bin, &args)
}

fn run_picker(bin: &str, args: &[String]) -> Outcome {
    match Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    {
        Err(err) if err.kind() == ErrorKind::NotFound => Outcome::Unavailable,
        Err(_) => Outcome::Unavailable,
        Ok(output) => {
            if !output.status.success() {
                return Outcome::Cancelled;
            }
            let trimmed = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if trimmed.is_empty() {
                Outcome::Cancelled
            } else {
                Outcome::Picked(PathBuf::from(trimmed))
            }
        }
    }
}

fn kdialog_start(req: &Request<'_>) -> String {
    if let Some(name) = req.file_name.as_deref() {
        req.start.join(name).display().to_string()
    } else {
        req.start.display().to_string()
    }
}

fn zenity_filename(req: &Request<'_>) -> String {
    if let Some(name) = req.file_name.as_deref() {
        req.start.join(name).display().to_string()
    } else {
        let mut dir = req.start.display().to_string();
        if req.kind == PathKind::Folder && !dir.ends_with('/') {
            dir.push('/');
        }
        dir
    }
}

fn kdialog_filter(filters: &[Filter]) -> String {
    if filters.is_empty() {
        return "*|All files".into();
    }
    filters
        .iter()
        .map(|filter| {
            let globs = filter
                .exts
                .iter()
                .map(|ext| {
                    if ext == "*" {
                        "*".to_string()
                    } else {
                        format!("*.{ext}")
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            format!("{globs}|{}", filter.name)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn zenity_filter(filter: &Filter) -> String {
    let globs = filter
        .exts
        .iter()
        .map(|ext| {
            if ext == "*" {
                "*".to_string()
            } else {
                format!("*.{ext}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!("{} | {globs}", filter.name)
}

fn log_save_filters(preferred: LogSaveFormat) -> Vec<Filter> {
    let mut formats = Vec::with_capacity(LogSaveFormat::ALL.len());
    formats.push(preferred);
    formats.extend(
        LogSaveFormat::ALL
            .iter()
            .copied()
            .filter(|f| *f != preferred),
    );
    let mut filters: Vec<Filter> = formats
        .into_iter()
        .map(|format| Filter {
            name: format.ext().to_string(),
            exts: format
                .filter_exts()
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        })
        .collect();
    filters.push(Filter {
        name: abs_i18n::t("gui.dialog.all_files").to_string(),
        exts: vec!["*".into()],
    });
    filters
}

fn probe_file_chooser_portal() -> PortalProbe {
    let attempts: [(&str, &[&str]); 3] = [
        (
            "gdbus",
            &[
                "call",
                "--session",
                "--timeout",
                "1",
                "--dest",
                "org.freedesktop.portal.Desktop",
                "--object-path",
                "/org/freedesktop/portal/desktop",
                "--method",
                "org.freedesktop.DBus.Properties.Get",
                "org.freedesktop.portal.FileChooser",
                "version",
            ],
        ),
        (
            "busctl",
            &[
                "--user",
                "--timeout=1",
                "call",
                "org.freedesktop.portal.Desktop",
                "/org/freedesktop/portal/desktop",
                "org.freedesktop.DBus.Properties",
                "Get",
                "ss",
                "org.freedesktop.portal.FileChooser",
                "version",
            ],
        ),
        (
            "dbus-send",
            &[
                "--session",
                "--print-reply",
                "--reply-timeout=1000",
                "--dest=org.freedesktop.portal.Desktop",
                "/org/freedesktop/portal/desktop",
                "org.freedesktop.DBus.Properties.Get",
                "string:org.freedesktop.portal.FileChooser",
                "string:version",
            ],
        ),
    ];
    let mut ran = false;
    for (program, args) in attempts {
        let Some(bin) = trusted_bin(program) else {
            continue;
        };
        ran = true;
        match Command::new(&bin)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        {
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(_) => continue,
            Ok(output) if output.status.success() => return PortalProbe::Available,
            Ok(_) => continue,
        }
    }
    if ran {
        PortalProbe::Missing
    } else {
        PortalProbe::Unknown
    }
}

fn trusted_bin(name: &str) -> Option<String> {
    let mut candidates = Vec::new();
    for dir in ["/usr/bin", "/usr/sbin", "/usr/local/bin"] {
        candidates.push(PathBuf::from(dir).join(name));
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            candidates.push(dir.join(name));
        }
    }
    candidates.into_iter().find_map(|p| {
        if p.is_file() && path_is_trusted_executable(&p) {
            Some(p.to_string_lossy().into_owned())
        } else {
            None
        }
    })
}

fn path_is_trusted_executable(p: &Path) -> bool {
    let Ok(real) = std::fs::canonicalize(p) else {
        return false;
    };
    let s = real.to_string_lossy();
    [
        "/usr/bin/",
        "/usr/sbin/",
        "/usr/local/bin/",
        "/usr/lib/",
        "/usr/libexec/",
        "/bin/",
        "/sbin/",
        "/nix/store/",
        "/run/current-system/sw/bin/",
        "/gnu/store/",
    ]
    .iter()
    .any(|prefix| s.starts_with(prefix))
}

fn start_dir(path: Option<&str>, want_parent_if_file: bool) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let Some(raw) = path.map(str::trim).filter(|s| !s.is_empty()) else {
        return home;
    };
    let p = PathBuf::from(raw);
    let candidate = if want_parent_if_file && p.is_file() {
        p.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| p.clone())
    } else if p.is_dir() {
        p
    } else if let Some(parent) = p.parent().filter(|parent| parent.is_dir()) {
        parent.to_path_buf()
    } else {
        home.clone()
    };
    if candidate.exists() {
        candidate
    } else {
        home
    }
}

fn folder_dialog_title(field: PathField) -> &'static str {
    match field {
        PathField::PackagesPath => abs_i18n::t("gui.dialog.packages_dir"),
        PathField::ChrootPath => abs_i18n::t("gui.dialog.chroot_dir"),
        PathField::ReadyPath => abs_i18n::t("gui.dialog.ready_dir"),
        PathField::RamdiskMountPoint => abs_i18n::t("gui.dialog.ramdisk_mount"),
        PathField::RamdiskSeedChroot => abs_i18n::t("gui.dialog.ramdisk_seed"),
        PathField::PgoArchiveDir => abs_i18n::t("gui.dialog.pgo_archive"),
        PathField::PgoBenchmarkWorkdir => abs_i18n::t("gui.dialog.pgo_benchmark_workdir"),
        PathField::PgoProfileScratchDir => abs_i18n::t("gui.dialog.pgo_scratch"),
        _ => abs_i18n::t("gui.dialog.folder"),
    }
}

fn file_dialog_title(field: PathField) -> &'static str {
    match field {
        PathField::ChrootMakepkgConf => abs_i18n::t("gui.dialog.makepkg_conf"),
        PathField::SelfUpdateInstallPath => abs_i18n::t("gui.dialog.abs_binary"),
        PathField::PgoBenchmark => abs_i18n::t("gui.dialog.benchmark_script"),
        PathField::PgoVmlinux => abs_i18n::t("gui.dialog.vmlinux"),
        PathField::PgoStateFile => abs_i18n::t("gui.dialog.pgo_state"),
        _ => abs_i18n::t("gui.dialog.file"),
    }
}

fn expand_for_dialog(path: &str) -> Option<String> {
    if path.trim().is_empty() {
        return dirs::home_dir().map(|p| p.display().to_string());
    }
    if path.starts_with('$') || path.starts_with('~') {
        return dirs::home_dir().map(|p| p.display().to_string());
    }
    Some(path.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        backends_for, cli_backends, kdialog_filter, parse_forced_backend, zenity_filename,
        zenity_filter, Filter, PortalProbe, Request,
    };
    use crate::messages::PathKind;
    use std::path::PathBuf;

    #[test]
    fn kde_prefers_kdialog() {
        let order = cli_backends(vec!["plasma".into(), "kde".into()]);
        assert_eq!(order[0], super::Backend::Kdialog);
    }

    #[test]
    fn gnome_prefers_zenity() {
        let order = cli_backends(vec!["gnome".into()]);
        assert_eq!(order[0], super::Backend::Zenity);
    }

    #[test]
    fn cosmic_uses_gtk_style_cli() {
        let order = cli_backends(vec!["cosmic".into()]);
        assert_eq!(order[0], super::Backend::Zenity);
    }

    #[test]
    fn mate_prefers_matedialog() {
        let order = cli_backends(vec!["mate".into()]);
        assert_eq!(order[0], super::Backend::Matedialog);
    }

    #[test]
    fn hyprland_uses_gtk_style_cli() {
        let order = cli_backends(vec!["hyprland".into()]);
        assert_eq!(order[0], super::Backend::Zenity);
        assert!(order.contains(&super::Backend::Kdialog));
    }

    #[test]
    fn portal_available_skips_cli() {
        let order = backends_for(PortalProbe::Available, vec!["plasma".into()]);
        assert_eq!(order, vec![super::Backend::Portal]);
    }

    #[test]
    fn portal_missing_does_not_call_rfd() {
        let order = backends_for(PortalProbe::Missing, vec!["gnome".into()]);
        assert!(!order.contains(&super::Backend::Portal));
        assert_eq!(order[0], super::Backend::Zenity);
    }

    #[test]
    fn portal_unknown_tries_cli_then_rfd() {
        let order = backends_for(PortalProbe::Unknown, vec!["hyprland".into()]);
        assert_eq!(order.last().copied(), Some(super::Backend::Portal));
        assert_eq!(order[0], super::Backend::Zenity);
    }

    #[test]
    fn kdialog_filter_joins_globs() {
        let filters = vec![
            Filter {
                name: "txt".into(),
                exts: vec!["txt".into(), "log".into()],
            },
            Filter {
                name: "All".into(),
                exts: vec!["*".into()],
            },
        ];
        assert_eq!(kdialog_filter(&filters), "*.txt *.log|txt\n*|All");
    }

    #[test]
    fn zenity_filter_format() {
        let filter = Filter {
            name: "html".into(),
            exts: vec!["html".into(), "htm".into()],
        };
        assert_eq!(zenity_filter(&filter), "html | *.html *.htm");
    }

    #[test]
    fn zenity_folder_filename_has_trailing_slash() {
        let req = Request {
            title: "t",
            start: PathBuf::from("/tmp"),
            file_name: None,
            filters: Vec::new(),
            kind: PathKind::Folder,
            save: false,
        };
        assert_eq!(zenity_filename(&req), "/tmp/");
    }

    #[test]
    fn abs_file_dialog_override() {
        assert_eq!(
            parse_forced_backend(Some("kdialog")),
            Some(super::Backend::Kdialog)
        );
        assert_eq!(
            parse_forced_backend(Some("portal")),
            Some(super::Backend::Portal)
        );
        assert_eq!(
            parse_forced_backend(Some("matedialog")),
            Some(super::Backend::Matedialog)
        );
        assert_eq!(parse_forced_backend(Some("nope")), None);
        assert_eq!(parse_forced_backend(None), None);
    }

    #[test]
    fn path_is_trusted_executable_rejects_tmp() {
        let tmp = std::env::temp_dir().join(format!(
            "abs-untrusted-dialog-bin-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&tmp, b"#!/bin/sh\n").unwrap();
        assert!(!super::path_is_trusted_executable(&tmp));
        let _ = std::fs::remove_file(&tmp);
        if std::path::Path::new("/usr/bin/true").is_file() {
            assert!(super::path_is_trusted_executable(std::path::Path::new(
                "/usr/bin/true"
            )));
        }
    }
}
