use super::ConfigDocument;
use std::fs;
use std::path::PathBuf;

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
    let text = toml::to_string_pretty(doc).map_err(|e| format!("serialize: {e}"))?;
    fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            .profiles_archive_dir = Some("/mnt/hdd/profiles".into());

        doc.ensure_kernel_from_defaults("linux-cachyos-bore");
        let pkg = doc.packages.get("linux-cachyos-bore").unwrap();
        assert_eq!(
            pkg.pgo.as_ref().unwrap().profiles_archive_dir.as_deref(),
            Some("/mnt/hdd/profiles")
        );
        assert_eq!(pkg.kernel.as_ref().unwrap().cpusched.as_deref(), Some("cachyos"));
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
            doc.packages["linux-cachyos"].kernel.as_ref().unwrap().cpusched.as_deref(),
            Some("eevdf")
        );
        assert_eq!(
            doc.packages["linux-cachyos-bore"].kernel.as_ref().unwrap().cpusched.as_deref(),
            Some("cachyos")
        );
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
}
