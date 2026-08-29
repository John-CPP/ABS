use crate::build::resolve_pkg_repo_for_manual;
use crate::cli::Cli;
use crate::config::Config;
use crate::git::prepare_repo;
use crate::package_spec::PackageSpec;
use crate::pkgbuild::parse_pkg_dependencies;
use crate::ramdisk;
use crate::vlog;
use std::collections::{HashMap, HashSet, VecDeque};

/// Sort the package specifications topologically based on their PKGBUILD dependencies
pub fn sort_packages_topologically(
    packages: &[PackageSpec],
    cli: &Cli,
    config: &Config,
) -> Result<Vec<PackageSpec>, String> {
    if packages.len() <= 1 {
        return Ok(packages.to_vec());
    }

    vlog!("Topological Sort: Building package dependency graph...");

    // Map base package name -> PackageSpec
    let mut base_to_spec = HashMap::new();
    // Map pkg_spec name -> base package name
    let mut spec_name_to_base = HashMap::new();

    for spec in packages {
        let (_, _, base_pkg) = crate::build::resolve_pkg_repo_for_manual(&spec.name, cli, config);
        base_to_spec.insert(base_pkg.clone(), spec.clone());
        spec_name_to_base.insert(spec.name.clone(), base_pkg);
    }

    // Graph representation: adjacency list (node -> list of neighbors it depends on)
    let mut graph: HashMap<String, HashSet<String>> = HashMap::new();
    // Keep track of incoming edges count (number of packages depending on this package)
    let mut indegree: HashMap<String, usize> = HashMap::new();

    // Initialize graph
    for base in base_to_spec.keys() {
        graph.insert(base.clone(), HashSet::new());
        indegree.insert(base.clone(), 0);
    }

    // Build the dependency graph. Edges resolve through pkgname/provides so a job keyed by
    // pkgbase still waits on dependents that list a split package (e.g. `foo-libs`).
    let mut provider_to_base: HashMap<String, String> = HashMap::new();
    let mut pkg_dirs: HashMap<String, std::path::PathBuf> = HashMap::new();

    for (base, spec) in &base_to_spec {
        let (repo_name, repo_url_string, base_pkg) =
            resolve_pkg_repo_for_manual(&spec.name, cli, config);

        // Fast, read-only prepare repo (does not clone/pull if it exists; smart dry-run bypasses commands)
        let pkg_config = config.packages.get(&spec.name);
        let targets = ramdisk::resolve_ramdisk_targets(
            config,
            pkg_config,
            Some(spec),
            cli.ramdisk.as_deref(),
        )
        .unwrap_or_default();
        let pkg_dir = prepare_repo(
            &spec.name,
            &base_pkg,
            &repo_name,
            &repo_url_string,
            &ramdisk::download_packages_path(config, &targets),
            false,
            false,
            None,
        )
        .pkg_dir;

        // Always map the job's base key itself.
        provider_to_base.insert(base.clone(), base.clone());
        for provided in crate::pkgbuild::parse_pkg_provided_names(pkg_dir.as_path()) {
            provider_to_base.insert(provided, base.clone());
        }
        pkg_dirs.insert(base.clone(), pkg_dir);
    }

    for (base, pkg_dir) in &pkg_dirs {
        let deps = parse_pkg_dependencies(pkg_dir.as_path());
        vlog!("Topological Sort: {} has dependencies {:?}", base, deps);

        for dep in deps {
            let Some(dep_base) = provider_to_base.get(&dep) else {
                continue;
            };
            if dep_base == base {
                continue;
            }
            // A depends on dep (dep must be built first)
            let dep_edges = graph
                .get_mut(dep_base)
                .expect("Internal error: target package missing from graph");
            if dep_edges.insert(base.clone()) {
                *indegree
                    .get_mut(base)
                    .expect("Internal error: target package missing from indegree map") += 1;
            }
        }
    }

    // Kahn's algorithm for topological sorting
    let mut queue = VecDeque::new();
    for (node, &deg) in &indegree {
        if deg == 0 {
            queue.push_back(node.clone());
        }
    }

    let mut sorted_base_names = Vec::new();
    while let Some(node) = queue.pop_front() {
        sorted_base_names.push(node.clone());
        if let Some(neighbors) = graph.get(&node) {
            for neighbor in neighbors {
                let deg = indegree
                    .get_mut(neighbor)
                    .expect("Internal error: package missing from indegree map");
                *deg -= 1;
                if *deg == 0 {
                    queue.push_back(neighbor.clone());
                }
            }
        }
    }

    if sorted_base_names.len() != base_to_spec.len() {
        // Find cyclic dependencies for reporting
        let mut cyclic = Vec::new();
        for (node, &deg) in &indegree {
            if deg > 0 {
                cyclic.push(node.clone());
            }
        }
        return Err(format!(
            "Cyclic dependency detected among packages: {:?}. Cannot determine safe build order.",
            cyclic
        ));
    }

    // Map sorted base names back to their original PackageSpecs in order
    let mut sorted_specs = Vec::new();
    for base in sorted_base_names {
        if let Some(spec) = base_to_spec.remove(&base) {
            sorted_specs.push(spec);
        }
    }

    vlog!(
        "Topological Sort: Determined safe build order: {:?}",
        sorted_specs.iter().map(|s| &s.name).collect::<Vec<_>>()
    );

    Ok(sorted_specs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn init_bare_pkg_dir(dir: &std::path::Path, srcinfo: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(".SRCINFO"), srcinfo).unwrap();
        // prepare_repo treats dirs without a usable .git as incomplete clones and tries to
        // delete them; seed a minimal git repo so unit tests stay read-only.
        let status = Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir)
            .status()
            .expect("git init");
        assert!(status.success());
    }

    #[test]
    fn test_topological_sort_success() {
        let temp = std::env::temp_dir().join(format!(
            "abs_test_topo_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let pkg_a_dir = temp.join("aur").join("package-a");
        let pkg_b_dir = temp.join("aur").join("package-b");
        let pkg_c_dir = temp.join("aur").join("package-c");

        init_bare_pkg_dir(&pkg_a_dir, "pkgname = package-a\ndepends = package-b\n");
        init_bare_pkg_dir(&pkg_b_dir, "pkgname = package-b\ndepends = package-c\n");
        init_bare_pkg_dir(&pkg_c_dir, "pkgname = package-c\n");

        let config_content = format!(
            "config_version = 1\nmanual_update_packages = []\nskip_install_packages = []\n\n[paths]\npackages_path = \"{}\"\nchroot_base_path = \"\"\nready_made_packages_path = \"\"\n\n[build]\ndefault_environment = \"local\"\n\n[system_update]\ncommand_to_update_repositories = \"\"\ncommand_to_perform_system_update = \"\"\nignore_flag = \"\"\nignore_packages = []\n\n[repositories]\naur = \"https://aur.archlinux.org\"\ndefault = \"aur\"\n\n[packages]\n",
            temp.to_str().unwrap().escape_default()
        );
        let config: Config = toml::from_str(&config_content).unwrap();

        let cli = Cli {
            download_only: false,
            local_build: false,
            chroot_build: false,
            compile_only: false,
            no_check: false,
            force_build: false,
            clean: false,
            clean_all: false,
            use_sudo_clean: false,
            remove_chroot: false,
            install_keys: false,
            update_sums: false,
            verbose: false,
            silent: false,
            force_repo_update: false,
            system_update: false,
            repo: Some("aur".to_string()),
            jobs: None,
            install_only: false,
            clean_install: false,
            dry_run: true,
            list: false,
            configure: None,
            check_update: false,
            self_update: false,
            help: None,
            ramdisk: None,
            packages: vec![],
            pgo: None,
            pgo_resume: None,
            pgo_status: None,
            pgo_abort: None,
            pgo_keep_stage: false,
            pgo_restart: None,
            pgo_stage: None,
            pgo_once: false,
            pgo_goto: false,
            pgo_auto: false,
            kernel_build: None,
            ramdisk_shutdown: false,
            json: false,
            event_log: None,
            purge: false,
            purge_abs_install: false,
            yes: false,
            no_wait: false,
            list_add: None,
            list_remove: None,
            config_wizard: false,
            config_wizard_form: false,
            config_wizard_check: false,
            config_wizard_apply: false,
            wizard: None,
            pkg_list: None,
            hold: None,
            hold_version: None,
            unhold: vec![],
            hold_check: false,
            trigger: vec![],
            pending_updates: false,
            install_repo_updates: false,
            install_aur: None,
            install_os_package: None,
            set_default_lang: None,
        };

        let specs = vec![
            PackageSpec::plain("package-a"),
            PackageSpec::plain("package-b"),
            PackageSpec::plain("package-c"),
        ];

        let sorted = sort_packages_topologically(&specs, &cli, &config).unwrap();
        let sorted_names: Vec<String> = sorted.iter().map(|s| s.name.clone()).collect();

        assert_eq!(sorted_names, vec!["package-c", "package-b", "package-a"]);

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_topological_sort_cycle() {
        let temp = std::env::temp_dir().join(format!(
            "abs_test_topo_cycle_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let pkg_a_dir = temp.join("aur").join("package-a");
        let pkg_b_dir = temp.join("aur").join("package-b");

        init_bare_pkg_dir(&pkg_a_dir, "pkgname = package-a\ndepends = package-b\n");
        init_bare_pkg_dir(&pkg_b_dir, "pkgname = package-b\ndepends = package-a\n");

        let config_content = format!(
            "config_version = 1\nmanual_update_packages = []\nskip_install_packages = []\n\n[paths]\npackages_path = \"{}\"\nchroot_base_path = \"\"\nready_made_packages_path = \"\"\n\n[build]\ndefault_environment = \"local\"\n\n[system_update]\ncommand_to_update_repositories = \"\"\ncommand_to_perform_system_update = \"\"\nignore_flag = \"\"\nignore_packages = []\n\n[repositories]\naur = \"https://aur.archlinux.org\"\ndefault = \"aur\"\n\n[packages]\n",
            temp.to_str().unwrap().escape_default()
        );
        let config: Config = toml::from_str(&config_content).unwrap();

        let cli = Cli {
            download_only: false,
            local_build: false,
            chroot_build: false,
            compile_only: false,
            no_check: false,
            force_build: false,
            clean: false,
            clean_all: false,
            use_sudo_clean: false,
            remove_chroot: false,
            install_keys: false,
            update_sums: false,
            verbose: false,
            silent: false,
            force_repo_update: false,
            system_update: false,
            repo: Some("aur".to_string()),
            jobs: None,
            install_only: false,
            clean_install: false,
            dry_run: true,
            list: false,
            configure: None,
            check_update: false,
            self_update: false,
            help: None,
            ramdisk: None,
            packages: vec![],
            pgo: None,
            pgo_resume: None,
            pgo_status: None,
            pgo_abort: None,
            pgo_keep_stage: false,
            pgo_restart: None,
            pgo_stage: None,
            pgo_once: false,
            pgo_goto: false,
            pgo_auto: false,
            kernel_build: None,
            ramdisk_shutdown: false,
            json: false,
            event_log: None,
            purge: false,
            purge_abs_install: false,
            yes: false,
            no_wait: false,
            list_add: None,
            list_remove: None,
            config_wizard: false,
            config_wizard_form: false,
            config_wizard_check: false,
            config_wizard_apply: false,
            wizard: None,
            pkg_list: None,
            hold: None,
            hold_version: None,
            unhold: vec![],
            hold_check: false,
            trigger: vec![],
            pending_updates: false,
            install_repo_updates: false,
            install_aur: None,
            install_os_package: None,
            set_default_lang: None,
        };

        let specs = vec![
            PackageSpec::plain("package-a"),
            PackageSpec::plain("package-b"),
        ];

        let result = sort_packages_topologically(&specs, &cli, &config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Cyclic dependency detected"));

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn topological_sort_resolves_deps_via_pkgname_provides() {
        let temp = std::env::temp_dir().join(format!(
            "abs_test_topo_provides_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let lib_dir = temp.join("aur").join("foo");
        let app_dir = temp.join("aur").join("bar");

        // Job keyed by pkgbase `foo` provides split package `foo-libs`.
        init_bare_pkg_dir(
            &lib_dir,
            "pkgbase = foo\npkgname = foo\npkgname = foo-libs\n",
        );
        init_bare_pkg_dir(&app_dir, "pkgname = bar\ndepends = foo-libs\n");

        let config_content = format!(
            "config_version = 1\nmanual_update_packages = []\nskip_install_packages = []\n\n[paths]\npackages_path = \"{}\"\nchroot_base_path = \"\"\nready_made_packages_path = \"\"\n\n[build]\ndefault_environment = \"local\"\n\n[system_update]\ncommand_to_update_repositories = \"\"\ncommand_to_perform_system_update = \"\"\nignore_flag = \"\"\nignore_packages = []\n\n[repositories]\naur = \"https://aur.archlinux.org\"\ndefault = \"aur\"\n\n[packages]\n",
            temp.to_str().unwrap().escape_default()
        );
        let config: Config = toml::from_str(&config_content).unwrap();
        let cli = Cli {
            download_only: false,
            local_build: false,
            chroot_build: false,
            compile_only: false,
            no_check: false,
            force_build: false,
            clean: false,
            clean_all: false,
            use_sudo_clean: false,
            remove_chroot: false,
            install_keys: false,
            update_sums: false,
            verbose: false,
            silent: false,
            force_repo_update: false,
            system_update: false,
            repo: Some("aur".to_string()),
            jobs: None,
            install_only: false,
            clean_install: false,
            dry_run: true,
            list: false,
            configure: None,
            check_update: false,
            self_update: false,
            help: None,
            ramdisk: None,
            packages: vec![],
            pgo: None,
            pgo_resume: None,
            pgo_status: None,
            pgo_abort: None,
            pgo_keep_stage: false,
            pgo_restart: None,
            pgo_stage: None,
            pgo_once: false,
            pgo_goto: false,
            pgo_auto: false,
            kernel_build: None,
            ramdisk_shutdown: false,
            json: false,
            event_log: None,
            purge: false,
            purge_abs_install: false,
            yes: false,
            no_wait: false,
            list_add: None,
            list_remove: None,
            config_wizard: false,
            config_wizard_form: false,
            config_wizard_check: false,
            config_wizard_apply: false,
            wizard: None,
            pkg_list: None,
            hold: None,
            hold_version: None,
            unhold: vec![],
            hold_check: false,
            trigger: vec![],
            pending_updates: false,
            install_repo_updates: false,
            install_aur: None,
            install_os_package: None,
            set_default_lang: None,
        };

        let specs = vec![PackageSpec::plain("bar"), PackageSpec::plain("foo")];
        let sorted = sort_packages_topologically(&specs, &cli, &config).unwrap();
        let sorted_names: Vec<String> = sorted.iter().map(|s| s.name.clone()).collect();
        assert_eq!(sorted_names, vec!["foo", "bar"]);

        let _ = fs::remove_dir_all(&temp);
    }
}
