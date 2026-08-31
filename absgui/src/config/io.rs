use super::pretty::{humanize_in_place, render_human_toml};
use super::ConfigDocument;
use std::fs;
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, Table};

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("abs").join("abs.toml"))
        .unwrap_or_else(|| PathBuf::from("abs.toml"))
}

pub fn load_config(path: &PathBuf) -> Result<ConfigDocument, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("parse TOML: {e}"))
}

pub fn save_config(path: &PathBuf, doc: &ConfigDocument) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create dir: {e}"))?;
    }
    let mut overlay: DocumentMut =
        toml_edit::ser::to_document(doc).map_err(|e| format!("serialize TOML: {e}"))?;
    humanize_in_place(&mut overlay);
    let text = if path.exists() {
        let existing =
            fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let mut orig: DocumentMut = existing
            .parse()
            .map_err(|e| format!("parse existing {}: {e}", path.display()))?;
        humanize_in_place(&mut orig);
        merge_tables(orig.as_table_mut(), overlay.as_table());
        if doc.lang.is_none() {
            orig.as_table_mut().remove("lang");
        }
        render_human_toml(&mut orig)
    } else {
        render_human_toml(&mut overlay)
    };
    write_mode_0600(path, &text)?;
    if let Some(want) = doc.install_absgui {
        let prefs = path
            .parent()
            .map(|p| p.join("install-prefs.toml"))
            .unwrap_or_else(|| PathBuf::from("install-prefs.toml"));
        write_mode_0600(&prefs, &format!("install_absgui = {want}\n"))?;
    }
    Ok(())
}

fn write_mode_0600(path: &Path, text: &str) -> Result<(), String> {
    use std::fs::OpenOptions;
    use std::io::Write as _;
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    file.write_all(text.as_bytes())
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn merge_tables(dst: &mut Table, src: &Table) {
    for (key, src_item) in src.iter() {
        if src_item.is_table() || src_item.as_inline_table().is_some() {
            ensure_std_table(dst, key);
            let src_table = match src_item.as_table() {
                Some(t) => t.clone(),
                None => src_item
                    .as_inline_table()
                    .map(|t| t.clone().into_table())
                    .expect("inline or table"),
            };
            if let Some(dst_table) = dst.get_mut(key).and_then(Item::as_table_mut) {
                merge_tables(dst_table, &src_table);
                continue;
            }
        }
        match dst.get_mut(key) {
            Some(dst_item) => {
                *dst_item = src_item.clone();
            }
            None => {
                dst.insert(key, src_item.clone());
            }
        }
    }
}

fn ensure_std_table(dst: &mut Table, key: &str) {
    let inline = dst.get(key).and_then(Item::as_inline_table).cloned();
    if let Some(inline) = inline {
        dst.insert(key, Item::Table(inline.into_table()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_preserves_comments_and_unknown_keys() {
        let dir = std::env::temp_dir().join(format!(
            "absgui_save_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("abs.toml");
        std::fs::write(
            &path,
            r#"config_version = 1
# keep this comment
mystery_key = "stay"
manual_update_packages = []
skip_install_packages = []

[paths]
packages_path = "/tmp/abs-test/packages"
chroot_base_path = "/tmp/abs-test/chroot"
ready_made_packages_path = "/tmp/abs-test/ready"

[build]
default_environment = "local"

[system_update]
command_to_update_repositories = "pacman -Sy"
command_to_perform_system_update = "pacman -Syu"
ignore_flag = "--ignore"
ignore_packages = []

[repositories]
default = "arch"
"#,
        )
        .unwrap();
        let mut doc = load_config(&path).expect("load");
        doc.build.default_environment = "chroot".into();
        save_config(&path, &doc).expect("save");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# keep this comment"));
        assert!(text.contains("mystery_key"));
        assert!(text.contains("chroot"));
        assert!(text.contains("[paths]"));
        assert!(!text.contains("paths = {"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_expands_inline_tables_and_spaces_sections() {
        let dir = std::env::temp_dir().join(format!(
            "absgui_inline_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("abs.toml");
        std::fs::write(
            &path,
            r#"config_version = 1
manual_update_packages = ["curl", "wget"]
skip_install_packages = []
paths = { packages_path = "/tmp/abs-test/packages", chroot_base_path = "/tmp/abs-test/chroot", ready_made_packages_path = "/tmp/abs-test/ready" }
build = { default_environment = "local" }
system_update = { command_to_update_repositories = "pacman -Sy", command_to_perform_system_update = "pacman -Syu", ignore_flag = "--ignore", ignore_packages = [] }
repositories = { default = "arch" }
"#,
        )
        .unwrap();
        let doc = load_config(&path).expect("load");
        save_config(&path, &doc).expect("save");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[paths]"), "{text}");
        assert!(text.contains("[build]"), "{text}");
        assert!(text.contains("[system_update]"), "{text}");
        assert!(text.contains("[repositories]"), "{text}");
        assert!(!text.contains("paths = {"), "{text}");
        assert!(!text.contains("build = {"), "{text}");
        assert!(text.contains("\n    \"curl\",\n    \"wget\",\n"), "{text}");
        let paths_at = text.find("[paths]").expect("[paths]");
        assert!(
            text[..paths_at].ends_with("\n\n"),
            "blank line before [paths]:\n{text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_removes_root_lang_when_unset() {
        let dir = std::env::temp_dir().join(format!(
            "absgui_lang_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("abs.toml");
        std::fs::write(
            &path,
            r#"config_version = 1
lang = "de"
manual_update_packages = []
skip_install_packages = []

[paths]
packages_path = "/tmp/abs-test/packages"
chroot_base_path = "/tmp/abs-test/chroot"
ready_made_packages_path = "/tmp/abs-test/ready"

[build]
default_environment = "local"

[system_update]
command_to_update_repositories = "pacman -Sy"
command_to_perform_system_update = "pacman -Syu"
ignore_flag = "--ignore"
ignore_packages = []

[repositories]
default = "arch"
"#,
        )
        .unwrap();
        let mut doc = load_config(&path).expect("load");
        assert_eq!(doc.lang.as_deref(), Some("de"));
        doc.lang = None;
        save_config(&path, &doc).expect("save");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.lines().any(|l| l.trim().starts_with("lang")),
            "lang key should be removed: {text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_document_roundtrips_through_toml() {
        let doc = ConfigDocument::default();
        let text = toml::to_string_pretty(&doc).expect("serialize default config");
        // Required CLI sections must be present.
        assert!(text.contains("[paths]"));
        assert!(text.contains("[build]"));
        assert!(text.contains("[system_update]"));
        assert!(text.contains("[repositories]"));
        // Must parse back without error (valid TOML, tables after values).
        let parsed: ConfigDocument = toml::from_str(&text).expect("re-parse serialized config");
        assert_eq!(parsed.paths.packages_path, doc.paths.packages_path);
    }

    #[test]
    fn ensure_kernel_copies_default_template() {
        let mut doc = ConfigDocument::default();
        doc.kernel_defaults
            .pgo
            .as_mut()
            .unwrap()
            .profiles_archive_dir = Some("/mnt/data/profiles".into());

        doc.ensure_kernel_from_defaults("linux-cachyos-bore");
        let pkg = doc.packages.get("linux-cachyos-bore").unwrap();
        assert_eq!(
            pkg.pgo.as_ref().unwrap().profiles_archive_dir.as_deref(),
            Some("/mnt/data/profiles")
        );
        assert_eq!(
            pkg.kernel.as_ref().unwrap().cpusched.as_deref(),
            Some("cachyos")
        );
    }

    #[test]
    fn default_kernel_template_selects_every_kernel_pick() {
        let k = crate::config::default_kernel_template()
            .kernel
            .expect("template kernel");
        assert_eq!(k.cpusched.as_deref(), Some("cachyos"));
        assert_eq!(k.processor_opt.as_deref(), Some("native"));
        assert_eq!(k.use_llvm_lto.as_deref(), Some("thin"));
        assert_eq!(k.hz_ticks.as_deref(), Some("1000"));
        assert_eq!(k.tickrate.as_deref(), Some("full"));
        assert_eq!(k.preempt.as_deref(), Some("full"));
        assert_eq!(k.hugepage.as_deref(), Some("always"));
        assert_eq!(k.cc_harder.as_deref(), Some("yes"));
    }

    #[test]
    fn ensure_kernel_seeds_existing_package_with_no_options() {
        let mut doc = ConfigDocument::default();
        doc.packages.insert(
            "linux-cachyos".into(),
            crate::config::PackageSection::default(),
        );

        doc.ensure_kernel_from_defaults("linux-cachyos");
        let pkg = doc.packages.get("linux-cachyos").unwrap();
        let k = pkg.kernel.as_ref().unwrap();
        assert_eq!(k.cpusched.as_deref(), Some("cachyos"));
        assert_eq!(k.processor_opt.as_deref(), Some("native"));
        assert_eq!(k.use_llvm_lto.as_deref(), Some("thin"));
        assert_eq!(k.hz_ticks.as_deref(), Some("1000"));
        assert_eq!(k.tickrate.as_deref(), Some("full"));
        assert_eq!(k.preempt.as_deref(), Some("full"));
        assert_eq!(k.hugepage.as_deref(), Some("always"));
        assert_eq!(k.cc_harder.as_deref(), Some("yes"));
        assert_eq!(pkg.source.as_deref(), Some("aur"));
        assert_eq!(pkg.build_env.as_deref(), Some("local"));
    }

    #[test]
    fn ensure_kernel_seeds_existing_empty_kernel_table() {
        let mut doc = ConfigDocument::default();
        doc.packages.insert(
            "linux-cachyos".into(),
            crate::config::PackageSection {
                kernel: Some(crate::config::KernelSection::default()),
                ..Default::default()
            },
        );

        doc.ensure_kernel_from_defaults("linux-cachyos");
        assert_eq!(
            doc.packages["linux-cachyos"]
                .kernel
                .as_ref()
                .unwrap()
                .cpusched
                .as_deref(),
            Some("cachyos")
        );
    }

    #[test]
    fn ensure_kernel_keeps_user_selected_scheduler() {
        let mut doc = ConfigDocument::default();
        doc.packages.insert(
            "linux-cachyos".into(),
            crate::config::PackageSection {
                kernel: Some(crate::config::KernelSection {
                    cpusched: Some("bore".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        doc.ensure_kernel_from_defaults("linux-cachyos");
        assert_eq!(
            doc.packages["linux-cachyos"]
                .kernel
                .as_ref()
                .unwrap()
                .cpusched
                .as_deref(),
            Some("bore")
        );
    }

    #[test]
    fn per_kernel_edits_are_independent() {
        let mut doc = ConfigDocument::default();
        doc.ensure_kernel_from_defaults("linux-cachyos");
        doc.ensure_kernel_from_defaults("linux-cachyos-bore");
        doc.packages
            .get_mut("linux-cachyos")
            .unwrap()
            .kernel
            .as_mut()
            .unwrap()
            .cpusched = Some("eevdf".into());

        assert_eq!(
            doc.packages["linux-cachyos"]
                .kernel
                .as_ref()
                .unwrap()
                .cpusched
                .as_deref(),
            Some("eevdf")
        );
        assert_eq!(
            doc.packages["linux-cachyos-bore"]
                .kernel
                .as_ref()
                .unwrap()
                .cpusched
                .as_deref(),
            Some("cachyos")
        );
    }

    #[test]
    fn package_fields_roundtrip_through_toml() {
        let mut doc = ConfigDocument::default();
        let pkg = crate::config::PackageSection {
            source: Some("aur".into()),
            alias: Some("firefox-bin".into()),
            custom_local_build_command: Some("makepkg -s".into()),
            custom_chroot_build_command: Some("makechrootpkg -r /x".into()),
            tests: Some(false),
            upstream_prereleases: Some(true),
            compilation_threads: Some(8),
            compile_alone: true,
            compilation_priority: 5,
            ..Default::default()
        };
        doc.packages.insert("firefox".into(), pkg);
        let text = toml::to_string_pretty(&doc).expect("serialize");
        let parsed: ConfigDocument = toml::from_str(&text).expect("parse");
        let p = parsed.packages.get("firefox").unwrap();
        assert_eq!(p.alias.as_deref(), Some("firefox-bin"));
        assert_eq!(p.custom_local_build_command.as_deref(), Some("makepkg -s"));
        assert_eq!(
            p.custom_chroot_build_command.as_deref(),
            Some("makechrootpkg -r /x")
        );
        assert_eq!(p.tests, Some(false));
        assert_eq!(p.upstream_prereleases, Some(true));
        assert_eq!(p.compilation_threads, Some(8));
        assert!(p.compile_alone);
        assert_eq!(p.compilation_priority, 5);
    }

    #[test]
    fn kernel_suffix_fields_roundtrip_through_toml() {
        let mut doc = ConfigDocument::default();
        doc.ensure_kernel_from_defaults("linux-cachyos");
        let k = doc
            .packages
            .get_mut("linux-cachyos")
            .unwrap()
            .kernel
            .as_mut()
            .unwrap();
        k.use_lto_suffix = Some("y".into());
        k.use_kcfi = Some("y".into());
        let text = toml::to_string_pretty(&doc).expect("serialize");
        assert!(text.contains("_use_lto_suffix"));
        let parsed: ConfigDocument = toml::from_str(&text).expect("parse");
        let k = parsed.packages["linux-cachyos"].kernel.as_ref().unwrap();
        assert_eq!(k.use_lto_suffix.as_deref(), Some("y"));
        assert_eq!(k.use_kcfi.as_deref(), Some("y"));
    }

    #[test]
    fn package_lists_serialize_as_toml_arrays() {
        let doc = ConfigDocument {
            manual_update_packages: vec!["linux-cachyos".into(), "nvidia".into()],
            skip_install_packages: vec!["mesa".into()],
            ..Default::default()
        };
        let text = toml::to_string_pretty(&doc).expect("serialize");
        assert!(text.contains("manual_update_packages = ["));
        assert!(text.contains("\"linux-cachyos\""));
        assert!(text.contains("\"nvidia\""));
        let parsed: ConfigDocument = toml::from_str(&text).expect("parse");
        assert_eq!(parsed.manual_update_packages, doc.manual_update_packages);
    }

    #[test]
    fn held_packages_roundtrip_through_toml() {
        use crate::config::{AutoRecompileTrigger, HeldPackage};
        use std::collections::HashMap;

        let mut doc = ConfigDocument::default();
        doc.held_packages.push(HeldPackage {
            name: "libfoo".into(),
            version: "1.2.3-1".into(),
            auto_recompile_trigger: AutoRecompileTrigger {
                on_packages_updated: HashMap::from([
                    ("glibc".into(), "2.41-1".into()),
                    ("icu".into(), "76.1-1".into()),
                ]),
            },
        });
        let text = toml::to_string_pretty(&doc).expect("serialize");
        assert!(text.contains("libfoo"));
        assert!(text.contains("1.2.3-1"));
        assert!(text.contains("glibc"));
        let parsed: ConfigDocument = toml::from_str(&text).expect("parse");
        assert_eq!(parsed.held_packages.len(), 1);
        assert_eq!(parsed.held_packages[0].name, "libfoo");
        assert_eq!(
            parsed.held_packages[0]
                .auto_recompile_trigger
                .on_packages_updated
                .get("glibc")
                .map(String::as_str),
            Some("2.41-1")
        );
        let text_form = parsed.held_packages[0].triggers_text();
        assert!(text_form.contains("glibc=2.41-1"));
    }

    #[test]
    fn system_update_legacy_aliases_parse() {
        let text = r#"
config_version = 1
manual_update_packages = []
skip_install_packages = []

[paths]
packages_path = "/tmp/abs-test/packages"
chroot_base_path = "/tmp/abs-test/chroot"
ready_made_packages_path = "/tmp/abs-test/ready"

[build]
default_environment = "local"

[system_update]
command = "pacman -Sy"
command_with_refresh = "pacman -Syu"
command_no_refresh = "pacman -Su"
ignore_flag = "--ignore"
ignore_packages = []

[repositories]
default = "arch"
"#;
        let doc: ConfigDocument = toml::from_str(text).expect("parse aliases");
        assert_eq!(
            doc.system_update.command_to_update_repositories,
            "pacman -Sy"
        );
        assert_eq!(
            doc.system_update.command_to_perform_system_update,
            "pacman -Syu"
        );
        assert_eq!(
            doc.system_update
                .command_to_perform_system_update_no_refresh
                .as_deref(),
            Some("pacman -Su")
        );
        assert_eq!(doc.system_update.auto_refresh_delay, 0);
        assert!(!doc.system_update.remember_sudo);
    }

    #[test]
    fn system_update_auto_refresh_delay_accepts_quoted_number() {
        let text = r#"
config_version = 1
manual_update_packages = []
skip_install_packages = []

[paths]
packages_path = "/tmp/abs-test/packages"
chroot_base_path = "/tmp/abs-test/chroot"
ready_made_packages_path = "/tmp/abs-test/ready"

[build]
default_environment = "local"

[system_update]
command_to_update_repositories = "pacman -Sy"
command_to_perform_system_update = "pacman -Syu"
auto_refresh_delay = "15"
remember_sudo = true
ignore_flag = "--ignore"
ignore_packages = []

[repositories]
default = "arch"
"#;
        let doc: ConfigDocument = toml::from_str(text).expect("parse delay");
        assert_eq!(doc.system_update.auto_refresh_delay, 15);
        assert!(doc.system_update.remember_sudo);
    }
}
