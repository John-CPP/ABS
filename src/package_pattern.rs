//! Glob-style patterns (`qemu*`, `lib32-vulkan-*`) for package skip/ignore lists.

use regex::Regex;
use std::collections::HashSet;

use crate::utils::run_command_with_output_silent;
use crate::vlog;

pub fn is_glob_pattern(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?')
}

pub fn package_matches_pattern(name: &str, pattern: &str) -> bool {
    if !is_glob_pattern(pattern) {
        return name == pattern;
    }
    glob_pattern_to_regex(pattern)
        .is_some_and(|re| re.is_match(name))
}

pub fn package_matches_any_pattern(name: &str, patterns: &[String]) -> bool {
    patterns
        .iter()
        .any(|pattern| package_matches_pattern(name, pattern))
}

/// Expand literal names and glob patterns into concrete pacman package names for `--ignore`.
pub fn expand_package_patterns(patterns: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut globs = Vec::new();

    for pattern in patterns {
        if is_glob_pattern(pattern) {
            globs.push(pattern.as_str());
        } else if seen.insert(pattern.clone()) {
            out.push(pattern.clone());
        }
    }

    if globs.is_empty() {
        return out;
    }

    let candidates = pacman_package_names_for_glob_expansion();
    for pattern in globs {
        let mut matched = false;
        for name in &candidates {
            if package_matches_pattern(name, pattern) && seen.insert(name.clone()) {
                out.push(name.to_string());
                matched = true;
            }
        }
        if !matched {
            vlog!(
                "Package pattern '{pattern}' matched no installed or repo packages; skipping for --ignore"
            );
        }
    }

    out
}

fn glob_pattern_to_regex(glob: &str) -> Option<Regex> {
    let mut out = String::from("^");
    for c in glob.chars() {
        match c {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            '.' | '+' | '^' | '$' | '|' | '\\' | '(' | ')' | '[' | ']' | '{' | '}' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out.push('$');
    Regex::new(&out).ok()
}

fn pacman_package_names_for_glob_expansion() -> Vec<String> {
    static NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    NAMES
        .get_or_init(collect_pacman_package_names)
        .clone()
}

fn collect_pacman_package_names() -> Vec<String> {
    let mut names = HashSet::new();
    if let Ok(installed) = run_command_with_output_silent("pacman", &["-Qq"], None::<&str>) {
        for line in installed.lines() {
            let name = line.trim();
            if !name.is_empty() {
                names.insert(name.to_string());
            }
        }
    }
    if let Ok(sync) = run_command_with_output_silent("pacman", &["-Slq"], None::<&str>) {
        for line in sync.lines() {
            if let Some(name) = line.split_whitespace().nth(1) {
                let name = name.trim();
                if !name.is_empty() {
                    names.insert(name.to_string());
                }
            }
        }
    }
    let mut out: Vec<String> = names.into_iter().collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_pattern_matches_only_same_name() {
        assert!(package_matches_pattern("qemu-full", "qemu-full"));
        assert!(!package_matches_pattern("qemu-common", "qemu-full"));
    }

    #[test]
    fn trailing_star_matches_prefix() {
        assert!(package_matches_pattern("qemu-full", "qemu*"));
        assert!(package_matches_pattern("qemu-common", "qemu*"));
        assert!(package_matches_pattern("qemu-user-static", "qemu*"));
        assert!(!package_matches_pattern("libqemu", "qemu*"));
    }

    #[test]
    fn question_mark_matches_single_character() {
        assert!(package_matches_pattern("mesa", "mes?"));
        assert!(!package_matches_pattern("mes", "mes?"));
        assert!(!package_matches_pattern("mesas", "mes?"));
    }

    #[test]
    fn expand_keeps_literals_and_dedupes() {
        let expanded = expand_package_patterns(&[
            "mesa".into(),
            "mesa".into(),
            "curl".into(),
        ]);
        assert_eq!(expanded, vec!["mesa", "curl"]);
    }
}
