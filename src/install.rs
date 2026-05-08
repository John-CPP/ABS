use crate::config::Config;
use crate::utils::{run_command, run_command_with_output, run_command_with_output_env};
use crate::{blog, ewarn, vlog};
use colored::Colorize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

fn package_name_from_file(pkg_file: &Path) -> Option<String> {
    let output = run_command_with_output(
        "pacman",
        &["-Qp", pkg_file.to_string_lossy().as_ref()],
        None::<&str>,
    )
    .ok()?;
    output.split_whitespace().next().map(|s| s.to_string())
}

fn resolve_packagelist_line(
    line: &str,
    repo_dir: &Path,
    ready_packages_path: &str,
) -> Option<PathBuf> {
    let trimmed = line.trim().trim_start_matches("./");
    if trimmed.is_empty() {
        return None;
    }
    let p = PathBuf::from(trimmed);
    if p.is_absolute() && p.exists() {
        return Some(p);
    }
    // makepkg --packagelist usually prints a bare filename; artifacts live under PKGDEST.
    let under_dest = PathBuf::from(ready_packages_path).join(trimmed);
    if under_dest.exists() {
        return Some(under_dest);
    }
    let under_repo = repo_dir.join(trimmed);
    if under_repo.exists() {
        return Some(under_repo);
    }
    p.exists().then_some(p)
}

fn collect_candidate_files(
    pkg_input: &str,
    base_pkg_name: &str,
    repo_dir: Option<&Path>,
    ready_packages_path: &str,
) -> Vec<PathBuf> {
    if let Some(dir) = repo_dir
        && let Ok(output) = run_command_with_output_env(
            "makepkg",
            &["--packagelist"],
            Some(dir),
            &[("PKGDEST", ready_packages_path)],
        )
    {
        let mut files = Vec::new();
        for line in output.lines() {
            if let Some(p) = resolve_packagelist_line(line, dir, ready_packages_path) {
                files.push(p);
            }
        }
        files.sort();
        files.dedup();
        if !files.is_empty() {
            return files;
        }
    }

    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(ready_packages_path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_file() {
                continue;
            }
            let name = match p.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            if !name.ends_with(".pkg.tar.zst") {
                continue;
            }
            if name.starts_with(&format!("{}-", base_pkg_name))
                || name.starts_with(&format!("{}-", pkg_input))
            {
                files.push(p);
            }
        }
    }
    files.sort();
    files
}

fn prompt_for_selection(files: &[PathBuf]) -> Option<Vec<PathBuf>> {
    if files.is_empty() {
        return Some(Vec::new());
    }

    if files.len() == 1 {
        println!("==> Only 1 package available: {}", files[0].display());
        loop {
            print!("Install it? [Y/n] ");
            let _ = io::stdout().flush();
            let mut input = String::new();
            if io::stdin().read_line(&mut input).is_err() {
                return None;
            }
            let v = input.trim().to_lowercase();
            if v.is_empty() || v == "y" || v == "yes" {
                return Some(vec![files[0].clone()]);
            }
            if v == "n" || v == "no" {
                return Some(Vec::new());
            }
        }
    }

    println!("==> Packages available for installation:");
    for (idx, f) in files.iter().enumerate() {
        println!("  {}) {}", idx + 1, f.display());
    }

    loop {
        print!("Enter numbers to install (e.g. 1,2 or 1-3), empty=all, n=skip: ");
        let _ = io::stdout().flush();
        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            return None;
        }
        let v = input.trim().replace(' ', "");
        if v.is_empty() {
            return Some(files.to_vec());
        }
        if v.eq_ignore_ascii_case("n") {
            return Some(Vec::new());
        }

        let mut selected = Vec::new();
        let mut valid = true;
        for part in v.split(',') {
            if part.is_empty() {
                continue;
            }
            if let Some((a, b)) = part.split_once('-') {
                let start = match a.parse::<usize>() {
                    Ok(x) => x,
                    Err(_) => {
                        valid = false;
                        break;
                    }
                };
                let end = match b.parse::<usize>() {
                    Ok(x) => x,
                    Err(_) => {
                        valid = false;
                        break;
                    }
                };
                if start == 0 || end == 0 || start > end {
                    valid = false;
                    break;
                }
                for i in start..=end {
                    if let Some(f) = files.get(i - 1) {
                        selected.push(f.clone());
                    }
                }
            } else {
                let idx = match part.parse::<usize>() {
                    Ok(x) => x,
                    Err(_) => {
                        valid = false;
                        break;
                    }
                };
                if idx == 0 {
                    valid = false;
                    break;
                }
                if let Some(f) = files.get(idx - 1) {
                    selected.push(f.clone());
                }
            }
        }
        if valid && !selected.is_empty() {
            selected.sort();
            selected.dedup();
            return Some(selected);
        }
        println!("Invalid selection.");
    }
}

fn dependency_names_from_file(pkg_file: &Path) -> Vec<String> {
    let output = run_command_with_output(
        "bsdtar",
        &["-xOf", pkg_file.to_string_lossy().as_ref(), ".PKGINFO"],
        None::<&str>,
    );
    let Ok(text) = output else {
        return Vec::new();
    };

    let mut deps = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(dep) = trimmed.strip_prefix("depend = ") {
            let name = dep.split(['<', '>', '=']).next().unwrap_or_default().trim();
            if !name.is_empty() {
                deps.push(name.to_string());
            }
        }
    }
    deps
}

fn auto_include_local_dependencies(
    selected: &mut Vec<PathBuf>,
    available: &[PathBuf],
    verbose: bool,
) {
    let mut file_by_pkg: HashMap<String, PathBuf> = HashMap::new();
    for file in available {
        if let Some(pkg_name) = package_name_from_file(file) {
            file_by_pkg.insert(pkg_name, file.clone());
        }
    }

    let mut selected_pkgs: HashSet<String> = HashSet::new();
    for file in selected.iter() {
        if let Some(pkg_name) = package_name_from_file(file) {
            selected_pkgs.insert(pkg_name);
        }
    }

    loop {
        let mut changed = false;
        let current = selected.clone();
        for file in &current {
            for dep_name in dependency_names_from_file(file) {
                if selected_pkgs.contains(&dep_name) {
                    continue;
                }
                if let Some(dep_file) = file_by_pkg.get(&dep_name) {
                    vlog!(
                        verbose,
                        "Auto-including dependency from built set: {}",
                        dep_name
                    );
                    selected_pkgs.insert(dep_name);
                    selected.push(dep_file.clone());
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

pub fn install_artifacts(
    pkg_input: &str,
    base_pkg_name: &str,
    repo_dir: Option<&Path>,
    config: &Config,
    verbose: bool,
) {
    let mut files = collect_candidate_files(
        pkg_input,
        base_pkg_name,
        repo_dir,
        &config.paths.ready_made_packages_path,
    );
    if files.is_empty() {
        vlog!(verbose, "No installable artifacts found for {}", pkg_input);
        return;
    }

    files.retain(|f| {
        let pkg_name = package_name_from_file(f);
        if let Some(name) = pkg_name
            && config.skip_install_packages.contains(&name)
        {
            vlog!(verbose, "Skipping ignored package artifact: {}", name);
            return false;
        }
        true
    });

    let Some(selected) = prompt_for_selection(&files) else {
        ewarn!("Failed to read install selection from stdin.");
        return;
    };
    if selected.is_empty() {
        blog!("Skipping installation.");
        return;
    }
    let mut selected = selected;
    auto_include_local_dependencies(&mut selected, &files, verbose);
    selected.sort();
    selected.dedup();

    let mut args: Vec<String> = vec![
        "pacman".to_string(),
        "-U".to_string(),
        "--noconfirm".to_string(),
    ];
    args.extend(selected.iter().map(|p| p.to_string_lossy().to_string()));
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    if let Err(e) = run_command("sudo", &refs, None::<&str>) {
        ewarn!("Failed to install selected packages: {}", e);
    } else {
        blog!("Installed selected package artifacts.");
    }
}

pub fn install_from_ready_dir(
    pkg_input: &str,
    base_pkg_name: &str,
    config: &Config,
    verbose: bool,
) {
    install_artifacts(pkg_input, base_pkg_name, None, config, verbose);
}
