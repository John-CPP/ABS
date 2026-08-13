//! Guided creation and reconfiguration of `abs.toml`.

use crate::config::{
    self, Config, etc_config_path, example_config_text, user_config_path, write_example_user_config,
};
use crate::die;
use crate::{blog, ewarn};
use colored::Colorize;
use std::collections::HashSet;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use toml_edit::{Array, DocumentMut, Item, Table, Value};

const STEPS: usize = 8;

const EDIT_LATER: &str = "\
          abs --config-wizard
          abs --configure
          abs --configure=nano
        (or another editor you use; tested with vim, nano, and kate)";

const KNOWN_REPOS: &[(&str, &str)] = &[
    (
        "arch",
        "https://gitlab.archlinux.org/archlinux/packaging/packages",
    ),
    ("aur", "https://aur.archlinux.org"),
    (
        "cachyos",
        "https://github.com/CachyOS/CachyOS-PKGBUILDS.git",
    ),
];

const SUGGEST_PACKAGES_PATH: &str = "$XDG_CACHE_HOME/abs/packages";
const SUGGEST_CHROOT_PATH: &str = "$XDG_CACHE_HOME/abs/chroot";
const SUGGEST_READY_PATH: &str = "$XDG_CACHE_HOME/abs/ready";
const SUGGEST_SYNC_CMD: &str = "sudo pacman -Sy";
const SUGGEST_UPDATE_CMD: &str = "sudo pacman -Syu";
const SUGGEST_NO_REFRESH_CMD: &str = "sudo pacman -Su";
const SUGGEST_IGNORE_FLAG: &str = "--ignore";
const SUGGEST_MOUNT: &str = "/run/abs-ram";
const SUGGEST_SIZE: &str = "16G";
const SUGGEST_MODE: &str = "0755";
const SUGGEST_INSTALL_PATH: &str = "/usr/bin/abs";

/// `abs --config-wizard`: create or reconfigure the user config, then exit.
pub fn run() {
    require_tty();
    let path = user_config_path();
    let (text, reconfigure) = load_wizard_source(&path);
    run_on_document(&path, &text, reconfigure);
    blog!("Configuration saved. You can re-run `abs --config-wizard` at any time.");
}

/// First launch with no config: offer the wizard, example defaults, or quit.
pub fn offer_first_run(assume_yes: bool) {
    if config::config_exists() {
        return;
    }

    if assume_yes || crate::is_silent_mode() || !io::stdin().is_terminal() {
        let path = write_example_user_config();
        blog!(
            "Created {} from the example. Customize with:",
            path.display()
        );
        println!("{EDIT_LATER}");
        return;
    }

    println!();
    println!("{}", "==> No ABS config found".yellow().bold());
    println!(
        "    ABS needs a config file at {} before it can build packages.",
        user_config_path().display()
    );
    println!("    You can add per-package options, version holds, and compilers later");
    println!("    with `abs --wizard` or absgui.");
    println!();

    let choice = prompt_choice(
        "How do you want to create it?",
        "",
        &[
            Choice::new(
                "wizard",
                "Create and configure interactively",
                "Step-by-step questions in plain language. Enter keeps each suggestion.",
                true,
            ),
            Choice::new(
                "example",
                "Create from example defaults",
                format!("Writes the example config as-is. Edit later with:\n{EDIT_LATER}"),
                false,
            ),
            Choice::new(
                "quit",
                "Quit",
                "Do nothing. Run abs again when you are ready.",
                false,
            ),
        ],
        Some("wizard"),
        false,
    );

    match choice.as_str() {
        "wizard" => {
            require_tty();
            let path = user_config_path();
            run_on_document(&path, example_config_text(), false);
            blog!("Configuration saved to {}.", path.display());
        }
        "example" => {
            let path = write_example_user_config();
            blog!("Created {}. Edit later with:", path.display());
            println!("{EDIT_LATER}");
        }
        _ => {
            println!("No config written.");
            crate::utils::wait_before_exit_if_needed();
            std::process::exit(0);
        }
    }
}

fn require_tty() {
    if !io::stdin().is_terminal() {
        die!(
            "--config-wizard needs an interactive terminal (or create a config with abs --configure)"
        );
    }
}

fn load_wizard_source(user_path: &Path) -> (String, bool) {
    if user_path.exists() {
        let text = fs::read_to_string(user_path).unwrap_or_else(|e| {
            die!("Failed to read {}: {e}", user_path.display());
        });
        return (text, true);
    }

    let etc = etc_config_path();
    if etc.exists() {
        println!();
        println!(
            "{}",
            format!("==> Found a system config at {}", etc.display()).yellow()
        );
        println!(
            "    The wizard always writes your user file {}.",
            user_path.display()
        );
        let choice = prompt_choice(
            "Start from which template?",
            "",
            &[
                Choice::new(
                    "system",
                    "Copy the system config, then reconfigure",
                    "Keeps existing packages, holds, and comments; you can change global settings.",
                    true,
                ),
                Choice::new(
                    "example",
                    "Start from the example defaults",
                    "Ignores /etc/abs/abs.toml for this new user file.",
                    false,
                ),
            ],
            Some("system"),
            false,
        );
        if choice == "system" {
            let text = fs::read_to_string(&etc).unwrap_or_else(|e| {
                die!("Failed to read {}: {e}", etc.display());
            });
            return (text, true);
        }
    }

    (example_config_text().to_string(), false)
}

fn run_on_document(path: &Path, text: &str, reconfigure: bool) {
    let mut doc: DocumentMut = text.parse().unwrap_or_else(|e| {
        die!("Failed to parse starting config as TOML: {e}");
    });

    println!();
    println!("{}", "==> ABS configuration wizard".green().bold());
    println!("    File: {}", path.display());
    if reconfigure {
        println!("    Reconfigure: each step shows your current value. Press Enter to keep it.");
        println!(
            "    Green {} still marks the ABS default, even if your file differs.",
            "(Suggested)".green()
        );
        println!("    [packages.*], held_packages, and [compilers] are left unchanged");
        println!("    (use `abs --wizard` / absgui).");
        if path.exists() {
            println!(
                "    The current file is copied to {}.bak before saving.",
                path.file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_else(|| "abs.toml".into())
            );
        }
    } else {
        println!("    New config: each step shows the Suggested value. Press Enter to accept it.");
        println!(
            "    Green {} = the default that ships with ABS. Use it if you are unsure.",
            "(Suggested)".green()
        );
    }
    println!("    Ctrl+C aborts without writing.");
    println!();

    step_paths(&mut doc, 1, reconfigure);
    step_build(&mut doc, 2, reconfigure);
    step_cpu(&mut doc, 3, reconfigure);
    step_system_update(&mut doc, 4, reconfigure);
    step_repositories(&mut doc, 5, reconfigure);
    step_ramdisk(&mut doc, 6, reconfigure);
    step_self_update(&mut doc, 7, reconfigure);
    step_package_lists(&mut doc, 8, reconfigure);

    let rendered = doc.to_string();
    if let Err(e) = Config::from_toml_text(&rendered) {
        ewarn!("The answers do not produce a valid config: {e}");
        die!("Config was not written. Fix the values and run `abs --config-wizard` again.");
    }

    print_summary(&doc, path);
    let write_help = if path.exists() {
        format!(
            "Saves to {} (mode 0600). Copies the current file to a .bak beside it first. Answer no to throw the answers away.",
            path.display()
        )
    } else {
        format!(
            "Saves to {} (mode 0600). Answer no to throw the answers away.",
            path.display()
        )
    };
    if !prompt_bool(
        "Write this configuration?",
        &write_help,
        true,
        true,
        reconfigure,
    ) {
        println!("No changes written.");
        crate::utils::wait_before_exit_if_needed();
        std::process::exit(0);
    }

    if let Some(parent) = path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        die!(
            "Failed to create config directory '{}': {e}",
            parent.display()
        );
    }
    if path.exists() {
        match backup_existing_config(path) {
            Ok(bak) => blog!("Previous config saved as {}", bak.display()),
            Err(e) => die!("Failed to back up '{}': {e}", path.display()),
        }
    }
    if let Err(e) = crate::utils::write_file_mode(path, &rendered, 0o600) {
        die!("Failed to write '{}': {e}", path.display());
    }
}

/// Next unused backup path beside `config_path` (`abs.toml.bak`, then `.bak.1`, …).
fn unique_backup_path(config_path: &Path) -> PathBuf {
    let file_name = config_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "abs.toml".into());
    let parent = config_path.parent().unwrap_or_else(|| Path::new("."));
    let first = parent.join(format!("{file_name}.bak"));
    if !first.exists() {
        return first;
    }
    for n in 1..10_000 {
        let candidate = parent.join(format!("{file_name}.bak.{n}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    parent.join(format!("{file_name}.bak.{nanos}"))
}

/// Copy an existing user config aside before the wizard overwrites it.
fn backup_existing_config(path: &Path) -> Result<PathBuf, String> {
    let dest = unique_backup_path(path);
    fs::copy(path, &dest).map_err(|e| {
        format!(
            "failed to copy {} to {}: {e}",
            path.display(),
            dest.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(0o600));
    }
    Ok(dest)
}

fn step_header(n: usize, title: &str, blurb: &str) {
    println!();
    println!(
        "{} {} — {}",
        "==>".green(),
        format!("Step {n}/{STEPS}").bold(),
        title.bold()
    );
    for line in blurb.lines() {
        println!("    {line}");
    }
    println!();
}

fn step_paths(doc: &mut DocumentMut, n: usize, reconfigure: bool) {
    step_header(
        n,
        "Paths",
        "These are folders on your disk. ABS will create them if they are missing.\n\
         You can use $HOME, $XDG_CONFIG_HOME, and $XDG_CACHE_HOME (ABS expands them when it runs).\n\
         Do not point them at your whole home directory or at system folders like /tmp.\n\
         Several computers: put ready_made_packages_path on a shared folder so only one PC compiles.",
    );
    let paths = table_mut(doc, "paths");
    let packages = prompt_path(
        "packages_path",
        "Folder where ABS saves the source code it downloads before compiling.\n\
         Example: if you build mesa, its git clone lives here.\n\
         Sharing this across PCs is optional; compiled packages live in ready_made_packages_path.",
        &get_str(paths, "packages_path", SUGGEST_PACKAGES_PATH),
        SUGGEST_PACKAGES_PATH,
        "paths.packages_path",
        reconfigure,
    );
    set_str(paths, "packages_path", &packages);

    let chroot = prompt_path(
        "chroot_base_path",
        "Folder for a clean mini-system used when you compile “in a chroot” (isolated from your real install).\n\
         Only needed if you choose chroot builds later.",
        &get_str(paths, "chroot_base_path", SUGGEST_CHROOT_PATH),
        SUGGEST_CHROOT_PATH,
        "paths.chroot_base_path",
        reconfigure,
    );
    set_str(paths, "chroot_base_path", &chroot);

    let ready = prompt_path(
        "ready_made_packages_path",
        "Folder where finished packages are stored after a successful compile\n\
         (the .pkg.tar files you can install). If several computers share this folder,\n\
         only one needs to compile; the others reuse these files, including ABS self-update.\n\
         Do not use pacman's download cache (CacheDir) as this folder.",
        &get_str(paths, "ready_made_packages_path", SUGGEST_READY_PATH),
        SUGGEST_READY_PATH,
        "paths.ready_made_packages_path",
        reconfigure,
    );
    set_str(paths, "ready_made_packages_path", &ready);

    let current_conf = get_optional_str(paths, "chroot_makepkg_conf");
    let conf = prompt_optional_string(
        "chroot_makepkg_conf (optional)",
        "A custom makepkg.conf to use inside the chroot. Leave empty unless you already have one.\n\
         Enter keeps current, '-' clears.",
        current_conf.as_deref(),
        None,
        reconfigure,
    );
    set_optional_str(paths, "chroot_makepkg_conf", conf.as_deref());
}

fn step_build(doc: &mut DocumentMut, n: usize, reconfigure: bool) {
    step_header(
        n,
        "How ABS compiles",
        "This is the default for every package. You can still override one package later in absgui or [packages.*].",
    );
    let build = table_mut(doc, "build");
    let env = prompt_choice(
        "default_environment",
        "Where the compile actually runs.",
        &[
            Choice::new(
                "local",
                "local — compile on this computer with makepkg",
                "Faster. Uses the compilers and libraries you already have installed.",
                true,
            ),
            Choice::new(
                "chroot",
                "chroot — compile in a clean mini-system",
                "The build does not pick up extra packages from your install.\n\
                 Needs more disk space and the `devtools` package.",
                false,
            ),
        ],
        Some(&get_str(build, "default_environment", "local")),
        reconfigure,
    );
    set_str(build, "default_environment", &env);

    let system_first = prompt_bool(
        "system_update_first",
        "Update your system first, then compile. Avoids broken programs when a library on disk does not match what you just built.",
        get_bool(build, "system_update_first", true),
        true,
        reconfigure,
    );
    set_bool(build, "system_update_first", system_first);

    let ignore_fail = prompt_bool(
        "ignore_compilation_failures",
        "If one package fails to compile, keep going with the others instead of stopping everything.",
        get_bool(build, "ignore_compilation_failures", false),
        false,
        reconfigure,
    );
    set_bool(build, "ignore_compilation_failures", ignore_fail);

    let compile_first = prompt_bool(
        "compile_first_install_after",
        "Compile every package first, and only then ask you to install them. Useful if you want to leave the computer compiling overnight without answering questions.",
        get_bool(build, "compile_first_install_after", false),
        false,
        reconfigure,
    );
    set_bool(build, "compile_first_install_after", compile_first);

    let clean_install = prompt_bool(
        "clean_install_by_default",
        "Delete leftover build folders (src/ and pkg/) before each compile, so you always start from a clean tree.",
        get_bool(build, "clean_install_by_default", false),
        false,
        reconfigure,
    );
    set_bool(build, "clean_install_by_default", clean_install);

    let ignore_made = prompt_bool(
        "ignore_already_made_packages",
        "Always compile again even if ABS already has a finished package of this version. If no, ABS reuses that file and skips compiling (you can still force a rebuild with -n). Choose no when other computers should reuse packages compiled on this machine.",
        get_bool(build, "ignore_already_made_packages", false),
        false,
        reconfigure,
    );
    set_bool(build, "ignore_already_made_packages", ignore_made);

    let dl = prompt_usize(
        "concurrent_repos_downloads_limit",
        "How many packages ABS may download at the same time. Higher is faster if your internet is good.",
        get_usize(build, "concurrent_repos_downloads_limit", 10),
        10,
        1,
        reconfigure,
    );
    set_usize(build, "concurrent_repos_downloads_limit", dl);

    let cc = prompt_usize(
        "concurrent_compilations_limit",
        "How many packages ABS may compile at the same time. 1 is safest (less RAM). Raise it if you have many CPU cores and a lot of memory.",
        get_usize(build, "concurrent_compilations_limit", 1),
        1,
        1,
        reconfigure,
    );
    set_usize(build, "concurrent_compilations_limit", cc);

    let clean_chroot = prompt_bool(
        "clean_chroot_after_compilation",
        "After a chroot build, throw away the temporary copy so it does not fill your disk.",
        get_bool(build, "clean_chroot_after_compilation", true),
        true,
        reconfigure,
    );
    set_bool(build, "clean_chroot_after_compilation", clean_chroot);

    let aur_rpc = prompt_bool(
        "fast_aur_rpc_update_checks",
        "When checking AUR packages for updates, ask the AUR website in one batch instead of opening every git clone. Faster.",
        get_bool(build, "fast_aur_rpc_update_checks", true),
        true,
        reconfigure,
    );
    set_bool(build, "fast_aur_rpc_update_checks", aur_rpc);
}

fn step_cpu(doc: &mut DocumentMut, n: usize, reconfigure: bool) {
    step_header(
        n,
        "CPU / how hard to push the machine",
        "When several packages compile at once, ABS can limit how many CPU threads they use together so the computer stays usable.",
    );
    let build = table_mut(doc, "build");
    let mode = prompt_choice(
        "global_cpu_threads_mode",
        "",
        &[
            Choice::new(
                "strict",
                "strict — never go over the cap",
                "If starting another compile would use too many CPU threads, wait.",
                true,
            ),
            Choice::new(
                "flexible",
                "flexible — soft cap, with an optional hard maximum",
                "Try to stay under a soft limit, but may go a bit over up to a hard maximum.",
                false,
            ),
        ],
        Some(&get_str(build, "global_cpu_threads_mode", "strict")),
        reconfigure,
    );
    set_str(build, "global_cpu_threads_mode", &mode);

    let default_j = prompt_optional_usize(
        "default_compilation_threads (optional)",
        "How many CPU cores to use for one package (-j) if that package does not set its own number. Empty = let makepkg decide. '-' clears.",
        get_optional_usize(build, "default_compilation_threads"),
        None,
        reconfigure,
    );
    set_optional_usize(build, "default_compilation_threads", default_j);

    let cap = prompt_optional_usize(
        "global_cpu_threads_cap (optional)",
        "Maximum total CPU threads used by all compiles running at once. Empty = no extra cap. '-' clears.",
        get_optional_usize(build, "global_cpu_threads_cap"),
        None,
        reconfigure,
    );
    set_optional_usize(build, "global_cpu_threads_cap", cap);

    if mode == "flexible" {
        let max = prompt_optional_usize(
            "maximum_cpu_threads_cap (optional, flexible only)",
            "Absolute maximum threads, even if ABS starts extra jobs. Must be at least the soft cap. '-' clears.",
            get_optional_usize(build, "maximum_cpu_threads_cap"),
            None,
            reconfigure,
        );
        set_optional_usize(build, "maximum_cpu_threads_cap", max);
    }
}

fn step_system_update(doc: &mut DocumentMut, n: usize, reconfigure: bool) {
    step_header(
        n,
        "System update commands",
        "These are the commands ABS runs when you ask it to refresh or upgrade the system (abs -R, abs -U, abs -RU).\n\
         Type a normal command; no pipes (|), &&, or sh -c.",
    );
    let sys = table_mut(doc, "system_update");
    let sync = prompt_command(
        "command_to_update_repositories",
        "Only refreshes the list of available packages. Does not upgrade anything yet. Used by abs -R.",
        &get_str(sys, "command_to_update_repositories", SUGGEST_SYNC_CMD),
        SUGGEST_SYNC_CMD,
        reconfigure,
    );
    set_str(sys, "command_to_update_repositories", &sync);

    let full = prompt_command(
        "command_to_perform_system_update",
        "Actually upgrades your system. Used by abs -RU. You can use yay -Syu if that is what you normally run.",
        &get_str(sys, "command_to_perform_system_update", SUGGEST_UPDATE_CMD),
        SUGGEST_UPDATE_CMD,
        reconfigure,
    );
    set_str(sys, "command_to_perform_system_update", &full);

    let no_refresh = prompt_optional_string(
        "command_to_perform_system_update_no_refresh (optional)",
        "Same upgrade, but skip refreshing the package list because it was already refreshed. Typical: sudo pacman -Su. '-' clears.",
        get_optional_str(sys, "command_to_perform_system_update_no_refresh").as_deref(),
        Some(SUGGEST_NO_REFRESH_CMD),
        reconfigure,
    );
    if let Some(cmd) = &no_refresh {
        validate_command(cmd).unwrap_or_else(|e| die!("{e}"));
    }
    set_optional_str(
        sys,
        "command_to_perform_system_update_no_refresh",
        no_refresh.as_deref(),
    );

    let flag = prompt_string(
        "ignore_flag",
        "The flag your updater uses to skip packages. For pacman and yay this is --ignore.",
        &get_str(sys, "ignore_flag", SUGGEST_IGNORE_FLAG),
        SUGGEST_IGNORE_FLAG,
        reconfigure,
    );
    set_str(sys, "ignore_flag", &flag);
}

fn step_repositories(doc: &mut DocumentMut, n: usize, reconfigure: bool) {
    step_header(
        n,
        "Repositories",
        "A repository is a git place ABS downloads build recipes (PKGBUILDs) from.\n\
         You can keep several, add your own, or remove ones you do not use.\n\
         When you type `abs mesa` without saying a repo, ABS uses the default.",
    );
    let (mut repos, mut default) = {
        let repos_table = table_mut(doc, "repositories");
        let repos = repo_entries(repos_table);
        let default = get_str(repos_table, "default", "arch");
        (repos, default)
    };
    if repos.is_empty() {
        repos = KNOWN_REPOS
            .iter()
            .map(|(k, u)| ((*k).to_string(), (*u).to_string()))
            .collect();
    }

    loop {
        println!("  Your repositories:");
        for (i, (name, url)) in repos.iter().enumerate() {
            let suggested = KNOWN_REPOS.iter().any(|(k, u)| *k == name && *u == url);
            println!(
                "    [{}] {:<8} {}{}",
                i + 1,
                name,
                url,
                tags(false, suggested)
            );
        }
        println!();
        let action = prompt_choice(
            "What do you want to do?",
            "",
            &[
                Choice::new(
                    "done",
                    "This list is good — choose the default and continue",
                    "Keep the list above, then pick which repo ABS uses when you do not specify one.",
                    true,
                ),
                Choice::new(
                    "add",
                    "Add a repository",
                    "Add one custom (or built-in) git URL, then return to this list.",
                    false,
                ),
                Choice::new(
                    "remove",
                    "Remove a repository",
                    "Drop a built-in or custom repo from the list, then return here.",
                    false,
                ),
            ],
            Some("done"),
            false,
        );
        match action.as_str() {
            "add" => {
                if let Some((name, url)) = prompt_add_repo(&repos) {
                    if let Some(existing) = repos.iter_mut().find(|(n, _)| n == &name) {
                        existing.1 = url;
                    } else {
                        repos.push((name, url));
                    }
                }
            }
            "remove" => {
                if repos.len() == 1 {
                    println!(
                        "    {} at least one repository must remain.",
                        "Invalid:".red()
                    );
                    continue;
                }
                println!("    Which number to remove? (or Enter to cancel)");
                let line = read_line("    Number: ");
                if line.is_empty() {
                    continue;
                }
                if let Ok(n) = line.parse::<usize>()
                    && n >= 1
                    && n <= repos.len()
                {
                    let removed = repos.remove(n - 1);
                    if default == removed.0 {
                        default = suggested_default_name(&repos);
                    }
                } else {
                    println!("    {} enter a listed number.", "Invalid:".red());
                }
            }
            _ => break,
        }
        println!();
    }

    let suggest_default = suggested_default_name(&repos);
    let choices: Vec<Choice> = repos
        .iter()
        .map(|(name, _)| {
            Choice::new(
                name.clone(),
                name.clone(),
                "Used when you type a package name without a repo.",
                name == &suggest_default,
            )
        })
        .collect();
    let current_default = if repos.iter().any(|(n, _)| n == &default) {
        default
    } else {
        suggest_default.clone()
    };
    let picked = prompt_choice(
        "Which repository should ABS use when you do not specify one?",
        "",
        &choices,
        Some(&current_default),
        reconfigure,
    );

    apply_repo_list(table_mut(doc, "repositories"), &repos, &picked);
}

fn prompt_add_repo(existing: &[(String, String)]) -> Option<(String, String)> {
    loop {
        let name = read_line("  Short name (letters/digits, e.g. myrepo): ");
        if name.is_empty() {
            return None;
        }
        if let Err(e) = validate_repo_name(&name) {
            println!("    {} {e}", "Invalid:".red());
            continue;
        }
        let known_url = KNOWN_REPOS
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, u)| *u);
        let url_prompt = if let Some(url) = known_url {
            format!("  Git URL (Enter = {url}): ")
        } else {
            "  Git URL (https://...): ".into()
        };
        let url_line = read_line(&url_prompt);
        let url = if url_line.is_empty() {
            known_url.unwrap_or("").to_string()
        } else {
            url_line
        };
        if let Err(e) = validate_repo_url(&url) {
            println!("    {} {e}", "Invalid:".red());
            continue;
        }
        if existing.iter().any(|(n, _)| n == &name) {
            let replace = prompt_bool(
                &format!("Repository {name} already exists"),
                "Replace its URL with the one you just typed?",
                false,
                false,
                false,
            );
            if !replace {
                return None;
            }
        }
        return Some((name, url));
    }
}

fn step_ramdisk(doc: &mut DocumentMut, n: usize, reconfigure: bool) {
    step_header(
        n,
        "Ramdisk (compile in RAM)",
        "A ramdisk is a temporary disk that lives in memory (tmpfs). Compiling there is often faster and wears the SSD less,\n\
         but it needs free RAM and sudo to mount. Your folders from Step 1 stay the real/permanent locations.\n\
         The ramdisk is created only when a build needs it, not when ABS starts.",
    );
    let ram = table_mut(doc, "ramdisk");
    let enabled = prompt_bool(
        "enabled",
        "Turn ramdisk support on.",
        get_bool(ram, "enabled", false),
        false,
        reconfigure,
    );
    set_bool(ram, "enabled", enabled);
    if !enabled {
        return;
    }

    loop {
        let mp = prompt_string(
            "mount_point",
            "Folder where that RAM disk appears, for example /run/abs-ram. The last part of the path must start with abs.",
            &get_str(ram, "mount_point", SUGGEST_MOUNT),
            SUGGEST_MOUNT,
            reconfigure,
        );
        match validate_mount_point(&mp) {
            Ok(()) => {
                set_str(ram, "mount_point", &mp);
                break;
            }
            Err(e) => println!("    {} {e}", "Invalid:".red()),
        }
    }

    loop {
        let size = prompt_string(
            "size",
            "Maximum RAM it may use (not reserved up front). Examples: 16G, 50%. No commas.",
            &get_str(ram, "size", SUGGEST_SIZE),
            SUGGEST_SIZE,
            reconfigure,
        );
        match crate::ramdisk::validate_ramdisk_size(&size) {
            Ok(()) => {
                set_str(ram, "size", &size);
                break;
            }
            Err(e) => println!("    {} {e}", "Invalid:".red()),
        }
    }

    loop {
        let mode = prompt_string(
            "mode",
            "Unix permissions for that folder, like 0755.",
            &get_str(ram, "mode", SUGGEST_MODE),
            SUGGEST_MODE,
            reconfigure,
        );
        match crate::ramdisk::validate_ramdisk_mode(&mode) {
            Ok(()) => {
                set_str(ram, "mode", &mode);
                break;
            }
            Err(e) => println!("    {} {e}", "Invalid:".red()),
        }
    }

    let w = prompt_bool(
        "build_workdir",
        "Put the heavy compile folders (src/ and pkg/, plus compiler caches) in RAM. Good for big packages like the kernel.",
        get_bool(ram, "build_workdir", false),
        false,
        reconfigure,
    );
    set_bool(ram, "build_workdir", w);

    let c = prompt_bool(
        "chroot",
        "Put the whole chroot (the mini-system) in RAM. Faster chroot builds, uses a lot of RAM.",
        get_bool(ram, "chroot", false),
        false,
        reconfigure,
    );
    set_bool(ram, "chroot", c);

    let p = prompt_bool(
        "packages",
        "Put all downloaded sources in RAM. Uses a lot of RAM; only if you have plenty to spare.",
        get_bool(ram, "packages", false),
        false,
        reconfigure,
    );
    set_bool(ram, "packages", p);

    let min_ram = prompt_usize(
        "min_free_ram_mb",
        "Do not mount the RAM disk if the computer has less than this many MB of free memory. Protects you from freezing the machine.",
        get_usize(ram, "min_free_ram_mb", 4096),
        4096,
        0,
        reconfigure,
    );
    set_usize(ram, "min_free_ram_mb", min_ram);
}

fn step_self_update(doc: &mut DocumentMut, n: usize, reconfigure: bool) {
    step_header(
        n,
        "Updating ABS itself",
        "ABS can look on GitHub to see if a newer ABS exists. With pacman install, built packages go into ready_made_packages_path so other computers can reuse them.",
    );
    let check = prompt_bool(
        "check_for_update_on_startup",
        "When you start abs, tell you if a newer ABS is available.",
        get_root_bool(doc, "check_for_update_on_startup", true),
        true,
        reconfigure,
    );
    set_root_bool(doc, "check_for_update_on_startup", check);

    let auto = prompt_bool(
        "auto_update_on_startup",
        "If a newer ABS exists, update ABS automatically before doing anything else. Can take a while.",
        get_root_bool(doc, "auto_update_on_startup", false),
        false,
        reconfigure,
    );
    set_root_bool(doc, "auto_update_on_startup", auto);

    let pacman = prompt_bool(
        "self_update_use_pacman",
        "Install the new ABS with pacman (the usual Arch way). Built packages are stored in ready_made_packages_path. If no, just copy the abs and absgui files to a folder.",
        get_root_bool(doc, "self_update_use_pacman", true),
        true,
        reconfigure,
    );
    set_root_bool(doc, "self_update_use_pacman", pacman);

    if !pacman {
        let install = prompt_string(
            "self_update_install_path",
            "Where to copy abs (absgui is placed next to it).",
            &get_root_str(doc, "self_update_install_path", SUGGEST_INSTALL_PATH),
            SUGGEST_INSTALL_PATH,
            reconfigure,
        );
        set_root_str(doc, "self_update_install_path", &install);
    }

    let at_updates = prompt_bool(
        "self_update_at_updates",
        "Also check for a newer ABS when you run a system update (abs -U / -RU).",
        get_root_bool(doc, "self_update_at_updates", false),
        false,
        reconfigure,
    );
    set_root_bool(doc, "self_update_at_updates", at_updates);
}

fn step_package_lists(doc: &mut DocumentMut, n: usize, reconfigure: bool) {
    step_header(
        n,
        "Package lists",
        "Optional. Enter keeps the current list (empty on a new config).\n\
         Type names separated by commas or spaces. '-' clears the list.\n\
         You can also edit these later with `abs --wizard` or `abs --list-add`.",
    );
    let manual = get_string_array(doc.as_table(), "manual_update_packages");
    let manual_new = prompt_string_list(
        "manual_update_packages",
        "Checks these packages in their repository and tells you if there are new versions available for automatic compilation.",
        &manual,
        reconfigure,
    );
    set_string_array(doc.as_table_mut(), "manual_update_packages", &manual_new);

    let skip = get_string_array(doc.as_table(), "skip_install_packages");
    let skip_new = prompt_string_list(
        "skip_install_packages",
        "Skip installing these packages from binaries (the pre-built ones pacman would download). Useful if you are going to compile them on your own. Globs work, for example qemu*.",
        &skip,
        reconfigure,
    );
    set_string_array(doc.as_table_mut(), "skip_install_packages", &skip_new);

    let after_present = doc
        .as_table()
        .get("skip_install_packages_after_compilation")
        .is_some();
    let after = get_string_array(doc.as_table(), "skip_install_packages_after_compilation");
    match prompt_skip_after_list(&after, after_present, reconfigure) {
        SkipAfterEdit::Keep => {}
        SkipAfterEdit::Unset => {
            doc.as_table_mut()
                .remove("skip_install_packages_after_compilation");
        }
        SkipAfterEdit::Set(items) => {
            set_string_array(
                doc.as_table_mut(),
                "skip_install_packages_after_compilation",
                &items,
            );
        }
    }

    let sys = table_mut(doc, "system_update");
    let ignore = get_string_array(sys, "ignore_packages");
    let ignore_new = prompt_string_list(
        "system_update.ignore_packages",
        "Extra names to skip during the system update command, on top of the lists above.",
        &ignore,
        reconfigure,
    );
    set_string_array(sys, "ignore_packages", &ignore_new);
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SkipAfterEdit {
    Keep,
    Unset,
    Set(Vec<String>),
}

fn prompt_skip_after_list(
    current: &[String],
    key_present: bool,
    reconfigure: bool,
) -> SkipAfterEdit {
    let shown = if !key_present {
        "(unset; inherit skip_install_packages)".to_string()
    } else if current.is_empty() {
        "(empty)".to_string()
    } else {
        current.join(", ")
    };
    print_field(
        "skip_install_packages_after_compilation",
        "Skip offering to install these packages after they were compiled, for a faster install step.\n\
         For example qemu-docs if you compiled qemu but do not want the docs package.\n\
         Enter keeps current. '-' writes an empty list. On a new file, Enter leaves this unset (inherit skip_install_packages).",
    );
    print_current_suggested(&shown, Some("(empty / unset)"), reconfigure && key_present);
    let line = read_line(&format!("    [{shown}]: "));
    if line.is_empty() {
        if !reconfigure && current.is_empty() {
            return SkipAfterEdit::Unset;
        }
        return SkipAfterEdit::Keep;
    }
    if line == "-" {
        return SkipAfterEdit::Set(Vec::new());
    }
    SkipAfterEdit::Set(parse_name_list(&line))
}

fn print_summary(doc: &DocumentMut, path: &Path) {
    let paths = doc.get("paths").and_then(|i| i.as_table());
    let build = doc.get("build").and_then(|i| i.as_table());
    let ram = doc.get("ramdisk").and_then(|i| i.as_table());
    let repos = doc.get("repositories").and_then(|i| i.as_table());
    println!();
    println!("{}", "==> Summary".green().bold());
    println!("    {}", path.display());
    if let Some(p) = paths {
        println!("    packages_path = {}", get_str(p, "packages_path", ""));
        println!(
            "    ready_made_packages_path = {}",
            get_str(p, "ready_made_packages_path", "")
        );
        println!(
            "    default_environment = {}",
            build
                .map(|b| get_str(b, "default_environment", "local"))
                .unwrap_or_else(|| "local".into())
        );
    }
    if let Some(r) = repos {
        let names: Vec<String> = repo_entries(r).into_iter().map(|(n, _)| n).collect();
        println!(
            "    repositories = {}  (default: {})",
            if names.is_empty() {
                "(none)".into()
            } else {
                names.join(", ")
            },
            get_str(r, "default", "")
        );
    }
    if let Some(r) = ram {
        println!(
            "    ramdisk.enabled = {}",
            if get_bool(r, "enabled", false) {
                "yes"
            } else {
                "no"
            }
        );
    }
    let manual = get_string_array(doc.as_table(), "manual_update_packages");
    println!(
        "    manual_update_packages = {}",
        if manual.is_empty() {
            "(none)".into()
        } else {
            manual.join(", ")
        }
    );
}

fn prompt_path(
    title: &str,
    explanation: &str,
    current: &str,
    suggested: &str,
    key: &str,
    reconfigure: bool,
) -> String {
    loop {
        let value = prompt_string(title, explanation, current, suggested, reconfigure);
        match validate_user_path(key, &value) {
            Ok(()) => return value,
            Err(e) => println!("    {} {e}", "Invalid:".red()),
        }
    }
}

fn prompt_command(
    title: &str,
    explanation: &str,
    current: &str,
    suggested: &str,
    reconfigure: bool,
) -> String {
    loop {
        let value = prompt_string(title, explanation, current, suggested, reconfigure);
        match validate_command(&value) {
            Ok(()) => return value,
            Err(e) => println!("    {} {e}", "Invalid:".red()),
        }
    }
}

fn validate_command(cmd: &str) -> Result<(), String> {
    crate::utils::parse_command_argv(cmd).map(|_| ())
}

fn validate_user_path(key: &str, raw: &str) -> Result<(), String> {
    let expanded = config::expand_user_path(raw);
    crate::utils::validate_config_path(key, &expanded.to_string_lossy())
}

fn validate_mount_point(raw: &str) -> Result<(), String> {
    let expanded = config::expand_user_path(raw);
    let s = expanded.to_string_lossy();
    crate::utils::validate_config_path("ramdisk.mount_point", &s)?;
    crate::ramdisk::validate_ramdisk_mount_point(&s)
}

fn validate_repo_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err("name cannot be empty".into());
    };
    if !first.is_ascii_alphabetic() {
        return Err("name must start with a letter".into());
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("use only letters, digits, '-' or '_'".into());
    }
    if name == "default" {
        return Err("the name 'default' is reserved".into());
    }
    Ok(())
}

fn validate_repo_url(url: &str) -> Result<(), String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("URL cannot be empty".into());
    }
    if url.starts_with("https://") || url.starts_with("http://") || url.starts_with("git@") {
        Ok(())
    } else {
        Err("URL must start with https://, http://, or git@".into())
    }
}

fn read_line(prompt: &str) -> String {
    print!("{prompt}");
    let _ = io::stdout().flush();
    let mut buf = String::new();
    if io::stdin().read_line(&mut buf).is_err() {
        die!("Failed to read stdin");
    }
    buf.trim().to_string()
}

fn print_field(title: &str, explanation: &str) {
    println!("  {}", title.bold());
    for line in explanation.lines() {
        let line = line.trim();
        if !line.is_empty() {
            println!("    {line}");
        }
    }
}

fn tags(is_current: bool, is_suggested: bool) -> String {
    let mut parts = Vec::new();
    if is_current {
        parts.push(format!("{}", "(current)".green()));
    }
    if is_suggested {
        parts.push(format!("{}", "(Suggested)".green()));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" {}", parts.join(" "))
    }
}

fn print_current_suggested(current: &str, suggested: Option<&str>, show_current_tag: bool) {
    let matches_suggested = suggested.is_some_and(|s| s == current);
    print!("    {} {current}", "Current:".dimmed());
    println!("{}", tags(show_current_tag, matches_suggested));
    if let Some(s) = suggested
        && !matches_suggested
    {
        println!(
            "    {} {} {}",
            "Suggested:".dimmed(),
            s,
            "(Suggested)".green()
        );
    }
}

struct Choice {
    value: String,
    label: String,
    help: String,
    suggested: bool,
}

impl Choice {
    fn new(
        value: impl Into<String>,
        label: impl Into<String>,
        help: impl Into<String>,
        suggested: bool,
    ) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            help: help.into(),
            suggested: suggested,
        }
    }
}

fn prompt_choice(
    title: &str,
    explanation: &str,
    options: &[Choice],
    current: Option<&str>,
    reconfigure: bool,
) -> String {
    print_field(title, explanation);
    println!("    Choose a number, or type the value.");
    for (i, opt) in options.iter().enumerate() {
        let is_current = current.is_some_and(|c| c.eq_ignore_ascii_case(&opt.value));
        println!(
            "    [{}] {}{}",
            i + 1,
            opt.label,
            tags(reconfigure && is_current, opt.suggested)
        );
        for line in opt.help.lines() {
            let line = line.trim();
            if !line.is_empty() {
                println!("        {line}");
            }
        }
    }
    let default_idx = if reconfigure {
        current.and_then(|c| {
            options
                .iter()
                .position(|o| o.value.eq_ignore_ascii_case(c))
                .map(|i| i + 1)
        })
    } else {
        options
            .iter()
            .position(|o| o.suggested)
            .or_else(|| {
                current.and_then(|c| options.iter().position(|o| o.value.eq_ignore_ascii_case(c)))
            })
            .map(|i| i + 1)
    };
    let hint = match default_idx {
        Some(i) => format!("    Choice [{i}]: "),
        None => "    Choice: ".into(),
    };
    loop {
        let line = read_line(&hint);
        if line.is_empty() {
            if let Some(i) = default_idx {
                return options[i - 1].value.clone();
            }
            println!("    Please enter a number.");
            continue;
        }
        if let Ok(n) = line.parse::<usize>()
            && n >= 1
            && n <= options.len()
        {
            return options[n - 1].value.clone();
        }
        if let Some(opt) = options
            .iter()
            .find(|o| o.value.eq_ignore_ascii_case(&line) || o.label.eq_ignore_ascii_case(&line))
        {
            return opt.value.clone();
        }
        println!("    Invalid choice.");
    }
}

fn prompt_bool(
    title: &str,
    explanation: &str,
    current: bool,
    suggested: bool,
    reconfigure: bool,
) -> bool {
    let current_s = if current { "yes" } else { "no" };
    let choice = prompt_choice(
        title,
        explanation,
        &[
            Choice::new("yes", "yes", "", suggested),
            Choice::new("no", "no", "", !suggested),
        ],
        Some(current_s),
        reconfigure,
    );
    choice == "yes"
}

fn prompt_string(
    title: &str,
    explanation: &str,
    current: &str,
    suggested: &str,
    reconfigure: bool,
) -> String {
    print_field(title, explanation);
    print_current_suggested(current, Some(suggested), reconfigure);
    let line = read_line(&format!("    [{current}]: "));
    if line.is_empty() {
        current.to_string()
    } else {
        line
    }
}

fn prompt_usize(
    title: &str,
    explanation: &str,
    current: usize,
    suggested: usize,
    min: usize,
    reconfigure: bool,
) -> usize {
    loop {
        let line = prompt_string(
            title,
            explanation,
            &current.to_string(),
            &suggested.to_string(),
            reconfigure,
        );
        match line.parse::<usize>() {
            Ok(n) if n >= min => return n,
            Ok(_) => println!("    {} must be >= {min}", "Invalid:".red()),
            Err(_) => println!("    {} enter a whole number", "Invalid:".red()),
        }
    }
}

fn prompt_optional_usize(
    title: &str,
    explanation: &str,
    current: Option<usize>,
    suggested: Option<usize>,
    reconfigure: bool,
) -> Option<usize> {
    let shown = current
        .map(|n| n.to_string())
        .unwrap_or_else(|| "(unset)".into());
    let suggested_s = suggested
        .map(|n| n.to_string())
        .unwrap_or_else(|| "(unset)".into());
    print_field(title, explanation);
    print_current_suggested(&shown, Some(&suggested_s), reconfigure);
    loop {
        let line = read_line(&format!("    [{shown}]: "));
        if line.is_empty() {
            return current;
        }
        if line == "-" || line.eq_ignore_ascii_case("none") || line.eq_ignore_ascii_case("unset") {
            return None;
        }
        match line.parse::<usize>() {
            Ok(n) if n >= 1 => return Some(n),
            Ok(_) => println!("    {} must be >= 1", "Invalid:".red()),
            Err(_) => {
                println!(
                    "    {} enter a whole number, '-' to clear, or Enter to keep",
                    "Invalid:".red()
                );
            }
        }
    }
}

fn prompt_optional_string(
    title: &str,
    explanation: &str,
    current: Option<&str>,
    suggested: Option<&str>,
    reconfigure: bool,
) -> Option<String> {
    let shown = current.unwrap_or("(unset)");
    let suggested_s = suggested.unwrap_or("(unset)");
    print_field(title, explanation);
    print_current_suggested(shown, Some(suggested_s), reconfigure);
    let line = read_line(&format!("    [{shown}]: "));
    if line.is_empty() {
        return current.map(str::to_string);
    }
    if line == "-" {
        return None;
    }
    Some(line)
}

fn prompt_string_list(
    title: &str,
    explanation: &str,
    current: &[String],
    reconfigure: bool,
) -> Vec<String> {
    let shown = if current.is_empty() {
        "(empty)".to_string()
    } else {
        current.join(", ")
    };
    print_field(title, explanation);
    print_current_suggested(&shown, Some("(empty)"), reconfigure);
    let line = read_line(&format!("    [{shown}]: "));
    if line.is_empty() {
        return current.to_vec();
    }
    if line == "-" {
        return Vec::new();
    }
    parse_name_list(&line)
}

fn parse_name_list(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = raw
        .split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    out.sort();
    out.dedup();
    out
}

fn table_mut<'a>(doc: &'a mut DocumentMut, name: &str) -> &'a mut Table {
    doc.entry(name)
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .unwrap_or_else(|| die!("[{name}] is not a table"))
}

fn get_str(table: &Table, key: &str, default: &str) -> String {
    table
        .get(key)
        .and_then(|i| i.as_str())
        .unwrap_or(default)
        .to_string()
}

fn get_bool(table: &Table, key: &str, default: bool) -> bool {
    table.get(key).and_then(|i| i.as_bool()).unwrap_or(default)
}

fn get_usize(table: &Table, key: &str, default: usize) -> usize {
    table
        .get(key)
        .and_then(|i| i.as_integer())
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(default)
}

fn get_optional_usize(table: &Table, key: &str) -> Option<usize> {
    table
        .get(key)
        .and_then(|i| i.as_integer())
        .and_then(|n| usize::try_from(n).ok())
}

fn get_optional_str(table: &Table, key: &str) -> Option<String> {
    table
        .get(key)
        .and_then(|i| i.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn get_string_array(table: &Table, key: &str) -> Vec<String> {
    let Some(arr) = table.get(key).and_then(|i| i.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

fn get_root_str(doc: &DocumentMut, key: &str, default: &str) -> String {
    get_str(doc.as_table(), key, default)
}

fn get_root_bool(doc: &DocumentMut, key: &str, default: bool) -> bool {
    get_bool(doc.as_table(), key, default)
}

fn set_str(table: &mut Table, key: &str, value: &str) {
    if table.get(key).and_then(|i| i.as_str()) == Some(value) {
        return;
    }
    table[key] = Item::Value(Value::from(value));
}

fn set_bool(table: &mut Table, key: &str, value: bool) {
    if table.get(key).and_then(|i| i.as_bool()) == Some(value) {
        return;
    }
    table[key] = Item::Value(Value::from(value));
}

fn set_usize(table: &mut Table, key: &str, value: usize) {
    if table
        .get(key)
        .and_then(|i| i.as_integer())
        .and_then(|n| usize::try_from(n).ok())
        == Some(value)
    {
        return;
    }
    table[key] = Item::Value(Value::from(value as i64));
}

fn set_optional_usize(table: &mut Table, key: &str, value: Option<usize>) {
    match value {
        Some(n) => set_usize(table, key, n),
        None => {
            table.remove(key);
        }
    }
}

fn set_optional_str(table: &mut Table, key: &str, value: Option<&str>) {
    match value {
        Some(s) => set_str(table, key, s),
        None => {
            table.remove(key);
        }
    }
}

fn set_string_array(table: &mut Table, key: &str, items: &[String]) {
    if get_string_array(table, key) == items {
        return;
    }
    let mut arr = Array::new();
    for s in items {
        arr.push(s.as_str());
    }
    table[key] = Item::Value(Value::Array(arr));
}

fn set_root_str(doc: &mut DocumentMut, key: &str, value: &str) {
    set_str(doc.as_table_mut(), key, value);
}

fn set_root_bool(doc: &mut DocumentMut, key: &str, value: bool) {
    set_bool(doc.as_table_mut(), key, value);
}

fn repo_entries(table: &Table) -> Vec<(String, String)> {
    table
        .iter()
        .filter(|(k, _)| *k != "default")
        .filter_map(|(k, v)| v.as_str().map(|s| (k.to_string(), s.to_string())))
        .collect()
}

fn suggested_default_name(repos: &[(String, String)]) -> String {
    for known in ["arch", "aur", "cachyos"] {
        if repos.iter().any(|(n, _)| n == known) {
            return known.to_string();
        }
    }
    repos
        .first()
        .map(|(n, _)| n.clone())
        .unwrap_or_else(|| "arch".into())
}

fn apply_repo_list(table: &mut Table, repos: &[(String, String)], default: &str) {
    let keep: HashSet<&str> = repos.iter().map(|(n, _)| n.as_str()).collect();
    let to_remove: Vec<String> = table
        .iter()
        .map(|(k, _)| k.to_string())
        .filter(|k| k != "default" && !keep.contains(k.as_str()))
        .collect();
    for k in to_remove {
        table.remove(&k);
    }
    for (name, url) in repos {
        set_str(table, name, url);
    }
    set_str(table, "default", default);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_template_validates() {
        Config::from_toml_text(example_config_text()).expect("abs.toml.example must validate");
    }

    #[test]
    fn wizard_defaults_roundtrip_document() {
        let mut doc: DocumentMut = example_config_text().parse().unwrap();
        let paths = table_mut(&mut doc, "paths");
        assert_eq!(get_str(paths, "packages_path", ""), SUGGEST_PACKAGES_PATH);
        set_str(paths, "packages_path", SUGGEST_PACKAGES_PATH);
        let build = table_mut(&mut doc, "build");
        set_str(build, "default_environment", "chroot");
        assert_eq!(get_str(build, "default_environment", ""), "chroot");
        Config::from_toml_text(&doc.to_string()).expect("mutated example still validates");
    }

    #[test]
    fn parse_name_list_splits_commas_and_spaces() {
        assert_eq!(
            parse_name_list("mesa, lib32-mesa  firefox"),
            vec!["firefox", "lib32-mesa", "mesa"]
        );
        assert!(parse_name_list("").is_empty());
    }

    #[test]
    fn unchanged_string_preserves_example_text() {
        let mut doc: DocumentMut = example_config_text().parse().unwrap();
        let text_before = doc.to_string();
        let paths = table_mut(&mut doc, "paths");
        let current = get_str(paths, "packages_path", "");
        set_str(paths, "packages_path", &current);
        assert_eq!(doc.to_string(), text_before);
    }

    #[test]
    fn repo_name_and_url_validation() {
        assert!(validate_repo_name("myrepo").is_ok());
        assert!(validate_repo_name("my-repo_1").is_ok());
        assert!(validate_repo_name("default").is_err());
        assert!(validate_repo_name("1bad").is_err());
        assert!(validate_repo_url("https://example.com/foo.git").is_ok());
        assert!(validate_repo_url("git@github.com:org/repo.git").is_ok());
        assert!(validate_repo_url("ftp://nope").is_err());
    }

    #[test]
    fn apply_repo_list_adds_removes_and_sets_default() {
        let mut doc: DocumentMut = example_config_text().parse().unwrap();
        let repos = table_mut(&mut doc, "repositories");
        let mut list = repo_entries(repos);
        assert!(list.iter().any(|(n, _)| n == "arch"));
        list.retain(|(n, _)| n != "cachyos");
        list.push(("myrepo".into(), "https://example.com/pkgbuilds.git".into()));
        apply_repo_list(repos, &list, "aur");
        let repos = table_mut(&mut doc, "repositories");
        let names: Vec<_> = repo_entries(repos).into_iter().map(|(n, _)| n).collect();
        assert!(names.contains(&"arch".into()));
        assert!(names.contains(&"aur".into()));
        assert!(names.contains(&"myrepo".into()));
        assert!(!names.contains(&"cachyos".into()));
        assert_eq!(get_str(repos, "default", ""), "aur");
        Config::from_toml_text(&doc.to_string()).expect("repo edits still validate");
    }

    #[test]
    fn unique_backup_path_uses_bak_then_numbers() {
        let dir = std::env::temp_dir().join(format!(
            "abs_wizard_bak_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("abs.toml");
        fs::write(&cfg, "current\n").unwrap();
        let bak = unique_backup_path(&cfg);
        assert_eq!(bak.file_name().unwrap(), "abs.toml.bak");
        fs::write(&bak, "first\n").unwrap();
        let bak1 = unique_backup_path(&cfg);
        assert_eq!(bak1.file_name().unwrap(), "abs.toml.bak.1");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_existing_config_copies_contents_and_leaves_original() {
        let dir = std::env::temp_dir().join(format!(
            "abs_wizard_bak_copy_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("abs.toml");
        fs::write(&cfg, "old-config\n").unwrap();
        let bak = backup_existing_config(&cfg).unwrap();
        assert_eq!(bak, dir.join("abs.toml.bak"));
        assert_eq!(fs::read_to_string(&bak).unwrap(), "old-config\n");
        assert_eq!(fs::read_to_string(&cfg).unwrap(), "old-config\n");
        fs::write(&cfg, "new-config\n").unwrap();
        let bak1 = backup_existing_config(&cfg).unwrap();
        assert_eq!(bak1, dir.join("abs.toml.bak.1"));
        assert_eq!(fs::read_to_string(&bak1).unwrap(), "new-config\n");
        assert_eq!(fs::read_to_string(&bak).unwrap(), "old-config\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn suggested_default_prefers_arch_then_aur() {
        let only_custom = vec![("mine".into(), "https://example.com/x.git".into())];
        assert_eq!(suggested_default_name(&only_custom), "mine");
        let aur_only = vec![
            ("mine".into(), "https://example.com/x.git".into()),
            ("aur".into(), "https://aur.archlinux.org".into()),
        ];
        assert_eq!(suggested_default_name(&aur_only), "aur");
    }
}
