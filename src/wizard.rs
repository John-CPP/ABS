//! Interactive config wizard (add/remove/edit/hold) with CLI prefills.

use crate::cli::Cli;
use crate::config::Config;
use crate::config_edit::{
    self, ConfigListKind, PackageEditFields, print_reproduce_undo, shell_join_packages,
};
use crate::die;
use crate::held::{self, snapshot_trigger_versions, split_pkgver_pkgrel};
use crate::utils::{pacman_query_version, read_pkg_full_version_from_dir};
use crate::{blog, ewarn};
use colored::Colorize;
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardAction {
    Add,
    Remove,
    Edit,
    Hold,
}

impl WizardAction {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "add" => Ok(Self::Add),
            "remove" | "rm" | "del" => Ok(Self::Remove),
            "edit" => Ok(Self::Edit),
            "hold" => Ok(Self::Hold),
            other => Err(format!(
                "unknown wizard action {:?}; expected add, remove, edit, or hold",
                other
            )),
        }
    }
}

fn read_line(prompt: &str) -> String {
    print!("{}", prompt);
    let _ = io::stdout().flush();
    let mut buf = String::new();
    if io::stdin().read_line(&mut buf).is_err() {
        die!("Failed to read stdin");
    }
    buf.trim().to_string()
}

fn prompt_choice(prompt: &str, options: &[&str], current: Option<&str>) -> String {
    println!("{}", prompt);
    for (i, opt) in options.iter().enumerate() {
        let marker = if current.is_some_and(|c| c.eq_ignore_ascii_case(opt)) {
            format!(" {}", "(current)".green())
        } else {
            String::new()
        };
        println!("  [{}] {}{}", i + 1, opt, marker);
    }
    let default_idx = current.and_then(|c| {
        options
            .iter()
            .position(|o| o.eq_ignore_ascii_case(c))
            .map(|i| i + 1)
    });
    let hint = match default_idx {
        Some(i) => format!("Choice [{}]: ", i),
        None => "Choice: ".into(),
    };
    loop {
        let line = read_line(&hint);
        if line.is_empty() {
            if let Some(i) = default_idx {
                return options[i - 1].to_string();
            }
            println!("Please enter a number.");
            continue;
        }
        if let Ok(n) = line.parse::<usize>()
            && n >= 1
            && n <= options.len()
        {
            return options[n - 1].to_string();
        }
        // Allow typing the value directly
        if let Some(opt) = options.iter().find(|o| o.eq_ignore_ascii_case(&line)) {
            return (*opt).to_string();
        }
        println!("Invalid choice.");
    }
}

fn parse_trigger_list(raw: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for item in raw {
        for part in item.split(',') {
            let p = part.trim();
            if !p.is_empty() {
                out.push(p.to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn resolve_action(cli: &Cli) -> WizardAction {
    if let Some(raw) = &cli.wizard {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return WizardAction::parse(trimmed).unwrap_or_else(|e| die!("{}", e));
        }
    }
    let choice = prompt_choice("Select action:", &["add", "remove", "edit", "hold"], None);
    WizardAction::parse(&choice).unwrap_or_else(|e| die!("{}", e))
}

fn resolve_list(cli: &Cli) -> ConfigListKind {
    if let Some(name) = &cli.pkg_list {
        return ConfigListKind::parse(name).unwrap_or_else(|e| die!("{}", e));
    }
    let labels: Vec<&str> = ConfigListKind::all()
        .iter()
        .map(|k| k.canonical_name())
        .collect();
    let choice = prompt_choice("Select package list:", &labels, None);
    ConfigListKind::parse(&choice).unwrap_or_else(|e| die!("{}", e))
}

fn resolve_packages(cli: &Cli, allow_empty: bool) -> Vec<String> {
    let mut pkgs: Vec<String> = cli
        .packages
        .iter()
        .map(|s| {
            // Strip bracket attrs if user passed specs; wizard wants bare names.
            s.split_once('[')
                .map(|(n, _)| n.to_string())
                .unwrap_or_else(|| s.clone())
        })
        .filter(|s| !s.is_empty())
        .collect();
    if !pkgs.is_empty() {
        return pkgs;
    }
    let line = read_line("Package name(s) (space or comma separated): ");
    for part in line.split(|c: char| c.is_whitespace() || c == ',') {
        let p = part.trim();
        if !p.is_empty() {
            pkgs.push(p.to_string());
        }
    }
    if pkgs.is_empty() && !allow_empty {
        die!("No packages specified.");
    }
    pkgs
}

fn run_add(cli: &Cli) {
    let kind = resolve_list(cli);
    let pkgs = resolve_packages(cli, false);
    let added = config_edit::list_add(kind, &pkgs);
    if added.is_empty() {
        blog!("Nothing to add (already present).");
    } else {
        blog!("Added to {}: {}", kind.canonical_name(), added.join(", "));
    }
    let pkg_args = shell_join_packages(&pkgs);
    print_reproduce_undo(
        &format!("abs --list-add={} {}", kind.canonical_name(), pkg_args),
        &format!("abs --list-remove={} {}", kind.canonical_name(), pkg_args),
    );
}

fn run_remove(cli: &Cli) {
    let kind = resolve_list(cli);
    let pkgs = resolve_packages(cli, false);
    let removed = config_edit::list_remove(kind, &pkgs);
    if removed.is_empty() {
        blog!("Nothing to remove (not present).");
    } else {
        blog!(
            "Removed from {}: {}",
            kind.canonical_name(),
            removed.join(", ")
        );
    }
    let pkg_args = shell_join_packages(&pkgs);
    print_reproduce_undo(
        &format!("abs --list-remove={} {}", kind.canonical_name(), pkg_args),
        &format!("abs --list-add={} {}", kind.canonical_name(), pkg_args),
    );
}

fn prompt_optional_string(label: &str, current: Option<&str>) -> Option<String> {
    let cur = current.unwrap_or("");
    let highlight = if cur.is_empty() {
        String::new()
    } else {
        format!(" {}", format!("(current: {})", cur).green())
    };
    let line = read_line(&format!(
        "{}{} (empty=keep{}, '-'=clear): ",
        label,
        highlight,
        if cur.is_empty() { " / unset" } else { "" }
    ));
    if line.is_empty() {
        return None; // keep — signal by returning None and caller skips
    }
    if line == "-" {
        return Some(String::new());
    }
    Some(line)
}

fn run_edit(cli: &Cli, config: &Config) {
    let pkgs = resolve_packages(cli, false);
    if pkgs.len() != 1 {
        die!("edit action expects exactly one package");
    }
    let pkg = &pkgs[0];
    let current = config_edit::read_package_fields(pkg);
    let default_env = config.build.default_environment.as_str();

    blog!("Editing compilation options for {}...", pkg);

    let build_env = {
        let cur = current.build_env.as_deref().unwrap_or(default_env);
        let choice = prompt_choice("build_env:", &["local", "chroot", "(keep)"], Some(cur));
        if choice == "(keep)" {
            None
        } else {
            Some(choice)
        }
    };

    let tests = {
        let cur = match current.tests {
            Some(true) => "true",
            Some(false) => "false",
            None => "(unset)",
        };
        let choice = prompt_choice("tests:", &["true", "false", "(keep)"], Some(cur));
        match choice.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    };

    let compiler = prompt_optional_string("compiler", current.compiler.as_deref());
    let ramdisk = prompt_optional_string("ramdisk (wcp / disabled)", current.ramdisk.as_deref());
    let source = prompt_optional_string("source (repo name)", current.source.as_deref());

    let compilation_threads = {
        let cur_disp = match &current.compilation_threads {
            Some(Some(n)) => n.to_string(),
            Some(None) | None => "(unset)".into(),
        };
        let line = read_line(&format!(
            "compilation_threads {} (empty=keep, '-'=clear): ",
            format!("(current: {})", cur_disp).green()
        ));
        if line.is_empty() {
            None
        } else if line == "-" {
            Some(None)
        } else {
            match line.parse::<usize>() {
                Ok(n) => Some(Some(n)),
                Err(_) => die!("Invalid compilation_threads: {}", line),
            }
        }
    };

    let compile_alone = {
        let cur = match current.compile_alone {
            Some(true) => "true",
            Some(false) => "false",
            None => "(unset)",
        };
        let choice = prompt_choice("compile_alone:", &["true", "false", "(keep)"], Some(cur));
        match choice.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    };

    let ignore_already = {
        let cur = match current.ignore_already_made_packages {
            Some(Some(true)) => "true",
            Some(Some(false)) => "false",
            Some(None) | None => "(unset)",
        };
        let choice = prompt_choice(
            "ignore_already_made_packages:",
            &["true", "false", "(unset)", "(keep)"],
            Some(cur),
        );
        match choice.as_str() {
            "true" => Some(Some(true)),
            "false" => Some(Some(false)),
            "(unset)" => Some(None),
            _ => None,
        }
    };

    let fields = PackageEditFields {
        build_env,
        tests,
        compiler,
        ramdisk,
        compilation_threads,
        compile_alone,
        ignore_already_made_packages: ignore_already,
        source,
    };

    if let Err(e) = config_edit::edit_package_fields(pkg, &fields) {
        die!("{}", e);
    }
    blog!("Updated [packages.{}]", pkg);

    // Reproduce/Undo for edit is approximate: undo clears the fields we set.
    let mut repro_parts = vec![format!("abs --wizard=edit {}", pkg)];
    let mut undo_note = format!("abs --configure  # manually revert [packages.{}]", pkg);
    let _ = (&mut repro_parts, &mut undo_note);
    print_reproduce_undo(
        &format!("abs --wizard=edit {}", pkg),
        &format!(
            "abs --configure  # manually revert [packages.{}] options",
            pkg
        ),
    );
}

fn resolve_hold_version(cli: &Cli, pkg: &str, config: &Config) -> String {
    if let Some(v) = &cli.hold_version {
        split_pkgver_pkgrel(v).unwrap_or_else(|e| die!("{}", e));
        return v.clone();
    }

    let installed = pacman_query_version(pkg).ok().flatten();
    let srcinfo = pkg_src_version(pkg, config);

    println!("Select version to hold for {}:", pkg.bold());
    let mut options: Vec<String> = Vec::new();
    if let Some(ref v) = installed {
        options.push(format!("{} (installed)", v));
    }
    if let Some(ref v) = srcinfo {
        let label = format!("{} (PKGBUILD)", v);
        if !options.iter().any(|o| o.starts_with(v.as_str())) {
            options.push(label);
        }
    }
    options.push("custom".into());

    let current_highlight = installed.as_deref();
    // Map display options for prompt — highlight installed entry
    let display: Vec<&str> = options.iter().map(|s| s.as_str()).collect();
    let default_label = installed.as_ref().map(|v| format!("{} (installed)", v));
    let choice = prompt_choice(
        "Versions:",
        &display,
        default_label.as_deref().or(current_highlight),
    );

    if choice == "custom" || choice.starts_with("custom") {
        let custom = read_line("Enter pkgver-pkgrel: ");
        split_pkgver_pkgrel(&custom).unwrap_or_else(|e| die!("{}", e));
        return custom;
    }
    // Strip " (installed)" / " (PKGBUILD)"
    choice
        .split_whitespace()
        .next()
        .unwrap_or(&choice)
        .to_string()
}

fn pkg_src_version(pkg: &str, config: &Config) -> Option<String> {
    let base = PathBuf::from(&config.paths.packages_path);
    // Try common layouts: packages_path/pkg and packages_path/repo/pkg
    let candidates = [
        base.join(pkg),
        base.join("arch").join(pkg),
        base.join("aur").join(pkg),
        base.join("cachyos").join(pkg),
    ];
    for dir in candidates {
        if dir.join("PKGBUILD").is_file()
            && let Ok(ver) = read_pkg_full_version_from_dir(&dir)
        {
            return Some(ver);
        }
    }
    None
}

fn resolve_triggers(cli: &Cli) -> HashMap<String, String> {
    let mut names = parse_trigger_list(&cli.trigger);
    if names.is_empty() {
        let line = read_line(
            "Trigger packages for on_packages_updated (comma/space separated, empty=none): ",
        );
        for part in line.split(|c: char| c.is_whitespace() || c == ',') {
            let p = part.trim();
            if !p.is_empty() {
                names.push(p.to_string());
            }
        }
    }
    snapshot_trigger_versions(&names)
}

fn run_hold(cli: &Cli, config: &Config) {
    let pkgs = resolve_packages(cli, false);
    if pkgs.len() != 1 {
        die!("hold expects exactly one package");
    }
    let pkg = pkgs[0].clone();
    let version = resolve_hold_version(cli, &pkg, config);
    let triggers = resolve_triggers(cli);

    let held = held::make_held_package(pkg.clone(), version.clone(), triggers.clone());
    if let Err(e) = config_edit::hold_package(&held) {
        die!("{}", e);
    }
    blog!("Held {} @ {}", pkg, version);
    if !triggers.is_empty() {
        let mut pairs: Vec<_> = triggers.iter().collect();
        pairs.sort_by(|a, b| a.0.cmp(b.0));
        for (n, v) in pairs {
            blog!("  trigger {} = {}", n, v);
        }
    }

    let mut repro = format!("abs --hold {} --hold-version={}", pkg, version);
    if !triggers.is_empty() {
        let mut names: Vec<_> = triggers.keys().cloned().collect();
        names.sort();
        repro.push_str(&format!(" --trigger={}", names.join(",")));
    }
    print_reproduce_undo(&repro, &format!("abs --unhold {}", pkg));
}

/// Entry point for `abs --wizard[=ACTION]`.
pub fn run_wizard(cli: &Cli, config: &Config) {
    let action = resolve_action(cli);
    match action {
        WizardAction::Add => run_add(cli),
        WizardAction::Remove => run_remove(cli),
        WizardAction::Edit => run_edit(cli, config),
        WizardAction::Hold => run_hold(cli, config),
    }
}

/// Non-interactive `--list-add`.
pub fn run_list_add(cli: &Cli) {
    let kind = cli
        .list_add
        .as_ref()
        .map(|s| ConfigListKind::parse(s))
        .transpose()
        .unwrap_or_else(|e| die!("{}", e))
        .unwrap_or_else(|| die!("--list-add requires a list name"));
    let pkgs = if cli.packages.is_empty() {
        die!("--list-add requires one or more package names");
    } else {
        cli.packages.clone()
    };
    let added = config_edit::list_add(kind, &pkgs);
    if added.is_empty() {
        blog!("Nothing to add (already present).");
    } else {
        blog!("Added to {}: {}", kind.canonical_name(), added.join(", "));
    }
    let pkg_args = shell_join_packages(&pkgs);
    print_reproduce_undo(
        &format!("abs --list-add={} {}", kind.canonical_name(), pkg_args),
        &format!("abs --list-remove={} {}", kind.canonical_name(), pkg_args),
    );
}

/// Non-interactive `--list-remove`.
pub fn run_list_remove(cli: &Cli) {
    let kind = cli
        .list_remove
        .as_ref()
        .map(|s| ConfigListKind::parse(s))
        .transpose()
        .unwrap_or_else(|e| die!("{}", e))
        .unwrap_or_else(|| die!("--list-remove requires a list name"));
    let pkgs = if cli.packages.is_empty() {
        die!("--list-remove requires one or more package names");
    } else {
        cli.packages.clone()
    };
    let removed = config_edit::list_remove(kind, &pkgs);
    if removed.is_empty() {
        blog!("Nothing to remove (not present).");
    } else {
        blog!(
            "Removed from {}: {}",
            kind.canonical_name(),
            removed.join(", ")
        );
    }
    let pkg_args = shell_join_packages(&pkgs);
    print_reproduce_undo(
        &format!("abs --list-remove={} {}", kind.canonical_name(), pkg_args),
        &format!("abs --list-add={} {}", kind.canonical_name(), pkg_args),
    );
}

/// Non-interactive `--hold PACKAGE`.
pub fn run_hold_cli(cli: &Cli, config: &Config) {
    let pkg = cli
        .hold
        .as_ref()
        .filter(|s| !s.is_empty())
        .cloned()
        .or_else(|| {
            cli.packages.first().map(|s| {
                s.split_once('[')
                    .map(|(n, _)| n.to_string())
                    .unwrap_or_else(|| s.clone())
            })
        })
        .unwrap_or_else(|| die!("--hold requires a package name"));

    let version = cli.hold_version.clone().unwrap_or_else(|| {
        // Fall back to installed version
        pacman_query_version(&pkg)
            .ok()
            .flatten()
            .unwrap_or_else(|| die!("--hold-version is required when package is not installed"))
    });
    split_pkgver_pkgrel(&version).unwrap_or_else(|e| die!("{}", e));

    let trigger_names = parse_trigger_list(&cli.trigger);
    let triggers = if trigger_names.is_empty() {
        HashMap::new()
    } else {
        snapshot_trigger_versions(&trigger_names)
    };

    let held = held::make_held_package(pkg.clone(), version.clone(), triggers.clone());
    if let Err(e) = config_edit::hold_package(&held) {
        die!("{}", e);
    }
    blog!("Held {} @ {}", pkg, version);

    let mut repro = format!("abs --hold {} --hold-version={}", pkg, version);
    if !trigger_names.is_empty() {
        repro.push_str(&format!(" --trigger={}", trigger_names.join(",")));
    }
    print_reproduce_undo(&repro, &format!("abs --unhold {}", pkg));
    let _ = config;
}

/// `--unhold`.
pub fn run_unhold(cli: &Cli) {
    let mut names = cli.unhold.clone();
    names.extend(cli.packages.clone());
    names.retain(|s| !s.is_empty());
    if names.is_empty() {
        die!("--unhold requires one or more package names");
    }
    let removed = config_edit::unhold_packages(&names);
    if removed.is_empty() {
        blog!("Nothing to unhold (not held).");
    } else {
        blog!("Unheld: {}", removed.join(", "));
    }
    let pkg_args = shell_join_packages(&names);
    // Undo cannot restore version/triggers without prior knowledge.
    print_reproduce_undo(
        &format!("abs --unhold {}", pkg_args),
        &format!(
            "abs --hold {} --hold-version=VERSION [--trigger=...]",
            names.first().map(|s| s.as_str()).unwrap_or("PACKAGE")
        ),
    );
}

/// `--hold-check`.
pub fn run_hold_check(cli: &Cli, config: &Config) {
    let filter: Vec<String> = if cli.packages.is_empty() {
        config
            .held_packages
            .iter()
            .map(|h| h.name.clone())
            .collect()
    } else {
        cli.packages.clone()
    };
    if filter.is_empty() {
        blog!("No held packages.");
        return;
    }
    for name in &filter {
        let Some(held) = held::find_held(config, name) else {
            ewarn!("{}: not held", name);
            continue;
        };
        let installed = pacman_query_version(name).ok().flatten();
        println!("{}", name.bold());
        println!("  held:      {}", held.version);
        match &installed {
            Some(v) if v == &held.version => {
                println!("  installed: {} {}", v, "(matches)".green());
            }
            Some(v) => {
                println!("  installed: {} {}", v, "(differs)".yellow());
            }
            None => println!("  installed: {}", "(not installed)".yellow()),
        }
        if held.auto_recompile_trigger.on_packages_updated.is_empty() {
            println!("  triggers:  (none)");
        } else {
            println!("  triggers (on_packages_updated):");
            let mut pairs: Vec<_> = held
                .auto_recompile_trigger
                .on_packages_updated
                .iter()
                .collect();
            pairs.sort_by(|a, b| a.0.cmp(b.0));
            for (trig, saved) in pairs {
                let cur = pacman_query_version(trig).ok().flatten();
                match cur {
                    Some(ref c) if c == saved => {
                        println!(
                            "    {} saved={} installed={} {}",
                            trig,
                            saved,
                            c,
                            "ok".green()
                        );
                    }
                    Some(ref c) => {
                        println!(
                            "    {} saved={} installed={} {}",
                            trig,
                            saved,
                            c,
                            "DRIFT".red().bold()
                        );
                    }
                    None => {
                        println!(
                            "    {} saved={} installed={} {}",
                            trig,
                            saved,
                            "-",
                            "MISSING".red().bold()
                        );
                    }
                }
            }
        }
    }
}
