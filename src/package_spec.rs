use crate::die;
use colored::Colorize;
use std::collections::HashMap;

/// Per-package request parsed from CLI positional args such as `xray[repo=aur,pkgver=26.5.9]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSpec {
    pub name: String,
    pub pkgbuild_overrides: HashMap<String, String>,
    /// Kernel build variables (e.g. `_use_llvm_lto`) baked into the PKGBUILD. Unlike
    /// `pkgbuild_overrides`, these never trigger `updpkgsums` since they do not affect sources.
    pub kernel_vars: HashMap<String, String>,
    pub repo: Option<String>,
    pub local_build: Option<bool>,
    pub chroot_build: Option<bool>,
    pub no_check: Option<bool>,
    pub compiler: Option<String>,
    /// CLI override for ramdisk targets (`w`, `c`, `p`, `r`) or `disabled`, e.g. `mesa[ramdisk=wcp]`.
    pub ramdisk: Option<String>,
}

impl PackageSpec {
    pub fn plain(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            pkgbuild_overrides: HashMap::new(),
            kernel_vars: HashMap::new(),
            repo: None,
            local_build: None,
            chroot_build: None,
            no_check: None,
            compiler: None,
            ramdisk: None,
        }
    }
}

fn normalize_repo_name(name: &str) -> String {
    name.to_ascii_lowercase()
}

fn merge_ramdisk_code(existing: &str, addition: &str) -> String {
    if crate::ramdisk::is_ramdisk_disabled(existing) || crate::ramdisk::is_ramdisk_disabled(addition) {
        return crate::ramdisk::RAMDISK_DISABLED.to_string();
    }
    let mut targets = crate::ramdisk::parse_ramdisk_targets(existing).unwrap_or_default();
    let add = crate::ramdisk::parse_ramdisk_targets(addition).unwrap_or_default();
    targets.build_workdir |= add.build_workdir;
    targets.chroot |= add.chroot;
    targets.packages |= add.packages;
    targets.profiles |= add.profiles;
    crate::ramdisk::format_ramdisk_targets(&targets)
}

fn normalize_ramdisk_code(value: &str) -> String {
    if crate::ramdisk::is_ramdisk_disabled(value) {
        return crate::ramdisk::RAMDISK_DISABLED.to_string();
    }
    crate::ramdisk::parse_ramdisk_targets(value)
        .map(|t| crate::ramdisk::format_ramdisk_targets(&t))
        .unwrap_or_default()
}

fn is_ramdisk_target_token(part: &str) -> bool {
    !part.is_empty()
        && !part.contains('=')
        && crate::ramdisk::parse_ramdisk_targets(part)
            .map(|t| t.any())
            .unwrap_or(false)
}

fn parse_attr(key: &str, value: &str, spec: &mut PackageSpec) {
    let key_lower = key.to_ascii_lowercase();
    let value = value.trim();

    match key_lower.as_str() {
        "repo" => {
            if value.is_empty() {
                die!("Package '{}': [repo] requires a value (e.g. repo=aur)", spec.name);
            }
            spec.repo = Some(normalize_repo_name(value));
        }
        "local" => {
            spec.local_build = Some(parse_bool_flag(value));
            if spec.chroot_build == Some(true) {
                die!(
                    "Package '{}': cannot set both [local] and [chroot] build options",
                    spec.name
                );
            }
        }
        "chroot" => {
            spec.chroot_build = Some(parse_bool_flag(value));
            if spec.local_build == Some(true) {
                die!(
                    "Package '{}': cannot set both [local] and [chroot] build options",
                    spec.name
                );
            }
        }
        "build" => match value.to_ascii_lowercase().as_str() {
            "local" => {
                spec.local_build = Some(true);
                spec.chroot_build = None;
            }
            "chroot" => {
                spec.chroot_build = Some(true);
                spec.local_build = None;
            }
            other => die!(
                "Package '{}': invalid [build={}] (expected local or chroot)",
                spec.name, other
            ),
        },
        "nocheck" | "no_check" => spec.no_check = Some(parse_bool_flag(value)),
        "compiler" => {
            if value.is_empty() {
                die!("Package '{}': [compiler] requires a value (e.g. compiler=llvm)", spec.name);
            }
            spec.compiler = Some(value.to_string());
        }
        "ramdisk" => {
            if value.is_empty() {
                die!(
                    "Package '{}': [ramdisk] requires a value (e.g. ramdisk=wr, ramdisk=disabled; w=workdir, c=chroot, p=packages, r=profiles)",
                    spec.name
                );
            }
            spec.ramdisk = Some(normalize_ramdisk_code(value));
        }
        _ => {
            if value.is_empty() {
                die!(
                    "Package '{}': [{}] requires a value (e.g. {}=26.5.9)",
                    spec.name, key, key
                );
            }
            spec.pkgbuild_overrides
                .insert(key.to_string(), value.to_string());
        }
    }
}

fn parse_bool_flag(value: &str) -> bool {
    value.is_empty()
        || value == "1"
        || value.eq_ignore_ascii_case("true")
        || value.eq_ignore_ascii_case("yes")
        || value.eq_ignore_ascii_case("on")
}

/// Parse `pkgname`, `pkgname[key=value,...]`, or `pkgname[flag,key=value]`.
pub fn parse_package_spec(input: &str) -> PackageSpec {
    let input = input.trim();
    if input.is_empty() {
        die!("Empty package name");
    }

    let Some(open) = input.find('[') else {
        return PackageSpec::plain(input);
    };

    if !input.ends_with(']') {
        die!(
            "Invalid package spec '{}': missing closing ']' (e.g. pkg[pkgver=1.0])",
            input
        );
    }

    let name = input[..open].trim();
    if name.is_empty() {
        die!("Invalid package spec '{}': missing package name before '['", input);
    }

    let inner = input[open + 1..input.len() - 1].trim();
    let mut spec = PackageSpec::plain(name);

    if inner.is_empty() {
        return spec;
    }

    for part in inner.split(['/', ',']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((key, value)) = part.split_once('=') {
            parse_attr(key.trim(), value, &mut spec);
        } else if crate::ramdisk::is_ramdisk_disabled(part) {
            spec.ramdisk = Some(crate::ramdisk::RAMDISK_DISABLED.to_string());
        } else if is_ramdisk_target_token(part) {
            let code = normalize_ramdisk_code(part);
            spec.ramdisk = Some(match spec.ramdisk.take() {
                Some(existing) => merge_ramdisk_code(&existing, &code),
                None => code,
            });
        } else {
            parse_attr(part, "", &mut spec);
        }
    }

    spec
}

pub fn parse_package_specs(packages: &[String]) -> Vec<PackageSpec> {
    packages.iter().map(|p| parse_package_spec(p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain_name() {
        let spec = parse_package_spec("mesa");
        assert_eq!(spec.name, "mesa");
        assert!(spec.pkgbuild_overrides.is_empty());
    }

    #[test]
    fn parse_pkgbuild_overrides() {
        let spec = parse_package_spec("xray[pkgver=26.5.9,pkgrel=2]");
        assert_eq!(spec.name, "xray");
        assert_eq!(spec.pkgbuild_overrides.get("pkgver").map(String::as_str), Some("26.5.9"));
        assert_eq!(spec.pkgbuild_overrides.get("pkgrel").map(String::as_str), Some("2"));
    }

    #[test]
    fn parse_slash_separated_overrides() {
        let spec = parse_package_spec("xray[pkgver=26.5.9/pkgrel=2]");
        assert_eq!(spec.pkgbuild_overrides.get("pkgver").map(String::as_str), Some("26.5.9"));
        assert_eq!(spec.pkgbuild_overrides.get("pkgrel").map(String::as_str), Some("2"));
    }

    #[test]
    fn parse_per_package_repo_and_flags() {
        let spec = parse_package_spec("xray[repo=aur,chroot,pkgver=1.0]");
        assert_eq!(spec.repo.as_deref(), Some("aur"));
        assert_eq!(spec.chroot_build, Some(true));
        assert_eq!(spec.pkgbuild_overrides.get("pkgver").map(String::as_str), Some("1.0"));
    }

    #[test]
    fn parse_compiler_override() {
        let spec = parse_package_spec("mesa[compiler=llvm17]");
        assert_eq!(spec.name, "mesa");
        assert_eq!(spec.compiler.as_deref(), Some("llvm17"));
        assert!(spec.pkgbuild_overrides.is_empty());
    }

    #[test]
    fn parse_ramdisk_targets_bracket() {
        let spec = parse_package_spec("mesa[ramdisk=wcp]");
        assert_eq!(spec.ramdisk.as_deref(), Some("wcp"));
        let spec = parse_package_spec("mesa[wcp]");
        assert_eq!(spec.ramdisk.as_deref(), Some("wcp"));
        let spec = parse_package_spec("mesa[w,c,p]");
        assert_eq!(spec.ramdisk.as_deref(), Some("wcp"));
        let spec = parse_package_spec("linux-cachyos[wr]");
        assert_eq!(spec.ramdisk.as_deref(), Some("wr"));
        let spec = parse_package_spec("linux-cachyos[ramdisk=wr]");
        assert_eq!(spec.ramdisk.as_deref(), Some("wr"));
    }

    #[test]
    fn parse_ramdisk_disabled_bracket() {
        let spec = parse_package_spec("electron40[ramdisk=disabled]");
        assert_eq!(spec.ramdisk.as_deref(), Some("disabled"));
        let spec = parse_package_spec("electron40[disabled]");
        assert_eq!(spec.ramdisk.as_deref(), Some("disabled"));
        let spec = parse_package_spec("electron40[ramdisk=disabled,wcp]");
        assert_eq!(spec.ramdisk.as_deref(), Some("disabled"));
    }
}
