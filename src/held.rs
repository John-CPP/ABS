//! Held packages: pin version, ignore on system update, optional auto-recompile triggers.

use crate::config::{AutoRecompileTrigger, Config, HeldPackage};
use crate::package_spec::PackageSpec;
use crate::utils::pacman_query_version;
use std::collections::{HashMap, HashSet};

/// Split `pkgver-pkgrel` (epoch may appear in pkgver, e.g. `1:0.56.1-1`).
pub fn split_pkgver_pkgrel(version: &str) -> Result<(String, String), String> {
    let version = version.trim();
    if version.is_empty() {
        return Err("held version cannot be empty".into());
    }
    let Some((pkgver, pkgrel)) = version.rsplit_once('-') else {
        return Err(format!(
            "invalid held version {:?}: expected pkgver-pkgrel (e.g. 1.2.3-1)",
            version
        ));
    };
    if pkgver.is_empty() || pkgrel.is_empty() {
        return Err(format!(
            "invalid held version {:?}: pkgver and pkgrel must be non-empty",
            version
        ));
    }
    Ok((pkgver.to_string(), pkgrel.to_string()))
}

/// Names of all held packages.
pub fn held_names(config: &Config) -> Vec<String> {
    config
        .held_packages
        .iter()
        .map(|h| h.name.clone())
        .collect()
}

pub fn is_held(config: &Config, pkg: &str) -> bool {
    config.held_packages.iter().any(|h| h.name == pkg)
}

pub fn find_held<'a>(config: &'a Config, pkg: &str) -> Option<&'a HeldPackage> {
    config.held_packages.iter().find(|h| h.name == pkg)
}

/// Inject held pkgver/pkgrel into `spec` unless the CLI already set them.
pub fn apply_held_overrides_to_spec(spec: &mut PackageSpec, config: &Config) {
    let Some(held) = find_held(config, &spec.name) else {
        return;
    };
    let Ok((pkgver, pkgrel)) = split_pkgver_pkgrel(&held.version) else {
        return;
    };
    if !spec.pkgbuild_overrides.contains_key("pkgver") {
        spec.pkgbuild_overrides.insert("pkgver".to_string(), pkgver);
    }
    if !spec.pkgbuild_overrides.contains_key("pkgrel") {
        spec.pkgbuild_overrides.insert("pkgrel".to_string(), pkgrel);
    }
}

pub fn apply_held_overrides_to_specs(specs: &mut [PackageSpec], config: &Config) {
    for spec in specs.iter_mut() {
        apply_held_overrides_to_spec(spec, config);
    }
}

/// Trigger packages whose installed version differs from the saved map (upgrade or downgrade).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerDrift {
    pub held_name: String,
    pub held_version: String,
    /// Trigger name -> (saved, installed). Installed is `None` if not installed.
    pub changed: Vec<(String, String, Option<String>)>,
}

/// Compare saved `on_packages_updated` versions to current `pacman -Q`.
pub fn detect_trigger_drifts(config: &Config) -> Vec<TriggerDrift> {
    let mut out = Vec::new();
    for held in &config.held_packages {
        let mut changed = Vec::new();
        for (trig, saved) in &held.auto_recompile_trigger.on_packages_updated {
            let installed = pacman_query_version(trig).ok().flatten();
            let drifted = match &installed {
                Some(inst) => inst != saved,
                None => true, // uninstalled counts as change
            };
            if drifted {
                changed.push((trig.clone(), saved.clone(), installed));
            }
        }
        if !changed.is_empty() {
            out.push(TriggerDrift {
                held_name: held.name.clone(),
                held_version: held.version.clone(),
                changed,
            });
        }
    }
    out
}

/// Pure helper for tests: compare maps without calling pacman.
#[cfg(test)]
pub fn drifts_from_installed_map(
    held: &HeldPackage,
    installed: &HashMap<String, Option<String>>,
) -> Option<TriggerDrift> {
    let mut changed = Vec::new();
    for (trig, saved) in &held.auto_recompile_trigger.on_packages_updated {
        let current = installed.get(trig).cloned().unwrap_or(None);
        let drifted = match &current {
            Some(inst) => inst != saved,
            None => true,
        };
        if drifted {
            changed.push((trig.clone(), saved.clone(), current));
        }
    }
    if changed.is_empty() {
        None
    } else {
        Some(TriggerDrift {
            held_name: held.name.clone(),
            held_version: held.version.clone(),
            changed,
        })
    }
}

/// Build PackageSpecs for held packages that need recompile due to trigger drift.
pub fn specs_for_trigger_drifts(drifts: &[TriggerDrift]) -> Vec<PackageSpec> {
    let mut seen = HashSet::new();
    let mut specs = Vec::new();
    for d in drifts {
        if seen.insert(d.held_name.clone()) {
            let mut spec = PackageSpec::plain(&d.held_name);
            if let Ok((pkgver, pkgrel)) = split_pkgver_pkgrel(&d.held_version) {
                spec.pkgbuild_overrides.insert("pkgver".to_string(), pkgver);
                spec.pkgbuild_overrides.insert("pkgrel".to_string(), pkgrel);
            }
            specs.push(spec);
        }
    }
    specs
}

/// Record current installed versions for trigger package names (skip missing).
pub fn snapshot_trigger_versions(trigger_pkgs: &[String]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pkg in trigger_pkgs {
        if let Ok(Some(ver)) = pacman_query_version(pkg) {
            map.insert(pkg.clone(), ver);
        }
    }
    map
}

/// Collect trigger package names across all holds.
pub fn all_trigger_names(config: &Config) -> HashSet<String> {
    let mut set = HashSet::new();
    for held in &config.held_packages {
        for name in held.auto_recompile_trigger.on_packages_updated.keys() {
            set.insert(name.clone());
        }
    }
    set
}

pub fn make_held_package(
    name: String,
    version: String,
    triggers: HashMap<String, String>,
) -> HeldPackage {
    HeldPackage {
        name,
        version,
        auto_recompile_trigger: AutoRecompileTrigger {
            on_packages_updated: triggers,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AutoRecompileTrigger;

    #[test]
    fn split_simple_version() {
        assert_eq!(
            split_pkgver_pkgrel("1.2.3-1").unwrap(),
            ("1.2.3".into(), "1".into())
        );
    }

    #[test]
    fn split_epoch_version() {
        assert_eq!(
            split_pkgver_pkgrel("1:0.56.1-1").unwrap(),
            ("1:0.56.1".into(), "1".into())
        );
    }

    #[test]
    fn split_rejects_missing_pkgrel() {
        assert!(split_pkgver_pkgrel("1.2.3").is_err());
        assert!(split_pkgver_pkgrel("").is_err());
        assert!(split_pkgver_pkgrel("-1").is_err());
    }

    #[test]
    fn drift_detects_upgrade_and_downgrade() {
        let held = HeldPackage {
            name: "libfoo".into(),
            version: "1.0.0-1".into(),
            auto_recompile_trigger: AutoRecompileTrigger {
                on_packages_updated: HashMap::from([
                    ("glibc".into(), "2.40-1".into()),
                    ("icu".into(), "76.1-1".into()),
                ]),
            },
        };
        let installed = HashMap::from([
            ("glibc".into(), Some("2.41-1".into())),
            ("icu".into(), Some("75.1-1".into())),
        ]);
        let drift = drifts_from_installed_map(&held, &installed).unwrap();
        assert_eq!(drift.held_name, "libfoo");
        assert_eq!(drift.changed.len(), 2);
    }

    #[test]
    fn no_drift_when_versions_match() {
        let held = HeldPackage {
            name: "libfoo".into(),
            version: "1.0.0-1".into(),
            auto_recompile_trigger: AutoRecompileTrigger {
                on_packages_updated: HashMap::from([("glibc".into(), "2.40-1".into())]),
            },
        };
        let installed = HashMap::from([("glibc".into(), Some("2.40-1".into()))]);
        assert!(drifts_from_installed_map(&held, &installed).is_none());
    }

    #[test]
    fn drift_when_trigger_uninstalled() {
        let held = HeldPackage {
            name: "libfoo".into(),
            version: "1.0.0-1".into(),
            auto_recompile_trigger: AutoRecompileTrigger {
                on_packages_updated: HashMap::from([("glibc".into(), "2.40-1".into())]),
            },
        };
        let installed = HashMap::from([("glibc".into(), None)]);
        assert!(drifts_from_installed_map(&held, &installed).is_some());
    }

    #[test]
    fn apply_overrides_skips_existing() {
        let mut config: Config = toml::from_str(
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

[system_update]
command_to_update_repositories = "pacman -Su"
command_to_perform_system_update = "pacman -Syu"
ignore_flag = "--ignore"
ignore_packages = []

[repositories]
default = "arch"

[packages]
"#,
        )
        .unwrap();
        config.held_packages = vec![HeldPackage {
            name: "foo".into(),
            version: "9.9.9-2".into(),
            auto_recompile_trigger: AutoRecompileTrigger::default(),
        }];
        let mut spec = PackageSpec::plain("foo");
        spec.pkgbuild_overrides
            .insert("pkgver".into(), "1.0.0".into());
        apply_held_overrides_to_spec(&mut spec, &config);
        assert_eq!(spec.pkgbuild_overrides.get("pkgver").unwrap(), "1.0.0");
        assert_eq!(spec.pkgbuild_overrides.get("pkgrel").unwrap(), "2");
    }
}
