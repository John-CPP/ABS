//! Multi-stage kernel PGO pipeline (CachyOS linux-cachyos preset).

use crate::build::{self, PgoBuildContext};
use crate::cli::Cli;
use crate::config::{self, Config, KernelBuildConfig, PgoConfig};
use crate::package_spec::PackageSpec;
use crate::utils::{
    run_command, run_command_with_output, sh_single_quote,
};
use crate::{blog, die, ewarn, vlog};
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// Pipeline stage identifiers persisted in state file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PgoStageId {
    Stage1Build,
    WaitReboot1,
    Stage2Profile,
    Stage2Build,
    WaitReboot2,
    Stage3Profile,
    Stage3Build,
    Done,
    Aborted,
}

impl PgoStageId {
    pub fn label(self) -> &'static str {
        match self {
            Self::Stage1Build => "Stage 1: debug kernel build",
            Self::WaitReboot1 => "Waiting for reboot (boot stage-1 kernel)",
            Self::Stage2Profile => "Stage 2: profile AutoFDO",
            Self::Stage2Build => "Stage 2: AutoFDO build",
            Self::WaitReboot2 => "Waiting for reboot (boot stage-2 kernel)",
            Self::Stage3Profile => "Stage 3: profile Propeller",
            Self::Stage3Build => "Stage 3: final build",
            Self::Done => "Done",
            Self::Aborted => "Aborted",
        }
    }

    /// Stages the user may select with `--pgo-stage` (excludes terminal `done` / `aborted`).
    pub fn selectable_stages() -> &'static [PgoStageId] {
        &[
            Self::Stage1Build,
            Self::WaitReboot1,
            Self::Stage2Profile,
            Self::Stage2Build,
            Self::WaitReboot2,
            Self::Stage3Profile,
            Self::Stage3Build,
        ]
    }
}

/// Parse `--pgo-stage` values (serde snake_case names and short aliases).
pub fn parse_pgo_stage(raw: &str) -> Result<PgoStageId, String> {
    let norm = raw.trim().to_lowercase().replace('-', "_");
    let stage = match norm.as_str() {
        "1" | "stage1" | "stage1_build" | "debug" | "debug_build" => PgoStageId::Stage1Build,
        "wait1" | "reboot1" | "wait_reboot1" => PgoStageId::WaitReboot1,
        "2p" | "profile" | "stage2_profile" | "autofdo_profile" | "profile_autofdo" => {
            PgoStageId::Stage2Profile
        }
        "2" | "stage2" | "stage2_build" | "autofdo" | "autofdo_build" => PgoStageId::Stage2Build,
        "wait2" | "reboot2" | "wait_reboot2" => PgoStageId::WaitReboot2,
        "3p" | "stage3_profile" | "propeller_profile" | "profile_propeller" => {
            PgoStageId::Stage3Profile
        }
        "3" | "stage3" | "stage3_build" | "final" | "final_build" => PgoStageId::Stage3Build,
        "done" => PgoStageId::Done,
        "aborted" => PgoStageId::Aborted,
        other => {
            return Err(format!(
                "unknown PGO stage '{other}' (examples: stage2_profile, profile, 2p, stage1_build, wait_reboot1)"
            ));
        }
    };
    Ok(stage)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PgoState {
    pub package: String,
    pub repo_dir: String,
    pub current_stage: PgoStageId,
    pub started_at: u64,
    pub updated_at: u64,
    pub expected_kernel_uname: Option<String>,
    pub expected_package_base: Option<String>,
    pub stage_history: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PgoEvent<'a> {
    StageStart {
        ts: u64,
        stage: PgoStageId,
        package: &'a str,
    },
    StageDone {
        ts: u64,
        stage: PgoStageId,
        package: &'a str,
    },
    Log {
        ts: u64,
        stream: &'a str,
        line: String,
    },
    RebootRequired {
        ts: u64,
        expected_uname: Option<String>,
        message: String,
    },
    Error {
        ts: u64,
        message: String,
    },
}

pub struct EventLog {
    path: Option<PathBuf>,
    json_mode: bool,
}

impl EventLog {
    pub fn new(path: Option<PathBuf>, json_mode: bool) -> Self {
        if let Some(ref p) = path {
            Self::prepare_path(p);
        }
        Self { path, json_mode }
    }

    fn prepare_path(path: &Path) {
        if let Some(parent) = path.parent()
            && let Err(e) = fs::create_dir_all(parent)
        {
            ewarn!(
                "Failed to create event log directory {}: {}",
                parent.display(),
                e
            );
            return;
        }
        if let Err(e) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            ewarn!("Failed to create event log file {}: {}", path.display(), e);
        }
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    pub fn emit(&self, event: &PgoEvent<'_>) {
        if let Ok(line) = serde_json::to_string(event) {
            if self.json_mode {
                println!("{line}");
            }
            if let Some(ref path) = self.path {
                Self::prepare_path(path);
                match OpenOptions::new().create(true).append(true).open(path) {
                    Ok(mut f) => {
                        if let Err(e) = writeln!(f, "{line}") {
                            ewarn!("Failed to write event log {}: {}", path.display(), e);
                        }
                    }
                    Err(e) => {
                        ewarn!("Failed to open event log {}: {}", path.display(), e);
                    }
                }
            }
        }
    }

    pub fn log_line(&self, stream: &str, line: String) {
        if !self.json_mode {
            if stream == "stderr" {
                eprintln!("{line}");
            } else {
                println!("{line}");
            }
        }
        self.emit(&PgoEvent::Log {
            ts: Self::now(),
            stream,
            line,
        });
    }
}

pub fn handle_cli(cli: &Cli, config: &Config) {
    let package = cli
        .pgo
        .clone()
        .or(cli.pgo_resume.clone())
        .or(cli.pgo_restart.clone())
        .or(cli.pgo_status.clone())
        .or(cli.pgo_abort.clone())
        .or_else(|| cli.packages.first().cloned())
        .unwrap_or_else(|| {
            if cli.pgo_goto {
                die!("--pgo-goto requires a package name (positional PKG or --pgo-resume PKG)");
            }
            die!("PGO requires a package name (--pgo PKG or positional PKG)");
        });

    let events = EventLog::new(cli.event_log.clone(), cli.json);

    if cli.pgo_abort.is_some() {
        run_abort(&package, cli, config, &events);
        return;
    }
    if !cli.pgo_goto {
        crate::utils::request_exit_pause();
    }
    if cli.pgo_restart.is_some() {
        crate::ramdisk::install_exit_handlers();
        run_restart(&package, cli, config, &events);
        return;
    }
    if cli.pgo_status.is_some() {
        run_status(&package, config, cli.json, &events);
        return;
    }
    if cli.pgo_goto {
        run_goto(&package, cli, config, &events);
        return;
    }
    if cli.pgo_resume.is_some() {
        // Install SIGTERM/SIGINT cleanup so an aborted/killed run stops builds and unmounts the
        // ramdisk even when no ramdisk session is active.
        crate::ramdisk::install_exit_handlers();
        run_resume(&package, cli, config, &events);
        return;
    }
    if cli.pgo.is_some() {
        crate::ramdisk::install_exit_handlers();
        run_start(&package, cli, config, &events);
        return;
    }
    die!("No PGO action specified");
}

fn load_pgo_config(package: &str, config: &Config) -> (PgoConfig, KernelBuildConfig) {
    let pkg = config
        .packages
        .get(package)
        .unwrap_or_else(|| die!("Package '{package}' is not configured in abs.toml"));
    let pgo = pkg
        .pgo
        .clone()
        .filter(|p| p.enabled)
        .unwrap_or_else(|| die!("Package '{package}' has no enabled [packages.{package}.pgo] section"));
    if pgo.preset != "cachyos-kernel" {
        die!(
            "Unsupported PGO preset '{}'; only 'cachyos-kernel' is implemented",
            pgo.preset
        );
    }
    let archive = pgo
        .resolved_archive_dir()
        .unwrap_or_else(|| die!("profiles_archive_dir is required for PGO (package '{package}')"));
    if !archive.exists()
        && let Err(e) = fs::create_dir_all(&archive)
    {
        die!("Failed to create profiles_archive_dir '{}': {}", archive.display(), e);
    }
    let kernel = pkg.kernel.clone().unwrap_or_default();
    (pgo, kernel)
}

fn load_state(path: &Path) -> Option<PgoState> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_state(path: &Path, state: &PgoState) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let text = serde_json::to_string_pretty(state).unwrap_or_default();
    if let Err(e) = fs::write(path, text) {
        die!("Failed to write PGO state '{}': {}", path.display(), e);
    }
}

fn pgo_auto_enabled(pgo: &PgoConfig, cli: &Cli) -> bool {
    cli.pgo_auto || pgo.auto_restart
}

fn pgo_auto_systemd_unit(package: &str) -> String {
    format!("abs-pgo@{package}.service")
}

fn pgo_auto_systemd_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("systemd").join("user"))
        .unwrap_or_else(|| PathBuf::from(".config/systemd/user"))
}

fn install_pgo_auto_resume_service(package: &str) -> Result<(), String> {
    let abs_bin = std::env::current_exe()
        .map_err(|e| format!("resolve abs binary: {e}"))?
        .display()
        .to_string();
    let unit_dir = pgo_auto_systemd_dir();
    fs::create_dir_all(&unit_dir)
        .map_err(|e| format!("create {}: {e}", unit_dir.display()))?;
    let template = unit_dir.join("abs-pgo@.service");
    let unit = format!(
        "[Unit]\n\
         Description=Resume ABS PGO pipeline for %i after reboot\n\
         After=network-online.target\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         ExecStart={abs_bin} --pgo-resume %i --pgo-auto\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    );
    fs::write(&template, unit).map_err(|e| format!("write {}: {e}", template.display()))?;
    run_command("systemctl", &["--user", "daemon-reload"], None::<&str>)
        .map_err(|e| e.to_string())?;
    let instance = pgo_auto_systemd_unit(package);
    run_command("systemctl", &["--user", "enable", instance.as_str()], None::<&str>)
        .map_err(|e| format!("enable {instance}: {e}"))?;
    blog!("Installed user systemd unit {instance} for PGO auto-restart");
    Ok(())
}

pub fn remove_pgo_auto_resume_service(package: &str) {
    let instance = pgo_auto_systemd_unit(package);
    let _ = Command::new("systemctl")
        .args(["--user", "disable", "--now", instance.as_str()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn trigger_pgo_auto_reboot(package: &str) -> Result<(), String> {
    install_pgo_auto_resume_service(package)?;
    blog!("PGO auto-restart: rebooting in 5 seconds to continue pipeline…");
    std::thread::sleep(std::time::Duration::from_secs(5));
    run_command("sudo", &["reboot"], None::<&str>).map_err(|e| e.to_string())
}

/// Continue the pipeline in a fresh abs process (profile → build) without rebooting.
fn trigger_pgo_auto_resume_now(package: &str) -> Result<(), String> {
    install_pgo_auto_resume_service(package)?;
    let instance = pgo_auto_systemd_unit(package);
    blog!("PGO auto-restart: scheduling {instance} to continue pipeline…");
    run_command("systemctl", &["--user", "start", instance.as_str()], None::<&str>)
        .map_err(|e| format!("start {instance}: {e}"))
}

fn transition(state: &mut PgoState, stage: PgoStageId) {
    state.stage_history.push(format!("{:?}", state.current_stage));
    state.current_stage = stage;
    state.updated_at = EventLog::now();
}

fn run_start(package: &str, cli: &Cli, config: &Config, events: &EventLog) {
    if cli.pgo_stage.is_some() || cli.pgo_once {
        die!("--pgo-stage and --pgo-once are for --pgo-resume or --pgo-goto, not --pgo");
    }
    let (pgo, _) = load_pgo_config(package, config);
    let state_path = pgo.resolved_state_file(package);
    blog!("Starting PGO pipeline for {}…", package);
    events.log_line("stdout", format!("Starting PGO pipeline for {package}…"));
    if let Some(existing) = load_state(&state_path)
        && !matches!(existing.current_stage, PgoStageId::Done | PgoStageId::Aborted)
    {
        blog!(
            "Existing PGO pipeline at {} ({}); resuming…",
            state_path.display(),
            existing.current_stage.label()
        );
        events.log_line(
            "stdout",
            format!(
                "Resuming PGO pipeline at {}…",
                existing.current_stage.label()
            ),
        );
        run_resume(package, cli, config, events);
        return;
    }
    preflight(&pgo, package, config);
    if pgo_auto_enabled(&pgo, cli) {
        blog!("PGO auto-restart enabled for {package}");
    }
    blog!(
        "Preparing package repository for {} (clone/pull may take a while)…",
        package
    );
    let repo = resolve_repo_dir(package, cli, config, true);
    blog!("Repository ready at {}", repo.pkg_dir.display());
    if repo.synced() {
        let spec = PackageSpec::plain(package);
        let pkg_config = config.packages.get(package);
        let ramdisk_targets = crate::ramdisk::resolve_ramdisk_targets(
            config,
            pkg_config,
            Some(&spec),
            cli.ramdisk.as_deref(),
        )
        .unwrap_or_default();
        events.log_line(
            "stdout",
            "Prefetching kernel sources (updpkgsums) before ramdisk setup…".to_string(),
        );
        if !crate::pkgbuild::prefetch_pgo_sources(&repo.pkg_dir, &ramdisk_targets) {
            ewarn!("Source prefetch failed; makepkg may download the kernel archive again");
        }
    }
    let mut state = PgoState {
        package: package.to_string(),
        repo_dir: repo.pkg_dir.to_string_lossy().into_owned(),
        current_stage: PgoStageId::Stage1Build,
        started_at: EventLog::now(),
        updated_at: EventLog::now(),
        expected_kernel_uname: None,
        expected_package_base: None,
        stage_history: Vec::new(),
    };
    save_state(&state_path, &state);
    execute_current_stage(
        &mut state,
        &StageRunCtx {
            state_path: &state_path,
            pgo: &pgo,
            package,
            cli,
            config,
            events,
            run_once: false,
        },
    );
    save_state(&state_path, &state);
    if matches!(state.current_stage, PgoStageId::Done) {
        remove_pgo_auto_resume_service(package);
    }
}

fn run_goto(package: &str, cli: &Cli, config: &Config, events: &EventLog) {
    let stage_raw = cli
        .pgo_stage
        .as_deref()
        .unwrap_or_else(|| die!("--pgo-goto requires --pgo-stage STAGE"));
    let target = parse_pgo_stage(stage_raw).unwrap_or_else(|e| die!("{e}"));
    if matches!(target, PgoStageId::Done | PgoStageId::Aborted) {
        die!("--pgo-goto cannot set terminal stage '{}'", target.label());
    }
    let (pgo, _) = load_pgo_config(package, config);
    let state_path = pgo.resolved_state_file(package);
    let mut state = load_state(&state_path)
        .unwrap_or_else(|| die!("No PGO state at '{}'; run --pgo first", state_path.display()));
    if state.current_stage != target {
        blog!(
            "PGO stage for {}: {} → {}",
            package,
            state.current_stage.label(),
            target.label()
        );
        events.log_line(
            "stdout",
            format!(
                "PGO stage set to {} (was {})",
                target.label(),
                state.current_stage.label()
            ),
        );
        transition(&mut state, target);
    } else {
        blog!("PGO stage for {} already at {}", package, target.label());
    }
    save_state(&state_path, &state);
    if cli.json {
        print_json_status(&state, &pgo);
    } else {
        run_status(package, config, false, events);
    }
}

fn run_resume(package: &str, cli: &Cli, config: &Config, events: &EventLog) {
    let (pgo, _) = load_pgo_config(package, config);
    let state_path = pgo.resolved_state_file(package);
    let mut state = load_state(&state_path)
        .unwrap_or_else(|| die!("No PGO state at '{}'; run --pgo first", state_path.display()));

    if let Some(stage_raw) = cli.pgo_stage.as_deref() {
        let target = parse_pgo_stage(stage_raw).unwrap_or_else(|e| die!("{e}"));
        if state.current_stage != target {
            blog!(
                "PGO stage override: {} → {}",
                state.current_stage.label(),
                target.label()
            );
            transition(&mut state, target);
        }
    } else {
        match state.current_stage {
            PgoStageId::WaitReboot1 => {
                if pgo.verify_boot {
                    verify_boot_kernel(&state, &pgo);
                }
                transition(&mut state, PgoStageId::Stage2Profile);
            }
            PgoStageId::WaitReboot2 => {
                if pgo.verify_boot {
                    verify_boot_kernel(&state, &pgo);
                }
                transition(&mut state, PgoStageId::Stage3Profile);
            }
            PgoStageId::Done => {
                blog!("PGO pipeline already complete for {}", package);
                if cli.json {
                    print_json_status(&state, &pgo);
                }
                return;
            }
            PgoStageId::Aborted => {
                die!("PGO pipeline was aborted; run --pgo to start a fresh pipeline");
            }
            _ => {
                blog!("Resuming in-progress stage: {}", state.current_stage.label());
            }
        }
    }

    save_state(&state_path, &state);
    execute_current_stage(
        &mut state,
        &StageRunCtx {
            state_path: &state_path,
            pgo: &pgo,
            package,
            cli,
            config,
            events,
            run_once: cli.pgo_once,
        },
    );
    save_state(&state_path, &state);
    if matches!(state.current_stage, PgoStageId::Done) {
        remove_pgo_auto_resume_service(package);
    }
}

fn run_status(package: &str, config: &Config, json: bool, _events: &EventLog) {
    let (pgo, _) = load_pgo_config(package, config);
    let state_path = pgo.resolved_state_file(package);
    let Some(state) = load_state(&state_path) else {
        if json {
            print_empty_json_status(package, &pgo);
        } else {
            blog!(
                "No PGO pipeline state for {} (file not found: {})",
                package,
                state_path.display()
            );
        }
        return;
    };
    if json {
        print_json_status(&state, &pgo);
    } else {
        blog!("PGO status for {}:", package);
        blog!("  Stage: {}", state.current_stage.label());
        if let Some(ref u) = state.expected_kernel_uname {
            blog!("  Expected kernel: {}", u);
        }
        blog!("  State file: {}", state_path.display());
        match state.current_stage {
            PgoStageId::WaitReboot1 | PgoStageId::WaitReboot2 => {
                blog!("  Action: reboot, then run: abs --pgo-resume {}", package);
            }
            PgoStageId::Done => blog!("  Action: none (complete)"),
            _ => {
                blog!("  Action: run: abs --pgo-resume {}", package);
                blog!(
                    "  Manual stage: abs --pgo-resume {} --pgo-stage STAGE [--pgo-once]",
                    package
                );
                blog!(
                    "  Set stage only: abs --pgo-goto --pgo-stage STAGE {}",
                    package
                );
            }
        }
        blog!("  Stages:");
        for stage in PgoStageId::selectable_stages() {
            let mark = if *stage == state.current_stage { " (current)" } else { "" };
            blog!("    {}{}", stage_id_name(*stage), mark);
        }
    }
}

fn stage_id_name(stage: PgoStageId) -> String {
    serde_json::to_value(stage)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| format!("{stage:?}"))
}

fn run_abort(package: &str, cli: &Cli, config: &Config, events: &EventLog) {
    let disposition = if cli.pgo_keep_stage {
        PgoAbortDisposition::KeepStage
    } else {
        PgoAbortDisposition::MarkAborted
    };
    run_abort_inner(package, config, events, disposition);
}

fn run_restart(package: &str, cli: &Cli, config: &Config, events: &EventLog) {
    if cli.pgo_stage.is_some() || cli.pgo_once || cli.pgo_goto {
        die!("--pgo-stage, --pgo-once, and --pgo-goto are not used with --pgo-restart");
    }
    run_abort_inner(package, config, events, PgoAbortDisposition::RemoveState);
    run_start(package, cli, config, events);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PgoAbortDisposition {
    KeepStage,
    MarkAborted,
    RemoveState,
}

fn run_abort_inner(
    package: &str,
    config: &Config,
    events: &EventLog,
    disposition: PgoAbortDisposition,
) {
    let (pgo, _) = load_pgo_config(package, config);
    let state_path = pgo.resolved_state_file(package);
    crate::utils::kill_abs_cli_processes(package);
    if let Some(state) = load_state(&state_path) {
        let repo = Path::new(&state.repo_dir);
        crate::utils::kill_processes_with_cwd_under(repo, "PGO repo");
        crate::pkgbuild::restore_pkgbuild(repo);
    } else {
        let packages_path = PathBuf::from(config.paths.packages_path.trim());
        if !packages_path.as_os_str().is_empty() && packages_path.exists() {
            crate::utils::kill_processes_with_cwd_under(&packages_path, "packages_path");
        }
    }
    if config.ramdisk.enabled {
        let mount = PathBuf::from(config.ramdisk.mount_point.trim());
        if !mount.as_os_str().is_empty() {
            crate::utils::kill_processes_with_cwd_under(&mount, "ramdisk");
        }
    }
    crate::utils::terminate_foreground_children();
    let preserved_stage = load_state(&state_path).map(|s| s.current_stage);
    remove_pgo_auto_resume_service(package);
    crate::pkgbuild::restore_pending_pkgbuilds();
    if let Err(e) = crate::ramdisk::force_unmount_configured(config) {
        ewarn!("Ramdisk cleanup after PGO abort failed: {e}");
    }
    match disposition {
        PgoAbortDisposition::KeepStage => {
            if let Some(stage) = preserved_stage {
                blog!(
                    "PGO run stopped for {} (pipeline preserved at {}; use Resume or a stage button to continue)",
                    package,
                    stage.label()
                );
                events.emit(&PgoEvent::Error {
                    ts: EventLog::now(),
                    message: format!(
                        "PGO stopped for {package} at {}; state preserved",
                        stage.label()
                    ),
                });
            } else {
                blog!("PGO run stopped for {}", package);
                events.emit(&PgoEvent::Error {
                    ts: EventLog::now(),
                    message: format!("PGO stopped for {package}"),
                });
            }
        }
        PgoAbortDisposition::MarkAborted => {
            if let Some(mut state) = load_state(&state_path) {
                transition(&mut state, PgoStageId::Aborted);
                save_state(&state_path, &state);
            }
            blog!(
                "PGO pipeline aborted for {} (kernel packages released from system-update hold; run --pgo to start fresh)",
                package
            );
            events.emit(&PgoEvent::Error {
                ts: EventLog::now(),
                message: format!("PGO pipeline aborted for {package}"),
            });
        }
        PgoAbortDisposition::RemoveState => {
            let _ = fs::remove_file(&state_path);
            blog!("PGO pipeline reset for {package}; starting from stage 1");
            events.log_line(
                "stdout",
                format!("PGO pipeline reset for {package}; starting from stage 1"),
            );
        }
    }
}

fn print_empty_json_status(package: &str, pgo: &PgoConfig) {
    #[derive(Serialize)]
    struct StageOut {
        id: String,
        label: String,
    }
    #[derive(Serialize)]
    struct StatusOut<'a> {
        package: &'a str,
        stage: &'static str,
        stage_label: &'static str,
        expected_kernel_uname: Option<&'a str>,
        expected_package_base: Option<&'a str>,
        state_file: String,
        archive_dir: Option<String>,
        reboot_required: bool,
        next_action: String,
        stages: Vec<StageOut>,
    }
    let out = StatusOut {
        package,
        stage: "",
        stage_label: "No pipeline",
        expected_kernel_uname: None,
        expected_package_base: None,
        state_file: pgo.resolved_state_file(package).display().to_string(),
        archive_dir: pgo
            .resolved_archive_dir()
            .map(|p| p.display().to_string()),
        reboot_required: false,
        next_action: format!("abs --pgo {package}"),
        stages: PgoStageId::selectable_stages()
            .iter()
            .map(|stage| StageOut {
                id: stage_id_name(*stage),
                label: stage.label().to_string(),
            })
            .collect(),
    };
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
}

fn print_json_status(state: &PgoState, pgo: &PgoConfig) {
    #[derive(Serialize)]
    struct StageOut {
        id: String,
        label: String,
    }
    #[derive(Serialize)]
    struct StatusOut<'a> {
        package: &'a str,
        stage: PgoStageId,
        stage_label: &'a str,
        expected_kernel_uname: Option<&'a str>,
        expected_package_base: Option<&'a str>,
        state_file: String,
        archive_dir: Option<String>,
        reboot_required: bool,
        boot_ready: bool,
        next_action: String,
        stages: Vec<StageOut>,
    }
    let boot_ready = matches!(
        state.current_stage,
        PgoStageId::WaitReboot1 | PgoStageId::WaitReboot2
    ) && boot_matches_expected(state);
    let (reboot_required, next_action) = match state.current_stage {
        PgoStageId::WaitReboot1 | PgoStageId::WaitReboot2 if boot_ready => (
            false,
            format!("abs --pgo-resume {}", state.package),
        ),
        PgoStageId::WaitReboot1 | PgoStageId::WaitReboot2 => (
            true,
            reboot_resume_message(state, &state.package),
        ),
        PgoStageId::Done => (false, "none".to_string()),
        PgoStageId::Aborted => (false, "run --pgo to start a fresh pipeline".to_string()),
        _ => (
            false,
            format!("abs --pgo-resume {}", state.package),
        ),
    };
    let out = StatusOut {
        package: &state.package,
        stage: state.current_stage,
        stage_label: state.current_stage.label(),
        expected_kernel_uname: state.expected_kernel_uname.as_deref(),
        expected_package_base: state.expected_package_base.as_deref(),
        state_file: pgo.resolved_state_file(&state.package).display().to_string(),
        archive_dir: pgo
            .resolved_archive_dir()
            .map(|p| p.display().to_string()),
        reboot_required,
        boot_ready,
        next_action,
        stages: PgoStageId::selectable_stages()
            .iter()
            .map(|stage| StageOut {
                id: stage_id_name(*stage),
                label: stage.label().to_string(),
            })
            .collect(),
    };
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
}

fn preflight(pgo: &PgoConfig, package: &str, config: &Config) {
    for tool in ["makepkg", "perf"] {
        if which(tool).is_none() {
            die!("PGO requires '{tool}' in PATH");
        }
    }
    if which(&pgo.afdo_tool).is_none() {
        die!("PGO requires '{}' in PATH (afdo_tool)", pgo.afdo_tool);
    }
    if which(&pgo.propeller_tool).is_none() {
        die!(
            "PGO requires '{}' in PATH (propeller_tool)",
            pgo.propeller_tool
        );
    }
    if let Err(e) = crate::pgo_benchmark::resolve_benchmark_command(&pgo.benchmark_command) {
        die!("{e}");
    }
    if let Some(pc) = config.packages.get(package)
        && let Ok(targets) = crate::ramdisk::resolve_ramdisk_targets(
            config,
            Some(pc),
            None,
            None,
        )
        && targets.packages
    {
        ewarn!(
            "Ramdisk target 'p' (repo on tmpfs) is enabled for {package}; \
             the git tree and source tarballs live on tmpfs and may be lost on reboot. \
             Prefer 'w' (compile on ramdisk) so downloads stay on disk."
        );
    }
}

fn which(cmd: &str) -> Option<PathBuf> {
    run_command_with_output("which", &[cmd], None::<&str>)
        .ok()
        .map(|s| PathBuf::from(s.trim()))
}

fn resolve_repo_dir(
    package: &str,
    cli: &Cli,
    config: &Config,
    force_update: bool,
) -> crate::git::PrepareRepoResult {
    let spec = PackageSpec::plain(package);
    let pkg_config = config.packages.get(package);
    let ramdisk_targets = crate::ramdisk::resolve_ramdisk_targets(
        config,
        pkg_config,
        Some(&spec),
        cli.ramdisk.as_deref(),
    )
    .unwrap_or_default();
    let packages_path = crate::ramdisk::download_packages_path(config, &ramdisk_targets);
    let (repo_name, repo_url, base_pkg) = build::resolve_pkg_repo(package, cli, config, Some(&spec));
    crate::git::prepare_repo(
        package,
        base_pkg.as_str(),
        &repo_name,
        repo_url.as_str(),
        &packages_path,
        false,
        force_update,
        None,
    )
}

struct StageRunCtx<'a> {
    state_path: &'a Path,
    pgo: &'a PgoConfig,
    package: &'a str,
    cli: &'a Cli,
    config: &'a Config,
    events: &'a EventLog,
    run_once: bool,
}

fn execute_current_stage(state: &mut PgoState, ctx: &StageRunCtx<'_>) {
    let StageRunCtx {
        state_path,
        pgo,
        package,
        cli,
        config,
        events,
        run_once,
    } = ctx;
    let package = *package;
    let run_once = *run_once;
    loop {
        save_state(state_path, state);
        let entry_stage = state.current_stage;
        events.emit(&PgoEvent::StageStart {
            ts: EventLog::now(),
            stage: entry_stage,
            package,
        });
        blog!("PGO {}: {}", package, state.current_stage.label());

        match state.current_stage {
            PgoStageId::Stage1Build => {
                let kernel = config
                    .packages
                    .get(package)
                    .and_then(|p| p.kernel.clone())
                    .unwrap_or_default();
                let mut env = stage1_env(&kernel);
                merge_user_kernel_overrides(&mut env, &kernel);
                run_pgo_build(
                    package,
                    cli,
                    config,
                    &PgoBuildContext {
                        env_vars: env,
                        makepkg_flags: "-f --skipinteg".to_string(),
                        clean_src: false,
                        clean_pkg: false,
                        defer_pkgbuild_restore: true,
                        skip_abs_install: false,
                    },
                    events,
                );
                record_installed_kernel(state, "linux-cachyos");
                transition(state, PgoStageId::WaitReboot1);
            }
            PgoStageId::Stage2Profile => {
                run_stage2_profile(state, pgo, package, cli, config, events);
                events.emit(&PgoEvent::StageDone {
                    ts: EventLog::now(),
                    stage: entry_stage,
                    package,
                });
                transition(state, PgoStageId::Stage2Build);
                save_state(state_path, state);
                if pgo_auto_enabled(pgo, cli) && !run_once {
                    if let Err(e) = trigger_pgo_auto_resume_now(package) {
                        die!("PGO auto-restart failed: {e}");
                    }
                }
                return;
            }
            PgoStageId::Stage2Build => {
                restore_profiles_to_repo(state, pgo, &["kernel-compilation.afdo"]);
                let kernel = config
                    .packages
                    .get(package)
                    .and_then(|p| p.kernel.clone())
                    .unwrap_or_default();
                let mut env = stage2_build_env(&kernel, &pgo.afdo_profile_name);
                merge_user_kernel_overrides(&mut env, &kernel);
                run_pgo_build(
                    package,
                    cli,
                    config,
                    &PgoBuildContext {
                        env_vars: env,
                        makepkg_flags: "-f --skipinteg".to_string(),
                        clean_src: true,
                        clean_pkg: true,
                        defer_pkgbuild_restore: true,
                        skip_abs_install: false,
                    },
                    events,
                );
                record_installed_kernel(state, "linux-cachyos-lto");
                transition(state, PgoStageId::WaitReboot2);
            }
            PgoStageId::Stage3Profile => {
                run_stage3_profile(state, pgo, package, cli, config, events);
                events.emit(&PgoEvent::StageDone {
                    ts: EventLog::now(),
                    stage: entry_stage,
                    package,
                });
                transition(state, PgoStageId::Stage3Build);
                save_state(state_path, state);
                if pgo_auto_enabled(pgo, cli) && !run_once {
                    if let Err(e) = trigger_pgo_auto_resume_now(package) {
                        die!("PGO auto-restart failed: {e}");
                    }
                }
                return;
            }
            PgoStageId::Stage3Build => {
                restore_profiles_to_repo(
                    state,
                    pgo,
                    &[
                        "kernel-compilation.afdo",
                        "propeller_cc_profile.txt",
                        "propeller_ld_profile.txt",
                    ],
                );
                let kernel = config
                    .packages
                    .get(package)
                    .and_then(|p| p.kernel.clone())
                    .unwrap_or_default();
                let mut env = stage3_build_env(&kernel, &pgo.afdo_profile_name);
                merge_user_kernel_overrides(&mut env, &kernel);
                run_pgo_build(
                    package,
                    cli,
                    config,
                    &PgoBuildContext {
                        env_vars: env,
                        makepkg_flags: "-f --skipinteg".to_string(),
                        clean_src: true,
                        clean_pkg: true,
                        defer_pkgbuild_restore: false,
                        skip_abs_install: false,
                    },
                    events,
                );
                transition(state, PgoStageId::Done);
                crate::pkgbuild::restore_pkgbuild(Path::new(&state.repo_dir));
                remove_pgo_auto_resume_service(package);
            }
            PgoStageId::WaitReboot1 | PgoStageId::WaitReboot2 => {
                let msg = if pgo_auto_enabled(pgo, cli) && !run_once {
                    format!(
                        "PGO auto-restart: rebooting to continue pipeline for {package}. {}",
                        bootloader_hint(state)
                    )
                } else {
                    reboot_resume_message(state, package)
                };
                blog!("{}", msg);
                events.emit(&PgoEvent::RebootRequired {
                    ts: EventLog::now(),
                    expected_uname: state.expected_kernel_uname.clone(),
                    message: msg.clone(),
                });
                events.emit(&PgoEvent::StageDone {
                    ts: EventLog::now(),
                    stage: state.current_stage,
                    package,
                });
                if pgo_auto_enabled(pgo, cli) && !run_once {
                    save_state(state_path, state);
                    if let Err(e) = trigger_pgo_auto_reboot(package) {
                        die!("PGO auto-restart failed: {e}");
                    }
                }
                return;
            }
            PgoStageId::Done | PgoStageId::Aborted => return,
        }

        // Reached only by build stages that fall through (Stage1Build/Stage2Build transition to a
        // WaitReboot stage; Stage3Build transitions to Done). Log completion of the stage that just
        // ran, then loop so the WaitReboot/Done arm handles messaging and returns.
        events.emit(&PgoEvent::StageDone {
            ts: EventLog::now(),
            stage: entry_stage,
            package,
        });
        if run_once {
            emit_reboot_hint_if_waiting(state, package, events);
            return;
        }
    }
}

fn emit_reboot_hint_if_waiting(state: &PgoState, package: &str, events: &EventLog) {
    if !matches!(
        state.current_stage,
        PgoStageId::WaitReboot1 | PgoStageId::WaitReboot2
    ) {
        return;
    }
    let msg = reboot_resume_message(state, package);
    blog!("{}", msg);
    events.emit(&PgoEvent::RebootRequired {
        ts: EventLog::now(),
        expected_uname: state.expected_kernel_uname.clone(),
        message: msg,
    });
}

fn stage1_env(_kernel: &KernelBuildConfig) -> HashMap<String, String> {
    HashMap::from([
        ("_use_llvm_lto".into(), "none".into()),
        ("_processor_opt".into(), "native".into()),
        ("_use_lto_suffix".into(), "no".into()),
        ("_use_kcfi".into(), "yes".into()),
        ("_build_debug".into(), "yes".into()),
        ("_autofdo".into(), "yes".into()),
        ("_use_gcc_suffix".into(), "no".into()),
    ])
}

fn stage2_build_env(_kernel: &KernelBuildConfig, profile: &str) -> HashMap<String, String> {
    HashMap::from([
        ("_use_llvm_lto".into(), "thin".into()),
        ("_processor_opt".into(), "native".into()),
        ("_use_lto_suffix".into(), "yes".into()),
        ("_use_kcfi".into(), "yes".into()),
        ("_build_debug".into(), "yes".into()),
        ("_autofdo".into(), "yes".into()),
        ("_autofdo_profile_name".into(), profile.into()),
        ("_use_gcc_suffix".into(), "no".into()),
        ("_propeller".into(), "yes".into()),
    ])
}

fn stage3_build_env(_kernel: &KernelBuildConfig, profile: &str) -> HashMap<String, String> {
    HashMap::from([
        ("_use_llvm_lto".into(), "thin".into()),
        ("_processor_opt".into(), "native".into()),
        ("_use_lto_suffix".into(), "yes".into()),
        ("_use_kcfi".into(), "yes".into()),
        ("_build_debug".into(), "no".into()),
        ("_autofdo".into(), "yes".into()),
        ("_autofdo_profile_name".into(), profile.into()),
        ("_use_gcc_suffix".into(), "no".into()),
        ("_propeller".into(), "yes".into()),
        ("_propeller_profiles".into(), "yes".into()),
    ])
}

fn merge_user_kernel_overrides(env: &mut HashMap<String, String>, kernel: &KernelBuildConfig) {
    for (key, val) in config::kernel_user_override_pairs(kernel) {
        if let Some(v) = val {
            env.insert(key.to_string(), v.clone());
        }
    }
}

fn run_pgo_build(
    package: &str,
    cli: &Cli,
    config: &Config,
    pgo_ctx: &PgoBuildContext,
    events: &EventLog,
) {
    let spec = PackageSpec::plain(package);
    vlog!("PGO build env: {:?}", pgo_ctx.env_vars);
    if !build::process_package_pgo(&spec, cli, config, pgo_ctx, events) {
        die!("PGO build failed for {package}");
    }
}

fn record_installed_kernel(state: &mut PgoState, package_base: &str) {
    state.expected_package_base = Some(package_base.to_string());
    if let Ok(out) = run_command_with_output("pacman", &["-Q", package_base], None::<&str>) {
        let parts: Vec<&str> = out.split_whitespace().collect();
        if parts.len() >= 2 {
            state.expected_kernel_uname = Some(format!("{}-{}", parts[1], infer_suffix(package_base)));
        }
    }
    if state.expected_kernel_uname.is_none()
        && let Ok(u) = run_command_with_output("uname", &["-r"], None::<&str>)
    {
        state.expected_kernel_uname = Some(u.trim().to_string());
    }
}

fn infer_suffix(base: &str) -> &str {
    if base.contains("lto") {
        "cachyos-lto"
    } else {
        "cachyos"
    }
}

/// Pacman package names to pin with `--ignore` while a PGO pipeline is in progress, so `yay -Syu`
/// does not remove locally built stage kernels (e.g. `linux-cachyos-lto`).
pub fn active_pipeline_hold_packages(config: &Config) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut holds = Vec::new();
    for (_name, state) in active_pgo_states(config) {
        for pkg_name in kernel_hold_package_names(&state) {
            if seen.insert(pkg_name.clone()) {
                holds.push(pkg_name);
            }
        }
    }
    holds
}

/// In-progress kernel PGO pipelines (excludes `Done` and `Aborted`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivePgoPipeline {
    pub package: String,
    pub stage_label: String,
}

pub fn active_pipelines(config: &Config) -> Vec<ActivePgoPipeline> {
    active_pgo_states(config)
        .into_iter()
        .map(|(package, state)| ActivePgoPipeline {
            package,
            stage_label: state.current_stage.label().to_string(),
        })
        .collect()
}

fn active_pgo_states(config: &Config) -> Vec<(String, PgoState)> {
    let mut seen_paths = std::collections::HashSet::new();
    let mut seen_packages = std::collections::HashSet::new();
    let mut out = Vec::new();

    let default_dir = default_pgo_state_dir();
    if default_dir.is_dir()
        && let Ok(entries) = fs::read_dir(&default_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                try_push_active_pgo_state(&mut out, &mut seen_paths, &mut seen_packages, &path);
            }
        }
    }

    for (name, pkg) in &config.packages {
        let Some(pgo) = pkg.pgo.as_ref() else {
            continue;
        };
        let state_path = pgo.resolved_state_file(name);
        try_push_active_pgo_state(&mut out, &mut seen_paths, &mut seen_packages, &state_path);
    }

    out
}

fn default_pgo_state_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("abs").join("pgo"))
        .unwrap_or_else(|| PathBuf::from("/tmp/abs-pgo"))
}

fn try_push_active_pgo_state(
    out: &mut Vec<(String, PgoState)>,
    seen_paths: &mut std::collections::HashSet<PathBuf>,
    seen_packages: &mut std::collections::HashSet<String>,
    path: &Path,
) {
    if !seen_paths.insert(path.to_path_buf()) {
        return;
    }
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    let Ok(state) = serde_json::from_str::<PgoState>(&text) else {
        return;
    };
    if matches!(
        state.current_stage,
        PgoStageId::Done | PgoStageId::Aborted
    ) {
        return;
    }
    let package = if state.package.is_empty() {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string()
    } else {
        state.package.clone()
    };
    if package.is_empty() || !seen_packages.insert(package.clone()) {
        return;
    }
    out.push((package, state));
}

fn kernel_hold_package_names(state: &PgoState) -> Vec<String> {
    let mut names = vec![state.package.clone()];
    let bases: Vec<String> = if let Some(base) = state.expected_package_base.as_deref() {
        if base == "linux-cachyos" || base == "linux-cachyos-lto" {
            vec!["linux-cachyos".into(), "linux-cachyos-lto".into()]
        } else if base == state.package {
            vec![state.package.clone()]
        } else {
            vec![base.to_string(), state.package.clone()]
        }
    } else {
        vec![state.package.clone()]
    };
    for base in &bases {
        for suffix in ["", "-dbg", "-headers"] {
            names.push(format!("{base}{suffix}"));
        }
    }
    names.sort();
    names.dedup();
    names
}

fn bootloader_hint(state: &PgoState) -> String {
    let pkgbase = state
        .expected_package_base
        .as_deref()
        .unwrap_or(&state.package);
    let uname = state
        .expected_kernel_uname
        .as_deref()
        .unwrap_or("(check `uname -r` after boot)");
    format!(
        "In the bootloader, choose the entry for {pkgbase} (kernel {uname}): \
         /boot/vmlinuz-{pkgbase} with /boot/initramfs-{pkgbase}.img"
    )
}

fn reboot_resume_message(state: &PgoState, package: &str) -> String {
    format!(
        "{}. Then run: abs --pgo-resume {package}",
        bootloader_hint(state)
    )
}

fn verify_boot_kernel(state: &PgoState, _pgo: &PgoConfig) {
    if !boot_matches_expected(state) {
        let running = running_kernel_uname();
        let expected = state
            .expected_kernel_uname
            .as_deref()
            .unwrap_or("(unknown)");
        die!(
            "Boot verification failed: running '{running}', expected kernel matching '{expected}'. \
             Select the correct bootloader entry and re-run --pgo-resume"
        );
    }
}

fn boot_matches_expected(state: &PgoState) -> bool {
    let running = running_kernel_uname();
    if let Some(ref expected) = state.expected_kernel_uname {
        running.contains(expected.split('-').next().unwrap_or(expected))
    } else {
        false
    }
}

fn running_kernel_uname() -> String {
    run_command_with_output("uname", &["-r"], None::<&str>)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn scratch_dir(state: &PgoState, pgo: &PgoConfig, cli: &Cli, config: &Config) -> PathBuf {
    if pgo.profile_scratch_dir != "auto" {
        return config::expand_user_path(&pgo.profile_scratch_dir);
    }

    let spec = PackageSpec::plain(&state.package);
    let pkg_config = config.packages.get(&state.package);
    let ramdisk_targets = crate::ramdisk::resolve_ramdisk_targets(
        config,
        pkg_config,
        Some(&spec),
        cli.ramdisk.as_deref(),
    )
    .unwrap_or_default();

    let want_ramdisk = ramdisk_targets.profiles
        || (pgo.perf_data_on_ram && ramdisk_targets.build_workdir);

    if want_ramdisk {
        match crate::ramdisk::ensure_pgo_scratch_dir(config, &state.package, &ramdisk_targets) {
            Ok(Some(scratch)) => {
                blog!(
                    "PGO profile scratch on ramdisk: {} (targets={})",
                    scratch.display(),
                    crate::ramdisk::format_ramdisk_targets(&ramdisk_targets),
                );
                return scratch;
            }
            Ok(None) => {
                ewarn!(
                    "Ramdisk profile scratch unavailable (targets={}); trying /tmp fallback",
                    crate::ramdisk::format_ramdisk_targets(&ramdisk_targets),
                );
            }
            Err(e) => {
                ewarn!("Ramdisk profile scratch setup failed: {e}; trying /tmp fallback");
            }
        }
        let tmp = std::env::temp_dir()
            .join("abs-pgo-scratch")
            .join(&state.package);
        if fs::create_dir_all(&tmp).is_ok() {
            blog!(
                "PGO profile scratch on /tmp: {} (ramdisk unavailable)",
                tmp.display()
            );
            return tmp;
        }
        ewarn!(
            "perf/profile scratch falling back to package repo on disk: {}",
            state.repo_dir
        );
    }

    PathBuf::from(&state.repo_dir)
}

/// Minimum perf.data size to treat a failed `perf record` as usable (benchmark may exit non-zero).
const MIN_USABLE_PERF_BYTES: u64 = 50 * 1024 * 1024;

fn perf_data_usable(path: &Path) -> Option<u64> {
    let len = fs::metadata(path).ok()?.len();
    if len >= MIN_USABLE_PERF_BYTES {
        Some(len)
    } else {
        None
    }
}

fn finish_profile_collection(
    pgo: &PgoConfig,
    repo: &Path,
    perf_data: &Path,
    build_user: &str,
) -> Result<(), String> {
    run_command(
        "sudo",
        &[
            "chown",
            "-hR",
            &format!("{build_user}:{build_user}"),
            &perf_data.to_string_lossy(),
        ],
        Some(repo),
    )
    .map_err(|e| e.to_string())?;
    std::thread::sleep(std::time::Duration::from_secs(2));
    sysctl_toggle(pgo, false)
}

fn run_stage2_profile(
    state: &PgoState,
    pgo: &PgoConfig,
    package: &str,
    cli: &Cli,
    config: &Config,
    events: &EventLog,
) {
    let repo = PathBuf::from(&state.repo_dir);
    let scratch = scratch_dir(state, pgo, cli, config);
    let _ = fs::create_dir_all(&scratch);
    let perf_data = scratch.join("kernel.data");
    let profile_out = scratch.join(&pgo.afdo_profile_name);
    let repo_profile = repo.join(&pgo.afdo_profile_name);

    remove_undersized_profile(&repo_profile);
    remove_undersized_profile(&profile_out);
    if let Some(archive) = pgo.resolved_archive_dir() {
        remove_undersized_profile(&archive.join(&pgo.afdo_profile_name));
    }

    run_profile_collection(pgo, package, &repo, &scratch, &perf_data, events)
        .unwrap_or_else(|e| die!("Stage 2 profile failed: {e}"));
    sync_perf_data_to_repo(&perf_data, &repo).unwrap_or_else(|e| die!("{e}"));

    let vmlinux = resolve_vmlinux(
        pgo,
        state.expected_kernel_uname.as_deref(),
        state.expected_package_base.as_deref(),
    )
    .unwrap_or_else(|e| die!("{e}"));
    let convert_cmd = if pgo.afdo_tool == "llvm-profgen" {
        format!(
            "llvm-profgen --kernel --binary={} --perfdata={} -o {}",
            sh_single_quote(&vmlinux.to_string_lossy()),
            sh_single_quote(&perf_data.to_string_lossy()),
            sh_single_quote(&profile_out.to_string_lossy()),
        )
    } else {
        format!(
            "{} --binary={} --profile={} --format=extbinary --out={}",
            pgo.afdo_tool,
            sh_single_quote(&vmlinux.to_string_lossy()),
            sh_single_quote(&perf_data.to_string_lossy()),
            sh_single_quote(&profile_out.to_string_lossy()),
        )
    };
    blog!("Converting AutoFDO profile using {}…", vmlinux.display());
    run_logged_shell(&repo, &convert_cmd, events).unwrap_or_else(|e| die!("{e}"));

    let profile_bytes =
        validate_afdo_profile(&profile_out).unwrap_or_else(|e| die!("{e}"));
    blog!(
        "AutoFDO profile OK ({} bytes) — review before continuing to the AutoFDO build stage",
        profile_bytes
    );

    archive_profile(pgo, &profile_out, &pgo.afdo_profile_name).unwrap_or_else(|e| die!("{e}"));
    copy_to_repo(&profile_out, &repo_profile).unwrap_or_else(|e| die!("{e}"));
}

fn run_stage3_profile(
    state: &PgoState,
    pgo: &PgoConfig,
    package: &str,
    cli: &Cli,
    config: &Config,
    events: &EventLog,
) {
    let repo = PathBuf::from(&state.repo_dir);
    let scratch = scratch_dir(state, pgo, cli, config);
    let _ = fs::create_dir_all(&scratch);
    let perf_data = scratch.join("propeller.data");

    run_profile_collection(pgo, package, &repo, &scratch, &perf_data, events)
        .unwrap_or_else(|e| die!("Stage 3 profile failed: {e}"));
    sync_perf_data_to_repo(&perf_data, &repo).unwrap_or_else(|e| die!("{e}"));

    let vmlinux = resolve_vmlinux(
        pgo,
        state.expected_kernel_uname.as_deref(),
        state.expected_package_base.as_deref(),
    )
    .unwrap_or_else(|e| die!("{e}"));
    let cc_out = repo.join("propeller_cc_profile.txt");
    let ld_out = repo.join("propeller_ld_profile.txt");
    let convert_cmd = format!(
        "{} --binary={} --profile={} --format=propeller --propeller_output_module_name \
         --out={} --propeller_symorder={}",
        pgo.propeller_tool,
        sh_single_quote(&vmlinux.to_string_lossy()),
        sh_single_quote(&perf_data.to_string_lossy()),
        sh_single_quote(&cc_out.to_string_lossy()),
        sh_single_quote(&ld_out.to_string_lossy()),
    );
    blog!("Converting Propeller profile using {}…", vmlinux.display());
    run_logged_shell(&repo, &convert_cmd, events).unwrap_or_else(|e| die!("{e}"));

    for name in ["propeller_cc_profile.txt", "propeller_ld_profile.txt"] {
        let path = repo.join(name);
        let bytes = validate_propeller_profile(&path).unwrap_or_else(|e| die!("{e}"));
        blog!("Propeller profile {name} OK ({bytes} bytes)");
        archive_profile(pgo, &path, name).unwrap_or_else(|e| die!("{e}"));
    }
    let _ = package;
}

fn run_profile_collection(
    pgo: &PgoConfig,
    package: &str,
    repo: &Path,
    scratch: &Path,
    perf_data: &Path,
    events: &EventLog,
) -> Result<(), String> {
    sysctl_toggle(pgo, true)?;
    let perf_events = detect_perf_event_args(pgo)?;
    let perf_extra = resolved_perf_extra_args(pgo);
    let preset = resolved_benchmark_preset(pgo);
    let benchmark = crate::pgo_benchmark::resolve_benchmark_command(&pgo.benchmark_command)?;
    let bench_cache = pgo.resolved_benchmark_workdir(package);
    fs::create_dir_all(&bench_cache).map_err(|e| {
        format!(
            "create benchmark cache {}: {e}",
            bench_cache.display()
        )
    })?;
    blog!("PGO benchmark script: {}", benchmark.display());
    blog!(
        "PGO benchmark asset cache (persistent): {}",
        bench_cache.display()
    );
    blog!(
        "PGO perf scratch (may be tmpfs): {}",
        scratch.display()
    );
    let build_user = pgo
        .build_user
        .clone()
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "nobody".to_string());

    blog!(
        "PGO profiling: quality={}, benchmark_preset={}, perf_extra={}",
        if profiling_quality_is_maximum(pgo) {
            "maximum"
        } else {
            "standard"
        },
        preset,
        perf_extra,
    );

    let bench_cmd = format!(
        "env ABS_PGO_PROFILE_DIR={} ABS_PGO_BENCHMARK_DIR={} ABS_PGO_BENCHMARK={} {}",
        sh_single_quote(&scratch.to_string_lossy()),
        sh_single_quote(&bench_cache.to_string_lossy()),
        sh_single_quote(preset),
        crate::pgo_benchmark::shell_benchmark_runner(&benchmark),
    );

    // Write perf data to the absolute scratch path; cwd may differ from `scratch`
    // (e.g. when perf_data_on_ram puts scratch on the ramdisk while cwd stays the repo).
    let sudo = crate::utils::shell_sudo();
    let perf_cmd = format!(
        "{sudo} perf record {perf_events} {extra} -o {perf_out} -- \
         {sudo} -H -u {user} {bench}",
        sudo = sudo,
        perf_events = perf_events,
        extra = perf_extra,
        perf_out = sh_single_quote(&perf_data.to_string_lossy()),
        user = sh_single_quote(&build_user),
        bench = bench_cmd,
    );
    blog!("Running benchmark with perf record...");
    blog!("PGO benchmark command: {bench_cmd}");
    events.log_line(
        "stdout",
        format!(
            "Profiling workload ({preset}): perf record runs until the benchmark script exits. \
             profiling_quality=maximum forces cachyos-benchmarker and denser sampling."
        ),
    );
    let perf_result = run_logged_shell(repo, &perf_cmd, events);
    if let Err(e) = perf_result {
        if let Some(bytes) = perf_data_usable(perf_data) {
            ewarn!(
                "Benchmark exited non-zero but perf captured {} ({} bytes); continuing with profile conversion",
                perf_data.display(),
                bytes,
            );
        } else {
            return Err(e);
        }
    }

    finish_profile_collection(pgo, repo, perf_data, &build_user)
}

fn sysctl_toggle(pgo: &PgoConfig, enable: bool) -> Result<(), String> {
    if let Some(cmd) = &pgo.sysctl_command {
        let action = if enable { "enable" } else { "disable" };
        run_command("sudo", &[cmd, action], None::<&str>).map_err(|e| e.to_string())
    } else {
        let (kptr, paranoid) = if enable { ("0", "-1") } else { ("1", "2") };
        run_command(
            "sudo",
            &["sysctl", "-w", &format!("kernel.kptr_restrict={kptr}")],
            None::<&str>,
        )
        .map_err(|e| e.to_string())?;
        run_command(
            "sudo",
            &["sysctl", "-w", &format!("kernel.perf_event_paranoid={paranoid}")],
            None::<&str>,
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn profiling_quality_is_maximum(pgo: &PgoConfig) -> bool {
    matches!(
        pgo.profiling_quality.trim().to_ascii_lowercase().as_str(),
        "maximum" | "max" | "perfect"
    )
}

fn resolved_benchmark_preset<'a>(pgo: &'a PgoConfig) -> &'a str {
    if profiling_quality_is_maximum(pgo) {
        return "cachyos";
    }
    let preset = pgo.benchmark_preset.trim();
    if preset.is_empty() {
        "fast"
    } else {
        preset
    }
}

fn resolved_perf_extra_args(pgo: &PgoConfig) -> String {
    use crate::config::{PERF_EXTRA_ARGS_MAXIMUM, PERF_EXTRA_ARGS_STANDARD};
    let custom = pgo.perf_extra_args.trim();
    let is_custom = custom != PERF_EXTRA_ARGS_STANDARD && custom != PERF_EXTRA_ARGS_MAXIMUM;
    if is_custom {
        return pgo.perf_extra_args.clone();
    }
    if profiling_quality_is_maximum(pgo) {
        PERF_EXTRA_ARGS_MAXIMUM.into()
    } else {
        PERF_EXTRA_ARGS_STANDARD.into()
    }
}

/// Branch sampling event for Intel CPUs (llvm-profgen / AutoFDO).
const INTEL_TAKEN_BRANCH_PERF_EVENT: &str = "-e BR_INST_RETIRED.NEAR_TAKEN:k";

/// Maps `gcc -march=native` CPU name to perf record event arguments.
/// Mirrors `kernel_scripts/config.sh` → `detect_perf_args()`.
pub(crate) fn auto_perf_event_args_for_march(arch: &str) -> &'static str {
    match arch {
        "znver1" => "--pfm-events amd64_fam17h_zen1::RETIRED_TAKEN_BRANCH_INSTRUCTIONS:k",
        "znver2" => "--pfm-events amd64_fam17h_zen2::RETIRED_TAKEN_BRANCH_INSTRUCTIONS:k",
        "znver3" => "--pfm-events amd64_fam19h_zen3::RETIRED_TAKEN_BRANCH_INSTRUCTIONS:k",
        "znver4" => "--pfm-events amd64_fam19h_zen4::RETIRED_TAKEN_BRANCH_INSTRUCTIONS:k",
        "znver5" => "--pfm-events amd64_fam1ah_zen5::RETIRED_TAKEN_BRANCH_INSTRUCTIONS:k",
        // Intel Core / Xeon (explicit list from kernel_scripts/config.sh).
        "sandybridge" | "ivybridge" | "haswell" | "broadwell" | "kabylake" | "coffeelake"
        | "cometlake" | "tigerlake" | "alderlake" | "raptorlake" | "meteorlake" | "arrowlake"
        | "lunarlake" | "pantherlake" | "sapphirerapids" | "emeraldrapids" | "graniterapids"
        | "nehalem" | "westmere" | "cascadelake" | "cooperlake" | "rocketlake" => {
            INTEL_TAKEN_BRANCH_PERF_EVENT
        }
        arch if arch.starts_with("skylake") || arch.starts_with("icelake") => {
            INTEL_TAKEN_BRANCH_PERF_EVENT
        }
        _ => INTEL_TAKEN_BRANCH_PERF_EVENT,
    }
}

pub fn detect_perf_event_args(pgo: &PgoConfig) -> Result<String, String> {
    if pgo.perf_event_args != "auto" {
        return Ok(pgo.perf_event_args.clone());
    }
    // Mirrors `kernel_scripts/config.sh` → `detect_perf_args()`.
    let march = run_command_with_output(
        "gcc",
        &[
            "-c",
            "-Q",
            "-march=native",
            "--help=target",
            "-o",
            "/dev/null",
        ],
        None::<&str>,
    )
    .unwrap_or_default();
    let arch = march
        .lines()
        .find_map(|l| l.split("-march=").nth(1).map(|s| s.trim().to_string()))
        .unwrap_or_else(|| "unknown".to_string());

    Ok(auto_perf_event_args_for_march(&arch).into())
}

fn running_kernel_release() -> Option<String> {
    std::process::Command::new("uname")
        .arg("-r")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// CachyOS AutoFDO profiles are typically 800 KB–1.5 MB; smaller output means symbol matching failed.
const MIN_AFDO_PROFILE_BYTES: u64 = 100_000;
const MIN_PROPELLER_PROFILE_BYTES: u64 = 64;

fn remove_undersized_profile(path: &Path) {
    let Ok(meta) = fs::metadata(path) else {
        return;
    };
    if meta.len() < MIN_AFDO_PROFILE_BYTES {
        blog!(
            "Removing undersized profile {} ({} bytes)",
            path.display(),
            meta.len()
        );
        let _ = fs::remove_file(path);
    }
}

fn validate_afdo_profile(path: &Path) -> Result<u64, String> {
    let len = fs::metadata(path)
        .map_err(|e| format!("cannot stat profile {}: {e}", path.display()))?
        .len();
    if len < MIN_AFDO_PROFILE_BYTES {
        return Err(format!(
            "AutoFDO profile at {} is only {len} bytes (expected at least {min} bytes, typically \
             800 KB–1.5 MB). llvm-profgen could not map perf samples to kernel symbols — install \
             the matching -dbg package (e.g. linux-cachyos-dbg) and use \
             /usr/src/debug/linux-cachyos/vmlinux, or set [packages.*.pgo] vmlinux in abs.toml",
            path.display(),
            min = MIN_AFDO_PROFILE_BYTES
        ));
    }
    Ok(len)
}

fn validate_propeller_profile(path: &Path) -> Result<u64, String> {
    let len = fs::metadata(path)
        .map_err(|e| format!("cannot stat profile {}: {e}", path.display()))?
        .len();
    if len < MIN_PROPELLER_PROFILE_BYTES {
        return Err(format!(
            "Propeller profile at {} is only {len} bytes — conversion likely failed; check vmlinux \
             matches the profiled kernel",
            path.display()
        ));
    }
    Ok(len)
}

fn elf_has_section(path: &Path, section: &str) -> bool {
    for tool in ["llvm-readelf", "readelf"] {
        let Ok(output) = Command::new(tool)
            .arg("-S")
            .arg(path)
            .output()
        else {
            continue;
        };
        if output.status.success()
            && String::from_utf8_lossy(&output.stdout).contains(section)
        {
            return true;
        }
    }
    false
}

fn vmlinux_usable_for_profiling(path: &Path) -> bool {
    path.is_file() && elf_has_section(path, ".debug_info")
}

fn dbg_vmlinux_path(package_base: &str) -> PathBuf {
    PathBuf::from(format!("/usr/src/debug/{package_base}/vmlinux"))
}

fn push_vmlinux_candidate(candidates: &mut Vec<PathBuf>, path: PathBuf) {
    if !candidates.iter().any(|p| p == &path) {
        candidates.push(path);
    }
}

fn newest_existing_file(paths: &[PathBuf]) -> Option<PathBuf> {
    let mut existing: Vec<&PathBuf> = paths.iter().filter(|p| p.is_file()).collect();
    existing.sort_by_key(|p| {
        fs::metadata(p)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH)
    });
    existing.last().map(|p| (*p).clone())
}

/// Resolve the unstripped kernel image used by llvm-profgen / create_llvm_prof.
pub fn resolve_vmlinux(
    pgo: &PgoConfig,
    kernel_release: Option<&str>,
    package_base: Option<&str>,
) -> Result<PathBuf, String> {
    if pgo.vmlinux != "auto" {
        let p = PathBuf::from(&pgo.vmlinux);
        if p.is_file() {
            if !vmlinux_usable_for_profiling(&p) {
                ewarn!(
                    "Configured vmlinux {} lacks .debug_info — llvm-profgen may produce an empty profile",
                    p.display()
                );
            }
            return Ok(p);
        }
        return Err(format!("vmlinux not found at {}", p.display()));
    }

    let mut searched = Vec::new();
    let release = kernel_release
        .map(str::to_string)
        .or_else(running_kernel_release);
    let dbg_hint = package_base
        .map(|b| format!("{b}-dbg"))
        .unwrap_or_else(|| "linux-cachyos-dbg".into());

    if let Some(base) = package_base {
        let p = dbg_vmlinux_path(base);
        push_vmlinux_candidate(&mut searched, p.clone());
        if vmlinux_usable_for_profiling(&p) {
            return Ok(p);
        }
    }

    let mut debug_candidates = Vec::new();
    if let Ok(entries) = fs::read_dir("/usr/src/debug") {
        for entry in entries.flatten() {
            let p = entry.path().join("vmlinux");
            push_vmlinux_candidate(&mut searched, p.clone());
            if vmlinux_usable_for_profiling(&p) {
                debug_candidates.push(p);
            }
        }
    }
    if let Some(p) = newest_existing_file(&debug_candidates) {
        return Ok(p);
    }

    if let Some(ref rel) = release {
        for sub in ["build/vmlinux", "vmlinux"] {
            let p = PathBuf::from(format!("/usr/lib/modules/{rel}/{sub}"));
            push_vmlinux_candidate(&mut searched, p.clone());
            if vmlinux_usable_for_profiling(&p) {
                return Ok(p);
            }
        }
    }

    let hint = release
        .as_deref()
        .map(|r| format!(" for kernel {r}"))
        .unwrap_or_default();
    Err(format!(
        "no suitable vmlinux found{hint} (searched: {}). The modules build tree is not enough — \
         install {dbg_hint} (provides /usr/src/debug/.../vmlinux with DWARF) or set \
         [packages.*.pgo] vmlinux = \"/path/to/vmlinux\" in abs.toml",
        searched
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Copy raw perf data from ramdisk scratch to the package repo when they differ.
fn sync_perf_data_to_repo(perf_data: &Path, repo: &Path) -> Result<(), String> {
    if perf_data.parent().is_some_and(|p| p == repo) {
        return Ok(());
    }
    if !perf_data.is_file() {
        return Err(format!(
            "perf data missing at {} after profile collection",
            perf_data.display()
        ));
    }
    let dest = repo.join(
        perf_data
            .file_name()
            .ok_or_else(|| "perf data path has no file name".to_string())?,
    );
    if dest == perf_data {
        return Ok(());
    }
    blog!(
        "Copying perf data {} → {}",
        perf_data.display(),
        dest.display()
    );
    fs::copy(perf_data, &dest).map_err(|e| format!("copy perf data to repo failed: {e}"))?;
    crate::utils::ensure_build_user_can_read(&dest)?;
    Ok(())
}

fn archive_profile(pgo: &PgoConfig, src: &Path, name: &str) -> Result<(), String> {
    let archive = pgo
        .resolved_archive_dir()
        .ok_or_else(|| "profiles_archive_dir not set".to_string())?;
    let dest = archive.join(name);
    fs::copy(src, &dest).map_err(|e| format!("archive copy failed: {e}"))?;
    Ok(())
}

fn copy_to_repo(src: &Path, dest: &Path) -> Result<(), String> {
    fs::copy(src, dest).map_err(|e| format!("copy to repo failed: {e}"))?;
    Ok(())
}

fn restore_profiles_to_repo(state: &PgoState, pgo: &PgoConfig, names: &[&str]) {
    let repo = PathBuf::from(&state.repo_dir);
    let archive = pgo
        .resolved_archive_dir()
        .unwrap_or_else(|| die!("profiles_archive_dir not set"));
    for name in names {
        let dest = repo.join(name);
        if dest.exists() {
            continue;
        }
        let src = archive.join(name);
        if src.exists() {
            if *name == "kernel-compilation.afdo" {
                if let Ok(meta) = fs::metadata(&src)
                    && meta.len() < MIN_AFDO_PROFILE_BYTES
                {
                    die!(
                        "Archived AutoFDO profile '{}' is only {} bytes — re-run stage 2 profiling \
                         after installing linux-cachyos-dbg",
                        name,
                        meta.len()
                    );
                }
            }
            blog!("Restoring profile {name} from archive...");
            if let Err(e) = fs::copy(&src, &dest) {
                die!("Failed to restore profile {name}: {e}");
            }
        } else if *name == "kernel-compilation.afdo" {
            die!("Required profile '{name}' missing in repo and archive");
        }
    }
}

fn run_logged_shell(cwd: &Path, cmd: &str, events: &EventLog) -> Result<(), String> {
    crate::utils::echo_shell_command(cmd, Some(cwd));
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn failed: {e}"))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // Stream stdout/stderr line-by-line as they arrive so long-running profile/convert
    // steps show live progress (and to the GUI which captures abs stdout/stderr).
    std::thread::scope(|s| {
        if let Some(out) = stdout {
            s.spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    events.log_line("stdout", line);
                }
            });
        }
        if let Some(err) = stderr {
            s.spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    events.log_line("stderr", line);
                }
            });
        }
    });

    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("command failed: {cmd}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PgoConfig;

    fn test_pgo() -> PgoConfig {
        PgoConfig {
            enabled: true,
            preset: "cachyos-kernel".into(),
            profiles_archive_dir: Some("/tmp/abs-pgo-test".into()),
            profile_scratch_dir: "auto".into(),
            perf_data_on_ram: true,
            benchmark_command: Some("/bin/true".into()),
            benchmark_workdir: None,
            benchmark_preset: "fast".into(),
            profiling_quality: "maximum".into(),
            build_user: None,
            perf_event_args: "auto".into(),
            perf_extra_args: crate::config::PERF_EXTRA_ARGS_STANDARD.into(),
            sysctl_command: None,
            vmlinux: "auto".into(),
            afdo_tool: "llvm-profgen".into(),
            propeller_tool: "create_llvm_prof".into(),
            afdo_profile_name: "kernel-compilation.afdo".into(),
            verify_boot: true,
            auto_restart: false,
            state_file: None,
        }
    }

    #[test]
    fn resolve_vmlinux_explicit_path() {
        let dir = std::env::temp_dir().join(format!("abs-vmlinux-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let vmlinux = dir.join("vmlinux");
        fs::write(&vmlinux, b"stub").unwrap();

        let mut pgo = test_pgo();
        pgo.vmlinux = vmlinux.to_string_lossy().into_owned();
        assert_eq!(resolve_vmlinux(&pgo, None, None).unwrap(), vmlinux);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_vmlinux_prefers_debug_package_path() {
        let pgo = test_pgo();
        let debug = PathBuf::from("/usr/src/debug/linux-cachyos/vmlinux");
        if !vmlinux_usable_for_profiling(&debug) {
            return;
        }
        assert_eq!(
            resolve_vmlinux(&pgo, None, Some("linux-cachyos")).unwrap(),
            debug
        );
    }

    #[test]
    fn validate_afdo_profile_rejects_tiny_files() {
        let dir = std::env::temp_dir().join(format!("abs-afdo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("kernel-compilation.afdo");
        fs::write(&path, vec![0u8; 330]).unwrap();
        assert!(validate_afdo_profile(&path).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn benchmark_workdir_defaults_under_archive() {
        let pgo = test_pgo();
        assert_eq!(
            pgo.resolved_benchmark_workdir("linux-cachyos"),
            PathBuf::from("/tmp/abs-pgo-test/benchmark-workdir")
        );
    }

    #[test]
    fn stage1_env_has_autofdo_and_debug() {
        let env = stage1_env(&KernelBuildConfig::default());
        assert_eq!(env.get("_autofdo").map(String::as_str), Some("yes"));
        assert_eq!(env.get("_build_debug").map(String::as_str), Some("yes"));
        assert_eq!(env.get("_use_llvm_lto").map(String::as_str), Some("none"));
    }

    #[test]
    fn stage1_user_overrides_do_not_clobber_pgo_flags() {
        let kernel = KernelBuildConfig {
            cpusched: Some("bore".into()),
            use_llvm_lto: Some("thin".into()),
            use_kcfi: Some("no".into()),
            ..Default::default()
        };
        let mut env = stage1_env(&kernel);
        merge_user_kernel_overrides(&mut env, &kernel);
        assert_eq!(env.get("_cpusched").map(String::as_str), Some("bore"));
        assert_eq!(env.get("_use_llvm_lto").map(String::as_str), Some("none"));
        assert_eq!(env.get("_use_kcfi").map(String::as_str), Some("yes"));
    }

    #[test]
    fn stage3_env_has_propeller_profiles() {
        let env = stage3_build_env(&KernelBuildConfig::default(), "kernel-compilation.afdo");
        assert_eq!(env.get("_propeller_profiles").map(String::as_str), Some("yes"));
        assert_eq!(env.get("_build_debug").map(String::as_str), Some("no"));
    }

    #[test]
    fn resolved_perf_extra_args_maximum_uses_densest_sampling() {
        let mut pgo = test_pgo();
        pgo.profiling_quality = "maximum".into();
        pgo.perf_extra_args = crate::config::PERF_EXTRA_ARGS_STANDARD.into();
        assert_eq!(
            super::resolved_perf_extra_args(&pgo),
            crate::config::PERF_EXTRA_ARGS_MAXIMUM
        );
    }

    #[test]
    fn resolved_perf_extra_args_custom_override() {
        let mut pgo = test_pgo();
        pgo.perf_extra_args = "--mmap-pages 131072 -a -N -b -c 42000".into();
        assert_eq!(
            super::resolved_perf_extra_args(&pgo),
            "--mmap-pages 131072 -a -N -b -c 42000"
        );
    }

    #[test]
    fn auto_perf_event_args_amd_zen() {
        assert!(auto_perf_event_args_for_march("znver3").contains("pfm-events"));
        assert!(auto_perf_event_args_for_march("znver5").contains("zen5"));
    }

    #[test]
    fn auto_perf_event_args_intel_platforms() {
        for arch in [
            "sandybridge",
            "haswell",
            "skylake",
            "skylake-avx512",
            "kabylake",
            "icelake-client",
            "icelake-server",
            "tigerlake",
            "alderlake",
            "raptorlake",
            "sapphirerapids",
            "unknown-cpu",
        ] {
            assert_eq!(
                auto_perf_event_args_for_march(arch),
                INTEL_TAKEN_BRANCH_PERF_EVENT,
                "arch={arch}"
            );
        }
    }

    #[test]
    fn detect_perf_intel_fallback() {
        let pgo = test_pgo();
        let args = detect_perf_event_args(&pgo).unwrap();
        assert!(args.contains("BR_INST_RETIRED") || args.contains("pfm-events"));
    }

    #[test]
    fn perf_data_usable_requires_minimum_size() {
        let dir = std::env::temp_dir().join(format!("abs-pgo-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let tiny = dir.join("tiny.data");
        fs::write(&tiny, vec![0u8; 1024]).unwrap();
        assert!(super::perf_data_usable(&tiny).is_none());
        let big = dir.join("big.data");
        fs::write(&big, vec![0u8; MIN_USABLE_PERF_BYTES as usize]).unwrap();
        assert_eq!(super::perf_data_usable(&big), Some(MIN_USABLE_PERF_BYTES));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_pgo_stage_aliases() {
        assert_eq!(
            parse_pgo_stage("stage2_profile").unwrap(),
            PgoStageId::Stage2Profile
        );
        assert_eq!(parse_pgo_stage("2p").unwrap(), PgoStageId::Stage2Profile);
        assert_eq!(parse_pgo_stage("profile").unwrap(), PgoStageId::Stage2Profile);
        assert_eq!(
            parse_pgo_stage("stage1_build").unwrap(),
            PgoStageId::Stage1Build
        );
        assert!(parse_pgo_stage("not-a-stage").is_err());
    }

    #[test]
    fn pgo_stage_labels_non_empty() {
        assert!(!PgoStageId::Stage1Build.label().is_empty());
        assert!(!PgoStageId::Done.label().is_empty());
        assert_eq!(PgoStageId::Stage2Build.label(), "Stage 2: AutoFDO build");
        assert_eq!(PgoStageId::Stage3Build.label(), "Stage 3: final build");
    }

    #[test]
    fn kernel_hold_package_names_uses_package_for_other_kernels() {
        let state = PgoState {
            package: "linux-zen".into(),
            repo_dir: "/tmp".into(),
            current_stage: PgoStageId::Stage2Build,
            started_at: 0,
            updated_at: 0,
            expected_kernel_uname: Some("6.12.1-zen1-1-zen".into()),
            expected_package_base: Some("linux-zen".into()),
            stage_history: Vec::new(),
        };
        let names = super::kernel_hold_package_names(&state);
        assert!(names.contains(&"linux-zen".to_string()));
        assert!(names.contains(&"linux-zen-dbg".to_string()));
        assert!(names.contains(&"linux-zen-headers".to_string()));
        assert!(!names.contains(&"linux-cachyos".to_string()));
    }

    #[test]
    fn active_pipelines_discovers_custom_state_file_for_any_kernel() {
        use crate::config::{Config, PackageConfig, PgoConfig};

        let dir = std::env::temp_dir().join(format!("abs-pgo-state-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let state_path = dir.join("linux-zen.json");
        let state = PgoState {
            package: "linux-zen".into(),
            repo_dir: "/tmp/repo".into(),
            current_stage: PgoStageId::Stage2Build,
            started_at: 0,
            updated_at: 0,
            expected_kernel_uname: None,
            expected_package_base: None,
            stage_history: Vec::new(),
        };
        fs::write(
            &state_path,
            serde_json::to_string_pretty(&state).unwrap(),
        )
        .unwrap();

        let mut config: Config = toml::from_str(
            r#"
config_version = 1
manual_update_packages = []
skip_install_packages = []

[paths]
packages_path = "/tmp"
chroot_base_path = "/tmp"
ready_made_packages_path = "/tmp"

[build]
default_environment = "local"

[system_update]
command_to_update_repositories = "pacman -Sy"
command_to_perform_system_update = "pacman -Syu"
ignore_flag = "--ignore"
ignore_packages = []

[repositories]
default = "aur"

[packages]
"#,
        )
        .unwrap();
        let pgo = PgoConfig {
            enabled: false,
            preset: "cachyos-kernel".into(),
            profiles_archive_dir: Some(dir.to_string_lossy().into_owned()),
            profile_scratch_dir: "auto".into(),
            perf_data_on_ram: true,
            benchmark_command: None,
            benchmark_workdir: None,
            benchmark_preset: "fast".into(),
            profiling_quality: "maximum".into(),
            build_user: None,
            perf_event_args: "auto".into(),
            perf_extra_args: crate::config::PERF_EXTRA_ARGS_STANDARD.into(),
            sysctl_command: None,
            vmlinux: "auto".into(),
            afdo_tool: "llvm-profgen".into(),
            propeller_tool: "create_llvm_prof".into(),
            afdo_profile_name: "kernel-compilation.afdo".into(),
            verify_boot: true,
            auto_restart: false,
            state_file: Some(state_path.to_string_lossy().into_owned()),
        };
        config.packages.insert(
            "linux-zen".into(),
            PackageConfig {
                pgo: Some(pgo),
                ..Default::default()
            },
        );

        let active = super::active_pipelines(&config);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].package, "linux-zen");
        assert_eq!(active[0].stage_label, PgoStageId::Stage2Build.label());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn kernel_hold_package_names_include_both_cachyos_variants() {
        let state = PgoState {
            package: "linux-cachyos".into(),
            repo_dir: "/tmp".into(),
            current_stage: PgoStageId::WaitReboot2,
            started_at: 0,
            updated_at: 0,
            expected_kernel_uname: Some("7.1.1-2.2-cachyos-lto".into()),
            expected_package_base: Some("linux-cachyos-lto".into()),
            stage_history: Vec::new(),
        };
        let names = super::kernel_hold_package_names(&state);
        assert!(names.contains(&"linux-cachyos-lto".to_string()));
        assert!(names.contains(&"linux-cachyos".to_string()));
        assert!(names.contains(&"linux-cachyos-lto-dbg".to_string()));
    }

    #[test]
    fn reboot_resume_message_mentions_lto_boot_files() {
        let state = PgoState {
            package: "linux-cachyos".into(),
            repo_dir: "/tmp".into(),
            current_stage: PgoStageId::WaitReboot2,
            started_at: 0,
            updated_at: 0,
            expected_kernel_uname: Some("7.1.1-2.2-cachyos-lto".into()),
            expected_package_base: Some("linux-cachyos-lto".into()),
            stage_history: Vec::new(),
        };
        let msg = super::reboot_resume_message(&state, "linux-cachyos");
        assert!(msg.contains("vmlinuz-linux-cachyos-lto"));
        assert!(msg.contains("initramfs-linux-cachyos-lto.img"));
    }

    #[test]
    fn event_log_creates_nested_file() {
        let dir = std::env::temp_dir().join(format!("abs-pgo-events-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("linux-cachyos.events.jsonl");
        let log = EventLog::new(Some(path.clone()), false);
        assert!(path.is_file(), "event log file should exist after EventLog::new");
        log.emit(&PgoEvent::Error {
            ts: 0,
            message: "test".into(),
        });
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("test"));
        let _ = fs::remove_dir_all(&dir);
    }
}
