use crate::config::Config;
use crate::utils::run_command;
use crate::{die, vlog};
use colored::Colorize;
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemUpdateMode {
    UpdateRepositories,
    PerformUpdateWithRefresh,
    PerformUpdateNoRefresh,
}

fn is_root() -> bool {
    if let Ok(output) = std::process::Command::new("id").arg("-u").output() {
        if let Ok(uid_str) = std::str::from_utf8(&output.stdout) {
            if let Ok(uid) = uid_str.trim().parse::<u32>() {
                return uid == 0;
            }
        }
    }
    if let Ok(user) = std::env::var("USER") {
        return user == "root";
    }
    false
}

fn transform_system_update_command(mut cmd_str: String, is_root_user: bool) -> String {
    let trimmed = cmd_str.trim();
    if (trimmed.starts_with("pacman ") || trimmed == "pacman")
        && !trimmed.starts_with("sudo ")
        && !is_root_user
    {
        cmd_str = format!("sudo {}", cmd_str);
    }
    cmd_str
}

/// Always appends `ignore_flag` for each entry in `ignore_packages` and `manual_update_packages`
/// (deduped), so repo packages never replace packages you build with emerge.
pub fn run_system_update(config: &Config, mode: SystemUpdateMode) {
    let mut cmd_str = match mode {
        SystemUpdateMode::UpdateRepositories => {
            config.system_update.command_to_update_repositories.clone()
        }
        SystemUpdateMode::PerformUpdateWithRefresh => {
            config.system_update.command_to_perform_system_update.clone()
        }
        SystemUpdateMode::PerformUpdateNoRefresh => {
            config.system_update.get_command_to_perform_system_update_no_refresh()
        }
    };

    cmd_str = transform_system_update_command(cmd_str, is_root());

    let mut seen = HashSet::new();
    for pkg in config
        .system_update
        .ignore_packages
        .iter()
        .chain(config.manual_update_packages.iter())
    {
        if seen.insert(pkg.clone()) {
            cmd_str.push_str(&format!(" {} {}", config.system_update.ignore_flag, pkg));
        }
    }

    vlog!("Executing system update: {}", cmd_str);

    // We run it via sh -c to allow complex yay commands from config
    if let Err(e) = run_command("sh", &["-c", &cmd_str], None::<&str>) {
        die!("System update failed: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transform_system_update_command() {
        // Non-root user: pacman commands should get sudo prepended
        assert_eq!(
            transform_system_update_command("pacman -Su".to_string(), false),
            "sudo pacman -Su"
        );
        assert_eq!(
            transform_system_update_command("pacman".to_string(), false),
            "sudo pacman"
        );

        // Root user: pacman commands should NOT get sudo prepended
        assert_eq!(
            transform_system_update_command("pacman -Su".to_string(), true),
            "pacman -Su"
        );

        // Already has sudo: should NOT get sudo prepended for either
        assert_eq!(
            transform_system_update_command("sudo pacman -Su".to_string(), false),
            "sudo pacman -Su"
        );
        assert_eq!(
            transform_system_update_command("sudo pacman -Su".to_string(), true),
            "sudo pacman -Su"
        );

        // Non-pacman command (e.g. yay): should NOT get sudo prepended
        assert_eq!(
            transform_system_update_command("yay -Su".to_string(), false),
            "yay -Su"
        );
    }
}
