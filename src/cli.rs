use clap::{ArgAction, Parser};

#[derive(Parser, Debug, Clone)]
#[command(name = "abs")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "ABS: package building and system updates", long_about = None)]
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

    /// Refresh git clones for `manual_update_packages` (arch: per package; others: once per repo).
    /// With `-U` (`-RU`): refresh, version report, compile what qualifies, then
    /// `command_to_perform_system_update`. Without `-U` (`-R` alone): refresh, report, then
    /// `command_to_update_repositories` (no compile).
    #[arg(short = 'R', action = ArgAction::SetTrue)]
    pub force_repo_update: bool,

    /// Perform full system update with manual compilation of configured packages
    #[arg(short = 'U', action = ArgAction::SetTrue)]
    pub system_update: bool,

    /// Specify which repository to pull the package from
    #[arg(long)]
    pub repo: Option<String>,

    /// Default compilation thread count (`-j`) for this run (does not override per-package settings)
    #[arg(short = 'j', long = "jobs", value_name = "N")]
    pub jobs: Option<usize>,

    /// Only install already built artifacts from READY_MADE_PACKAGES_PATH
    #[arg(long, action = ArgAction::SetTrue)]
    pub install_only: bool,

    /// Before compilation, remove `src/` and `pkg/` under the package directory (overrides config when enabling clean install)
    #[arg(long, action = ArgAction::SetTrue)]
    pub clean_install: bool,

    /// Print commands without executing them
    #[arg(long, action = ArgAction::SetTrue)]
    pub dry_run: bool,

    /// List configured packages and exit
    #[arg(long, action = ArgAction::SetTrue)]
    pub list: bool,

    /// Open the config file in an editor ($EDITOR by default)
    #[arg(long, value_name = "EDITOR", num_args = 0..=1, default_missing_value = "")]
    pub configure: Option<String>,

    /// Check for new versions of ABS from GitHub
    #[arg(long, action = ArgAction::SetTrue)]
    pub check_update: bool,

    /// Check for new versions of ABS and update if available
    #[arg(long, action = ArgAction::SetTrue)]
    pub self_update: bool,

    /// Show help information
    #[arg(long, action = clap::ArgAction::Help)]
    pub help: Option<bool>,

    /// Ramdisk targets for all packages on this run (`w`=workdir, `c`=chroot, `p`=packages, `disabled`=off).
    /// Avoids shell glob issues with bracket syntax in zsh. Bracket `pkg[ramdisk=wcp]` overrides this.
    #[arg(long, value_name = "WCP|disabled")]
    pub ramdisk: Option<String>,

    /// Packages to build. Per-package options: `pkg[repo=aur,pkgver=1.0,pkgrel=2,local,chroot,nocheck]`
    /// Quote the argument when using `[` (required in zsh): `'mesa[repo=aur]'`
    pub packages: Vec<String>,

    /// Start kernel PGO pipeline for PACKAGE
    #[arg(long, value_name = "PACKAGE")]
    pub pgo: Option<String>,

    /// Resume kernel PGO pipeline after reboot
    #[arg(long, value_name = "PACKAGE")]
    pub pgo_resume: Option<String>,

    /// Show kernel PGO pipeline status
    #[arg(long, value_name = "PACKAGE")]
    pub pgo_status: Option<String>,

    /// Abort kernel PGO pipeline (stops builds and marks the pipeline aborted so kernel packages
    /// are released from system-update holds; use `--pgo-keep-stage` to preserve the saved stage)
    #[arg(long, value_name = "PACKAGE")]
    pub pgo_abort: Option<String>,

    /// With `--pgo-abort`: keep the saved pipeline stage for later resume (GUI stop button)
    #[arg(long, action = ArgAction::SetTrue)]
    pub pgo_keep_stage: bool,

    /// Stop any in-flight PGO work, clear saved pipeline state, and start stage 1 from scratch
    #[arg(long, value_name = "PACKAGE")]
    pub pgo_restart: Option<String>,

    /// PGO pipeline stage to run or set (with `--pgo-resume` / `--pgo-goto`). Examples:
    /// `stage2_profile`, `profile`, `2p`, `stage1_build`, `wait_reboot1`
    #[arg(long, value_name = "STAGE")]
    pub pgo_stage: Option<String>,

    /// With `--pgo-resume`: run only the selected stage, then stop (state still advances on success)
    #[arg(long, action = ArgAction::SetTrue)]
    pub pgo_once: bool,

    /// Set PGO pipeline stage in the state file without running (requires `--pgo-stage` and PACKAGE)
    #[arg(long, action = ArgAction::SetTrue)]
    pub pgo_goto: bool,

    /// Unattended PGO: resume in-progress pipelines on start, reboot at wait stages, and continue
    /// after boot via a transient user systemd unit (also enabled by `auto_restart` in abs.toml).
    #[arg(long, action = ArgAction::SetTrue)]
    pub pgo_auto: bool,

    /// One-shot kernel build for PACKAGE applying its [packages.PKG.kernel] options (no PGO)
    #[arg(long, value_name = "PACKAGE")]
    pub kernel_build: Option<String>,

    /// Unmount the configured ramdisk tmpfs (e.g. after abort or GUI exit)
    #[arg(long, action = ArgAction::SetTrue)]
    pub ramdisk_shutdown: bool,

    /// Emit machine-readable JSON (status/events)
    #[arg(long, action = ArgAction::SetTrue)]
    pub json: bool,

    /// Append structured JSON-lines events to PATH during PGO runs
    #[arg(long, value_name = "PATH")]
    pub event_log: Option<std::path::PathBuf>,

    /// Remove ABS from the system: binaries, config, state, cache, and build directories
    #[arg(long, action = ArgAction::SetTrue)]
    pub purge: bool,

    /// Skip the interactive “Press Enter to exit” prompt (for scripts and automation)
    #[arg(long, action = ArgAction::SetTrue)]
    pub no_wait: bool,

    /// Skip confirmation prompts (used with --purge)
    #[arg(long, short = 'y', action = ArgAction::SetTrue)]
    pub yes: bool,
}
