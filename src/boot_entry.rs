//! Pick the PGO stage kernel for the next reboot (oneshot, not the permanent default).
//!
//! Order: systemd Boot Loader Interface (`bootctl`, Limine and systemd-boot), then GRUB.

use crate::utils::{run_command, run_command_with_output};
use crate::{blog, ewarn};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootEntry {
    pub id: String,
    pub linux: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextBoot {
    /// systemd Boot Loader Interface (Limine, systemd-boot, …).
    Bli { id: String },
    /// `grub-reboot` menu path (`submenu>entry`).
    Grub { id: String },
}

pub fn linux_path_pkgbase(linux: &str) -> Option<&str> {
    let name = linux.rsplit(['/', '\\']).next()?;
    let name = name.strip_prefix("vmlinuz-").unwrap_or(name);
    if name.is_empty() { None } else { Some(name) }
}

pub fn id_last_component(id: &str) -> &str {
    let last = id.rsplit(['/', '\\']).next().unwrap_or(id);
    let last = last.strip_suffix(".conf").unwrap_or(last);
    last.strip_suffix(".efi").unwrap_or(last)
}

pub fn entry_matches_pkgbase(entry: &BootEntry, pkgbase: &str) -> bool {
    if let Some(linux) = entry.linux.as_deref()
        && linux_path_pkgbase(linux) == Some(pkgbase)
    {
        return true;
    }
    id_last_component(&entry.id) == pkgbase
}

pub fn pick_boot_entry<'a>(entries: &'a [BootEntry], pkgbase: &str) -> Option<&'a BootEntry> {
    let mut matches: Vec<&BootEntry> = entries
        .iter()
        .filter(|e| entry_matches_pkgbase(e, pkgbase))
        .collect();
    if matches.is_empty() {
        return None;
    }
    matches.sort_by_key(|e| e.id.matches('>').count());
    Some(matches[0])
}

pub fn bootctl_status_has_oneshot(status: &str) -> bool {
    status.lines().any(|line| {
        line.contains("One-shot entry control") && line.contains('✓') && !line.contains('✗')
    })
}

pub fn bootctl_status_current_is_bli(status: &str) -> bool {
    status.lines().any(|line| {
        let line = line.trim().to_ascii_lowercase();
        line.starts_with("product:") && (line.contains("limine") || line.contains("systemd-boot"))
    })
}

pub fn parse_bootctl_list(text: &str) -> Vec<BootEntry> {
    let trimmed = text.trim_start();
    if trimmed.starts_with('[')
        && let Some(entries) = parse_bootctl_list_json(trimmed)
    {
        return entries;
    }
    parse_bootctl_list_text(text)
}

fn parse_bootctl_list_json(text: &str) -> Option<Vec<BootEntry>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let arr = value.as_array()?;
    let mut out = Vec::new();
    for item in arr {
        let id = item.get("id")?.as_str()?.trim();
        if id.is_empty() {
            continue;
        }
        let linux = item
            .get("linux")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        out.push(BootEntry {
            id: id.to_string(),
            linux,
            title,
        });
    }
    Some(out)
}

fn parse_bootctl_list_text(text: &str) -> Vec<BootEntry> {
    let mut out = Vec::new();
    let mut id = None;
    let mut linux = None;
    let mut title = None;
    let flush = |out: &mut Vec<BootEntry>,
                 id: &mut Option<String>,
                 linux: &mut Option<String>,
                 title: &mut Option<String>| {
        if let Some(id) = id.take() {
            out.push(BootEntry {
                id,
                linux: linux.take(),
                title: title.take(),
            });
        } else {
            linux.take();
            title.take();
        }
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("id:") {
            flush(&mut out, &mut id, &mut linux, &mut title);
            let v = rest.trim();
            if !v.is_empty() {
                id = Some(v.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("linux:") {
            let v = rest.trim();
            if !v.is_empty() {
                linux = Some(v.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("title:") {
            let v = rest.trim();
            if !v.is_empty() {
                title = Some(v.to_string());
            }
        }
    }
    flush(&mut out, &mut id, &mut linux, &mut title);
    out
}

pub fn parse_grub_cfg(text: &str) -> Vec<BootEntry> {
    let mut stack: Vec<String> = Vec::new();
    let mut entries = Vec::new();
    let mut current: Option<BootEntry> = None;
    let mut depths: Vec<i32> = Vec::new();
    let mut depth = 0i32;

    for raw in text.lines() {
        let open = raw.matches('{').count() as i32;
        let close = raw.matches('}').count() as i32;
        let trimmed = raw.trim();

        if let Some(title) = parse_grub_title(trimmed, "submenu") {
            stack.push(title);
            depths.push(depth);
        } else if let Some(title) = parse_grub_title(trimmed, "menuentry") {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            let id = parse_grub_id_flag(trimmed).unwrap_or_else(|| {
                let mut id = stack.join(">");
                if !id.is_empty() {
                    id.push('>');
                }
                id.push_str(&title);
                id
            });
            current = Some(BootEntry {
                id,
                linux: None,
                title: Some(title),
            });
            depths.push(depth);
        } else if let Some(linux) = parse_grub_linux(trimmed)
            && let Some(entry) = current.as_mut()
            && entry.linux.is_none()
        {
            entry.linux = Some(linux);
        }

        depth += open - close;
        while let Some(&start) = depths.last()
            && depth <= start
        {
            depths.pop();
            if current.is_some() && depths.len() == stack.len() {
                if let Some(entry) = current.take() {
                    entries.push(entry);
                }
            } else if depths.len() < stack.len() {
                stack.pop();
            }
        }
    }
    if let Some(entry) = current {
        entries.push(entry);
    }
    entries
}

fn parse_grub_title(line: &str, kind: &str) -> Option<String> {
    let rest = line.strip_prefix(kind)?.trim_start();
    let quote = rest.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let rest = &rest[1..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

fn parse_grub_id_flag(line: &str) -> Option<String> {
    let mut parts = line.split_whitespace();
    while let Some(part) = parts.next() {
        if part == "--id" {
            let id = parts.next()?;
            let id = id.trim_matches('\'').trim_matches('"');
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
        if let Some(id) = part.strip_prefix("--id=") {
            let id = id.trim_matches('\'').trim_matches('"');
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    None
}

fn parse_grub_linux(line: &str) -> Option<String> {
    let rest = line
        .strip_prefix("linuxefi")
        .or_else(|| line.strip_prefix("linux16"))
        .or_else(|| line.strip_prefix("linux"))?
        .trim_start();
    if rest.is_empty() {
        return None;
    }
    Some(rest.split_whitespace().next()?.to_string())
}

pub fn parse_limine_conf(text: &str) -> Vec<BootEntry> {
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut entries = Vec::new();
    let mut current: Option<(usize, BootEntry)> = None;

    for raw in text.lines() {
        if raw.trim().is_empty() || raw.trim_start().starts_with('#') {
            continue;
        }
        let indent = raw.chars().take_while(|c| *c == ' ' || *c == '\t').count();
        let trimmed = raw.trim();
        if let Some(name) = trimmed.strip_prefix('/') {
            let name = name.trim().trim_start_matches('+').trim();
            if name.is_empty() {
                continue;
            }
            while stack.last().is_some_and(|(i, _)| *i >= indent) {
                stack.pop();
            }
            if let Some((_, entry)) = current.take() {
                entries.push(entry);
            }
            stack.push((indent, name.to_string()));
            let id = stack
                .iter()
                .map(|(_, n)| n.as_str())
                .collect::<Vec<_>>()
                .join("/");
            current = Some((
                indent,
                BootEntry {
                    id,
                    linux: None,
                    title: Some(name.to_string()),
                },
            ));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("kernel_path:")
            && let Some((_, entry)) = current.as_mut()
        {
            let path = rest.trim();
            let path = path.rsplit(':').next().unwrap_or(path).trim();
            if let Some(pkg) = linux_path_pkgbase(path) {
                entry.linux = Some(format!("/vmlinuz-{pkg}"));
            }
        }
    }
    if let Some((_, entry)) = current {
        entries.push(entry);
    }
    entries
}

/// Set the next reboot to `pkgbase`. Does not change the permanent default.
pub fn set_next_boot_kernel(pkgbase: &str) -> Result<NextBoot, String> {
    if pkgbase.is_empty() {
        return Err("no package base recorded for this PGO stage".into());
    }
    if crate::is_dry_run_mode() {
        blog!("[DRY RUN] would set one-shot bootloader entry for {pkgbase}");
        return Ok(NextBoot::Bli {
            id: pkgbase.to_string(),
        });
    }
    if let Some(next) = try_bli(pkgbase)? {
        return Ok(next);
    }
    if let Some(next) = try_grub(pkgbase)? {
        return Ok(next);
    }
    Err(format!(
        "could not find a bootloader entry for '{pkgbase}' (tried bootctl / Limine BLI and GRUB)"
    ))
}

pub fn reboot(next: Option<&NextBoot>) -> Result<(), String> {
    match next {
        Some(NextBoot::Bli { id }) => {
            blog!("Rebooting into bootloader entry {id}");
            let flag = format!("--boot-loader-entry={id}");
            match run_command("sudo", &["systemctl", "reboot", &flag], None::<&str>) {
                Ok(()) => Ok(()),
                Err(e) => {
                    ewarn!(
                        "systemctl reboot {flag} failed ({e}); rebooting with oneshot already set"
                    );
                    run_command("sudo", &["reboot"], None::<&str>).map_err(|e| e.to_string())
                }
            }
        }
        Some(NextBoot::Grub { id }) => {
            blog!("GRUB next boot: {id}");
            let bin = grub_reboot_bin().unwrap_or("grub-reboot");
            run_command("sudo", &[bin, id], None::<&str>).map_err(|e| e.to_string())?;
            run_command("sudo", &["reboot"], None::<&str>).map_err(|e| e.to_string())
        }
        None => run_command("sudo", &["reboot"], None::<&str>).map_err(|e| e.to_string()),
    }
}

fn try_bli(pkgbase: &str) -> Result<Option<NextBoot>, String> {
    let status = match run_command_with_output("sudo", &["bootctl", "status"], None::<&str>) {
        Ok(s) => s,
        Err(e) => {
            ewarn!("bootctl status failed ({e}); trying other bootloaders");
            return Ok(None);
        }
    };
    if !bootctl_status_has_oneshot(&status) {
        if bootctl_status_current_is_bli(&status) {
            return Err("current bootloader does not advertise one-shot entry control".into());
        }
        return Ok(None);
    }
    let mut entries = match bootctl_list_output() {
        Ok(list) => parse_bootctl_list(&list),
        Err(e) => {
            ewarn!("bootctl list failed ({e}); trying limine.conf");
            Vec::new()
        }
    };
    if pick_boot_entry(&entries, pkgbase).is_none() {
        entries.extend(load_limine_entries());
    }
    let Some(entry) = pick_boot_entry(&entries, pkgbase) else {
        return Err(format!(
            "bootctl has no entry for '{pkgbase}' (ids: {})",
            entries
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    };
    let id = entry.id.clone();
    run_command("sudo", &["bootctl", "set-oneshot", &id], None::<&str>)
        .map_err(|e| format!("bootctl set-oneshot {id}: {e}"))?;
    blog!("Next boot (oneshot): {id}");
    Ok(Some(NextBoot::Bli { id }))
}

fn bootctl_list_output() -> Result<String, String> {
    match run_command_with_output("sudo", &["bootctl", "list", "--json=short"], None::<&str>) {
        Ok(text) if text.trim_start().starts_with('[') => Ok(text),
        Ok(_) | Err(_) => run_command_with_output("sudo", &["bootctl", "list"], None::<&str>),
    }
}

fn try_grub(pkgbase: &str) -> Result<Option<NextBoot>, String> {
    let Some(bin) = grub_reboot_bin() else {
        return Ok(None);
    };
    let Some(cfg) = grub_cfg_path() else {
        return Ok(None);
    };
    let text = read_maybe_sudo(&cfg)?;
    let entries = parse_grub_cfg(&text);
    let Some(entry) = pick_boot_entry(&entries, pkgbase) else {
        return Err(format!("GRUB has no menuentry whose kernel is '{pkgbase}'"));
    };
    let id = entry.id.clone();
    run_command("sudo", &[bin, &id], None::<&str>).map_err(|e| format!("{bin} {id}: {e}"))?;
    blog!("GRUB next boot (oneshot): {id}");
    Ok(Some(NextBoot::Grub { id }))
}

fn grub_reboot_bin() -> Option<&'static str> {
    ["grub-reboot", "grub2-reboot"]
        .into_iter()
        .find(|name| command_exists(name))
}

fn load_limine_entries() -> Vec<BootEntry> {
    for path in limine_conf_paths() {
        if let Ok(text) = read_maybe_sudo(&path) {
            let parsed = parse_limine_conf(&text);
            if !parsed.is_empty() {
                return parsed;
            }
        }
    }
    Vec::new()
}

fn limine_conf_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(esp) = run_command_with_output("bootctl", &["-p"], None::<&str>) {
        let esp = esp.trim();
        if !esp.is_empty() {
            paths.push(PathBuf::from(esp).join("limine.conf"));
            paths.push(PathBuf::from(esp).join("EFI/limine/limine.conf"));
        }
    }
    paths.extend(
        [
            "/boot/limine.conf",
            "/boot/limine/limine.conf",
            "/boot/EFI/limine/limine.conf",
            "/boot/efi/limine.conf",
            "/boot/efi/EFI/limine/limine.conf",
            "/efi/limine.conf",
            "/efi/EFI/limine/limine.conf",
        ]
        .into_iter()
        .map(PathBuf::from),
    );
    paths
}

fn grub_cfg_path() -> Option<PathBuf> {
    for p in ["/boot/grub/grub.cfg", "/boot/grub2/grub.cfg"] {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn read_maybe_sudo(path: &Path) -> Result<String, String> {
    match fs_read(path) {
        Ok(text) => Ok(text),
        Err(_) => {
            run_command_with_output("sudo", &["cat", &path.display().to_string()], None::<&str>)
        }
    }
}

fn fs_read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| e.to_string())
}

fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(id: &str, linux: Option<&str>) -> BootEntry {
        BootEntry {
            id: id.into(),
            linux: linux.map(str::to_string),
            title: None,
        }
    }

    #[test]
    fn pkgbase_match_does_not_confuse_lto_sibling() {
        let entries = vec![
            e("CachyOS/linux-cachyos", Some("/vmlinuz-linux-cachyos")),
            e(
                "CachyOS/linux-cachyos-lto",
                Some("/vmlinuz-linux-cachyos-lto"),
            ),
        ];
        assert_eq!(
            pick_boot_entry(&entries, "linux-cachyos").unwrap().id,
            "CachyOS/linux-cachyos"
        );
        assert_eq!(
            pick_boot_entry(&entries, "linux-cachyos-lto").unwrap().id,
            "CachyOS/linux-cachyos-lto"
        );
    }

    #[test]
    fn id_last_component_strips_conf_suffix() {
        assert_eq!(id_last_component("linux-cachyos.conf"), "linux-cachyos");
        assert_eq!(id_last_component("CachyOS/linux-cachyos"), "linux-cachyos");
        assert_eq!(
            id_last_component(r"EFI\Linux\linux-cachyos.efi"),
            "linux-cachyos"
        );
    }

    #[test]
    fn bootctl_text_and_json_lists_parse() {
        let text = "\
         type: Boot Loader Specification Type #1 (.conf)\n\
        title: CachyOS\n\
           id: CachyOS/linux-cachyos\n\
        linux: /vmlinuz-linux-cachyos\n\
\n\
           id: CachyOS/linux-cachyos-lto\n\
        linux: /vmlinuz-linux-cachyos-lto\n";
        let parsed = parse_bootctl_list(text);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].id, "CachyOS/linux-cachyos");
        let json = r#"[{"id":"CachyOS/linux-cachyos","title":"CachyOS","linux":"/vmlinuz-linux-cachyos"}]"#;
        let parsed = parse_bootctl_list(json);
        assert_eq!(parsed[0].id, "CachyOS/linux-cachyos");
    }

    #[test]
    fn oneshot_feature_requires_check_mark() {
        assert!(bootctl_status_has_oneshot(
            "      Features: ✓ One-shot entry control\n"
        ));
        assert!(!bootctl_status_has_oneshot(
            "                 ✗ One-shot entry control\n"
        ));
    }

    #[test]
    fn current_loader_detects_limine_and_systemd_boot() {
        assert!(bootctl_status_current_is_bli(
            "Current Boot Loader:\n      Product: Limine 12.6.1\n"
        ));
        assert!(bootctl_status_current_is_bli(
            "      Product: systemd-boot 257.2-1-cachyos\n"
        ));
        assert!(!bootctl_status_current_is_bli("      Product: GRUB 2.12\n"));
    }

    #[test]
    fn grub_cfg_nested_advanced_options() {
        let cfg = r#"
submenu 'Advanced options for CachyOS Linux' {
	menuentry 'CachyOS Linux, with Linux 7.2.2-1-cachyos' {
		linux	/boot/vmlinuz-linux-cachyos root=UUID=x
	}
	menuentry 'CachyOS Linux, with Linux 7.2.2-1-cachyos-lto' {
		linux	/boot/vmlinuz-linux-cachyos-lto
	}
}
menuentry 'CachyOS Linux' {
	linux /boot/vmlinuz-linux-cachyos
}
"#;
        let entries = parse_grub_cfg(cfg);
        let plain = pick_boot_entry(&entries, "linux-cachyos").unwrap();
        assert!(!plain.id.contains('>'), "{}", plain.id);
        assert_eq!(
            pick_boot_entry(&entries, "linux-cachyos-lto").unwrap().id,
            "Advanced options for CachyOS Linux>CachyOS Linux, with Linux 7.2.2-1-cachyos-lto"
        );
    }

    #[test]
    fn grub_cfg_prefers_explicit_id_flag() {
        let cfg = r#"
menuentry 'CachyOS Linux' --id 'linux-cachyos' {
	linux /boot/vmlinuz-linux-cachyos
}
"#;
        let entries = parse_grub_cfg(cfg);
        assert_eq!(
            pick_boot_entry(&entries, "linux-cachyos").unwrap().id,
            "linux-cachyos"
        );
    }

    #[test]
    fn limine_conf_nested_names() {
        let conf = r#"
/+CachyOS
    /linux-cachyos
        protocol: linux
        kernel_path: boot():/vmlinuz-linux-cachyos
    /linux-cachyos-lto
        protocol: linux
        kernel_path: boot():/vmlinuz-linux-cachyos-lto
"#;
        let entries = parse_limine_conf(conf);
        assert!(entries.iter().any(|e| e.id == "CachyOS/linux-cachyos"
            && e.linux.as_deref() == Some("/vmlinuz-linux-cachyos")));
        assert_eq!(
            pick_boot_entry(&entries, "linux-cachyos-lto").unwrap().id,
            "CachyOS/linux-cachyos-lto"
        );
    }
}
