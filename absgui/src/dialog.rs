use crate::messages::{PathField, PathKind};
use std::path::PathBuf;

pub fn pick_path(field: PathField, kind: PathKind, current: &str) -> Option<String> {
    let initial = expand_for_dialog(current);
    let picked = match kind {
        PathKind::Folder => rfd::FileDialog::new()
            .set_title(folder_dialog_title(field))
            .set_directory(initial.as_deref().unwrap_or("/"))
            .pick_folder(),
        PathKind::File => rfd::FileDialog::new()
            .set_title(file_dialog_title(field))
            .set_directory(
                initial
                    .as_ref()
                    .and_then(|p| PathBuf::from(p).parent().map(|p| p.to_path_buf()))
                    .unwrap_or_else(|| PathBuf::from("/")),
            )
            .pick_file(),
    };
    picked.map(|p| p.display().to_string())
}

fn folder_dialog_title(field: PathField) -> &'static str {
    match field {
        PathField::PackagesPath => "Select packages directory",
        PathField::ChrootPath => "Select chroot base directory",
        PathField::ReadyPath => "Select ready packages directory",
        PathField::RamdiskMountPoint => "Select ramdisk mount point",
        PathField::RamdiskSeedChroot => "Select chroot seed directory",
        PathField::PgoArchiveDir => "Select PGO profiles archive directory",
        PathField::PgoBenchmarkWorkdir => "Select PGO benchmark asset cache directory",
        PathField::PgoProfileScratchDir => "Select PGO profile scratch directory",
        _ => "Select folder",
    }
}

fn file_dialog_title(field: PathField) -> &'static str {
    match field {
        PathField::ChrootMakepkgConf => "Select makepkg.conf",
        PathField::SelfUpdateInstallPath => "Select ABS binary path",
        PathField::PgoBenchmark => "Select benchmark script",
        PathField::PgoVmlinux => "Select vmlinux (DWARF)",
        PathField::PgoStateFile => "Select PGO state JSON file",
        _ => "Select file",
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
