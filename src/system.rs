use crate::config::Config;
use crate::utils::{run_command, run_command_with_output};
use crate::{blog, die, ewarn};
use colored::Colorize;

pub fn check_updates() -> String {
    // checkupdates || yay -Qu
    match run_command_with_output("checkupdates", &[], None::<&str>) {
        Ok(output) => output,
        Err(e1) => match run_command_with_output("yay", &["-Qu"], None::<&str>) {
            Ok(output) => output,
            Err(e2) => {
                ewarn!(
                    "Could not query pending updates: checkupdates failed ({}); yay -Qu failed ({})",
                    e1,
                    e2
                );
                String::new()
            }
        },
    }
}

pub fn run_system_update(
    config: &Config,
    force_repo_update: bool,
    extra_ignored_packages: &[String],
) {
    let mut cmd_str = if force_repo_update {
        config.system_update.command_with_refresh.clone()
    } else {
        config.system_update.command.clone()
    };

    // Append ignore packages
    for pkg in &config.system_update.ignore_packages {
        cmd_str.push_str(&format!(" {} {}", config.system_update.ignore_flag, pkg));
    }
    for pkg in extra_ignored_packages {
        cmd_str.push_str(&format!(" {} {}", config.system_update.ignore_flag, pkg));
    }

    blog!("Executing system update: {}", cmd_str);

    // We run it via sh -c to allow complex yay commands from config
    if let Err(e) = run_command("sh", &["-c", &cmd_str], None::<&str>) {
        die!("System update failed: {}", e);
    }
}
