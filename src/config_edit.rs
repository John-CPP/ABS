//! Surgical edits to `abs.toml` via `toml_edit` (preserves comments/formatting).

use crate::config::{HeldPackage, ensure_config_file, user_config_path};
use crate::die;
use crate::held::split_pkgver_pkgrel;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};

/// Which root/system_update string array to mutate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigListKind {
    ManualUpdate,
    SkipInstall,
    SkipInstallAfter,
    Ignore,
}

impl ConfigListKind {
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::ManualUpdate => "manual_update_packages",
            Self::SkipInstall => "skip_install_packages",
            Self::SkipInstallAfter => "skip_install_packages_after_compilation",
            Self::Ignore => "ignore_packages",
        }
    }

    pub fn parse(name: &str) -> Result<Self, String> {
        match name.trim().to_ascii_lowercase().as_str() {
            "manual_update_packages" | "manual_update" | "manual" | "watched" => {
                Ok(Self::ManualUpdate)
            }
            "skip_install_packages" | "skip_install" | "skip" => Ok(Self::SkipInstall),
            "skip_install_packages_after_compilation"
            | "skip_install_after"
            | "skip_after"
            | "skip_after_compilation" => Ok(Self::SkipInstallAfter),
            "ignore_packages" | "ignore" | "sys_ignore" | "system_update.ignore_packages" => {
                Ok(Self::Ignore)
            }
            other => Err(format!(
                "unknown package list {:?}; expected manual_update_packages, skip_install_packages, skip_install_packages_after_compilation, or ignore_packages",
                other
            )),
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::ManualUpdate,
            Self::SkipInstall,
            Self::SkipInstallAfter,
            Self::Ignore,
        ]
    }
}

/// Fields writable under `[packages.<name>]` by the edit wizard / CLI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageEditFields {
    pub build_env: Option<String>,
    pub tests: Option<bool>,
    pub compiler: Option<String>,
    pub ramdisk: Option<String>,
    pub compilation_threads: Option<Option<usize>>,
    pub compile_alone: Option<bool>,
    pub ignore_already_made_packages: Option<Option<bool>>,
    pub source: Option<String>,
}

fn load_document(path: &Path) -> DocumentMut {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|e| die!("Failed to read config '{}': {}", path.display(), e));
    text.parse::<DocumentMut>()
        .unwrap_or_else(|e| die!("Failed to parse config '{}': {}", path.display(), e))
}

fn save_document(path: &Path, doc: &DocumentMut) {
    if let Some(parent) = path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        die!(
            "Failed to create config directory '{}': {}",
            parent.display(),
            e
        );
    }
    let mut doc = doc.clone();
    let text = crate::toml_pretty::render_human_toml(&mut doc);
    if let Err(e) = crate::utils::write_file_mode(path, &text, 0o600) {
        die!("Failed to write config '{}': {}", path.display(), e);
    }
}

fn with_document<R>(f: impl FnOnce(&mut DocumentMut) -> R) -> R {
    let path = ensure_config_file();
    let mut doc = load_document(&path);
    let result = f(&mut doc);
    save_document(&path, &doc);
    result
}

fn ensure_string_array<'a>(doc: &'a mut DocumentMut, kind: ConfigListKind) -> &'a mut Array {
    match kind {
        ConfigListKind::Ignore => {
            let sys = doc
                .entry("system_update")
                .or_insert(Item::Table(Table::new()))
                .as_table_mut()
                .unwrap_or_else(|| die!("[system_update] is not a table"));
            let item = sys
                .entry("ignore_packages")
                .or_insert(Item::Value(Value::Array(Array::new())));
            array_mut(item, "system_update.ignore_packages")
        }
        other => {
            let key = other.canonical_name();
            let item = doc
                .entry(key)
                .or_insert(Item::Value(Value::Array(Array::new())));
            array_mut(item, key)
        }
    }
}

fn array_mut<'a>(item: &'a mut Item, label: &str) -> &'a mut Array {
    match item {
        Item::Value(Value::Array(a)) => a,
        Item::None => {
            *item = Item::Value(Value::Array(Array::new()));
            item.as_array_mut().unwrap()
        }
        other => {
            // Convert formatted array-of-tables style isn't expected for these keys.
            if let Some(a) = other.as_array_mut() {
                a
            } else {
                die!("{} must be a TOML array of strings", label);
            }
        }
    }
}

fn array_contains(arr: &Array, pkg: &str) -> bool {
    arr.iter().any(|v| v.as_str() == Some(pkg))
}

fn push_unique_string(arr: &mut Array, pkg: &str) -> bool {
    if array_contains(arr, pkg) {
        return false;
    }
    arr.push(pkg.to_string());
    true
}

fn remove_string(arr: &mut Array, pkg: &str) -> bool {
    let before = arr.len();
    let mut i = 0;
    while i < arr.len() {
        if arr.get(i).and_then(|v| v.as_str()) == Some(pkg) {
            arr.remove(i);
        } else {
            i += 1;
        }
    }
    arr.len() != before
}

/// Add packages to a config list. Returns names that were newly added.
pub fn list_add(kind: ConfigListKind, packages: &[String]) -> Vec<String> {
    with_document(|doc| {
        let arr = ensure_string_array(doc, kind);
        let mut added = Vec::new();
        for pkg in packages {
            let pkg = pkg.trim();
            if pkg.is_empty() {
                continue;
            }
            if push_unique_string(arr, pkg) {
                added.push(pkg.to_string());
            }
        }
        added
    })
}

/// Remove packages from a config list. Returns names that were removed.
pub fn list_remove(kind: ConfigListKind, packages: &[String]) -> Vec<String> {
    with_document(|doc| {
        let arr = ensure_string_array(doc, kind);
        let mut removed = Vec::new();
        for pkg in packages {
            let pkg = pkg.trim();
            if pkg.is_empty() {
                continue;
            }
            if remove_string(arr, pkg) {
                removed.push(pkg.to_string());
            }
        }
        removed
    })
}

fn upsert_held_as_aot(doc: &mut DocumentMut, held: &HeldPackage) {
    // Prefer array-of-tables for nested trigger maps.
    let needs_migrate = match doc.get("held_packages") {
        Some(item) => !item.is_array_of_tables(),
        None => true,
    };
    if needs_migrate {
        let existing = extract_held_entries(doc);
        let mut aot = toml_edit::ArrayOfTables::new();
        for e in existing {
            if e.name != held.name {
                aot.push(held_to_table(&e));
            }
        }
        aot.push(held_to_table(held));
        doc["held_packages"] = Item::ArrayOfTables(aot);
        return;
    }
    let aot = doc["held_packages"].as_array_of_tables_mut().unwrap();
    let existing_idx = aot
        .iter()
        .position(|t| t.get("name").and_then(|v| v.as_str()) == Some(held.name.as_str()));
    if let Some(idx) = existing_idx {
        *aot.get_mut(idx).unwrap() = held_to_table(held);
    } else {
        aot.push(held_to_table(held));
    }
}

fn held_to_table(held: &HeldPackage) -> Table {
    let mut t = Table::new();
    t["name"] = value_str(&held.name);
    t["version"] = value_str(&held.version);
    if !held.auto_recompile_trigger.on_packages_updated.is_empty() {
        let mut on_upd = Table::new();
        let mut triggers: Vec<_> = held
            .auto_recompile_trigger
            .on_packages_updated
            .iter()
            .collect();
        triggers.sort_by(|a, b| a.0.cmp(b.0));
        for (k, v) in triggers {
            on_upd[k.as_str()] = value_str(v);
        }
        let mut trigger = Table::new();
        trigger["on_packages_updated"] = Item::Table(on_upd);
        t["auto_recompile_trigger"] = Item::Table(trigger);
    }
    t
}

fn value_str(s: &str) -> Item {
    Item::Value(Value::from(s))
}

fn extract_held_entries(doc: &DocumentMut) -> Vec<HeldPackage> {
    let mut out = Vec::new();
    let Some(item) = doc.get("held_packages") else {
        return out;
    };
    if let Some(aot) = item.as_array_of_tables() {
        for t in aot.iter() {
            if let Some(h) = table_to_held(t) {
                out.push(h);
            }
        }
        return out;
    }
    if let Some(arr) = item.as_array() {
        for v in arr.iter() {
            if let Some(it) = v.as_inline_table() {
                if let Some(h) = inline_to_held(it) {
                    out.push(h);
                }
            }
        }
    }
    out
}

fn table_to_held(t: &Table) -> Option<HeldPackage> {
    let name = t.get("name")?.as_str()?.to_string();
    let version = t.get("version")?.as_str()?.to_string();
    let mut on_packages_updated = HashMap::new();
    if let Some(trig) = t.get("auto_recompile_trigger").and_then(|i| i.as_table())
        && let Some(on) = trig.get("on_packages_updated").and_then(|i| i.as_table())
    {
        for (k, v) in on.iter() {
            if let Some(s) = v.as_str() {
                on_packages_updated.insert(k.to_string(), s.to_string());
            }
        }
    }
    Some(HeldPackage {
        name,
        version,
        auto_recompile_trigger: crate::config::AutoRecompileTrigger {
            on_packages_updated,
        },
    })
}

fn inline_to_held(it: &InlineTable) -> Option<HeldPackage> {
    let name = it.get("name")?.as_str()?.to_string();
    let version = it.get("version")?.as_str()?.to_string();
    Some(HeldPackage {
        name,
        version,
        auto_recompile_trigger: crate::config::AutoRecompileTrigger::default(),
    })
}

/// Upsert a held package entry.
pub fn hold_package(held: &HeldPackage) -> Result<(), String> {
    split_pkgver_pkgrel(&held.version)?;
    with_document(|doc| {
        upsert_held_as_aot(doc, held);
    });
    Ok(())
}

/// Remove held packages by name. Returns removed names.
pub fn unhold_packages(names: &[String]) -> Vec<String> {
    with_document(|doc| {
        let mut removed = Vec::new();
        let Some(item) = doc.get_mut("held_packages") else {
            return removed;
        };
        if let Some(aot) = item.as_array_of_tables_mut() {
            let mut i = 0;
            while i < aot.len() {
                let name = aot
                    .get(i)
                    .and_then(|t| t.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if names.iter().any(|n| n == &name) {
                    aot.remove(i);
                    removed.push(name);
                } else {
                    i += 1;
                }
            }
        } else if let Some(arr) = item.as_array_mut() {
            let mut i = 0;
            while i < arr.len() {
                let name = arr
                    .get(i)
                    .and_then(|v| v.as_inline_table())
                    .and_then(|t| t.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if names.iter().any(|n| n == &name) {
                    arr.remove(i);
                    removed.push(name);
                } else {
                    i += 1;
                }
            }
        }
        removed
    })
}

/// Update saved `on_packages_updated` versions for any trigger that appears in `versions`.
pub fn update_trigger_versions(versions: &HashMap<String, String>) -> bool {
    if versions.is_empty() {
        return false;
    }
    with_document(|doc| {
        let mut changed = false;
        let Some(item) = doc.get_mut("held_packages") else {
            return false;
        };
        let Some(aot) = item.as_array_of_tables_mut() else {
            return false;
        };
        for table in aot.iter_mut() {
            let Some(trig) = table
                .get_mut("auto_recompile_trigger")
                .and_then(|i| i.as_table_mut())
            else {
                continue;
            };
            let Some(on) = trig
                .get_mut("on_packages_updated")
                .and_then(|i| i.as_table_mut())
            else {
                continue;
            };
            for (pkg, new_ver) in versions {
                if let Some(item) = on.get_mut(pkg.as_str()) {
                    let old = item.as_str().unwrap_or("");
                    if old != new_ver.as_str() {
                        *item = value_str(new_ver);
                        changed = true;
                    }
                }
            }
        }
        changed
    })
}

/// Apply package compile-option edits under `[packages.<name>]`.
pub fn edit_package_fields(pkg: &str, fields: &PackageEditFields) -> Result<(), String> {
    if pkg.trim().is_empty() {
        return Err("package name cannot be empty".into());
    }
    let path = ensure_config_file();
    let mut doc = load_document(&path);
    {
        let packages = doc
            .entry("packages")
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .ok_or_else(|| "[packages] is not a table".to_string())?;
        let entry = packages
            .entry(pkg)
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .ok_or_else(|| format!("[packages.{}] is not a table", pkg))?;

        if let Some(v) = &fields.build_env {
            entry["build_env"] = value_str(v);
        }
        if let Some(v) = fields.tests {
            entry["tests"] = Item::Value(Value::from(v));
        }
        if let Some(v) = &fields.compiler {
            if v.is_empty() {
                entry.remove("compiler");
            } else {
                entry["compiler"] = value_str(v);
            }
        }
        if let Some(v) = &fields.ramdisk {
            if v.is_empty() {
                entry.remove("ramdisk");
            } else {
                entry["ramdisk"] = value_str(v);
            }
        }
        if let Some(opt) = fields.compilation_threads {
            match opt {
                Some(n) => entry["compilation_threads"] = Item::Value(Value::from(n as i64)),
                None => {
                    entry.remove("compilation_threads");
                }
            }
        }
        if let Some(v) = fields.compile_alone {
            entry["compile_alone"] = Item::Value(Value::from(v));
        }
        if let Some(opt) = fields.ignore_already_made_packages {
            match opt {
                Some(b) => entry["ignore_already_made_packages"] = Item::Value(Value::from(b)),
                None => {
                    entry.remove("ignore_already_made_packages");
                }
            }
        }
        if let Some(v) = &fields.source {
            if v.is_empty() {
                entry.remove("source");
            } else {
                entry["source"] = value_str(v);
            }
        }
    }
    save_document(&path, &doc);
    Ok(())
}

/// Read current `[packages.<name>]` fields for wizard highlighting (from disk).
pub fn read_package_fields(pkg: &str) -> PackageEditFields {
    let path = user_config_path();
    if !path.exists() {
        return PackageEditFields::default();
    }
    let doc = load_document(&path);
    let Some(entry) = doc
        .get("packages")
        .and_then(|i| i.as_table())
        .and_then(|t| t.get(pkg))
        .and_then(|i| i.as_table())
    else {
        return PackageEditFields::default();
    };
    let compilation_threads = if entry.get("compilation_threads").is_some() {
        Some(
            entry
                .get("compilation_threads")
                .and_then(|v| v.as_integer())
                .map(|n| n as usize),
        )
    } else {
        None
    };
    let ignore_already_made_packages = if entry.get("ignore_already_made_packages").is_some() {
        Some(
            entry
                .get("ignore_already_made_packages")
                .and_then(|v| v.as_bool()),
        )
    } else {
        None
    };
    PackageEditFields {
        build_env: entry
            .get("build_env")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        tests: entry.get("tests").and_then(|v| v.as_bool()),
        compiler: entry
            .get("compiler")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        ramdisk: entry
            .get("ramdisk")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        compilation_threads,
        compile_alone: entry.get("compile_alone").and_then(|v| v.as_bool()),
        ignore_already_made_packages,
        source: entry
            .get("source")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    }
}

/// Format Reproduce / Undo lines for the user.
pub fn print_reproduce_undo(reproduce: &str, undo: &str) {
    println!();
    println!("Reproduce: {}", reproduce);
    println!("Undo: {}", undo);
}

pub fn shell_join_packages(packages: &[String]) -> String {
    packages
        .iter()
        .map(|p| {
            if p.chars().any(|c| c.is_whitespace() || "[]'\"".contains(c)) {
                format!("'{}'", p.replace('\'', "'\\''"))
            } else {
                p.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Test helper: mutate a document string without touching the live config.
#[cfg(test)]
pub fn list_add_in_doc(
    doc: &mut DocumentMut,
    kind: ConfigListKind,
    packages: &[&str],
) -> Vec<String> {
    let arr = ensure_string_array(doc, kind);
    let mut added = Vec::new();
    for pkg in packages {
        if push_unique_string(arr, pkg) {
            added.push((*pkg).to_string());
        }
    }
    added
}

#[cfg(test)]
pub fn list_remove_in_doc(
    doc: &mut DocumentMut,
    kind: ConfigListKind,
    packages: &[&str],
) -> Vec<String> {
    let arr = ensure_string_array(doc, kind);
    let mut removed = Vec::new();
    for pkg in packages {
        if remove_string(arr, pkg) {
            removed.push((*pkg).to_string());
        }
    }
    removed
}

#[cfg(test)]
pub fn hold_in_doc(doc: &mut DocumentMut, held: &HeldPackage) {
    upsert_held_as_aot(doc, held);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AutoRecompileTrigger;

    fn sample_doc() -> DocumentMut {
        r#"
# comment keep me
manual_update_packages = []
skip_install_packages = ["keep"]
# trailing

[system_update]
ignore_packages = []

[packages]
"#
        .parse()
        .unwrap()
    }

    #[test]
    fn list_aliases_parse() {
        assert_eq!(
            ConfigListKind::parse("manual").unwrap(),
            ConfigListKind::ManualUpdate
        );
        assert_eq!(
            ConfigListKind::parse("skip_after").unwrap(),
            ConfigListKind::SkipInstallAfter
        );
        assert_eq!(
            ConfigListKind::parse("ignore").unwrap(),
            ConfigListKind::Ignore
        );
        assert!(ConfigListKind::parse("nope").is_err());
    }

    #[test]
    fn add_remove_preserves_comment() {
        let mut doc = sample_doc();
        list_add_in_doc(&mut doc, ConfigListKind::ManualUpdate, &["foo", "bar"]);
        let text = doc.to_string();
        assert!(text.contains("# comment keep me"));
        assert!(text.contains("foo"));
        list_remove_in_doc(&mut doc, ConfigListKind::ManualUpdate, &["foo"]);
        let text = doc.to_string();
        assert!(text.contains("# comment keep me"));
        assert!(!text.contains("\"foo\"") && !text.contains("foo,"));
        assert!(text.contains("bar"));
    }

    #[test]
    fn hold_roundtrip_with_triggers() {
        let mut doc = sample_doc();
        let held = HeldPackage {
            name: "libfoo".into(),
            version: "1.2.3-1".into(),
            auto_recompile_trigger: AutoRecompileTrigger {
                on_packages_updated: HashMap::from([
                    ("glibc".into(), "2.41-1".into()),
                    ("icu".into(), "76.1-1".into()),
                ]),
            },
        };
        hold_in_doc(&mut doc, &held);
        let text = doc.to_string();
        assert!(text.contains("libfoo"));
        assert!(text.contains("1.2.3-1"));
        assert!(text.contains("glibc"));
        let entries = extract_held_entries(&doc);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "libfoo");
        assert_eq!(
            entries[0]
                .auto_recompile_trigger
                .on_packages_updated
                .get("glibc")
                .unwrap(),
            "2.41-1"
        );
    }

    #[test]
    fn edit_package_fields_in_memory() {
        let mut doc = sample_doc();
        // Simulate edit_package_fields core
        let packages = doc["packages"].as_table_mut().unwrap();
        let entry = packages
            .entry("mesa")
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .unwrap();
        entry["build_env"] = value_str("chroot");
        entry["tests"] = Item::Value(Value::from(false));
        let text = doc.to_string();
        assert!(text.contains("[packages.mesa]") || text.contains("mesa"));
        assert!(text.contains("chroot"));
    }
}
