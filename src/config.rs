use crate::die;
use crate::utils::{run_command, sh_single_quote};
use colored::Colorize;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default = "default_config_version")]
    pub config_version: u32,
    pub paths: PathsConfig,
    pub build: BuildConfig,
    pub system_update: SystemUpdateConfig,
    pub repositories: HashMap<String, String>,
    pub manual_update_packages: Vec<String>,
    pub skip_install_packages: Vec<String>,
    pub packages: HashMap<String, PackageConfig>,
    #[serde(default = "default_check_for_update_on_startup")]
    pub check_for_update_on_startup: bool,
    #[serde(default = "default_auto_update_on_startup")]
    pub auto_update_on_startup: bool,
    #[serde(default = "default_self_update_github_url")]
    pub self_update_github_url: String,
    #[serde(default = "default_self_update_raw_url")]
    pub self_update_raw_url: String,
    #[serde(default = "default_self_update_install_path")]
    pub self_update_install_path: String,
    #[serde(default = "default_self_update_at_updates")]
    pub self_update_at_updates: bool,
}

fn default_config_version() -> u32 {
    1
}

fn default_check_for_update_on_startup() -> bool {
    true
}

fn default_auto_update_on_startup() -> bool {
    false
}

fn default_self_update_at_updates() -> bool {
    false
}

fn default_self_update_github_url() -> String {
    "https://github.com/John-CPP/ABS".to_string()
}

fn default_self_update_install_path() -> String {
    "/usr/bin/abs".to_string()
}

fn default_self_update_raw_url() -> String {
    "https://raw.githubusercontent.com/John-CPP/ABS/master/Cargo.toml".to_string()
}

#[derive(Debug, Deserialize)]
pub struct PathsConfig {
    pub packages_path: String,
    pub chroot_base_path: String,
    pub ready_made_packages_path: String,
    #[serde(default)]
    pub chroot_makepkg_conf: Option<String>,
}

fn default_concurrent_repos_downloads_limit() -> usize {
    10
}

fn default_concurrent_compilations_limit() -> usize {
    1
}

fn default_fast_aur_rpc_update_checks() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct BuildConfig {
    pub default_environment: String,
    /// Continue with the next package when a build fails instead of exiting.
    #[serde(default, alias = "IGNORE_COMPILATION_FAILURES")]
    pub ignore_compilation_failures: bool,
    /// Build every scheduled package first, then run install prompts (so long unattended compile runs finish before any questions).
    #[serde(default, alias = "COMPILE_FIRST_INSTALL_AFTER")]
    pub compile_first_install_after: bool,
    /// Before **`makepkg`**, remove **`src/`** and **`pkg/`** in the package directory. **`--clean-install`** enables the same for that invocation even when this is false.
    #[serde(default)]
    pub clean_install_by_default: bool,
    /// Maximum number of repository directories to sync concurrently.
    #[serde(default = "default_concurrent_repos_downloads_limit")]
    pub concurrent_repos_downloads_limit: usize,
    /// Maximum number of clean chroot compilations to run concurrently.
    #[serde(default = "default_concurrent_compilations_limit")]
    pub concurrent_compilations_limit: usize,
    /// Whether to check AUR package versions using the AUR RPC API in batch.
    #[serde(default = "default_fast_aur_rpc_update_checks")]
    pub fast_aur_rpc_update_checks: bool,

    // Optional self-update fields for backwards-compatibility/placement under [build]
    pub check_for_update_on_startup: Option<bool>,
    pub auto_update_on_startup: Option<bool>,
    pub self_update_at_updates: Option<bool>,
    pub self_update_github_url: Option<String>,
    pub self_update_raw_url: Option<String>,
    pub self_update_install_path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SystemUpdateConfig {
    /// Shown with **`-R`** / **`-U`** (no full refresh). TOML key: `command_to_update_repositories`
    /// (alias: `command`).
    #[serde(alias = "command")]
    pub command_to_update_repositories: String,
    /// Shown with **`-RU`**. TOML key: `command_to_perform_system_update` (alias: `command_with_refresh`).
    #[serde(alias = "command_with_refresh")]
    pub command_to_perform_system_update: String,
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
    /// GitHub `owner/repo` (or `https://github.com/owner/repo`) checked on **`-R`** / **`-RU`** when
    /// the AUR (or other) PKGBUILD lags behind upstream releases.
    #[serde(default)]
    pub upstream_github: Option<String>,
    /// When true, consider GitHub prereleases when choosing the newest upstream version.
    #[serde(default)]
    pub upstream_prereleases: bool,
}

const CONFIG_TEMPLATE: &str = include_str!("../abs.toml.example");

fn user_config_path() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("abs").join("abs.toml"))
        .unwrap_or_else(|| die!("Could not determine config directory ($XDG_CONFIG_HOME)"))
}

fn ensure_user_config_exists() -> PathBuf {
    let path = user_config_path();
    if path.exists() {
        return path;
    }

    if let Some(parent) = path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        die!("Failed to create config directory '{}': {}", parent.display(), e);
    }

    if let Err(e) = fs::write(&path, CONFIG_TEMPLATE) {
        die!("Failed to write config file '{}': {}", path.display(), e);
    }

    path
}

fn resolve_editor(explicit: Option<&str>) -> String {
    if let Some(editor) = explicit.filter(|s| !s.is_empty()) {
        return editor.to_string();
    }

    std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string())
}

fn run_editor(editor: &str, path: &Path) {
    let path_str = path.to_string_lossy();
    let editor_trimmed = editor.trim();
    let cmd_name = editor_trimmed.split_whitespace().next().unwrap_or(editor_trimmed);

    let result = if editor.chars().any(char::is_whitespace) {
        let script = format!("{} {}", editor, sh_single_quote(&path_str));
        run_command("sh", &["-c", &script], None::<&str>)
    } else {
        let mut args = Vec::new();
        if cmd_name == "kate" {
            args.push("-b");
        } else if cmd_name == "code"
            || cmd_name == "vscode"
            || cmd_name == "codium"
            || cmd_name == "vscodium"
            || cmd_name == "cursor"
            || cmd_name == "subl"
            || cmd_name == "sublime-text"
            || cmd_name == "gedit"
            || cmd_name == "pluma"
            || cmd_name == "xed"
            || cmd_name == "atom"
            || cmd_name == "lumiere"
        {
            args.push("-w");
        }
        args.push(path_str.as_ref());
        run_command(editor, &args, None::<&str>)
    };

    if let Err(e) = result {
        die!("Failed to open config in editor: {}", e);
    }
}

impl Config {
    pub fn open_in_editor(editor: Option<&str>) {
        use std::io::{self, Write};
        let path = ensure_user_config_exists();
        let editor_str = resolve_editor(editor);
        loop {
            run_editor(&editor_str, &path);

            let config_content = match fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => {
                    println!("==> ERROR: Failed to read config file for validation.");
                    break;
                }
            };
            match toml::from_str::<Config>(&config_content) {
                Ok(config) => {
                    let env = config.build.default_environment.as_str();
                    if env != "local" && env != "chroot" {
                        println!("{} Invalid [build] default_environment: {:?} (expected \"local\" or \"chroot\")", "==> ERROR:".red(), env);
                    } else {
                        println!("{}", "==> Configuration validated successfully!".green());
                        break;
                    }
                }
                Err(e) => {
                    println!("{} Failed to parse configuration file: {}", "==> ERROR:".red(), e);
                }
            }

            print!("Would you like to re-open the editor to fix the configuration? [Y/n]: ");
            let _ = io::stdout().flush();
            let mut input = String::new();
            if io::stdin().read_line(&mut input).is_err() {
                break;
            }
            let v = input.trim().to_lowercase();
            if v == "n" || v == "no" {
                break;
            }
        }
    }

    pub fn load_config() -> Config {
        // Same order as README: XDG config dir, then /etc.
        let user_config = user_config_path();
        let etc_config = PathBuf::from("/etc/abs/abs.toml");

        let config_path = if user_config.exists() {
            user_config
        } else if etc_config.exists() {
            etc_config
        } else {
            let path = ensure_user_config_exists();
            println!(
                "ABS config has been created from the example. Please configure using --configure"
            );
            path
        };

        let config_content = match fs::read_to_string(&config_path) {
            Ok(c) => c,
            Err(_) => {
                die!("Failed to read config file at {:?}", config_path);
            }
        };

        let mut config: Config = match toml::from_str(&config_content) {
            Ok(c) => c,
            Err(e) => {
                die!("Failed to parse config '{:?}': {}", config_path, e);
            }
        };

        // Merge self-update settings parsed under [build] for backwards-compatibility
        if let Some(val) = config.build.check_for_update_on_startup {
            config.check_for_update_on_startup = val;
        }
        if let Some(val) = config.build.auto_update_on_startup {
            config.auto_update_on_startup = val;
        }
        if let Some(val) = config.build.self_update_at_updates {
            config.self_update_at_updates = val;
        }
        if let Some(val) = &config.build.self_update_github_url {
            config.self_update_github_url = val.clone();
        }
        if let Some(val) = &config.build.self_update_raw_url {
            config.self_update_raw_url = val.clone();
        }
        if let Some(val) = &config.build.self_update_install_path {
            config.self_update_install_path = val.clone();
        }

        config.validate();
        config
    }

    fn validate(&self) {
        if self.config_version == 0 {
            die!("Invalid config_version: 0 (expected >= 1)");
        }
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
        println!("{}", "ABS Configuration".blue().bold());
        println!("{}", "-------------------------".blue());
        println!("config_version: {}", self.config_version);

        println!("\n{}", "Paths".green().bold());
        println!("  packages_path: {}", self.paths.packages_path);
        println!("  chroot_base_path: {}", self.paths.chroot_base_path);
        println!(
            "  ready_made_packages_path: {}",
            self.paths.ready_made_packages_path
        );
        println!(
            "  chroot_makepkg_conf: {}",
            self.paths.chroot_makepkg_conf.as_deref().unwrap_or("(none)")
        );

        println!("\n{}", "Build".green().bold());
        println!("  default_environment: {}", self.build.default_environment);
        println!(
            "  ignore_compilation_failures: {}",
            self.build.ignore_compilation_failures
        );
        println!(
            "  compile_first_install_after: {}",
            self.build.compile_first_install_after
        );
        println!(
            "  clean_install_by_default: {}",
            self.build.clean_install_by_default
        );
        println!(
            "  concurrent_repos_downloads_limit: {}",
            self.build.concurrent_repos_downloads_limit
        );
        println!(
            "  concurrent_compilations_limit: {}",
            self.build.concurrent_compilations_limit
        );
        println!(
            "  fast_aur_rpc_update_checks: {}",
            self.build.fast_aur_rpc_update_checks
        );

        println!("\n{}", "System Update".green().bold());
        println!(
            "  command_to_update_repositories: {}",
            self.system_update.command_to_update_repositories
        );
        println!(
            "  command_to_perform_system_update: {}",
            self.system_update.command_to_perform_system_update
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

        println!("\n{}", "Self-Updates".green().bold());
        println!("  check_for_update_on_startup: {}", self.check_for_update_on_startup);
        println!("  auto_update_on_startup: {}", self.auto_update_on_startup);
        println!("  self_update_at_updates: {}", self.self_update_at_updates);
        println!("  self_update_github_url: {}", self.self_update_github_url);
        println!("  self_update_raw_url: {}", self.self_update_raw_url);
        println!("  self_update_install_path: {}", self.self_update_install_path);

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
            if let Some(repo) = &cfg.upstream_github {
                println!(
                    "    upstream_github: {} (prereleases: {})",
                    repo, cfg.upstream_prereleases
                );
            }
        }
    }
}
