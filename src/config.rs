use crate::die;
use colored::Colorize;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub paths: PathsConfig,
    pub build: BuildConfig,
    pub system_update: SystemUpdateConfig,
    pub repositories: HashMap<String, String>,
    pub manual_update_packages: Vec<String>,
    pub skip_install_packages: Vec<String>,
    pub packages: HashMap<String, PackageConfig>,
}

#[derive(Debug, Deserialize)]
pub struct PathsConfig {
    pub packages_path: String,
    pub chroot_base_path: String,
    pub ready_made_packages_path: String,
}

#[derive(Debug, Deserialize)]
pub struct BuildConfig {
    pub default_environment: String,
}

#[derive(Debug, Deserialize)]
pub struct SystemUpdateConfig {
    pub command: String,
    pub command_with_refresh: String,
    pub ignore_flag: String,
    pub ignore_packages: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct PackageConfig {
    pub source: Option<String>,
    pub build_env: Option<String>,
    pub tests: Option<bool>,
    pub alias: Option<String>,
    pub custom_local_build_command: Option<String>,
    pub custom_chroot_build_command: Option<String>,
    pub pre_update_command: Option<String>,
    pub post_update_command: Option<String>,
}

impl Config {
    pub fn load_config() -> Config {
        // Same order as README: XDG config dir, then /etc, then ./emerge.toml (cwd).
        let xdg_config = dirs::config_dir().map(|d| d.join("arch-emerge").join("emerge.toml"));
        let etc_config = PathBuf::from("/etc/arch-emerge/emerge.toml");
        let cwd_config = PathBuf::from("emerge.toml");

        let config_path = match xdg_config {
            Some(p) if p.exists() => p,
            _ => {
                if etc_config.exists() {
                    etc_config
                } else {
                    cwd_config
                }
            }
        };

        let config_content = match fs::read_to_string(&config_path) {
            Ok(c) => c,
            Err(_) => {
                die!("Failed to read config file at {:?}", config_path);
            }
        };

        let config: Config = match toml::from_str(&config_content) {
            Ok(c) => c,
            Err(e) => {
                die!("Failed to parse config '{:?}': {}", config_path, e);
            }
        };
        config.validate();
        config
    }

    fn validate(&self) {
        let env = self.build.default_environment.as_str();
        if env != "local" && env != "chroot" {
            die!(
                "Invalid [build] default_environment: {:?} (expected \"local\" or \"chroot\")",
                env
            );
        }
        for (pkg_name, pkg) in &self.packages {
            if let Some(be) = &pkg.build_env {
                let be = be.as_str();
                if be != "local" && be != "chroot" {
                    die!(
                        "Invalid build_env for package {:?}: {:?} (expected \"local\" or \"chroot\")",
                        pkg_name,
                        be
                    );
                }
            }
        }
    }

    pub fn print_human_readable(&self) {
        println!("{}", "Arch-Emerge Configuration".blue().bold());
        println!("{}", "-------------------------".blue());

        println!("\n{}", "Paths".green().bold());
        println!("  packages_path: {}", self.paths.packages_path);
        println!("  chroot_base_path: {}", self.paths.chroot_base_path);
        println!(
            "  ready_made_packages_path: {}",
            self.paths.ready_made_packages_path
        );

        println!("\n{}", "Build".green().bold());
        println!("  default_environment: {}", self.build.default_environment);

        println!("\n{}", "System Update".green().bold());
        println!("  command: {}", self.system_update.command);
        println!(
            "  command_with_refresh: {}",
            self.system_update.command_with_refresh
        );
        println!("  ignore_flag: {}", self.system_update.ignore_flag);
        if self.system_update.ignore_packages.is_empty() {
            println!("  ignore_packages: (none)");
        } else {
            println!("  ignore_packages:");
            for pkg in &self.system_update.ignore_packages {
                println!("    - {}", pkg);
            }
        }

        println!("\n{}", "Repositories".green().bold());
        let mut repo_entries: Vec<_> = self.repositories.iter().collect();
        let default_entry = repo_entries
            .iter()
            .position(|(name, _)| *name == "default")
            .map(|i| repo_entries.swap_remove(i));
        repo_entries.sort_by(|a, b| a.0.cmp(b.0));
        if let Some((name, url)) = default_entry {
            println!("  {} -> {}", name, url);
        }
        for (name, url) in repo_entries {
            println!("  {} -> {}", name, url);
        }

        println!("\n{}", "Manual Update Packages".green().bold());
        if self.manual_update_packages.is_empty() {
            println!("  (none)");
        } else {
            for pkg in &self.manual_update_packages {
                println!("  - {}", pkg);
            }
        }

        println!("\n{}", "Skip Install Packages".green().bold());
        if self.skip_install_packages.is_empty() {
            println!("  (none)");
        } else {
            for pkg in &self.skip_install_packages {
                println!("  - {}", pkg);
            }
        }

        println!("\n{}", "Package Profiles".green().bold());
        let mut pkg_entries: Vec<_> = self.packages.iter().collect();
        pkg_entries.sort_by(|a, b| a.0.cmp(b.0));
        for (name, cfg) in pkg_entries {
            println!("  {}", format!("- {}", name).bold());
            let mut profile_line = format!(
                "    source={} build_env={} tests={}",
                cfg.source.as_deref().unwrap_or("-"),
                cfg.build_env.as_deref().unwrap_or("-"),
                cfg.tests
                    .map(|v| if v { "on" } else { "off" })
                    .unwrap_or("-"),
            );
            if let Some(alias) = &cfg.alias {
                profile_line.push_str(&format!(" alias={}", alias));
            }
            println!("{}", profile_line);
            if let Some(cmd) = &cfg.custom_local_build_command {
                println!("    custom_local_build_command: {}", cmd);
            }
            if let Some(cmd) = &cfg.custom_chroot_build_command {
                println!("    custom_chroot_build_command: {}", cmd);
            }
            if let Some(cmd) = &cfg.pre_update_command {
                println!("    pre_update_command: {}", cmd);
            }
            if let Some(cmd) = &cfg.post_update_command {
                println!("    post_update_command: {}", cmd);
            }
        }
    }
}
