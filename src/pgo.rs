//! Multi-stage kernel PGO pipeline (CachyOS linux-cachyos preset).

use crate::build::{self, PgoBuildContext};
use crate::cli::Cli;
use crate::config::{self, Config, KernelBuildConfig, PgoConfig};
use crate::package_spec::PackageSpec;
use crate::utils::{run_command, run_command_with_output, sh_single_quote};
use crate::{blog, die, ewarn, vlog};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const PROPELLER_TOOL_GENERATE: &str = "generate_propeller_profiles";
const PROPELLER_TOOL_CREATE_LLVM_PROF: &str = "create_llvm_prof";
/// Current LLVM SHT_LLVM_BB_ADDR_MAP (see llvm/BinaryFormat/ELF.h).
const SHT_LLVM_BB_ADDR_MAP: u32 = 0x6fff_4c0a;
/// Legacy BB_ADDR_MAP type (SHT_LLVM_BB_ADDR_MAP_V0).
const SHT_LLVM_BB_ADDR_MAP_V0: u32 = 0x6fff_4c08;
const PROPELLER_BUILD_SCRIPT: &str = include_str!("../assets/build-generate-propeller-profiles.sh");

/// Pipeline stage identifiers persisted in state file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PgoStageId {
    WaitReboot0,
    Stage1Build,
    WaitReboot1,
    Stage2Profile,
    Stage2Build,
    WaitReboot2,
    Stage3Profile,
    Stage3Build,
    WaitReboot3,
    Done,
    Aborted,
}

impl PgoStageId {
    pub fn label(self) -> &'static str {
        match self {
            Self::WaitReboot0 => "Waiting for reboot (fresh boot before first comparison)",
            Self::Stage1Build => "Stage 1: debug kernel build",
            Self::WaitReboot1 => "Waiting for reboot (boot stage-1 kernel)",
            Self::Stage2Profile => "Stage 2: profile AutoFDO",
            Self::Stage2Build => "Stage 2: AutoFDO build",
            Self::WaitReboot2 => "Waiting for reboot (boot stage-2 kernel)",
            Self::Stage3Profile => "Stage 3: profile Propeller",
            Self::Stage3Build => "Stage 3: final build",
            Self::WaitReboot3 => "Waiting for reboot (boot final PGO kernel)",
            Self::Done => "Done",
            Self::Aborted => "Aborted",
        }
    }

    pub fn is_wait_reboot(self) -> bool {
        matches!(
            self,
            Self::WaitReboot0 | Self::WaitReboot1 | Self::WaitReboot2 | Self::WaitReboot3
        )
    }

    /// Stages the user may select with `--pgo-stage` (excludes terminal `done` / `aborted`).
    pub fn selectable_stages() -> &'static [PgoStageId] {
        &[
            Self::WaitReboot0,
            Self::Stage1Build,
            Self::WaitReboot1,
            Self::Stage2Profile,
            Self::Stage2Build,
            Self::WaitReboot2,
            Self::Stage3Profile,
            Self::Stage3Build,
            Self::WaitReboot3,
        ]
    }
}

/// Parse `--pgo-stage` values (serde snake_case names and short aliases).
pub fn parse_pgo_stage(raw: &str) -> Result<PgoStageId, String> {
    let norm = raw.trim().to_lowercase().replace('-', "_");
    let stage = match norm.as_str() {
        "wait0" | "reboot0" | "wait_reboot0" | "start_reboot" => PgoStageId::WaitReboot0,
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
        "wait3" | "reboot3" | "wait_reboot3" => PgoStageId::WaitReboot3,
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
        if let Err(e) = OpenOptions::new().create(true).append(true).open(path) {
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

    if should_handoff_to_visible_terminal(cli, config) {
        match crate::terminal::handoff_auto_resume() {
            Ok(0) => return,
            Ok(code) => std::process::exit(code),
            Err(e) => {
                die!(
                    "Could not continue PGO auto-resume ({e}). Log in graphically or on a console/SSH TTY, then run: abs --pgo-resume {package}"
                );
            }
        }
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
    let pgo = pkg.pgo.clone().filter(|p| p.enabled).unwrap_or_else(|| {
        die!("Package '{package}' has no enabled [packages.{package}.pgo] section")
    });
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
        die!(
            "Failed to create profiles_archive_dir '{}': {}",
            archive.display(),
            e
        );
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

/// Auto PGO started without a TTY (systemd after reboot) should open a terminal.
pub fn should_handoff_to_visible_terminal(cli: &Cli, config: &Config) -> bool {
    if cli.json || cli.pgo_status.is_some() || cli.pgo_goto || cli.pgo_abort.is_some() {
        return false;
    }
    if cli.pgo.is_none() && cli.pgo_resume.is_none() && cli.pgo_restart.is_none() {
        return false;
    }
    let package = cli
        .pgo
        .as_deref()
        .or(cli.pgo_resume.as_deref())
        .or(cli.pgo_restart.as_deref())
        .or_else(|| cli.packages.first().map(String::as_str));
    let auto = cli.pgo_auto
        || package
            .and_then(|pkg| config.packages.get(pkg))
            .and_then(|p| p.pgo.as_ref())
            .is_some_and(|p| p.auto_restart);
    crate::terminal::should_handoff(
        auto,
        cli.json,
        crate::terminal::stdin_is_tty(),
        crate::terminal::already_in_visible_terminal(),
    )
}

fn pgo_auto_systemd_unit(package: &str) -> String {
    format!("abs-pgo@{package}.service")
}

fn pgo_auto_resume_enable_links(unit_dir: &Path, package: &str) -> Vec<PathBuf> {
    let instance = pgo_auto_systemd_unit(package);
    ["default.target.wants", "graphical-session.target.wants"]
        .iter()
        .map(|wants| unit_dir.join(wants).join(&instance))
        .collect()
}

fn unlink_pgo_auto_resume_links(unit_dir: &Path, package: &str) {
    for path in pgo_auto_resume_enable_links(unit_dir, package) {
        let _ = fs::remove_file(path);
    }
}

fn systemd_quote_exec(path: &str) -> String {
    if path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '+'))
    {
        path.to_string()
    } else {
        format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
    }
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
    fs::create_dir_all(&unit_dir).map_err(|e| format!("create {}: {e}", unit_dir.display()))?;
    let template = unit_dir.join("abs-pgo@.service");
    // Note: no After=network-online.target — user units cannot order against that system
    // target, and git/network steps in the pipeline fail with clear errors on their own.
    let unit = pgo_auto_unit_text(&abs_bin);
    fs::write(&template, unit).map_err(|e| format!("write {}: {e}", template.display()))?;
    run_command("systemctl", &["--user", "daemon-reload"], None::<&str>)
        .map_err(|e| e.to_string())?;
    if let Err(e) = crate::pgo_priv::install_dropin() {
        return Err(format!(
            "could not install passwordless PGO sudo helper (needed after reboot): {e}"
        ));
    }
    let instance = pgo_auto_systemd_unit(package);
    // reenable drops a previous WantedBy= (e.g. graphical-session.target) so the unit follows the
    // current template after a reboot.
    run_command(
        "systemctl",
        &["--user", "reenable", instance.as_str()],
        None::<&str>,
    )
    .map_err(|e| format!("enable {instance}: {e}"))?;
    match run_command("loginctl", &["enable-linger"], None::<&str>) {
        Ok(()) => {
            blog!(
                "Enabled lingering for this user so PGO can wait for a console login after reboot"
            );
        }
        Err(e) => {
            ewarn!(
                "Could not enable linger ({e}); without a graphical login, log in on a TTY after reboot or run: loginctl enable-linger"
            );
        }
    }
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
    unlink_pgo_auto_resume_links(&pgo_auto_systemd_dir(), package);
}

fn pgo_auto_unit_text(abs_bin: &str) -> String {
    format!(
        "[Unit]\n\
         Description=Resume ABS PGO pipeline for %i after reboot\n\
         After=graphical-session.target\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         TimeoutStartSec=infinity\n\
         ExecStart={} --pgo-resume %i --pgo-auto\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        systemd_quote_exec(abs_bin)
    )
}

fn trigger_pgo_auto_reboot(
    package: &str,
    next: Option<&crate::boot_entry::NextBoot>,
) -> Result<(), String> {
    install_pgo_auto_resume_service(package)?;
    blog!("PGO auto-restart: rebooting in 5 seconds to continue pipeline…");
    std::thread::sleep(std::time::Duration::from_secs(5));
    crate::boot_entry::reboot(next)
}

fn transition(state: &mut PgoState, stage: PgoStageId) {
    state
        .stage_history
        .push(format!("{:?}", state.current_stage));
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
        && !matches!(
            existing.current_stage,
            PgoStageId::Done | PgoStageId::Aborted
        )
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
    let plan = live_work_plan(&pgo);
    let mut state = PgoState {
        package: package.to_string(),
        repo_dir: repo.pkg_dir.to_string_lossy().into_owned(),
        current_stage: if plan.reboot_before_start {
            PgoStageId::WaitReboot0
        } else {
            first_post_start_reboot_stage(plan)
        },
        started_at: EventLog::now(),
        updated_at: EventLog::now(),
        expected_kernel_uname: None,
        expected_package_base: None,
        stage_history: Vec::new(),
    };
    if plan.reboot_before_start {
        stamp_running_kernel(&mut state);
    } else {
        maybe_run_compare_benchmark(&pgo, package, CompareStage::Current, events, false);
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
    let mut state = load_state(&state_path).unwrap_or_else(|| {
        die!(
            "No PGO state at '{}'; run --pgo first",
            state_path.display()
        )
    });
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
    let mut state = load_state(&state_path).unwrap_or_else(|| {
        die!(
            "No PGO state at '{}'; run --pgo first",
            state_path.display()
        )
    });

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
            PgoStageId::WaitReboot0 => {
                if pgo.verify_boot {
                    verify_boot_kernel(&state, &pgo);
                }
                maybe_run_compare_benchmark(&pgo, package, CompareStage::Current, events, false);
                transition(
                    &mut state,
                    first_post_start_reboot_stage(live_work_plan(&pgo)),
                );
            }
            PgoStageId::WaitReboot1 => {
                if pgo.verify_boot {
                    verify_boot_kernel(&state, &pgo);
                }
                let plan = live_work_plan(&pgo);
                if plan.run_afdo_collect {
                    transition(&mut state, PgoStageId::Stage2Profile);
                } else {
                    maybe_run_compare_benchmark(
                        &pgo,
                        package,
                        CompareStage::DebugClean,
                        events,
                        false,
                    );
                    if plan.run_autofdo_build {
                        transition(&mut state, PgoStageId::Stage2Build);
                    } else {
                        transition(&mut state, PgoStageId::Stage3Build);
                    }
                }
            }
            PgoStageId::WaitReboot2 => {
                if pgo.verify_boot {
                    verify_boot_kernel(&state, &pgo);
                }
                let plan = live_work_plan(&pgo);
                if plan.run_propeller_collect {
                    transition(&mut state, PgoStageId::Stage3Profile);
                } else {
                    maybe_run_compare_benchmark(
                        &pgo,
                        package,
                        CompareStage::AutofdoClean,
                        events,
                        false,
                    );
                    transition(&mut state, PgoStageId::Stage3Build);
                }
            }
            PgoStageId::WaitReboot3 => {
                if pgo.verify_boot {
                    verify_boot_kernel(&state, &pgo);
                }
                maybe_run_compare_benchmark(&pgo, package, CompareStage::Final, events, false);
                transition(&mut state, PgoStageId::Done);
                crate::pkgbuild::restore_pkgbuild(Path::new(&state.repo_dir));
                remove_pgo_auto_resume_service(package);
                save_state(&state_path, &state);
                blog!("PGO pipeline complete for {}", package);
                if cli.json {
                    print_json_status(&state, &pgo);
                }
                return;
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
                blog!(
                    "Resuming in-progress stage: {}",
                    state.current_stage.label()
                );
            }
        }
    }

    // Re-check required tools on every resume: a system update between stages (or across the
    // reboot) can remove perf/llvm-profgen, and failing here beats dying mid-stage.
    preflight(&pgo, package, config);

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
            PgoStageId::WaitReboot0
            | PgoStageId::WaitReboot1
            | PgoStageId::WaitReboot2
            | PgoStageId::WaitReboot3 => {
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
            let mark = if *stage == state.current_stage {
                " (current)"
            } else {
                ""
            };
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

fn abort_disposition(keep_stage: bool) -> PgoAbortDisposition {
    if keep_stage {
        PgoAbortDisposition::KeepStage
    } else {
        PgoAbortDisposition::RemoveState
    }
}

fn apply_abort_state(state_path: &Path, disposition: PgoAbortDisposition) {
    match disposition {
        PgoAbortDisposition::KeepStage => {}
        PgoAbortDisposition::RemoveState => {
            let _ = fs::remove_file(state_path);
        }
    }
}

fn run_abort(package: &str, cli: &Cli, config: &Config, events: &EventLog) {
    run_abort_inner(
        package,
        config,
        events,
        abort_disposition(cli.pgo_keep_stage),
    );
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
        PgoAbortDisposition::RemoveState => {
            apply_abort_state(&state_path, disposition);
            blog!("PGO pipeline reset for {package}");
            events.log_line("stdout", format!("PGO pipeline reset for {package}"));
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
        archive_dir: pgo.resolved_archive_dir().map(|p| p.display().to_string()),
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
    let boot_ready = state.current_stage.is_wait_reboot() && boot_matches_expected(state);
    let (reboot_required, next_action) = match state.current_stage {
        stage if stage.is_wait_reboot() && boot_ready => {
            (false, format!("abs --pgo-resume {}", state.package))
        }
        stage if stage.is_wait_reboot() => (true, reboot_resume_message(state, &state.package)),
        PgoStageId::Done => (false, "none".to_string()),
        PgoStageId::Aborted => (false, "run --pgo to start a fresh pipeline".to_string()),
        _ => (false, format!("abs --pgo-resume {}", state.package)),
    };
    let out = StatusOut {
        package: &state.package,
        stage: state.current_stage,
        stage_label: state.current_stage.label(),
        expected_kernel_uname: state.expected_kernel_uname.as_deref(),
        expected_package_base: state.expected_package_base.as_deref(),
        state_file: pgo
            .resolved_state_file(&state.package)
            .display()
            .to_string(),
        archive_dir: pgo.resolved_archive_dir().map(|p| p.display().to_string()),
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
    match resolve_propeller_tool(&pgo.propeller_tool) {
        Ok(_) => {}
        Err(_) if can_bootstrap_generate_propeller_profiles() => {
            ewarn!(
                "No Propeller converter in PATH; stage 3 will build {PROPELLER_TOOL_GENERATE} \
                 from https://github.com/google/llvm-propeller against system LLVM"
            );
        }
        Err(e) => die!("{e}"),
    }
    if let Err(e) = crate::pgo_benchmark::resolve_benchmark_command(&pgo.benchmark_command) {
        die!("{e}");
    }
    if pgo.compare_any() && which("cachyos-benchmarker").is_none() {
        die!(
            "Comparison benchmarks require cachyos-benchmarker in PATH \
             (enable compare_current / compare_debug_clean / compare_autofdo_clean / compare_final)"
        );
    }
    if let Some(pc) = config.packages.get(package)
        && let Ok(targets) = crate::ramdisk::resolve_ramdisk_targets(config, Some(pc), None, None)
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
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return None;
    }
    let given = Path::new(cmd);
    if given.components().count() > 1 {
        return given.is_file().then(|| given.to_path_buf());
    }
    if let Ok(s) = run_command_with_output("which", &[cmd], None::<&str>) {
        let p = PathBuf::from(s.trim());
        if p.is_file() {
            return Some(p);
        }
    }
    // systemd --user often has a slim PATH; still look in the usual bins.
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for extra in ["/usr/local/bin", "/usr/bin", "/bin"] {
        let extra = PathBuf::from(extra);
        if !dirs.iter().any(|d| d == &extra) {
            dirs.push(extra);
        }
    }
    dirs.into_iter().map(|d| d.join(cmd)).find(|p| p.is_file())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompareStage {
    Current,
    Debug,
    DebugClean,
    Autofdo,
    AutofdoClean,
    Final,
}

impl CompareStage {
    fn slug(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Debug => "debug",
            Self::DebugClean => "debug_clean",
            Self::Autofdo => "autofdo",
            Self::AutofdoClean => "autofdo_clean",
            Self::Final => "final",
        }
    }

    fn enabled(self, pgo: &PgoConfig) -> bool {
        match self {
            Self::Current => pgo.compare_current,
            Self::Debug => pgo.compare_debug,
            Self::DebugClean => pgo.compare_debug_clean,
            Self::Autofdo => pgo.compare_autofdo,
            Self::AutofdoClean => pgo.compare_autofdo_clean,
            Self::Final => pgo.compare_final,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Current => "current (pre-PGO) kernel",
            Self::Debug => "debug PGO kernel (with perf)",
            Self::DebugClean => "debug PGO kernel (no perf)",
            Self::Autofdo => "AutoFDO kernel (with perf)",
            Self::AutofdoClean => "AutoFDO kernel (no perf)",
            Self::Final => "final Propeller kernel",
        }
    }

    /// Debug and AutoFDO comparison scores come from the AutoFDO/Propeller
    /// `perf record` pass. Current, final, and the extra *_clean stages stay
    /// as clean standalone runs.
    fn shares_profiling_run(self) -> bool {
        matches!(self, Self::Debug | Self::Autofdo)
    }
}

fn should_run_standalone_compare(pgo: &PgoConfig, stage: CompareStage) -> bool {
    stage.enabled(pgo) && !stage.shares_profiling_run()
}

fn profile_compare_stage(pgo: &PgoConfig, stage: CompareStage) -> Option<CompareStage> {
    if stage.shares_profiling_run() && pgo.compare_any() {
        Some(stage)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PgoWorkPlan {
    reboot_before_start: bool,
    run_debug_build: bool,
    run_afdo_collect: bool,
    run_autofdo_build: bool,
    run_propeller_collect: bool,
}

fn pgo_work_plan(pgo: &PgoConfig, afdo_reusable: bool, propeller_reusable: bool) -> PgoWorkPlan {
    let afdo_ok = pgo.reuse_afdo_profile && afdo_reusable;
    let prop_ok = pgo.reuse_propeller_profile && propeller_reusable;
    PgoWorkPlan {
        reboot_before_start: pgo.reboot_before_start,
        run_debug_build: pgo.compare_debug_clean || !afdo_ok,
        run_afdo_collect: !afdo_ok,
        run_autofdo_build: pgo.compare_autofdo_clean || !prop_ok,
        run_propeller_collect: !prop_ok,
    }
}

fn live_work_plan(pgo: &PgoConfig) -> PgoWorkPlan {
    pgo_work_plan(
        pgo,
        archived_afdo_reusable(pgo),
        archived_propeller_reusable(pgo),
    )
}

fn first_post_start_reboot_stage(plan: PgoWorkPlan) -> PgoStageId {
    if plan.run_debug_build {
        PgoStageId::Stage1Build
    } else if plan.run_autofdo_build {
        PgoStageId::Stage2Build
    } else {
        PgoStageId::Stage3Build
    }
}

fn archived_afdo_reusable(pgo: &PgoConfig) -> bool {
    let Some(archive) = pgo.resolved_archive_dir() else {
        return false;
    };
    validate_afdo_profile(&archive.join(&pgo.afdo_profile_name)).is_ok()
}

fn archived_propeller_reusable(pgo: &PgoConfig) -> bool {
    let Some(archive) = pgo.resolved_archive_dir() else {
        return false;
    };
    validate_propeller_profile(&archive.join("propeller_cc_profile.txt")).is_ok()
        && validate_propeller_profile(&archive.join("propeller_ld_profile.txt")).is_ok()
}

fn profiling_workload(pgo: &PgoConfig, combine: Option<CompareStage>) -> (&str, Option<String>) {
    let preset = resolved_benchmark_preset(pgo, combine.is_some());
    let label = combine.map(|s| crate::pgo_benchmark::compare_run_label(s.slug()));
    (preset, label)
}

fn benchie_logs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut logs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("benchie_") && n.ends_with(".log"))
        })
        .collect();
    logs.sort();
    logs
}

fn drop_page_cache() {
    if let Err(e) = run_command(
        "sudo",
        &["sh", "-c", "sync; echo 1 > /proc/sys/vm/drop_caches"],
        None::<&str>,
    ) {
        ewarn!("Could not drop page cache before comparison benchmark: {e}");
    }
}

/// Clean cachyos-benchmarker run (no perf record) plus comparison charts.
/// Debug/AutoFDO stages skip this: their comparison is the profiling run.
/// `after_perf`: the profiling pass just ran on this kernel, so skip cache-drop + fast warm-up.
fn maybe_run_compare_benchmark(
    pgo: &PgoConfig,
    package: &str,
    stage: CompareStage,
    events: &EventLog,
    after_perf: bool,
) {
    if !should_run_standalone_compare(pgo, stage) {
        return;
    }
    let assets = pgo.resolved_benchmark_workdir(package);
    if let Err(e) = fs::create_dir_all(&assets) {
        die!("create benchmark workdir {}: {e}", assets.display());
    }

    let run_label = crate::pgo_benchmark::compare_run_label(stage.slug());
    blog!(
        "Comparison benchmark: {} (label={run_label}, no perf record)…",
        stage.title()
    );
    events.log_line(
        "stdout",
        format!(
            "Clean cachyos-benchmarker for {} — charts in {}",
            stage.title(),
            pgo.resolved_compare_dir(package).display()
        ),
    );

    let benchmark = crate::pgo_benchmark::resolve_benchmark_command(&pgo.benchmark_command)
        .unwrap_or_else(|e| die!("{e}"));
    if after_perf {
        blog!("Skipping warm-up (profiling run on this kernel just finished)");
    } else {
        drop_page_cache();
        blog!("Warm-up (fast sysbench/stress-ng) before scored comparison…");
        let warm = crate::pgo_benchmark::warmup_compare_command(&assets, &benchmark);
        if let Err(e) = run_logged_shell(&assets, &warm, events) {
            ewarn!("Comparison warm-up failed (continuing with scored run): {e}");
        }
    }

    let before = benchie_logs(&assets);
    let cmd = crate::pgo_benchmark::standalone_compare_command(&assets, &run_label, &benchmark);
    if let Err(e) = run_logged_shell(&assets, &cmd, events) {
        die!("Comparison benchmark ({}) failed: {e}", stage.title());
    }

    publish_compare_log(pgo, package, stage, events, Some(&before), true);
}

fn publish_compare_log(
    pgo: &PgoConfig,
    package: &str,
    stage: CompareStage,
    events: &EventLog,
    before: Option<&[PathBuf]>,
    required: bool,
) {
    let assets = pgo.resolved_benchmark_workdir(package);
    let compare_dir = pgo.resolved_compare_dir(package);
    if let Err(e) = fs::create_dir_all(&compare_dir) {
        die!("create compare-benchmarks {}: {e}", compare_dir.display());
    }

    let uname = running_kernel_uname();
    let run_label = crate::pgo_benchmark::compare_run_label(stage.slug());
    let after = benchie_logs(&assets);
    let new_logs: Vec<_> = before
        .map(|old| {
            after
                .iter()
                .filter(|p| !old.iter().any(|prev| prev == *p))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let src = new_logs
        .last()
        .cloned()
        .or_else(|| after_last_matching(&assets, &run_label));
    let Some(src) = src else {
        let msg = format!(
            "Comparison benchmark produced no benchie_*.log in {}",
            assets.display()
        );
        if required {
            die!("{msg}");
        }
        ewarn!("{msg}");
        return;
    };

    let raw = fs::read_to_string(&src).unwrap_or_else(|e| die!("read {}: {e}", src.display()));
    let relabeled = crate::pgo_benchmark::relabel_compare_log(&raw, stage.slug(), &uname);
    let dest_name = format!(
        "benchie_{}_{}.log",
        crate::pgo_benchmark::compare_run_label(stage.slug()),
        uname.replace('/', "-")
    );
    // Replace any previous log for this stage so scraper series stay 1:1 with kernels.
    if let Ok(entries) = fs::read_dir(&compare_dir) {
        let prefix = format!(
            "benchie_{}_",
            crate::pgo_benchmark::compare_run_label(stage.slug())
        );
        for e in entries.flatten() {
            let p = e.path();
            if p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".log"))
            {
                let _ = fs::remove_file(&p);
            }
        }
    }
    let dest = compare_dir.join(dest_name);
    if let Err(e) = fs::write(&dest, relabeled) {
        die!("write {}: {e}", dest.display());
    }
    blog!("Comparison log: {}", dest.display());
    compile_compare_chart_sets(pgo, package, events);
}

fn compile_compare_chart_sets(pgo: &PgoConfig, package: &str, events: &EventLog) {
    use crate::pgo_benchmark::{
        chart_kernel_token, chart_set_dir_name, compare_index_html, compare_stage_is_overhead,
        include_stage_in_chart_set, relabel_kernel_token, slug_from_benchie_name,
    };

    let compare_dir = pgo.resolved_compare_dir(package);
    let mut logs: Vec<(String, PathBuf)> = Vec::new();
    for path in benchie_logs(&compare_dir) {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(slug) = slug_from_benchie_name(name) else {
            continue;
        };
        logs.push((slug.to_string(), path));
    }
    if logs.is_empty() {
        return;
    }
    logs.sort_by(|a, b| chart_kernel_token(&a.0).cmp(chart_kernel_token(&b.0)));
    let has_overhead = logs.iter().any(|(s, _)| compare_stage_is_overhead(s));

    let mut without_tokens: Vec<&str> = Vec::new();
    let mut with_tokens: Vec<&str> = Vec::new();
    for with_overhead in [false, true] {
        if with_overhead && !has_overhead {
            continue;
        }
        let set_dir = compare_dir.join(chart_set_dir_name(with_overhead));
        let _ = fs::remove_dir_all(&set_dir);
        if let Err(e) = fs::create_dir_all(&set_dir) {
            ewarn!("create {}: {e}", set_dir.display());
            continue;
        }
        for (slug, src) in &logs {
            if !include_stage_in_chart_set(slug, with_overhead) {
                continue;
            }
            let token = chart_kernel_token(slug);
            let Ok(raw) = fs::read_to_string(src) else {
                continue;
            };
            let dest = set_dir.join(format!("benchie_{token}.log"));
            if let Err(e) = fs::write(&dest, relabel_kernel_token(&raw, token)) {
                ewarn!("write {}: {e}", dest.display());
                continue;
            }
            if with_overhead {
                if !with_tokens.contains(&token) {
                    with_tokens.push(token);
                }
            } else if !without_tokens.contains(&token) {
                without_tokens.push(token);
            }
        }
        scrape_compare_dir(&set_dir, events);
    }

    let html = compare_index_html(has_overhead, &without_tokens, &with_tokens);
    let index = compare_dir.join("index.html");
    if let Err(e) = fs::write(&index, html) {
        ewarn!("write {}: {e}", index.display());
        return;
    }
    blog!("Comparison report: {}", index.display());
}

fn scrape_compare_dir(dir: &Path, _events: &EventLog) {
    match crate::pgo_scraper::scrape_benchie_dir(dir) {
        Ok(true) => {
            blog!(
                "Comparison charts updated in {} (categorized_comparison_All.svg, \
                 kernel_version_comparison_All.svg, test_performance.html)",
                dir.display()
            );
        }
        Ok(false) => {}
        Err(e) => {
            ewarn!(
                "comparison charts failed ({e}); logs are still in {}",
                dir.display()
            );
        }
    }
}

fn after_last_matching(dir: &Path, run_label: &str) -> Option<PathBuf> {
    let needle = format!("benchie_{run_label}_");
    benchie_logs(dir).into_iter().rev().find(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(&needle))
    })
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
    let (repo_name, repo_url, base_pkg) =
        build::resolve_pkg_repo(package, cli, config, Some(&spec));
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
    let kernel_cfg = || {
        config
            .packages
            .get(package)
            .and_then(|p| p.kernel.clone())
            .unwrap_or_default()
    };
    loop {
        save_state(state_path, state);
        let entry_stage = state.current_stage;
        events.emit(&PgoEvent::StageStart {
            ts: EventLog::now(),
            stage: entry_stage,
            package,
        });
        blog!("PGO {}: {}", package, state.current_stage.label());

        let plan = live_work_plan(pgo);
        match state.current_stage {
            PgoStageId::Stage1Build => {
                let kernel = kernel_cfg();
                let mut env = stage1_env(package, &kernel);
                merge_user_kernel_overrides(&mut env, &kernel);
                let build_ctx = PgoBuildContext {
                    env_vars: env,
                    makepkg_flags: "-f --skipinteg".to_string(),
                    clean_src: false,
                    clean_pkg: false,
                    defer_pkgbuild_restore: true,
                    skip_abs_install: false,
                };
                run_pgo_build(package, cli, config, &build_ctx, events);
                let pkgbase = build::pgo_pkgbase_from_env(package, &build_ctx.env_vars);
                record_installed_kernel(state, &pkgbase);
                transition(state, PgoStageId::WaitReboot1);
            }
            PgoStageId::Stage2Profile => {
                run_stage2_profile(state, pgo, package, cli, config, events);
                maybe_run_compare_benchmark(pgo, package, CompareStage::DebugClean, events, true);
                if plan.run_autofdo_build {
                    transition(state, PgoStageId::Stage2Build);
                } else {
                    transition(state, PgoStageId::Stage3Build);
                }
                save_state(state_path, state);
            }
            PgoStageId::Stage2Build => {
                restore_profiles_to_repo(state, pgo, &["kernel-compilation.afdo"], None);
                let kernel = kernel_cfg();
                let mut env = stage2_build_env(package, &kernel, &pgo.afdo_profile_name);
                merge_user_kernel_overrides(&mut env, &kernel);
                let build_ctx = PgoBuildContext {
                    env_vars: env,
                    makepkg_flags: "-f --skipinteg".to_string(),
                    clean_src: true,
                    clean_pkg: true,
                    defer_pkgbuild_restore: true,
                    skip_abs_install: false,
                };
                run_pgo_build(package, cli, config, &build_ctx, events);
                let pkgbase = build::pgo_pkgbase_from_env(package, &build_ctx.env_vars);
                record_installed_kernel(state, &pkgbase);
                if plan.run_propeller_collect || pgo.compare_autofdo_clean {
                    transition(state, PgoStageId::WaitReboot2);
                } else {
                    transition(state, PgoStageId::Stage3Build);
                }
            }
            PgoStageId::Stage3Profile => {
                run_stage3_profile(state, pgo, package, cli, config, events);
                maybe_run_compare_benchmark(pgo, package, CompareStage::AutofdoClean, events, true);
                transition(state, PgoStageId::Stage3Build);
                save_state(state_path, state);
            }
            PgoStageId::Stage3Build => {
                let scratch = scratch_dir(state, pgo, cli, config);
                restore_profiles_to_repo(
                    state,
                    pgo,
                    &[
                        "kernel-compilation.afdo",
                        "propeller_cc_profile.txt",
                        "propeller_ld_profile.txt",
                    ],
                    Some(&scratch),
                );
                let kernel = kernel_cfg();
                let mut env = stage3_build_env(package, &kernel, &pgo.afdo_profile_name);
                merge_user_kernel_overrides(&mut env, &kernel);
                let build_ctx = PgoBuildContext {
                    env_vars: env,
                    makepkg_flags: "-f --skipinteg".to_string(),
                    clean_src: true,
                    clean_pkg: true,
                    defer_pkgbuild_restore: false,
                    skip_abs_install: false,
                };
                run_pgo_build(package, cli, config, &build_ctx, events);
                let pkgbase = build::pgo_pkgbase_from_env(package, &build_ctx.env_vars);
                record_installed_kernel(state, &pkgbase);
                transition(state, PgoStageId::WaitReboot3);
            }
            PgoStageId::WaitReboot0
            | PgoStageId::WaitReboot1
            | PgoStageId::WaitReboot2
            | PgoStageId::WaitReboot3 => {
                let auto = pgo_auto_enabled(pgo, cli) && !run_once;
                let next = select_pgo_boot_kernel(pgo, state, auto);
                let msg = if auto {
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
                if auto {
                    save_state(state_path, state);
                    if let Err(e) = trigger_pgo_auto_reboot(package, next.as_ref()) {
                        die!("PGO auto-restart failed: {e}");
                    }
                }
                return;
            }
            PgoStageId::Done | PgoStageId::Aborted => return,
        }

        // Build stages fall through to WaitReboot; profile stages fall through to the matching
        // build (same boot). Log completion of the stage that just ran, then loop. `--pgo-once`
        // stops after this one stage.
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

fn stamp_running_kernel(state: &mut PgoState) {
    let id = running_kernel_identity();
    state.expected_kernel_uname = Some(id.uname);
    state.expected_package_base = id.pkgbase;
}

fn emit_reboot_hint_if_waiting(state: &PgoState, package: &str, events: &EventLog) {
    if !state.current_stage.is_wait_reboot() {
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

/// CachyOS PKGBUILD appends `-lto` to pkgbase when this is `yes` and LLVM LTO is on.
/// Keep the package name the user started PGO with so a later `linux-cachyos-lto` is not
/// a different repo package that CachyOS will try to replace.
fn pgo_lto_suffix_flag(package: &str) -> &'static str {
    let name = package.strip_suffix("-dbg").unwrap_or(package);
    if name.ends_with("-lto") { "yes" } else { "no" }
}

fn stage1_env(package: &str, _kernel: &KernelBuildConfig) -> HashMap<String, String> {
    HashMap::from([
        ("_use_llvm_lto".into(), "none".into()),
        ("_processor_opt".into(), "native".into()),
        (
            "_use_lto_suffix".into(),
            pgo_lto_suffix_flag(package).into(),
        ),
        ("_use_kcfi".into(), "yes".into()),
        ("_build_debug".into(), "yes".into()),
        ("_autofdo".into(), "yes".into()),
        ("_use_gcc_suffix".into(), "no".into()),
    ])
}

fn stage2_build_env(
    package: &str,
    _kernel: &KernelBuildConfig,
    profile: &str,
) -> HashMap<String, String> {
    HashMap::from([
        ("_use_llvm_lto".into(), "thin".into()),
        ("_processor_opt".into(), "native".into()),
        (
            "_use_lto_suffix".into(),
            pgo_lto_suffix_flag(package).into(),
        ),
        ("_use_kcfi".into(), "yes".into()),
        ("_build_debug".into(), "yes".into()),
        ("_autofdo".into(), "yes".into()),
        ("_autofdo_profile_name".into(), profile.into()),
        ("_use_gcc_suffix".into(), "no".into()),
        ("_propeller".into(), "yes".into()),
    ])
}

fn stage3_build_env(
    package: &str,
    _kernel: &KernelBuildConfig,
    profile: &str,
) -> HashMap<String, String> {
    HashMap::from([
        ("_use_llvm_lto".into(), "thin".into()),
        ("_processor_opt".into(), "native".into()),
        (
            "_use_lto_suffix".into(),
            pgo_lto_suffix_flag(package).into(),
        ),
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
            env.insert(key.to_string(), config::normalize_kernel_override(key, v));
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
    state.expected_kernel_uname = None;
    if let Ok(out) = run_command_with_output("pacman", &["-Q", package_base], None::<&str>) {
        let parts: Vec<&str> = out.split_whitespace().collect();
        if parts.len() >= 2 {
            state.expected_kernel_uname =
                Some(format!("{}-{}", parts[1], infer_suffix(package_base)));
        }
    }
    // No fallback to the running `uname -r` here: that is the kernel we booted *before* this
    // stage installed the new one, so recording it would make boot verification demand the
    // wrong kernel. Verification can still match via /usr/lib/modules/<uname>/pkgbase.
    if state.expected_kernel_uname.is_none() {
        ewarn!(
            "Could not determine installed version of {package_base}; boot verification will \
             use the running kernel's pkgbase file"
        );
    }
}

/// Uname localversion suffix for a kernel pkgbase (e.g. `linux-cachyos-bore-lto` → `cachyos-bore-lto`).
fn infer_suffix(base: &str) -> &str {
    base.strip_prefix("linux-").unwrap_or(base)
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
    if matches!(state.current_stage, PgoStageId::Done | PgoStageId::Aborted) {
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
    // Hold both stage kernels of the pipeline: the plain stage-1 kernel and the -lto
    // stage-2/3 kernel (whichever the package name denotes, derive its counterpart).
    let mut bases = vec![state.package.clone()];
    match state.package.strip_suffix("-lto") {
        Some(plain) => bases.push(plain.to_string()),
        None => bases.push(format!("{}-lto", state.package)),
    }
    if let Some(base) = state.expected_package_base.as_deref() {
        bases.push(base.to_string());
    }
    let mut names = Vec::new();
    for base in &bases {
        for suffix in ["", "-dbg", "-headers"] {
            names.push(format!("{base}{suffix}"));
        }
    }
    names.sort();
    names.dedup();
    names
}

fn select_pgo_boot_kernel(
    pgo: &PgoConfig,
    state: &PgoState,
    auto: bool,
) -> Option<crate::boot_entry::NextBoot> {
    if !pgo.select_boot_kernel {
        return None;
    }
    let pkgbase = state
        .expected_package_base
        .as_deref()
        .unwrap_or(&state.package);
    match crate::boot_entry::set_next_boot_kernel(pkgbase) {
        Ok(next) => {
            let id = match &next {
                crate::boot_entry::NextBoot::Bli { id }
                | crate::boot_entry::NextBoot::Grub { id } => id.as_str(),
            };
            blog!("Next boot (oneshot) set to {id} for {pkgbase}");
            Some(next)
        }
        Err(e) if auto => {
            die!(
                "Could not select bootloader entry for '{pkgbase}': {e}. \
                 Refusing to reboot into the wrong kernel. {}",
                bootloader_hint(state)
            );
        }
        Err(e) => {
            ewarn!(
                "Could not select bootloader entry for '{pkgbase}': {e}. {}",
                bootloader_hint(state)
            );
            None
        }
    }
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
        let expected_base = state
            .expected_package_base
            .as_deref()
            .unwrap_or("(unknown)");
        let expected_uname = state
            .expected_kernel_uname
            .as_deref()
            .unwrap_or("(unknown)");
        die!(
            "Boot verification failed: running '{running}', expected the {expected_base} kernel \
             (uname matching '{expected_uname}'). Select the correct bootloader entry and re-run \
             --pgo-resume"
        );
    }
}

fn boot_matches_expected(state: &PgoState) -> bool {
    let running = running_kernel_uname();
    if running.is_empty() {
        return false;
    }
    let pkgbase = fs::read_to_string(format!("/usr/lib/modules/{running}/pkgbase")).ok();
    boot_matches(state, &running, pkgbase.as_deref())
}

/// True when the running kernel matches the pipeline's expected stage kernel.
///
/// `running_pkgbase` is the content of `/usr/lib/modules/<uname -r>/pkgbase` (installed by Arch
/// kernel packages) when available. It is the authoritative signal: version strings alone cannot
/// distinguish the stage-1 kernel from the stage-2 `-lto` kernel of the same version, so relying
/// on them could let stage 3 profile the wrong kernel.
fn boot_matches(state: &PgoState, running: &str, running_pkgbase: Option<&str>) -> bool {
    if let (Some(expected), Some(actual)) =
        (state.expected_package_base.as_deref(), running_pkgbase)
    {
        return actual.trim() == expected;
    }
    // Fallback for kernels without a pkgbase file or old states: version-prefix match.
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
        || ((pgo.perf_data_on_ram || pgo.propeller_profiles_on_ram)
            && ramdisk_targets.build_workdir);

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PerfKernelIdentity {
    uname: String,
    #[serde(default)]
    pkgbase: Option<String>,
}

fn perf_data_usable(path: &Path) -> Option<u64> {
    let len = fs::metadata(path).ok()?.len();
    if len >= MIN_USABLE_PERF_BYTES {
        Some(len)
    } else {
        None
    }
}

fn perf_identity_sidecar_path(perf_data: &Path) -> PathBuf {
    let mut name = perf_data.as_os_str().to_os_string();
    name.push(".kernel.json");
    PathBuf::from(name)
}

fn write_perf_kernel_identity(perf_data: &Path, id: &PerfKernelIdentity) -> Result<(), String> {
    let path = perf_identity_sidecar_path(perf_data);
    let text = serde_json::to_string_pretty(id)
        .map_err(|e| format!("serialize perf kernel identity: {e}"))?;
    fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))
}

fn read_perf_kernel_identity(perf_data: &Path) -> Option<PerfKernelIdentity> {
    let text = fs::read_to_string(perf_identity_sidecar_path(perf_data)).ok()?;
    serde_json::from_str(&text).ok()
}

fn perf_identity_matches(stored: &PerfKernelIdentity, running: &PerfKernelIdentity) -> bool {
    if stored.uname != running.uname {
        return false;
    }
    match (&stored.pkgbase, &running.pkgbase) {
        (Some(stored_base), Some(running_base)) => stored_base.trim() == running_base.trim(),
        _ => false,
    }
}

fn running_kernel_identity() -> PerfKernelIdentity {
    let uname = running_kernel_uname();
    let pkgbase = fs::read_to_string(format!("/usr/lib/modules/{uname}/pkgbase"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    PerfKernelIdentity { uname, pkgbase }
}

/// Ramdisk captures vanish on reboot. Reuse a sidecar-less file there so a failed
/// `chown` after `perf record` does not throw away a multi-GB profile in the same boot.
fn allow_anonymous_perf_reuse(path: &Path) -> bool {
    crate::utils::path_has_prefix(Path::new("/run"), path)
}

fn usable_matching_perf(path: &Path, running: &PerfKernelIdentity) -> Option<u64> {
    let n = perf_data_usable(path)?;
    match read_perf_kernel_identity(path) {
        Some(stored) if perf_identity_matches(&stored, running) => Some(n),
        Some(stored) => {
            ewarn!(
                "Ignoring existing perf data {} (collected on {} / {}); running kernel is {} / {}",
                path.display(),
                stored.uname,
                stored.pkgbase.as_deref().unwrap_or("unknown"),
                running.uname,
                running.pkgbase.as_deref().unwrap_or("unknown"),
            );
            None
        }
        None if allow_anonymous_perf_reuse(path) => Some(n),
        None => {
            ewarn!(
                "Ignoring existing perf data {} (no kernel identity sidecar); will collect a fresh profile",
                path.display()
            );
            None
        }
    }
}

/// Prefer scratch, then the repo copy left after a previous capture (convert can
/// fail after collection; resume must not throw away a gigabyte of LBR data).
/// Reuse only when a sidecar records the same running kernel (`uname` + `pkgbase`).
fn existing_perf_data(
    scratch_file: &Path,
    repo: &Path,
    running: &PerfKernelIdentity,
) -> Option<(PathBuf, u64)> {
    if let Some(n) = usable_matching_perf(scratch_file, running) {
        return Some((scratch_file.to_path_buf(), n));
    }
    let name = scratch_file.file_name()?;
    let repo_file = repo.join(name);
    if repo_file == *scratch_file {
        return None;
    }
    usable_matching_perf(&repo_file, running).map(|n| (repo_file, n))
}

fn compare_stage_for_perf_data(perf_data: &Path) -> Option<CompareStage> {
    match perf_data.file_name().and_then(|n| n.to_str()) {
        Some("kernel.data") => Some(CompareStage::Debug),
        Some("propeller.data") => Some(CompareStage::Autofdo),
        _ => None,
    }
}

fn collect_or_reuse_perf_data(
    pgo: &PgoConfig,
    package: &str,
    repo: &Path,
    scratch: &Path,
    perf_data: &Path,
    events: &EventLog,
    persist_to_repo: bool,
) -> Result<PathBuf, String> {
    let combine =
        compare_stage_for_perf_data(perf_data).and_then(|stage| profile_compare_stage(pgo, stage));
    let running = running_kernel_identity();
    let build_user = pgo_build_user(pgo);
    if let Some((path, bytes)) = existing_perf_data(perf_data, repo, &running) {
        blog!(
            "Reusing existing perf data {} ({bytes} bytes) — skipping collection",
            path.display()
        );
        write_perf_kernel_identity(&path, &running)?;
        chown_perf_to_build_user(repo, &path, &build_user)?;
        if let Some(stage) = combine {
            publish_compare_log(pgo, package, stage, events, None, false);
        }
        return Ok(path);
    }
    run_profile_collection(pgo, package, repo, scratch, perf_data, events, combine)?;
    write_perf_kernel_identity(perf_data, &running)?;
    if persist_to_repo {
        sync_perf_data_to_repo(perf_data, repo)?;
    } else {
        blog!(
            "Leaving {} on ramdisk scratch (not copying to the package repo)",
            perf_data.display()
        );
    }
    Ok(perf_data.to_path_buf())
}

fn pgo_build_user(pgo: &PgoConfig) -> String {
    pgo.build_user
        .clone()
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "nobody".to_string())
}

fn chown_perf_to_build_user(repo: &Path, perf_data: &Path, build_user: &str) -> Result<(), String> {
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
    .map_err(|e| e.to_string())
}

fn finish_profile_collection(
    pgo: &PgoConfig,
    repo: &Path,
    perf_data: &Path,
    build_user: &str,
) -> Result<(), String> {
    chown_perf_to_build_user(repo, perf_data, build_user)?;
    // Brief grace period so perf finishes flushing buffers to perf.data before the
    // profiling sysctls (kptr_restrict / perf_event_paranoid) are restored.
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

    let perf_data =
        collect_or_reuse_perf_data(pgo, package, &repo, &scratch, &perf_data, events, true)
            .unwrap_or_else(|e| die!("Stage 2 profile failed: {e}"));

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
            sh_single_quote(&pgo.afdo_tool),
            sh_single_quote(&vmlinux.to_string_lossy()),
            sh_single_quote(&perf_data.to_string_lossy()),
            sh_single_quote(&profile_out.to_string_lossy()),
        )
    };
    blog!("Converting AutoFDO profile using {}…", vmlinux.display());
    run_logged_shell(&repo, &convert_cmd, events).unwrap_or_else(|e| die!("{e}"));

    let profile_bytes = validate_afdo_profile(&profile_out).unwrap_or_else(|e| die!("{e}"));
    blog!(
        "AutoFDO profile OK ({} bytes) — review before continuing to the AutoFDO build stage",
        profile_bytes
    );

    archive_profile(pgo, &profile_out, &pgo.afdo_profile_name).unwrap_or_else(|e| die!("{e}"));
    copy_to_repo(&profile_out, &repo_profile).unwrap_or_else(|e| die!("{e}"));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PropellerToolKind {
    CreateLlvmProf,
    GeneratePropellerProfiles,
}

fn propeller_tool_is_auto(tool: &str) -> bool {
    let t = tool.trim();
    t.is_empty() || t.eq_ignore_ascii_case("auto")
}

fn propeller_tool_kind(tool: &str) -> PropellerToolKind {
    let name = Path::new(tool)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(tool);
    if name == PROPELLER_TOOL_GENERATE || name.contains("generate_propeller") {
        PropellerToolKind::GeneratePropellerProfiles
    } else {
        PropellerToolKind::CreateLlvmProf
    }
}

fn propeller_tool_exists(tool: &str) -> bool {
    let path = Path::new(tool);
    if path.is_absolute() || tool.contains('/') {
        path.is_file()
    } else {
        which(tool).is_some()
    }
}

fn cached_propeller_root() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("abs")
        .join("llvm-propeller")
}

fn find_generate_propeller_profiles_in(root: &Path) -> Option<String> {
    let p = root.join("bin").join(PROPELLER_TOOL_GENERATE);
    p.is_file().then(|| p.to_string_lossy().into_owned())
}

fn find_generate_propeller_profiles() -> Option<String> {
    if let Some(p) = which(PROPELLER_TOOL_GENERATE) {
        return Some(p.to_string_lossy().into_owned());
    }
    find_generate_propeller_profiles_in(&cached_propeller_root())
}

fn bootstrap_tool_present(name: &str) -> bool {
    which(name).is_some()
}

fn can_bootstrap_generate_propeller_profiles() -> bool {
    ["git", "cmake", "ninja", "clang", "clang++"]
        .into_iter()
        .all(bootstrap_tool_present)
        && (bootstrap_tool_present("llvm-config") || bootstrap_tool_present("llvm-config-22"))
}

fn bb_addr_map_needs_llvm_propeller(version: Option<u8>) -> bool {
    version.is_some_and(|v| v >= 5)
}

fn bb_addr_map_version(path: &Path) -> Option<u8> {
    let mut f = File::open(path).ok()?;
    let mut ehdr = [0u8; 64];
    f.read_exact(&mut ehdr).ok()?;
    if ehdr.get(0..4) != Some(b"\x7fELF") || ehdr[4] != 2 || ehdr[5] != 1 {
        return None;
    }
    let e_shoff = u64::from_le_bytes(ehdr[40..48].try_into().ok()?);
    let e_shentsize = u16::from_le_bytes(ehdr[58..60].try_into().ok()?);
    let mut e_shnum = u16::from_le_bytes(ehdr[60..62].try_into().ok()?);
    let mut e_shstrndx = u16::from_le_bytes(ehdr[62..64].try_into().ok()?);
    if e_shentsize != 64 {
        return None;
    }
    // ELF extended numbering: e_shnum/e_shstrndx of 0/SHN_XINDEX store the
    // real counts in section 0.
    if e_shnum == 0 || e_shstrndx == 0xffff {
        let mut sh0 = [0u8; 64];
        f.seek(SeekFrom::Start(e_shoff)).ok()?;
        f.read_exact(&mut sh0).ok()?;
        if e_shnum == 0 {
            e_shnum = u64::from_le_bytes(sh0[32..40].try_into().ok()?)
                .try_into()
                .ok()?;
        }
        if e_shstrndx == 0xffff {
            e_shstrndx = u32::from_le_bytes(sh0[40..44].try_into().ok()?)
                .try_into()
                .ok()?;
        }
    }
    if e_shnum == 0 {
        return None;
    }
    let mut shdrs = vec![0u8; e_shnum as usize * 64];
    f.seek(SeekFrom::Start(e_shoff)).ok()?;
    f.read_exact(&mut shdrs).ok()?;

    let str_off = 64 * e_shstrndx as usize;
    let strtab = if str_off + 64 <= shdrs.len() {
        let off = u64::from_le_bytes(shdrs[str_off + 24..str_off + 32].try_into().ok()?);
        let size = u64::from_le_bytes(shdrs[str_off + 32..str_off + 40].try_into().ok()?);
        let size = usize::try_from(size).ok()?.min(1 << 20);
        let mut buf = vec![0u8; size];
        if f.seek(SeekFrom::Start(off)).is_ok() && f.read_exact(&mut buf).is_ok() {
            buf
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    for i in 0..e_shnum as usize {
        let rec = &shdrs[i * 64..(i + 1) * 64];
        let sh_name = u32::from_le_bytes(rec[0..4].try_into().ok()?);
        let sh_type = u32::from_le_bytes(rec[4..8].try_into().ok()?);
        let sh_offset = u64::from_le_bytes(rec[24..32].try_into().ok()?);
        let sh_size = u64::from_le_bytes(rec[32..40].try_into().ok()?);
        let name = {
            let start = sh_name as usize;
            if start < strtab.len() {
                let end = strtab[start..]
                    .iter()
                    .position(|&b| b == 0)
                    .map(|p| start + p)
                    .unwrap_or(strtab.len());
                String::from_utf8_lossy(&strtab[start..end]).into_owned()
            } else {
                String::new()
            }
        };
        let by_type = sh_type == SHT_LLVM_BB_ADDR_MAP || sh_type == SHT_LLVM_BB_ADDR_MAP_V0;
        let by_name = name.contains("bb_addr_map");
        if (by_type || by_name) && sh_size > 0 {
            f.seek(SeekFrom::Start(sh_offset)).ok()?;
            let mut b = [0u8; 1];
            f.read_exact(&mut b).ok()?;
            return Some(b[0]);
        }
    }
    None
}

fn resolve_propeller_tool(configured: &str) -> Result<String, String> {
    if propeller_tool_is_auto(configured) {
        if let Some(p) = find_generate_propeller_profiles() {
            return Ok(p);
        }
        if propeller_tool_exists(PROPELLER_TOOL_CREATE_LLVM_PROF) {
            return Ok(PROPELLER_TOOL_CREATE_LLVM_PROF.to_string());
        }
        return Err(format!(
            "PGO requires a Propeller converter (tried {PROPELLER_TOOL_GENERATE}, \
             {PROPELLER_TOOL_CREATE_LLVM_PROF}). LLVM 22+ kernels need {PROPELLER_TOOL_GENERATE}; \
             ABS can build it from https://github.com/google/llvm-propeller if git, cmake, ninja, \
             clang, and llvm are installed. {PROPELLER_TOOL_CREATE_LLVM_PROF} 0.30.x cannot read \
             SHT_LLVM_BB_ADDR_MAP version 5."
        ));
    }
    let configured = configured.trim();
    if propeller_tool_exists(configured) {
        return Ok(configured.to_string());
    }
    if let Some(p) = find_generate_propeller_profiles()
        && (configured == PROPELLER_TOOL_GENERATE || configured.contains("generate_propeller"))
    {
        return Ok(p);
    }
    Err(format!(
        "PGO requires '{configured}' in PATH (propeller_tool)"
    ))
}

fn materialize_propeller_build_script() -> Result<PathBuf, String> {
    let path = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("abs")
        .join("build-generate-propeller-profiles.sh");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o755)
            .open(&path)
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        file.write_all(PROPELLER_BUILD_SCRIPT.as_bytes())
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod {}: {e}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        fs::write(&path, PROPELLER_BUILD_SCRIPT.as_bytes())
            .map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(path)
}

fn bootstrap_generate_propeller_profiles(repo: &Path, events: &EventLog) -> Result<String, String> {
    if !can_bootstrap_generate_propeller_profiles() {
        return Err(format!(
            "Need {PROPELLER_TOOL_GENERATE} for this vmlinux (SHT_LLVM_BB_ADDR_MAP v5 / LLVM 22+). \
             Install build deps and retry: sudo pacman -S --needed cmake ninja clang llvm git libelf. \
             {PROPELLER_TOOL_CREATE_LLVM_PROF} 0.30.x cannot convert LLVM 22 kernels."
        ));
    }
    let root = cached_propeller_root();
    fs::create_dir_all(root.join("bin")).map_err(|e| format!("create {}: {e}", root.display()))?;
    let script = materialize_propeller_build_script()?;
    blog!(
        "Building {PROPELLER_TOOL_GENERATE} from llvm-propeller against system LLVM \
         (needed for SHT_LLVM_BB_ADDR_MAP v5)…"
    );
    let cmd = format!(
        "{} {}",
        sh_single_quote(&script.to_string_lossy()),
        sh_single_quote(&root.to_string_lossy())
    );
    run_logged_shell(repo, &cmd, events).map_err(|e| {
        format!(
            "Failed to build {PROPELLER_TOOL_GENERATE} from llvm-propeller: {e}. \
             Install: sudo pacman -S --needed cmake ninja clang llvm git libelf"
        )
    })?;
    find_generate_propeller_profiles_in(&root).ok_or_else(|| {
        format!(
            "llvm-propeller build finished but {} is missing",
            root.join("bin").join(PROPELLER_TOOL_GENERATE).display()
        )
    })
}

fn ensure_propeller_tool(
    configured: &str,
    vmlinux: &Path,
    repo: &Path,
    events: &EventLog,
) -> Result<String, String> {
    let version = bb_addr_map_version(vmlinux);
    if let Some(v) = version {
        blog!("vmlinux SHT_LLVM_BB_ADDR_MAP version {v}");
    }
    let need_generate = bb_addr_map_needs_llvm_propeller(version);
    if !propeller_tool_is_auto(configured) {
        let configured = configured.trim();
        if propeller_tool_exists(configured) {
            if propeller_tool_kind(configured) == PropellerToolKind::GeneratePropellerProfiles
                || !need_generate
            {
                return Ok(configured.to_string());
            }
            ewarn!(
                "{configured} cannot read SHT_LLVM_BB_ADDR_MAP v{} on this vmlinux; \
                 switching to {PROPELLER_TOOL_GENERATE}",
                version.unwrap_or(5)
            );
        } else if propeller_tool_kind(configured) != PropellerToolKind::GeneratePropellerProfiles
            && !need_generate
        {
            return Err(format!(
                "PGO requires '{configured}' in PATH (propeller_tool)"
            ));
        }
    }
    if let Some(p) = find_generate_propeller_profiles() {
        return Ok(p);
    }
    if need_generate || propeller_tool_is_auto(configured) {
        return bootstrap_generate_propeller_profiles(repo, events);
    }
    resolve_propeller_tool(configured)
}

fn propeller_convert_command(
    tool: &str,
    vmlinux: &Path,
    perf_data: &Path,
    cc_out: &Path,
    ld_out: &Path,
) -> String {
    let tool_q = sh_single_quote(tool);
    let binary = sh_single_quote(&vmlinux.to_string_lossy());
    let profile = sh_single_quote(&perf_data.to_string_lossy());
    let cc = sh_single_quote(&cc_out.to_string_lossy());
    let ld = sh_single_quote(&ld_out.to_string_lossy());
    match propeller_tool_kind(tool) {
        PropellerToolKind::GeneratePropellerProfiles => format!(
            "{tool_q} --binary={binary} --profile={profile} --cc_profile={cc} --ld_profile={ld} \
             --propeller_options={}",
            sh_single_quote("output_module_name: true"),
        ),
        PropellerToolKind::CreateLlvmProf => format!(
            "{tool_q} --binary={binary} --profile={profile} --format=propeller \
             --propeller_output_module_name --out={cc} --propeller_symorder={ld}",
        ),
    }
}

fn is_unsupported_bb_addr_map(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("bb_addr_map") || e.contains("bb-addr-map")
}

fn propeller_bb_addr_map_hint(tried: &str) -> String {
    format!(
        "{tried} cannot read this vmlinux (unsupported SHT_LLVM_BB_ADDR_MAP, typical of LLVM 22+). \
         ABS builds {PROPELLER_TOOL_GENERATE} from llvm-propeller when git, cmake, ninja, clang, \
         and llvm are installed. {PROPELLER_TOOL_CREATE_LLVM_PROF} 0.30.x cannot convert LLVM 22 kernels."
    )
}

fn convert_propeller_profiles(
    repo: &Path,
    tool: &str,
    vmlinux: &Path,
    perf_data: &Path,
    cc_out: &Path,
    ld_out: &Path,
    events: &EventLog,
) -> Result<(), String> {
    let cmd = propeller_convert_command(tool, vmlinux, perf_data, cc_out, ld_out);
    blog!(
        "Converting Propeller profile with {tool} using {}…",
        vmlinux.display()
    );
    match run_logged_shell(repo, &cmd, events) {
        Ok(()) => Ok(()),
        Err(err)
            if is_unsupported_bb_addr_map(&err)
                && propeller_tool_kind(tool) != PropellerToolKind::GeneratePropellerProfiles =>
        {
            let retry_tool = match find_generate_propeller_profiles() {
                Some(p) => p,
                None => bootstrap_generate_propeller_profiles(repo, events)?,
            };
            ewarn!(
                "{tool} cannot read SHT_LLVM_BB_ADDR_MAP on this vmlinux; retrying with {retry_tool}"
            );
            let retry = propeller_convert_command(&retry_tool, vmlinux, perf_data, cc_out, ld_out);
            run_logged_shell(repo, &retry, events).map_err(|err2| {
                if is_unsupported_bb_addr_map(&err2) {
                    propeller_bb_addr_map_hint(&retry_tool)
                } else {
                    err2
                }
            })
        }
        Err(err) if is_unsupported_bb_addr_map(&err) => Err(propeller_bb_addr_map_hint(tool)),
        Err(err) => Err(err),
    }
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
    let persist_disk = persist_propeller_to_disk(pgo);
    let perf_data = scratch.join("propeller.data");
    let perf_data = collect_or_reuse_perf_data(
        pgo,
        package,
        &repo,
        &scratch,
        &perf_data,
        events,
        persist_disk,
    )
    .unwrap_or_else(|e| die!("Stage 3 profile failed: {e}"));

    let vmlinux = resolve_vmlinux(
        pgo,
        state.expected_kernel_uname.as_deref(),
        state.expected_package_base.as_deref(),
    )
    .unwrap_or_else(|e| die!("{e}"));
    // Convert on scratch when keeping profiles on ramdisk; copy the small cc/ld
    // texts into the package dir for makepkg. Skip the HDD archive — stage 3
    // build runs in the same boot.
    let profile_dir = if persist_disk { &repo } else { &scratch };
    let cc_out = profile_dir.join("propeller_cc_profile.txt");
    let ld_out = profile_dir.join("propeller_ld_profile.txt");
    let tool = ensure_propeller_tool(&pgo.propeller_tool, &vmlinux, &repo, events)
        .unwrap_or_else(|e| die!("{e}"));
    convert_propeller_profiles(&repo, &tool, &vmlinux, &perf_data, &cc_out, &ld_out, events)
        .unwrap_or_else(|e| die!("{e}"));

    for name in ["propeller_cc_profile.txt", "propeller_ld_profile.txt"] {
        let path = profile_dir.join(name);
        let bytes = validate_propeller_profile(&path).unwrap_or_else(|e| die!("{e}"));
        blog!("Propeller profile {name} OK ({bytes} bytes)");
        if persist_disk {
            archive_profile(pgo, &path, name).unwrap_or_else(|e| die!("{e}"));
        } else {
            blog!("Keeping Propeller profile {name} on ramdisk scratch (skipping HDD archive)");
        }
        let repo_path = repo.join(name);
        if path != repo_path {
            copy_to_repo(&path, &repo_path).unwrap_or_else(|e| die!("{e}"));
        }
    }
}

fn run_profile_collection(
    pgo: &PgoConfig,
    package: &str,
    repo: &Path,
    scratch: &Path,
    perf_data: &Path,
    events: &EventLog,
    combine: Option<CompareStage>,
) -> Result<(), String> {
    sysctl_toggle(pgo, true)?;
    let perf_events = detect_perf_event_args(pgo)?;
    let perf_extra = resolved_perf_extra_args(pgo);
    let (preset, compare_label) = profiling_workload(pgo, combine);
    let benchmark = crate::pgo_benchmark::resolve_benchmark_command(&pgo.benchmark_command)?;
    let bench_cache = pgo.resolved_benchmark_workdir(package);
    fs::create_dir_all(&bench_cache)
        .map_err(|e| format!("create benchmark cache {}: {e}", bench_cache.display()))?;
    blog!("PGO benchmark script: {}", benchmark.display());
    blog!(
        "PGO benchmark asset cache (persistent): {}",
        bench_cache.display()
    );
    blog!("PGO perf scratch (may be tmpfs): {}", scratch.display());
    let build_user = pgo_build_user(pgo);

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

    probe_branch_stack_sampling(repo, scratch, &perf_events, &perf_extra, events)?;

    if combine.is_some() {
        drop_page_cache();
    }
    let before = if combine.is_some() {
        benchie_logs(&bench_cache)
    } else {
        Vec::new()
    };

    let mut bench_cmd = format!(
        "env ABS_PGO_PROFILE_DIR={} ABS_PGO_BENCHMARK_DIR={} ABS_PGO_BENCHMARK={}",
        sh_single_quote(&scratch.to_string_lossy()),
        sh_single_quote(&bench_cache.to_string_lossy()),
        sh_single_quote(preset),
    );
    if let Some(label) = &compare_label {
        bench_cmd.push_str(" ABS_PGO_COMPARE_LABEL=");
        bench_cmd.push_str(&sh_single_quote(label));
    }
    bench_cmd.push(' ');
    bench_cmd.push_str(&crate::pgo_benchmark::shell_benchmark_runner(&benchmark));

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
    if let Some(stage) = combine {
        blog!(
            "Running {} comparison + profile collection under perf record (one pass)…",
            stage.title()
        );
    } else {
        blog!("Running benchmark with perf record...");
    }
    blog!("PGO benchmark command: {bench_cmd}");
    events.log_line(
        "stdout",
        if let Some(stage) = combine {
            format!(
                "Profiling + comparison ({preset}, label={}): perf record runs until \
                 cachyos-benchmarker exits. This is the {} chart series, not a second run.",
                compare_label.as_deref().unwrap_or(""),
                stage.title()
            )
        } else {
            format!(
                "Profiling workload ({preset}): perf record runs until the benchmark script exits. \
                 profiling_quality=maximum forces cachyos-benchmarker and denser sampling."
            )
        },
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

    if let Some(stage) = combine {
        publish_compare_log(pgo, package, stage, events, Some(&before), true);
    }

    // Stamp identity before chown so a helper rejection cannot strand a multi-GB
    // capture without a sidecar (resume would otherwise collect again).
    write_perf_kernel_identity(perf_data, &running_kernel_identity())?;
    finish_profile_collection(pgo, repo, perf_data, &build_user)
}

fn branch_stack_sampling_unavailable(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("doesn't support branch stack sampling")
        || s.contains("does not support branch stack sampling")
}

fn branch_stack_unavailable_message(detail: &str) -> String {
    format!(
        "perf cannot record branch stacks on this machine. \
         AutoFDO/Propeller kernel PGO requires Last Branch Records (`perf record -b`). \
         This is a CPU/hypervisor limit, not an ABS package-selection bug. \
         On bare metal, enable the PMU and disable the NMI watchdog. \
         In a VM, the hypervisor must expose the host CPU's branch-sampling facility; \
         if it does not, run PGO on the host instead.\n\
         {detail}"
    )
}

/// Fail fast before cachyos-benchmarker if this CPU/VM cannot open LBR sampling.
fn probe_branch_stack_sampling(
    repo: &Path,
    scratch: &Path,
    perf_events: &str,
    perf_extra: &str,
    events: &EventLog,
) -> Result<(), String> {
    let sudo = crate::utils::shell_sudo();
    let probe = scratch.join("abs-pgo-branch-stack-probe.data");
    let _ = fs::remove_file(&probe);
    let cmd = format!(
        "{sudo} perf record {perf_events} {extra} -o {out} -- true",
        extra = perf_extra,
        out = sh_single_quote(&probe.to_string_lossy()),
    );
    events.log_line(
        "stdout",
        "Probing perf branch-stack sampling before the profiling workload...".to_string(),
    );
    let result = run_logged_shell(repo, &cmd, events);
    let _ = fs::remove_file(&probe);
    match result {
        Ok(()) => Ok(()),
        Err(e) if branch_stack_sampling_unavailable(&e) => {
            Err(branch_stack_unavailable_message(&e))
        }
        Err(e) => Err(e),
    }
}

fn sysctl_toggle(pgo: &PgoConfig, enable: bool) -> Result<(), String> {
    if let Some(cmd) = &pgo.sysctl_command {
        let path = Path::new(cmd);
        let allowed = path.is_absolute()
            && (dirs::home_dir().is_some_and(|h| crate::utils::path_has_prefix(&h, path))
                || crate::utils::path_has_prefix(Path::new("/usr/share/abs"), path));
        if !allowed {
            return Err(format!(
                "pgo.sysctl_command must be an absolute path under $HOME or /usr/share/abs (got {cmd:?})"
            ));
        }
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
            &[
                "sysctl",
                "-w",
                &format!("kernel.perf_event_paranoid={paranoid}"),
            ],
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

fn resolved_benchmark_preset(pgo: &PgoConfig, combine_compare: bool) -> &str {
    if combine_compare || profiling_quality_is_maximum(pgo) {
        return "cachyos";
    }
    let preset = pgo.benchmark_preset.trim();
    if preset.is_empty() { "fast" } else { preset }
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
        let Ok(output) = Command::new(tool).arg("-S").arg(path).output() else {
            continue;
        };
        if output.status.success() && String::from_utf8_lossy(&output.stdout).contains(section) {
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

/// Resolve the unstripped kernel image used by llvm-profgen / Propeller converters.
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
    let src_id = perf_identity_sidecar_path(perf_data);
    if src_id.is_file() {
        let dest_id = perf_identity_sidecar_path(&dest);
        fs::copy(&src_id, &dest_id)
            .map_err(|e| format!("copy perf kernel identity to repo failed: {e}"))?;
        let _ = crate::utils::ensure_build_user_can_read(&dest_id);
    }
    Ok(())
}

fn persist_propeller_to_disk(pgo: &PgoConfig) -> bool {
    !pgo.propeller_profiles_on_ram
}

fn is_propeller_profile_name(name: &str) -> bool {
    name == "propeller_cc_profile.txt" || name == "propeller_ld_profile.txt"
}

#[derive(Debug, PartialEq, Eq)]
enum ProfileRestore {
    AlreadyPresent,
    From(PathBuf),
    Missing,
}

fn resolve_profile_restore(
    repo: &Path,
    archive: &Path,
    scratch: Option<&Path>,
    name: &str,
    skip_archive: bool,
) -> ProfileRestore {
    if repo.join(name).is_file() {
        return ProfileRestore::AlreadyPresent;
    }
    if let Some(scratch) = scratch {
        let src = scratch.join(name);
        if src.is_file() {
            return ProfileRestore::From(src);
        }
    }
    if !skip_archive {
        let src = archive.join(name);
        if src.is_file() {
            return ProfileRestore::From(src);
        }
    }
    ProfileRestore::Missing
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

fn restore_profiles_to_repo(
    state: &PgoState,
    pgo: &PgoConfig,
    names: &[&str],
    scratch: Option<&Path>,
) {
    let repo = PathBuf::from(&state.repo_dir);
    let archive = pgo
        .resolved_archive_dir()
        .unwrap_or_else(|| die!("profiles_archive_dir not set"));
    for name in names {
        let dest = repo.join(name);
        let skip_archive = pgo.propeller_profiles_on_ram && is_propeller_profile_name(name);
        match resolve_profile_restore(&repo, &archive, scratch, name, skip_archive) {
            ProfileRestore::AlreadyPresent => continue,
            ProfileRestore::From(src) => {
                if *name == "kernel-compilation.afdo"
                    && let Ok(meta) = fs::metadata(&src)
                    && meta.len() < MIN_AFDO_PROFILE_BYTES
                {
                    die!(
                        "Archived AutoFDO profile '{}' is only {} bytes — re-run stage 2 profiling \
                         after installing linux-cachyos-dbg",
                        name,
                        meta.len()
                    );
                }
                let origin = if scratch.is_some_and(|s| src.starts_with(s)) {
                    "ramdisk scratch"
                } else {
                    "archive"
                };
                blog!("Restoring profile {name} from {origin}...");
                if let Err(e) = fs::copy(&src, &dest) {
                    die!("Failed to restore profile {name}: {e}");
                }
            }
            ProfileRestore::Missing if *name == "kernel-compilation.afdo" => {
                die!("Required profile '{name}' missing in repo and archive");
            }
            ProfileRestore::Missing if is_propeller_profile_name(name) => {
                die!(
                    "Required Propeller profile '{name}' missing in the package dir{}. \
                     Re-run stage 3 profiling.",
                    if skip_archive {
                        " and ramdisk scratch"
                    } else {
                        " and archive"
                    }
                );
            }
            ProfileRestore::Missing => {}
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
    let captured_stderr = Arc::new(Mutex::new(String::new()));

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
            let captured = captured_stderr.clone();
            s.spawn(move || {
                let mut buf = String::new();
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    buf.push_str(&line);
                    buf.push('\n');
                    events.log_line("stderr", line);
                }
                *captured.lock().unwrap() = buf;
            });
        }
    });

    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        let detail = captured_stderr.lock().unwrap().clone();
        let detail = detail.trim();
        if detail.is_empty() {
            return Err(format!("command failed: {cmd}"));
        }
        return Err(format!("command failed: {cmd}\n{detail}"));
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
            propeller_profiles_on_ram: true,
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
            select_boot_kernel: true,
            auto_restart: false,
            reboot_before_start: false,
            reuse_afdo_profile: false,
            reuse_propeller_profile: false,
            compare_current: false,
            compare_debug: false,
            compare_debug_clean: false,
            compare_autofdo: false,
            compare_autofdo_clean: false,
            compare_final: false,
            state_file: None,
        }
    }

    #[test]
    fn propeller_convert_command_create_llvm_prof_cli() {
        let cmd = propeller_convert_command(
            "create_llvm_prof",
            Path::new("/usr/src/debug/vmlinux"),
            Path::new("/tmp/propeller.data"),
            Path::new("/tmp/propeller_cc_profile.txt"),
            Path::new("/tmp/propeller_ld_profile.txt"),
        );
        assert!(cmd.contains("--format=propeller"), "{cmd}");
        assert!(cmd.contains("--propeller_output_module_name"), "{cmd}");
        assert!(cmd.contains("--propeller_symorder="), "{cmd}");
        assert!(!cmd.contains("--cc_profile="), "{cmd}");
    }

    #[test]
    fn propeller_convert_command_generate_propeller_profiles_cli() {
        let cmd = propeller_convert_command(
            "/opt/llvm-propeller/generate_propeller_profiles",
            Path::new("/usr/src/debug/vmlinux"),
            Path::new("/tmp/propeller.data"),
            Path::new("/tmp/propeller_cc_profile.txt"),
            Path::new("/tmp/propeller_ld_profile.txt"),
        );
        assert!(cmd.contains("--cc_profile="), "{cmd}");
        assert!(cmd.contains("--ld_profile="), "{cmd}");
        assert!(cmd.contains("output_module_name: true"), "{cmd}");
        assert!(!cmd.contains("--format=propeller"), "{cmd}");
    }

    #[test]
    fn pgo_auto_unit_has_no_start_timeout_and_waits_for_session() {
        let unit = super::pgo_auto_unit_text("/usr/bin/abs");
        assert!(unit.contains("TimeoutStartSec=infinity"), "{unit}");
        assert!(unit.contains("After=graphical-session.target"), "{unit}");
        assert!(
            !unit.contains("PartOf=graphical-session.target"),
            "PartOf would stop TTY-only resume: {unit}"
        );
        assert!(unit.contains("--pgo-resume %i --pgo-auto"), "{unit}");
        assert!(unit.contains("WantedBy=default.target"), "{unit}");
        assert!(
            !unit.contains("WantedBy=graphical-session.target"),
            "graphical-session never starts on a console-only machine: {unit}"
        );
    }

    #[test]
    fn compare_benchmarks_dir_is_separate_from_profiling_assets() {
        let pgo = test_pgo();
        assert_eq!(
            pgo.resolved_compare_dir("linux-cachyos"),
            PathBuf::from("/tmp/abs-pgo-test/compare-benchmarks")
        );
        assert_eq!(
            pgo.resolved_benchmark_workdir("linux-cachyos"),
            PathBuf::from("/tmp/abs-pgo-test/benchmark-workdir")
        );
        assert!(!pgo.compare_any());
        let mut on = test_pgo();
        on.compare_current = true;
        on.compare_final = true;
        assert!(on.compare_any());
    }

    #[test]
    fn debug_and_autofdo_compare_share_the_profiling_run() {
        assert!(CompareStage::Debug.shares_profiling_run());
        assert!(CompareStage::Autofdo.shares_profiling_run());
        assert!(!CompareStage::Current.shares_profiling_run());
        assert!(!CompareStage::Final.shares_profiling_run());
        assert!(!CompareStage::DebugClean.shares_profiling_run());
        assert!(!CompareStage::AutofdoClean.shares_profiling_run());
    }

    #[test]
    fn standalone_compare_includes_optional_clean_debug_and_autofdo() {
        let mut pgo = test_pgo();
        pgo.compare_current = true;
        pgo.compare_debug = true;
        pgo.compare_debug_clean = true;
        pgo.compare_autofdo = true;
        pgo.compare_autofdo_clean = true;
        pgo.compare_final = true;
        assert!(should_run_standalone_compare(&pgo, CompareStage::Current));
        assert!(!should_run_standalone_compare(&pgo, CompareStage::Debug));
        assert!(should_run_standalone_compare(
            &pgo,
            CompareStage::DebugClean
        ));
        assert!(!should_run_standalone_compare(&pgo, CompareStage::Autofdo));
        assert!(should_run_standalone_compare(
            &pgo,
            CompareStage::AutofdoClean
        ));
        assert!(should_run_standalone_compare(&pgo, CompareStage::Final));
    }

    #[test]
    fn profile_compare_stage_follows_any_comparison() {
        let mut pgo = test_pgo();
        assert_eq!(profile_compare_stage(&pgo, CompareStage::Debug), None);
        pgo.compare_final = true;
        assert_eq!(
            profile_compare_stage(&pgo, CompareStage::Debug),
            Some(CompareStage::Debug)
        );
        assert_eq!(profile_compare_stage(&pgo, CompareStage::Current), None);
        assert_eq!(
            profile_compare_stage(&pgo, CompareStage::Autofdo),
            Some(CompareStage::Autofdo)
        );
    }

    #[test]
    fn work_plan_reuse_skips_upstream_unless_clean_bench_needs_the_kernel() {
        let mut pgo = test_pgo();
        pgo.reuse_afdo_profile = true;
        pgo.reuse_propeller_profile = true;
        let plan = pgo_work_plan(&pgo, true, true);
        assert!(!plan.run_debug_build);
        assert!(!plan.run_afdo_collect);
        assert!(!plan.run_autofdo_build);
        assert!(!plan.run_propeller_collect);
        assert_eq!(first_post_start_reboot_stage(plan), PgoStageId::Stage3Build);

        pgo.compare_debug_clean = true;
        let plan = pgo_work_plan(&pgo, true, true);
        assert!(plan.run_debug_build);
        assert!(!plan.run_afdo_collect);
        assert!(!plan.run_autofdo_build);

        pgo.compare_debug_clean = false;
        pgo.compare_autofdo_clean = true;
        let plan = pgo_work_plan(&pgo, true, true);
        assert!(!plan.run_debug_build);
        assert!(plan.run_autofdo_build);
        assert!(!plan.run_propeller_collect);
        assert_eq!(first_post_start_reboot_stage(plan), PgoStageId::Stage2Build);
    }

    #[test]
    fn work_plan_without_reuse_runs_the_full_chain() {
        let mut pgo = test_pgo();
        pgo.reboot_before_start = true;
        let plan = pgo_work_plan(&pgo, true, true);
        assert!(plan.reboot_before_start);
        assert!(plan.run_debug_build);
        assert!(plan.run_afdo_collect);
        assert!(plan.run_autofdo_build);
        assert!(plan.run_propeller_collect);
        assert_eq!(first_post_start_reboot_stage(plan), PgoStageId::Stage1Build);
    }

    #[test]
    fn new_pipeline_flags_default_off() {
        let pgo: PgoConfig = toml::from_str("").unwrap();
        assert!(!pgo.reboot_before_start);
        assert!(!pgo.reuse_afdo_profile);
        assert!(!pgo.reuse_propeller_profile);
        assert!(!pgo.compare_current);
        assert!(!pgo.compare_debug_clean);
        assert!(!pgo.compare_autofdo_clean);
        assert!(!pgo.compare_final);
    }

    #[test]
    fn combined_compare_forces_cachyos_preset() {
        let mut pgo = test_pgo();
        pgo.profiling_quality = "standard".into();
        pgo.benchmark_preset = "fast".into();
        assert_eq!(resolved_benchmark_preset(&pgo, false), "fast");
        assert_eq!(resolved_benchmark_preset(&pgo, true), "cachyos");
    }

    #[test]
    fn profiling_workload_sets_compare_label_when_combined() {
        let pgo = test_pgo();
        let (preset, label) = profiling_workload(&pgo, Some(CompareStage::Debug));
        assert_eq!(preset, "cachyos");
        assert_eq!(label.as_deref(), Some("abs-debug"));
        let (preset, label) = profiling_workload(&pgo, Some(CompareStage::Autofdo));
        assert_eq!(preset, "cachyos");
        assert_eq!(label.as_deref(), Some("abs-autofdo"));
        let mut pgo = test_pgo();
        pgo.profiling_quality = "standard".into();
        pgo.benchmark_preset = "fast".into();
        let (preset, label) = profiling_workload(&pgo, None);
        assert_eq!(preset, "fast");
        assert!(label.is_none());
    }

    #[test]
    fn compare_stage_follows_perf_data_filename() {
        assert_eq!(
            compare_stage_for_perf_data(Path::new("/scratch/kernel.data")),
            Some(CompareStage::Debug)
        );
        assert_eq!(
            compare_stage_for_perf_data(Path::new("/scratch/propeller.data")),
            Some(CompareStage::Autofdo)
        );
        assert_eq!(
            compare_stage_for_perf_data(Path::new("/scratch/other.data")),
            None
        );
    }

    #[test]
    fn propeller_tool_auto_and_bb_addr_map_detection() {
        assert!(propeller_tool_is_auto("auto"));
        assert!(propeller_tool_is_auto("AUTO"));
        assert!(propeller_tool_is_auto(""));
        assert!(!propeller_tool_is_auto("create_llvm_prof"));
        assert!(is_unsupported_bb_addr_map(
            "INTERNAL: unsupported SHT_LLVM_BB_ADDR_MAP version: 5"
        ));
        assert!(!is_unsupported_bb_addr_map("missing profile file"));
        let err = resolve_propeller_tool("definitely-not-a-propeller-tool-xyz").unwrap_err();
        assert!(err.contains("definitely-not-a-propeller-tool-xyz"), "{err}");
    }

    fn write_elf64_with_bb_addr_map(path: &Path, version: u8, section_type: u32) {
        fn put_u16(buf: &mut Vec<u8>, v: u16) {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        fn put_u32(buf: &mut Vec<u8>, v: u32) {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        fn put_u64(buf: &mut Vec<u8>, v: u64) {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        fn shdr(name: u32, typ: u32, offset: u64, size: u64) -> Vec<u8> {
            let mut s = Vec::new();
            put_u32(&mut s, name);
            put_u32(&mut s, typ);
            put_u64(&mut s, 0);
            put_u64(&mut s, 0);
            put_u64(&mut s, offset);
            put_u64(&mut s, size);
            put_u32(&mut s, 0);
            put_u32(&mut s, 0);
            put_u64(&mut s, 1);
            put_u64(&mut s, 0);
            s
        }

        let strtab = b"\0.llvm_bb_addr_map\0.shstrtab\0";
        let data_off = 64u64;
        let str_off = data_off + 1;
        let sh_off = 128u64;

        let mut ehdr = vec![0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        put_u16(&mut ehdr, 2); // ET_EXEC
        put_u16(&mut ehdr, 62); // EM_X86_64
        put_u32(&mut ehdr, 1);
        put_u64(&mut ehdr, 0); // e_entry
        put_u64(&mut ehdr, 0); // e_phoff
        put_u64(&mut ehdr, sh_off);
        put_u32(&mut ehdr, 0);
        put_u16(&mut ehdr, 64);
        put_u16(&mut ehdr, 0);
        put_u16(&mut ehdr, 0);
        put_u16(&mut ehdr, 64);
        put_u16(&mut ehdr, 3);
        put_u16(&mut ehdr, 2);
        assert_eq!(ehdr.len(), 64);

        let mut buf = ehdr;
        buf.push(version);
        buf.extend_from_slice(strtab);
        buf.resize(sh_off as usize, 0);
        buf.extend(shdr(0, 0, 0, 0));
        buf.extend(shdr(1, section_type, data_off, 1));
        buf.extend(shdr(19, 3, str_off, strtab.len() as u64));
        fs::write(path, buf).unwrap();
    }

    #[test]
    fn bb_addr_map_version_reads_v5_from_elf() {
        let dir = std::env::temp_dir().join(format!("abs-bbmap-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("vmlinux");
        write_elf64_with_bb_addr_map(&path, 5, SHT_LLVM_BB_ADDR_MAP);
        assert_eq!(bb_addr_map_version(&path), Some(5));
        assert!(bb_addr_map_needs_llvm_propeller(Some(5)));
        assert!(!bb_addr_map_needs_llvm_propeller(Some(4)));
        assert!(!bb_addr_map_needs_llvm_propeller(None));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bb_addr_map_version_reads_installed_vmlinux_if_present() {
        for path in [
            "/usr/src/debug/linux-cachyos-lto/vmlinux",
            "/usr/src/debug/linux-cachyos/vmlinux",
        ] {
            let path = Path::new(path);
            if path.is_file() {
                let version = bb_addr_map_version(path);
                assert!(
                    version.is_some_and(|v| v >= 2),
                    "{}: {version:?}",
                    path.display()
                );
                return;
            }
        }
    }

    #[test]
    fn find_generate_propeller_profiles_uses_cache_bin() {
        let dir = std::env::temp_dir().join(format!("abs-prop-cache-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let bin = dir.join("bin").join("generate_propeller_profiles");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::write(&bin, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(
            find_generate_propeller_profiles_in(&dir).as_deref(),
            Some(bin.to_str().unwrap())
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn propeller_bootstrap_script_is_embedded() {
        assert!(PROPELLER_BUILD_SCRIPT.starts_with("#!/"));
        assert!(PROPELLER_BUILD_SCRIPT.contains("generate_propeller_profiles"));
        assert!(PROPELLER_BUILD_SCRIPT.contains("find_package(LLVM"));
        assert!(PROPELLER_BUILD_SCRIPT.contains("google/llvm-propeller"));
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
        let env = stage1_env("linux-cachyos", &KernelBuildConfig::default());
        assert_eq!(env.get("_autofdo").map(String::as_str), Some("yes"));
        assert_eq!(env.get("_build_debug").map(String::as_str), Some("yes"));
        assert_eq!(env.get("_use_llvm_lto").map(String::as_str), Some("none"));
        assert_eq!(env.get("_use_lto_suffix").map(String::as_str), Some("no"));
    }

    #[test]
    fn pgo_lto_suffix_follows_starting_package() {
        assert_eq!(pgo_lto_suffix_flag("linux-cachyos"), "no");
        assert_eq!(pgo_lto_suffix_flag("linux-cachyos-bore"), "no");
        assert_eq!(pgo_lto_suffix_flag("linux-cachyos-lto"), "yes");
        assert_eq!(pgo_lto_suffix_flag("linux-cachyos-bore-lto"), "yes");
    }

    #[test]
    fn stage2_env_keeps_starting_package_name() {
        let env = stage2_build_env(
            "linux-cachyos",
            &KernelBuildConfig::default(),
            "kernel-compilation.afdo",
        );
        assert_eq!(env.get("_use_llvm_lto").map(String::as_str), Some("thin"));
        assert_eq!(env.get("_use_lto_suffix").map(String::as_str), Some("no"));
        let lto = stage2_build_env(
            "linux-cachyos-lto",
            &KernelBuildConfig::default(),
            "kernel-compilation.afdo",
        );
        assert_eq!(lto.get("_use_lto_suffix").map(String::as_str), Some("yes"));
    }

    #[test]
    fn stage1_user_overrides_do_not_clobber_pgo_flags() {
        let kernel = KernelBuildConfig {
            cpusched: Some("bore".into()),
            use_llvm_lto: Some("thin".into()),
            use_kcfi: Some("no".into()),
            ..Default::default()
        };
        let mut env = stage1_env("linux-cachyos", &kernel);
        merge_user_kernel_overrides(&mut env, &kernel);
        assert_eq!(env.get("_cpusched").map(String::as_str), Some("bore"));
        assert_eq!(env.get("_use_llvm_lto").map(String::as_str), Some("none"));
        assert_eq!(env.get("_use_kcfi").map(String::as_str), Some("yes"));
    }

    #[test]
    fn stage3_env_has_propeller_profiles() {
        let env = stage3_build_env(
            "linux-cachyos",
            &KernelBuildConfig::default(),
            "kernel-compilation.afdo",
        );
        assert_eq!(
            env.get("_propeller_profiles").map(String::as_str),
            Some("yes")
        );
        assert_eq!(env.get("_build_debug").map(String::as_str), Some("no"));
        assert_eq!(env.get("_use_lto_suffix").map(String::as_str), Some("no"));
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
    fn branch_stack_probe_detects_missing_lbr() {
        let amd = "amd64_fam1ah_zen5::RETIRED_TAKEN_BRANCH_INSTRUCTIONS:k: \
                   PMU Hardware or event type doesn't support branch stack sampling.";
        let intel = "BR_INST_RETIRED.NEAR_TAKEN:k: \
                     PMU Hardware or event type doesn't support branch stack sampling.";
        assert!(super::branch_stack_sampling_unavailable(amd));
        assert!(super::branch_stack_sampling_unavailable(intel));
        let msg = super::branch_stack_unavailable_message(intel);
        assert!(msg.contains("perf record -b"));
        assert!(msg.contains("CPU/hypervisor"));
        assert!(!msg.contains("LbrExtV2"));
        assert!(!msg.contains("kvm-amd"));
        assert!(!super::branch_stack_sampling_unavailable(
            "sudo: perf: command not found"
        ));
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
    fn ramdisk_perf_without_sidecar_may_be_reused() {
        assert!(super::allow_anonymous_perf_reuse(Path::new(
            "/run/abs-ram/pgo-scratch/linux-cachyos/kernel.data"
        )));
        assert!(super::allow_anonymous_perf_reuse(Path::new(
            "/run/abs-ram/pgo-scratch/linux-cachyos/propeller.data"
        )));
        assert!(!super::allow_anonymous_perf_reuse(Path::new(
            "/tmp/kernel.data"
        )));
        assert!(!super::allow_anonymous_perf_reuse(Path::new(
            "/home/john/.cache/abs/kernel.data"
        )));
    }

    fn running_debug() -> super::PerfKernelIdentity {
        super::PerfKernelIdentity {
            uname: "7.2.2-1-cachyos".into(),
            pkgbase: Some("linux-cachyos".into()),
        }
    }

    fn write_usable_perf(path: &Path, extra: usize) {
        fs::write(path, vec![0u8; MIN_USABLE_PERF_BYTES as usize + extra]).unwrap();
    }

    #[test]
    fn existing_perf_data_falls_back_to_repo_copy() {
        let pid = std::process::id();
        let scratch = std::env::temp_dir().join(format!("abs-pgo-scratch-{pid}"));
        let repo = std::env::temp_dir().join(format!("abs-pgo-repo-{pid}"));
        let _ = fs::remove_dir_all(&scratch);
        let _ = fs::remove_dir_all(&repo);
        fs::create_dir_all(&scratch).unwrap();
        fs::create_dir_all(&repo).unwrap();
        let scratch_file = scratch.join("propeller.data");
        let repo_file = repo.join("propeller.data");
        let running = running_debug();
        assert!(super::existing_perf_data(&scratch_file, &repo, &running).is_none());
        write_usable_perf(&repo_file, 0);
        super::write_perf_kernel_identity(&repo_file, &running).unwrap();
        let (path, n) = super::existing_perf_data(&scratch_file, &repo, &running).unwrap();
        assert_eq!(path, repo_file);
        assert_eq!(n, MIN_USABLE_PERF_BYTES);
        write_usable_perf(&scratch_file, 8);
        super::write_perf_kernel_identity(&scratch_file, &running).unwrap();
        let (path, n) = super::existing_perf_data(&scratch_file, &repo, &running).unwrap();
        assert_eq!(path, scratch_file);
        assert_eq!(n, MIN_USABLE_PERF_BYTES + 8);
        let _ = fs::remove_dir_all(&scratch);
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn existing_perf_data_ignores_file_without_kernel_identity() {
        let pid = std::process::id();
        let scratch = std::env::temp_dir().join(format!("abs-pgo-noid-{pid}"));
        let repo = std::env::temp_dir().join(format!("abs-pgo-noid-repo-{pid}"));
        let _ = fs::remove_dir_all(&scratch);
        let _ = fs::remove_dir_all(&repo);
        fs::create_dir_all(&scratch).unwrap();
        fs::create_dir_all(&repo).unwrap();
        let scratch_file = scratch.join("kernel.data");
        let repo_file = repo.join("kernel.data");
        write_usable_perf(&repo_file, 0);
        assert!(
            super::existing_perf_data(&scratch_file, &repo, &running_debug()).is_none(),
            "size-only leftover from an unknown kernel must not be reused"
        );
        let _ = fs::remove_dir_all(&scratch);
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn existing_perf_data_ignores_other_kernel_identity() {
        let pid = std::process::id();
        let scratch = std::env::temp_dir().join(format!("abs-pgo-otherk-{pid}"));
        let repo = std::env::temp_dir().join(format!("abs-pgo-otherk-repo-{pid}"));
        let _ = fs::remove_dir_all(&scratch);
        let _ = fs::remove_dir_all(&repo);
        fs::create_dir_all(&scratch).unwrap();
        fs::create_dir_all(&repo).unwrap();
        let scratch_file = scratch.join("kernel.data");
        let repo_file = repo.join("kernel.data");
        write_usable_perf(&repo_file, 0);
        super::write_perf_kernel_identity(
            &repo_file,
            &super::PerfKernelIdentity {
                uname: "7.2.2-1-cachyos".into(),
                pkgbase: Some("linux-cachyos-lto".into()),
            },
        )
        .unwrap();
        assert!(
            super::existing_perf_data(&scratch_file, &repo, &running_debug()).is_none(),
            "same uname, different pkgbase (debug vs AutoFDO -lto) must not be reused"
        );
        super::write_perf_kernel_identity(
            &repo_file,
            &super::PerfKernelIdentity {
                uname: "7.1.9-1-cachyos".into(),
                pkgbase: Some("linux-cachyos".into()),
            },
        )
        .unwrap();
        assert!(
            super::existing_perf_data(&scratch_file, &repo, &running_debug()).is_none(),
            "same pkgbase, different uname must not be reused"
        );
        let _ = fs::remove_dir_all(&scratch);
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn perf_identity_matches_requires_uname_and_pkgbase() {
        let debug = running_debug();
        assert!(super::perf_identity_matches(&debug, &debug));
        assert!(!super::perf_identity_matches(
            &debug,
            &super::PerfKernelIdentity {
                uname: debug.uname.clone(),
                pkgbase: Some("linux-cachyos-lto".into()),
            }
        ));
        assert!(!super::perf_identity_matches(
            &debug,
            &super::PerfKernelIdentity {
                uname: "7.1.9-1-cachyos".into(),
                pkgbase: debug.pkgbase.clone(),
            }
        ));
        assert!(!super::perf_identity_matches(
            &debug,
            &super::PerfKernelIdentity {
                uname: debug.uname.clone(),
                pkgbase: None,
            }
        ));
    }

    #[test]
    fn sync_perf_data_to_repo_copies_kernel_identity() {
        let pid = std::process::id();
        let root = std::env::temp_dir().join(format!("abs-pgo-sync-id-{pid}"));
        let _ = fs::remove_dir_all(&root);
        let scratch = root.join("scratch");
        let repo = root.join("repo");
        fs::create_dir_all(&scratch).unwrap();
        fs::create_dir_all(&repo).unwrap();
        let src = scratch.join("kernel.data");
        write_usable_perf(&src, 0);
        super::write_perf_kernel_identity(&src, &running_debug()).unwrap();
        super::sync_perf_data_to_repo(&src, &repo).unwrap();
        let dest = repo.join("kernel.data");
        let id = super::read_perf_kernel_identity(&dest).expect("sidecar copied");
        assert_eq!(id, running_debug());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn propeller_profiles_on_ram_defaults_true() {
        let pgo: PgoConfig = toml::from_str("").unwrap();
        assert!(pgo.propeller_profiles_on_ram);
        assert!(pgo.perf_data_on_ram);
        let pgo: PgoConfig = toml::from_str("propeller_profiles_on_ram = false\n").unwrap();
        assert!(!pgo.propeller_profiles_on_ram);
    }

    #[test]
    fn persist_propeller_to_disk_follows_checkbox() {
        let mut pgo = test_pgo();
        pgo.propeller_profiles_on_ram = true;
        assert!(!super::persist_propeller_to_disk(&pgo));
        pgo.propeller_profiles_on_ram = false;
        assert!(super::persist_propeller_to_disk(&pgo));
    }

    #[test]
    fn propeller_restore_prefers_scratch_and_skips_archive() {
        let pid = std::process::id();
        let root = std::env::temp_dir().join(format!("abs-pgo-restore-{pid}"));
        let _ = fs::remove_dir_all(&root);
        let repo = root.join("repo");
        let archive = root.join("archive");
        let scratch = root.join("scratch");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&archive).unwrap();
        fs::create_dir_all(&scratch).unwrap();
        let name = "propeller_cc_profile.txt";
        fs::write(archive.join(name), "from-archive").unwrap();
        assert_eq!(
            super::resolve_profile_restore(&repo, &archive, Some(&scratch), name, true),
            super::ProfileRestore::Missing
        );
        fs::write(scratch.join(name), "from-scratch").unwrap();
        match super::resolve_profile_restore(&repo, &archive, Some(&scratch), name, true) {
            super::ProfileRestore::From(path) => assert_eq!(path, scratch.join(name)),
            other => panic!("expected scratch, got {other:?}"),
        }
        fs::write(repo.join(name), "already").unwrap();
        assert_eq!(
            super::resolve_profile_restore(&repo, &archive, Some(&scratch), name, true),
            super::ProfileRestore::AlreadyPresent
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parse_pgo_stage_aliases() {
        assert_eq!(
            parse_pgo_stage("stage2_profile").unwrap(),
            PgoStageId::Stage2Profile
        );
        assert_eq!(parse_pgo_stage("2p").unwrap(), PgoStageId::Stage2Profile);
        assert_eq!(
            parse_pgo_stage("profile").unwrap(),
            PgoStageId::Stage2Profile
        );
        assert_eq!(
            parse_pgo_stage("stage1_build").unwrap(),
            PgoStageId::Stage1Build
        );
        assert_eq!(parse_pgo_stage("wait0").unwrap(), PgoStageId::WaitReboot0);
        assert_eq!(
            parse_pgo_stage("start_reboot").unwrap(),
            PgoStageId::WaitReboot0
        );
        assert!(PgoStageId::WaitReboot0.is_wait_reboot());
        assert!(parse_pgo_stage("not-a-stage").is_err());
    }

    #[test]
    fn pgo_stage_labels_non_empty() {
        assert!(!PgoStageId::Stage1Build.label().is_empty());
        assert!(!PgoStageId::Done.label().is_empty());
        assert_eq!(PgoStageId::Stage2Build.label(), "Stage 2: AutoFDO build");
        assert_eq!(PgoStageId::Stage3Build.label(), "Stage 3: final build");
    }

    fn state_with(package: &str, base: Option<&str>, uname: Option<&str>) -> PgoState {
        PgoState {
            package: package.into(),
            repo_dir: "/tmp".into(),
            current_stage: PgoStageId::WaitReboot1,
            started_at: 0,
            updated_at: 0,
            expected_kernel_uname: uname.map(str::to_string),
            expected_package_base: base.map(str::to_string),
            stage_history: Vec::new(),
        }
    }

    #[test]
    fn boot_matches_uses_pkgbase_file_to_tell_stage_kernels_apart() {
        // Same version, different pkgbase: only the pkgbase file can tell these apart.
        let state = state_with(
            "linux-cachyos",
            Some("linux-cachyos-lto"),
            Some("6.15.4-2-cachyos-lto"),
        );
        assert!(super::boot_matches(
            &state,
            "6.15.4-2-cachyos-lto",
            Some("linux-cachyos-lto\n")
        ));
        // Booted the stage-1 kernel of the same version: must NOT match.
        assert!(!super::boot_matches(
            &state,
            "6.15.4-2-cachyos",
            Some("linux-cachyos")
        ));
    }

    #[test]
    fn boot_matches_falls_back_to_version_prefix_without_pkgbase_file() {
        let state = state_with(
            "linux-cachyos",
            Some("linux-cachyos"),
            Some("6.15.4-2-cachyos"),
        );
        assert!(super::boot_matches(&state, "6.15.4-2-cachyos", None));
        assert!(!super::boot_matches(&state, "6.14.0-1-cachyos", None));
        // Neither an expected uname nor a pkgbase file: never match.
        let empty = state_with("linux-cachyos", None, None);
        assert!(!super::boot_matches(&empty, "6.15.4-2-cachyos", None));
    }

    #[test]
    fn infer_suffix_handles_kernel_variants() {
        assert_eq!(super::infer_suffix("linux-cachyos"), "cachyos");
        assert_eq!(super::infer_suffix("linux-cachyos-lto"), "cachyos-lto");
        assert_eq!(
            super::infer_suffix("linux-cachyos-bore-lto"),
            "cachyos-bore-lto"
        );
        assert_eq!(super::infer_suffix("linux-zen"), "zen");
    }

    #[test]
    fn kernel_hold_package_names_cover_variant_stage_kernels() {
        let state = state_with(
            "linux-cachyos-bore",
            Some("linux-cachyos-bore"),
            Some("6.15.4-2-cachyos-bore"),
        );
        let names = super::kernel_hold_package_names(&state);
        assert!(names.contains(&"linux-cachyos-bore".to_string()));
        assert!(names.contains(&"linux-cachyos-bore-lto".to_string()));
        assert!(names.contains(&"linux-cachyos-bore-lto-headers".to_string()));
        assert!(!names.contains(&"linux-cachyos".to_string()));
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
        fs::write(&state_path, serde_json::to_string_pretty(&state).unwrap()).unwrap();

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
            propeller_profiles_on_ram: true,
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
            select_boot_kernel: true,
            auto_restart: false,
            reboot_before_start: false,
            reuse_afdo_profile: false,
            reuse_propeller_profile: false,
            compare_current: false,
            compare_debug: false,
            compare_debug_clean: false,
            compare_autofdo: false,
            compare_autofdo_clean: false,
            compare_final: false,
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
        let zen = active.iter().find(|p| p.package == "linux-zen").expect(
            "custom state_file pipeline should be discovered even if other PGO state exists",
        );
        assert_eq!(zen.stage_label, PgoStageId::Stage2Build.label());

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
        assert!(
            path.is_file(),
            "event log file should exist after EventLog::new"
        );
        log.emit(&PgoEvent::Error {
            ts: 0,
            message: "test".into(),
        });
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("test"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_pgo_abort_clears_pipeline_state() {
        assert_eq!(
            super::abort_disposition(false),
            super::PgoAbortDisposition::RemoveState
        );
        assert_eq!(
            super::abort_disposition(true),
            super::PgoAbortDisposition::KeepStage
        );
    }

    #[test]
    fn remove_state_deletes_the_pipeline_file() {
        let dir = std::env::temp_dir().join(format!("abs-pgo-abort-state-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("linux-cachyos.json");
        fs::write(&path, "{\"package\":\"linux-cachyos\"}").unwrap();
        super::apply_abort_state(&path, super::PgoAbortDisposition::RemoveState);
        assert!(!path.exists(), "abort should delete saved PGO state");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_resume_enable_links_include_default_and_graphical_wants() {
        let dir = PathBuf::from("/tmp/abs-systemd-user");
        let links = super::pgo_auto_resume_enable_links(&dir, "linux-cachyos");
        assert!(
            links
                .iter()
                .any(|p| p.ends_with("default.target.wants/abs-pgo@linux-cachyos.service")),
            "{links:?}"
        );
        assert!(
            links.iter().any(
                |p| p.ends_with("graphical-session.target.wants/abs-pgo@linux-cachyos.service")
            ),
            "{links:?}"
        );
    }

    #[test]
    fn unlinking_auto_resume_links_removes_installed_wants() {
        let dir = std::env::temp_dir().join(format!("abs-pgo-wants-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for link in super::pgo_auto_resume_enable_links(&dir, "linux-cachyos") {
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            fs::write(&link, "").unwrap();
            assert!(link.exists());
        }
        super::unlink_pgo_auto_resume_links(&dir, "linux-cachyos");
        for link in super::pgo_auto_resume_enable_links(&dir, "linux-cachyos") {
            assert!(!link.exists(), "{}", link.display());
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
