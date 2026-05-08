use clap::{ArgAction, Parser};

#[derive(Parser, Debug)]
#[command(name = "arch-emerge")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "A smart wrapper for Arch Linux package building and system updates", long_about = None)]
#[command(disable_help_flag = true)]
pub struct Cli {
    /// Download package sources only. Do not build.
    #[arg(short = 'd', action = ArgAction::SetTrue)]
    pub download_only: bool,

    /// Build locally with makepkg (overrides default)
    #[arg(short = 'l', action = ArgAction::SetTrue)]
    pub local_build: bool,

    /// Build inside a chroot with makechrootpkg (overrides default)
    #[arg(short = 'h', action = ArgAction::SetTrue)]
    pub chroot_build: bool,

    /// Compile only. Skip the package installation prompt
    #[arg(short = 'o', action = ArgAction::SetTrue)]
    pub compile_only: bool,

    /// Skip package test suite (--nocheck)
    #[arg(short = 't', action = ArgAction::SetTrue)]
    pub no_check: bool,

    /// Force a new build even if package artifacts already exist
    #[arg(short = 'n', action = ArgAction::SetTrue)]
    pub force_build: bool,

    /// Delete the existing package repository and clone it again
    #[arg(short = 'c', action = ArgAction::SetTrue)]
    pub clean: bool,

    /// Run full cleaning, including removing downloaded repositories and built packages
    #[arg(short = 'e', action = ArgAction::SetTrue)]
    pub clean_all: bool,

    /// Use sudo when deleting repositories or build artifacts
    #[arg(short = 's', action = ArgAction::SetTrue)]
    pub use_sudo_clean: bool,

    /// Remove the configured chroot
    #[arg(short = 'r', action = ArgAction::SetTrue)]
    pub remove_chroot: bool,

    /// Install and populate Arch Linux / CachyOS signing keys
    #[arg(short = 'k', action = ArgAction::SetTrue)]
    pub install_keys: bool,

    /// Update PKGBUILD checksums before building
    #[arg(short = 'u', action = ArgAction::SetTrue)]
    pub update_sums: bool,

    /// Enable verbose output
    #[arg(short = 'v', action = ArgAction::SetTrue)]
    pub verbose: bool,

    /// Silent mode. Hide normal status output
    #[arg(short = 'i', action = ArgAction::SetTrue)]
    pub silent: bool,

    /// Force git pull / repository refresh for all custom repositories
    #[arg(short = 'R', action = ArgAction::SetTrue)]
    pub force_repo_update: bool,

    /// Perform full system update with manual compilation of configured packages
    #[arg(short = 'U', action = ArgAction::SetTrue)]
    pub system_update: bool,

    /// Specify which repository to pull the package from
    #[arg(long)]
    pub repo: Option<String>,

    /// Only install already built artifacts from READY_MADE_PACKAGES_PATH
    #[arg(long, action = ArgAction::SetTrue)]
    pub install_only: bool,

    /// Print commands without executing them
    #[arg(long, action = ArgAction::SetTrue)]
    pub dry_run: bool,

    /// List configured packages and exit
    #[arg(long, action = ArgAction::SetTrue)]
    pub list: bool,

    /// Show help information
    #[arg(long, action = clap::ArgAction::Help)]
    pub help: Option<bool>,

    /// Packages to build
    pub packages: Vec<String>,
}
