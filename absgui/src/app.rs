use crate::abs_runner::{
    self, fetch_pgo_status, run_ramdisk_shutdown, stream_abs_pgo, AbsPgoStreamItem, PgoAction,
    PgoRunHandle, PgoStatus,
};
use iced::futures::StreamExt;
use crate::app_settings::{load_window_icon, AppTheme, GuiSettings};
use crate::list_editors::{self, ListEditors, PackageListField};
use crate::config::{config_path, load_config, save_config, ConfigDocument, PackageSection};
use crate::dialog;
use crate::field_help;
use crate::messages::{EditTarget, KBool, KOptBool, KStr, Message, Page, PathField, RamdiskLetter};
use crate::style;
use crate::views::abs_settings;
use crate::widgets::{
    card_section, encode_ramdisk_flags, field_checkbox, field_number, field_path, field_pick,
    field_text, optional_bool_field, parse_ramdisk_flags, kernel_ramdisk_targets_field,
    page_title, ramdisk_targets_field, PathField as WPathField, PathKind as WPathKind,
};
use iced::event;
use iced::widget::{
    button, column, container, image, row, rule, scrollable, text, text_editor, text_input, Space,
};
use iced::{clipboard, window, time};
use iced::{
    Alignment, Element, Font, Length, Padding, Subscription, Task, Theme,
};
use std::sync::Arc;

const PGO_STATUS_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

const PGO_STEPS: [(&str, &str); 7] = [
    ("Debug build", "stage1_build"),
    ("Reboot", "wait_reboot1"),
    ("Profile AutoFDO", "stage2_profile"),
    ("AutoFDO build", "stage2_build"),
    ("Reboot", "wait_reboot2"),
    ("Profile Propeller", "stage3_profile"),
    ("Final build", "stage3_build"),
];

fn pgo_first_phase_key() -> &'static str {
    PGO_STEPS[0].1
}

fn is_valid_pgo_phase_key(key: &str) -> bool {
    PGO_STEPS.iter().any(|(_, k)| *k == key)
}

fn pgo_stage_label(key: &str) -> &str {
    PGO_STEPS
        .iter()
        .find(|(_, k)| *k == key)
        .map(|(label, _)| *label)
        .unwrap_or(key)
}

fn pgo_auto_restart_enabled(pkg: &PackageSection) -> bool {
    pkg.pgo
        .as_ref()
        .map(|p| p.auto_restart)
        .unwrap_or(false)
}

fn pgo_stage_index(stage: &str) -> Option<usize> {
    if stage == "done" {
        return Some(PGO_STEPS.len());
    }
    PGO_STEPS.iter().position(|(_, key)| *key == stage)
}

/// Next runnable phase after a reboot wait gate.
fn pgo_next_phase_after_wait(wait_key: &str) -> Option<&'static str> {
    match wait_key {
        "wait_reboot1" => Some("stage2_profile"),
        "wait_reboot2" => Some("stage3_profile"),
        _ => None,
    }
}

fn pgo_default_selected_stage(saved: &str) -> String {
    if let Some(next) = pgo_next_phase_after_wait(saved) {
        next.to_string()
    } else if is_valid_pgo_phase_key(saved) {
        saved.to_string()
    } else {
        pgo_first_phase_key().to_string()
    }
}

/// Stage passed to `--pgo-resume --pgo-stage`. Wait gates are not runnable; omit the flag so abs
/// auto-advances to the next profile/build step.
fn pgo_resume_stage_arg<'a>(selected: &'a str, saved: &str) -> Option<&'a str> {
    if matches!(selected, "wait_reboot1" | "wait_reboot2") {
        return None;
    }
    if matches!(saved, "wait_reboot1" | "wait_reboot2") && selected == saved {
        return None;
    }
    Some(selected)
}

fn pgo_saved_at_wait_reboot(saved: &str) -> bool {
    matches!(saved, "wait_reboot1" | "wait_reboot2")
}

const SCHED_OPTS: &[&str] = &[
    "cachyos", "bore", "eevdf", "rt", "rt-bore", "hardened", "bmq", "sched-ext",
];
const LTO_OPTS: &[&str] = &["none", "thin", "full"];
const HZ_OPTS: &[&str] = &["100", "250", "300", "500", "600", "750", "1000"];
const TICK_OPTS: &[&str] = &["full", "idle", "periodic"];
const PREEMPT_OPTS: &[&str] = &["full", "voluntary", "server", "lazy"];
const HUGE_OPTS: &[&str] = &["always", "madvise"];
const SOURCE_OPTS: &[&str] = &["aur", "cachyos", "arch"];
const ENV_OPTS: &[&str] = &["local", "chroot"];
const PGO_BENCHMARK_PRESET_OPTS: &[&str] = &["fast", "cachyos"];
const PGO_PROFILING_QUALITY_OPTS: &[&str] = &["standard", "maximum"];

pub fn run() -> iced::Result {
    let gui_settings = GuiSettings::load();
    let icon = load_window_icon();
    let window = gui_settings.window_settings(icon);
    let boot_settings = gui_settings.clone();
    iced::application(move || App::new(boot_settings.clone()), App::update, App::view)
        .title(App::title)
        .theme(App::theme)
        .subscription(App::subscription)
        .window(window)
        .exit_on_close_request(false)
        .run()
}

pub struct App {
    page: Page,
    gui_settings: GuiSettings,
    config_path: std::path::PathBuf,
    config: ConfigDocument,
    config_error: Option<String>,
    status_message: Option<String>,
    selected_kernel: Option<String>,
    custom_kernel: String,
    selected_package: Option<String>,
    new_package_name: String,
    pgo_status: Option<PgoStatus>,
    pgo_status_error: Option<String>,
    /// Phase selected in the PGO UI (used by Start from current phase).
    pgo_selected_stage: String,
    log_lines: Vec<String>,
    log_content: text_editor::Content,
    last_event_log_path: Option<std::path::PathBuf>,
    list_editors: ListEditors,
    busy: bool,
    /// True while a one-shot (non-PGO) kernel build is running; suppresses PGO status polling.
    building_oneshot: bool,
    pgo_run: PgoRunHandle,
    /// When true, new log lines scroll the build log to the bottom automatically.
    log_follow_tail: bool,
    /// Set when the active build runs in a separate terminal window (not streamed in-app).
    /// Holds the launch time so status polling can ignore stale state during a short grace period.
    external_run_since: Option<std::time::Instant>,
    /// PID file for the abs process running in the external terminal (see [`abs_runner::external_run_pid_path`]).
    external_pid_path: Option<std::path::PathBuf>,
}

impl App {
    fn title(_state: &Self) -> String {
        "absgui".into()
    }

    fn theme(state: &Self) -> Theme {
        style::iced_theme(state.gui_settings.theme)
    }

    fn subscription(state: &Self) -> Subscription<Message> {
        let mut subs: Vec<Subscription<Message>> = vec![
            window::resize_events().map(|(_id, size)| Message::WindowResized(size)),
            event::listen_with(|event, _status, _id| {
                if let iced::Event::Window(window::Event::Moved(point)) = event {
                    Some(Message::WindowMoved(point))
                } else {
                    None
                }
            }),
            window::close_requests().map(|_| Message::WindowCloseRequested),
        ];
        if state.pgo_status_poll_active() {
            subs.push(
                time::every(PGO_STATUS_POLL_INTERVAL).map(|_| Message::RefreshPgoStatus),
            );
        }
        Subscription::batch(subs)
    }

    fn new(gui_settings: GuiSettings) -> (Self, Task<Message>) {
        let path = config_path();
        (
            Self {
                page: Page::Kernels,
                gui_settings,
                config_path: path.clone(),
                config: ConfigDocument::default(),
                config_error: None,
                status_message: None,
                selected_kernel: None,
                custom_kernel: String::new(),
                selected_package: None,
                new_package_name: String::new(),
                pgo_status: None,
                pgo_status_error: None,
                pgo_selected_stage: pgo_first_phase_key().to_string(),
                log_lines: Vec::new(),
                log_content: text_editor::Content::new(),
                last_event_log_path: None,
                list_editors: ListEditors::from_config(&ConfigDocument::default()),
                busy: false,
                building_oneshot: false,
                pgo_run: PgoRunHandle::new(),
                log_follow_tail: true,
                external_run_since: None,
                external_pid_path: None,
            },
            Task::perform(async move { load_config(&path) }, |r| {
                Message::ConfigLoaded(Box::new(r))
            }),
        )
    }

    fn app_theme(&self) -> AppTheme {
        self.gui_settings.theme
    }

    fn sync_log_content_from_lines(&mut self) {
        self.log_content = text_editor::Content::with_text(&self.log_lines.join("\n"));
    }

    fn append_log_to_content(&mut self, line: &str, first_line: bool) {
        if self.log_follow_tail {
            self.log_content
                .perform(text_editor::Action::Move(text_editor::Motion::DocumentEnd));
        }
        if !first_line {
            self.log_content.perform(text_editor::Action::Edit(text_editor::Edit::Paste(
                Arc::new("\n".to_string()),
            )));
        }
        self.log_content.perform(text_editor::Action::Edit(text_editor::Edit::Paste(
            Arc::new(line.to_string()),
        )));
    }

    fn append_log(&mut self, line: impl Into<String>) {
        let line = line.into();
        let first_line = self.log_lines.is_empty();
        self.log_lines.push(line.clone());
        // Always mirror into the editor so Abort / status lines are visible even when the user
        // scrolled up and disabled follow-tail.
        self.append_log_to_content(&line, first_line);
    }

    fn scroll_log_if_following(&self) -> Task<Message> {
        if self.log_follow_tail {
            Task::done(Message::LogFollowTail)
        } else {
            Task::none()
        }
    }

    fn push_log(&mut self, line: impl Into<String>) -> Task<Message> {
        self.append_log(line);
        self.scroll_log_if_following()
    }

    fn sync_pgo_selected_stage_from_status(&mut self, status: &PgoStatus) {
        let saved = status.stage.as_str();
        let stage_changed = self
            .pgo_status
            .as_ref()
            .map(|s| s.stage.as_str())
            != Some(saved);

        if stage_changed || !is_valid_pgo_phase_key(&self.pgo_selected_stage) {
            self.pgo_selected_stage = pgo_default_selected_stage(saved);
        }
    }

    fn launch_pgo_run(
        &mut self,
        action: PgoAction,
        stage: Option<&str>,
        once: bool,
        status_msg: &str,
    ) -> Task<Message> {
        let Some(pkg) = self.selected_kernel.clone() else {
            return Task::none();
        };
        if self.busy {
            return self.push_log("PGO already running — wait for it to finish or click Abort.");
        }
        self.list_editors.apply_all(&mut self.config);
        let Some(section) = self.config.packages.get(&pkg).cloned() else {
            let msg = format!(
                "{pkg} is not saved in abs.toml yet — configure fields below and click Save config"
            );
            self.status_message = Some(msg.clone());
            return self.push_log(format!("Cannot start PGO: {msg}"));
        };
        if let Err(msg) = validate_pgo_start(&section, &pkg) {
            self.status_message = Some(msg.clone());
            return self.push_log(format!("Cannot start PGO: {msg}"));
        }
        if let Err(e) = abs_runner::verify_abs_binary() {
            self.status_message = Some(e.clone());
            return self.push_log(format!("Cannot start PGO: {e}"));
        }
        let event_log = abs_runner::default_event_log_path(&pkg);
        if let Err(e) = abs_runner::ensure_event_log_path(&event_log) {
            self.status_message = Some(e.clone());
            return self.push_log(format!("Cannot start PGO: {e}"));
        }
        self.busy = true;
        self.pgo_run.reset();
        self.log_follow_tail = true;
        self.last_event_log_path = Some(event_log.clone());
        self.status_message = Some(status_msg.to_string());
        self.append_log(status_msg);
        self.append_log(format!("Detailed events: {}", event_log.display()));
        let path = self.config_path.clone();
        let doc = self.config.clone();
        if let Err(e) = save_config(&path, &doc) {
            self.busy = false;
            self.status_message = Some(e.clone());
            return self.push_log(format!("Cannot start PGO: failed to save config: {e}"));
        }
        self.append_log(format!("Saved {}", path.display()));
        let pgo_auto = pgo_auto_restart_enabled(&section);
        let abs_cmd = abs_runner::format_abs_pgo_command(
            action,
            &pkg,
            Some(&event_log),
            stage,
            once,
            pgo_auto,
        );
        self.append_log(format!("$ {abs_cmd}"));
        let pid_path = abs_runner::external_run_pid_path(&pkg);
        let stage_owned = stage.map(str::to_string);
        match abs_runner::launch_in_terminal(&abs_cmd, Some(&pid_path)) {
            Ok(term) => {
                self.external_run_since = Some(std::time::Instant::now());
                self.external_pid_path = Some(pid_path);
                self.append_log(format!(
                    "Launched abs in a new {term} window. Enter your sudo password there; \
                     full build output appears in that terminal. This panel tracks pipeline progress. \
                     Click Abort here to stop the build."
                ));
                Task::batch([
                    self.scroll_log_if_following(),
                    Task::done(Message::RefreshPgoStatus),
                ])
            }
            Err(e) => {
                self.append_log(format!(
                    "Could not open a terminal window ({e}). Running in-app instead."
                ));
                let handle = self.pgo_run.clone();
                Task::batch([
                    self.scroll_log_if_following(),
                    Task::done(Message::RefreshPgoStatus),
                    Task::stream(
                        stream_abs_pgo(
                            action,
                            pkg,
                            Some(event_log),
                            stage_owned,
                            once,
                            pgo_auto,
                            handle,
                        )
                        .map(|item| match item {
                            AbsPgoStreamItem::Line(line) => Message::PgoLogLine(line),
                            AbsPgoStreamItem::Finished(result) => Message::PgoRunFinished(result),
                        }),
                    ),
                ])
            }
        }
    }

    fn effective_pgo_stage(&self) -> &str {
        self.pgo_status
            .as_ref()
            .map(|s| s.stage.as_str())
            .unwrap_or("")
    }

    fn pgo_status_poll_active(&self) -> bool {
        if !matches!(self.page, Page::KernelConfig) || self.selected_kernel.is_none() {
            return false;
        }
        if self.building_oneshot {
            return false;
        }
        if self.busy {
            return true;
        }
        matches!(
            self.pgo_status.as_ref().map(|s| s.stage.as_str()),
            Some(stage) if stage != "done" && stage != "aborted" && !stage.is_empty()
        )
    }

    fn path_value(&self, field: PathField) -> String {
        match field {
            PathField::PackagesPath => self.config.paths.packages_path.clone(),
            PathField::ChrootPath => self.config.paths.chroot_base_path.clone(),
            PathField::ReadyPath => self.config.paths.ready_made_packages_path.clone(),
            PathField::ChrootMakepkgConf => self
                .config
                .paths
                .chroot_makepkg_conf
                .clone()
                .unwrap_or_default(),
            PathField::RamdiskMountPoint => self.config.ramdisk.mount_point.clone(),
            PathField::RamdiskSeedChroot => self
                .config
                .ramdisk
                .seed_chroot_from
                .clone()
                .unwrap_or_default(),
            PathField::SelfUpdateInstallPath => self
                .config
                .self_update_install_path
                .clone()
                .unwrap_or_default(),
            PathField::PgoArchiveDir => self
                .selected_kernel
                .as_ref()
                .and_then(|n| self.config.packages.get(n))
                .and_then(|p| p.pgo.as_ref())
                .and_then(|p| p.profiles_archive_dir.clone())
                .unwrap_or_default(),
            PathField::PgoBenchmark => self
                .selected_kernel
                .as_ref()
                .and_then(|n| self.config.packages.get(n))
                .and_then(|p| p.pgo.as_ref())
                .and_then(|p| p.benchmark_command.clone())
                .unwrap_or_default(),
            PathField::PgoBenchmarkWorkdir => self
                .selected_kernel
                .as_ref()
                .and_then(|n| self.config.packages.get(n))
                .and_then(|p| p.pgo.as_ref())
                .and_then(|p| p.benchmark_workdir.clone())
                .unwrap_or_default(),
            PathField::PgoProfileScratchDir => self
                .selected_kernel
                .as_ref()
                .and_then(|n| self.config.packages.get(n))
                .and_then(|p| p.pgo.as_ref())
                .map(|p| p.profile_scratch_dir.clone())
                .unwrap_or_else(|| "auto".into()),
            PathField::PgoVmlinux => self
                .selected_kernel
                .as_ref()
                .and_then(|n| self.config.packages.get(n))
                .and_then(|p| p.pgo.as_ref())
                .map(|p| p.vmlinux.clone())
                .unwrap_or_else(|| "auto".into()),
            PathField::PgoStateFile => self
                .selected_kernel
                .as_ref()
                .and_then(|n| self.config.packages.get(n))
                .and_then(|p| p.pgo.as_ref())
                .and_then(|p| p.state_file.clone())
                .unwrap_or_default(),
        }
    }

    fn apply_path(&mut self, field: PathField, value: String) {
        let trimmed = value.trim().to_string();
        let opt = if trimmed.is_empty() { None } else { Some(trimmed) };
        match field {
            PathField::PackagesPath => {
                if let Some(v) = opt {
                    self.config.paths.packages_path = v;
                }
            }
            PathField::ChrootPath => {
                if let Some(v) = opt {
                    self.config.paths.chroot_base_path = v;
                }
            }
            PathField::ReadyPath => {
                if let Some(v) = opt {
                    self.config.paths.ready_made_packages_path = v;
                }
            }
            PathField::ChrootMakepkgConf => self.config.paths.chroot_makepkg_conf = opt,
            PathField::RamdiskMountPoint => {
                if let Some(v) = opt {
                    self.config.ramdisk.mount_point = v;
                }
            }
            PathField::RamdiskSeedChroot => self.config.ramdisk.seed_chroot_from = opt,
            PathField::SelfUpdateInstallPath => self.config.self_update_install_path = opt,
            PathField::PgoArchiveDir
            | PathField::PgoBenchmark
            | PathField::PgoBenchmarkWorkdir
            | PathField::PgoProfileScratchDir
            | PathField::PgoVmlinux
            | PathField::PgoStateFile => {
                if let Some(name) = self.selected_kernel.clone() {
                    self.config.ensure_kernel_from_defaults(&name);
                    if let Some(pkg) = self.config.packages.get_mut(&name) {
                        let pgo = pkg.pgo.get_or_insert_with(Default::default);
                        match field {
                            PathField::PgoArchiveDir => pgo.profiles_archive_dir = opt,
                            PathField::PgoBenchmark => pgo.benchmark_command = opt,
                            PathField::PgoBenchmarkWorkdir => pgo.benchmark_workdir = opt,
                            PathField::PgoProfileScratchDir => {
                                pgo.profile_scratch_dir =
                                    opt.clone().unwrap_or_else(|| "auto".into());
                            }
                            PathField::PgoVmlinux => {
                                pgo.vmlinux = opt.clone().unwrap_or_else(|| "auto".into());
                            }
                            PathField::PgoStateFile => pgo.state_file = opt,
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    fn target_pkg(&self, target: EditTarget) -> Option<&PackageSection> {
        match target {
            EditTarget::Default => Some(&self.config.kernel_defaults),
            EditTarget::Selected => self
                .selected_kernel
                .as_ref()
                .and_then(|n| self.config.packages.get(n)),
            EditTarget::Package => self
                .selected_package
                .as_ref()
                .and_then(|n| self.config.packages.get(n)),
        }
    }

    fn target_pkg_mut(&mut self, target: EditTarget) -> Option<&mut PackageSection> {
        match target {
            EditTarget::Default => Some(&mut self.config.kernel_defaults),
            EditTarget::Selected => {
                let name = self.selected_kernel.clone()?;
                self.config.ensure_kernel_from_defaults(&name);
                self.config.packages.get_mut(&name)
            }
            EditTarget::Package => {
                let name = self.selected_package.clone()?;
                Some(self.config.packages.entry(name).or_default())
            }
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::OpenKernels => self.page = Page::Kernels,
            Message::OpenDefaultConfig => self.page = Page::DefaultKernelConfig,
            Message::OpenPackages => self.page = Page::Packages,
            Message::OpenPackage(name) => {
                self.selected_package = Some(name);
                self.page = Page::PackageConfig;
            }
            Message::NewPackageNameChanged(v) => self.new_package_name = v,
            Message::PackageAdd => {
                let name = self.new_package_name.trim().to_string();
                if name.is_empty() {
                    return Task::none();
                }
                if self.config.packages.contains_key(&name) {
                    self.status_message =
                        Some(format!("{name} is already configured — opening it."));
                } else {
                    self.config
                        .packages
                        .insert(name.clone(), PackageSection::default());
                    self.status_message = Some(format!(
                        "Added [packages.{name}] — configure it below and click Save config."
                    ));
                }
                self.new_package_name.clear();
                self.selected_package = Some(name);
                self.page = Page::PackageConfig;
            }
            Message::PackageRemove(name) => {
                self.config.packages.remove(&name);
                if self.selected_package.as_deref() == Some(name.as_str()) {
                    self.selected_package = None;
                    self.page = Page::Packages;
                }
                self.status_message = Some(format!(
                    "Removed [packages.{name}] — click Save config to write abs.toml."
                ));
            }
            Message::OpenAbsSettings => self.page = Page::AbsSettings,
            Message::OpenAppSettings => self.page = Page::AppSettings,
            Message::Back => {
                self.page = match self.page {
                    Page::PackageConfig => Page::Packages,
                    _ => Page::Kernels,
                };
            }
            Message::OpenKernel(name) => {
                self.config.ensure_kernel_from_defaults(&name);
                self.selected_kernel = Some(name);
                self.pgo_selected_stage = pgo_first_phase_key().to_string();
                self.page = Page::KernelConfig;
                return Task::done(Message::RefreshPgoStatus);
            }
            Message::ReloadConfig => {
                let path = self.config_path.clone();
                return Task::perform(async move { load_config(&path) }, |r| {
                    Message::ConfigLoaded(Box::new(r))
                });
            }
            Message::SaveConfig => {
                self.list_editors.apply_all(&mut self.config);
                let path = self.config_path.clone();
                let doc = self.config.clone();
                return Task::perform(async move { save_config(&path, &doc) }, Message::ConfigSaved);
            }
            Message::SaveAppSettings => {
                let settings = self.gui_settings.clone();
                return Task::perform(async move { settings.save() }, Message::AppSettingsSaved);
            }
            Message::AppThemeSelected(theme) => {
                self.gui_settings.theme = theme;
                let _ = self.gui_settings.save();
            }
            Message::ConfigLoaded(result) => match *result {
                Ok(doc) => {
                    self.config = doc;
                    self.list_editors = ListEditors::from_config(&self.config);
                    self.config_error = None;
                    self.status_message = Some("Config loaded.".into());
                }
                Err(e) => self.config_error = Some(e),
            },
            Message::ConfigSaved(Ok(())) => {
                self.status_message = Some(format!("Saved {}", self.config_path.display()));
                return self.push_log("Config saved.");
            }
            Message::ConfigSaved(Err(e)) => {
                self.status_message = Some(format!("Save failed: {e}"));
            }
            Message::AppSettingsSaved(Ok(())) => {
                self.status_message = Some("App settings saved.".into());
            }
            Message::AppSettingsSaved(Err(e)) => {
                self.status_message = Some(format!("Settings save failed: {e}"));
            }
            Message::PathPackages(v) => self.config.paths.packages_path = v,
            Message::PathChroot(v) => self.config.paths.chroot_base_path = v,
            Message::PathReady(v) => self.config.paths.ready_made_packages_path = v,
            Message::PathChrootMakepkg(v) => {
                self.config.paths.chroot_makepkg_conf = opt_str(v);
            }
            Message::BuildDefaultEnv(v) => self.config.build.default_environment = v,
            Message::BuildDefaultCompiler(v) => {
                self.config.build.default_compiler = opt_str(v);
            }
            Message::BuildConcurrentRepos(v) => {
                if let Ok(n) = v.parse() {
                    self.config.build.concurrent_repos_downloads_limit = n;
                }
            }
            Message::BuildConcurrentCompilations(v) => {
                if let Ok(n) = v.parse() {
                    self.config.build.concurrent_compilations_limit = n;
                }
            }
            Message::BuildGlobalCpuThreadsMode(v) => {
                self.config.build.global_cpu_threads_mode = v;
            }
            Message::BuildGlobalCpuThreadsCap(v) => {
                self.config.build.global_cpu_threads_cap = parse_opt_usize(&v);
            }
            Message::BuildMaximumCpuThreadsCap(v) => {
                self.config.build.maximum_cpu_threads_cap = parse_opt_usize(&v);
            }
            Message::BuildDefaultCompilationThreads(v) => {
                self.config.build.default_compilation_threads = parse_opt_usize(&v);
            }
            Message::BuildSystemUpdateFirst(v) => self.config.build.system_update_first = v,
            Message::BuildIgnoreFailures(v) => self.config.build.ignore_compilation_failures = v,
            Message::BuildCompileFirstInstall(v) => {
                self.config.build.compile_first_install_after = v;
            }
            Message::BuildCleanInstallDefault(v) => {
                self.config.build.clean_install_by_default = v;
            }
            Message::BuildIgnoreAlreadyMade(v) => {
                self.config.build.ignore_already_made_packages = v;
            }
            Message::BuildFastAurRpc(v) => self.config.build.fast_aur_rpc_update_checks = v,
            Message::BuildCleanChrootAfter(v) => {
                self.config.build.clean_chroot_after_compilation = v;
            }
            Message::CheckForUpdateOnStartup(v) => self.config.check_for_update_on_startup = v,
            Message::AutoUpdateOnStartup(v) => self.config.auto_update_on_startup = v,
            Message::SelfUpdateAtUpdates(v) => self.config.self_update_at_updates = v,
            Message::SelfUpdateRawUrl(v) => self.config.self_update_raw_url = opt_str(v),
            Message::SelfUpdateInstallPath(v) => {
                self.config.self_update_install_path = opt_str(v);
            }
            Message::SelfUpdateUsePacman(v) => self.config.self_update_use_pacman = v,
            Message::InstallTestingPhaseArchPackages(v) => {
                self.config.install_testing_phase_archlinux_packages = v;
            }
            Message::PackageListEdited(field, action) => {
                self.list_editors.content_mut(field).perform(action);
                self.list_editors.apply_field(field, &mut self.config);
            }
            Message::UseSeparateSkipInstallAfter(on) => {
                if on {
                    self.config.skip_install_packages_after_compilation = Some(
                        list_editors::parse_lines(&self.list_editors.skip_install_after.text()),
                    );
                    if self.list_editors.skip_install_after.text().trim().is_empty() {
                        self.list_editors.skip_install_after = iced::widget::text_editor::Content::with_text(
                            &list_editors::lines_to_text(&self.config.skip_install_packages),
                        );
                        self.list_editors.apply_field(
                            PackageListField::SkipInstallAfter,
                            &mut self.config,
                        );
                    }
                } else {
                    self.config.skip_install_packages_after_compilation = None;
                }
            }
            Message::SysUpdateReposCmd(v) => {
                self.config.system_update.command_to_update_repositories = v;
            }
            Message::SysUpdateFullCmd(v) => {
                self.config.system_update.command_to_perform_system_update = v;
            }
            Message::SysUpdateNoRefreshCmd(v) => {
                self.config.system_update.command_to_perform_system_update_no_refresh = opt_str(v);
            }
            Message::SysUpdateIgnoreFlag(v) => self.config.system_update.ignore_flag = v,
            Message::RamdiskEnabled(v) => self.config.ramdisk.enabled = v,
            Message::RamdiskWorkdir(v) => self.config.ramdisk.build_workdir = v,
            Message::RamdiskChroot(v) => self.config.ramdisk.chroot = v,
            Message::RamdiskPackages(v) => self.config.ramdisk.packages = v,
            Message::RamdiskSize(v) => self.config.ramdisk.size = v,
            Message::RamdiskMode(v) => self.config.ramdisk.mode = v,
            Message::RamdiskMountPoint(v) => self.config.ramdisk.mount_point = v,
            Message::RamdiskSeedChroot(v) => self.config.ramdisk.seed_chroot_from = opt_str(v),
            Message::RamdiskSyncOnExit(v) => self.config.ramdisk.sync_chroot_on_exit = v,
            Message::RamdiskMinFreeRam(v) => {
                if let Ok(n) = v.parse() {
                    self.config.ramdisk.min_free_ram_mb = n;
                }
            }
            Message::RamdiskWarnPackages(v) => self.config.ramdisk.warn_packages_ram = v,
            Message::RamdiskReclaimOnStartup(v) => {
                self.config.ramdisk.reclaim_mount_on_startup = v;
            }
            Message::RepoUrlChanged(name, url) => {
                self.config.repositories.insert(name, url);
            }
            Message::RepoAdd => {
                let mut i = 1;
                loop {
                    let key = format!("repo-{i}");
                    if let std::collections::hash_map::Entry::Vacant(e) =
                        self.config.repositories.entry(key)
                    {
                        e.insert(String::new());
                        break;
                    }
                    i += 1;
                }
            }
            Message::RepoRemove(name) => {
                self.config.repositories.remove(&name);
            }
            Message::CompilerCcChanged(name, cc) => {
                if let Some(c) = self.config.compilers.get_mut(&name) {
                    c.cc = cc;
                }
            }
            Message::CompilerCxxChanged(name, cxx) => {
                if let Some(c) = self.config.compilers.get_mut(&name) {
                    c.cxx = cxx;
                }
            }
            Message::CompilerAdd => {
                let mut i = 1;
                loop {
                    let key = format!("compiler-{i}");
                    if let std::collections::hash_map::Entry::Vacant(e) =
                        self.config.compilers.entry(key)
                    {
                        e.insert(crate::config::CompilerSection {
                            cc: "gcc".into(),
                            cxx: "g++".into(),
                        });
                        break;
                    }
                    i += 1;
                }
            }
            Message::CompilerRemove(name) => {
                self.config.compilers.remove(&name);
            }
            Message::BrowsePath(field, kind) => {
                let current = self.path_value(field);
                return Task::perform(
                    async move { dialog::pick_path(field, kind, &current) },
                    move |picked| Message::PathPicked(field, picked),
                );
            }
            Message::PathPicked(field, picked) => {
                if let Some(path) = picked {
                    self.apply_path(field, path);
                }
            }
            Message::SetKernelStr(target, field, value) => {
                if let Some(pkg) = self.target_pkg_mut(target) {
                    set_kstr(pkg, field, value);
                }
            }
            Message::SetKernelBool(target, field, value) => {
                if let Some(pkg) = self.target_pkg_mut(target) {
                    set_kbool(pkg, field, value);
                }
            }
            Message::SetPackageOptBool(target, field, value) => {
                if let Some(pkg) = self.target_pkg_mut(target) {
                    match field {
                        KOptBool::Tests => pkg.tests = value,
                        KOptBool::UpstreamPrereleases => pkg.upstream_prereleases = value,
                    }
                }
            }
            Message::PackageCompilationThreads(target, value) => {
                if let Some(pkg) = self.target_pkg_mut(target) {
                    pkg.compilation_threads = parse_opt_usize(&value);
                }
            }
            Message::PackageCompileAlone(target, value) => {
                if let Some(pkg) = self.target_pkg_mut(target) {
                    pkg.compile_alone = value;
                }
            }
            Message::PackageCompilationPriority(target, value) => {
                if let Some(pkg) = self.target_pkg_mut(target) {
                    if let Ok(n) = value.trim().parse::<usize>() {
                        pkg.compilation_priority = n.max(1);
                    }
                }
            }
            Message::SetRamdiskTarget(target, letter, enabled) => {
                if let Some(pkg) = self.target_pkg_mut(target) {
                    let current = kstr_value(pkg, KStr::Ramdisk);
                    let (mut w, mut c, mut p, mut r) = parse_ramdisk_flags(&current);
                    match letter {
                        RamdiskLetter::Workdir => w = enabled,
                        RamdiskLetter::Chroot => c = enabled,
                        RamdiskLetter::Packages => p = enabled,
                        RamdiskLetter::Profiles => r = enabled,
                    }
                    set_kstr(pkg, KStr::Ramdisk, encode_ramdisk_flags(w, c, p, r));
                    if w || c || p || r {
                        self.config.ramdisk.enabled = true;
                    }
                }
            }
            Message::CustomKernelChanged(v) => self.custom_kernel = v,
            Message::RefreshPgoStatus => {
                if let Some(pkg) = self.selected_kernel.clone() {
                    return Task::perform(
                        async move { fetch_pgo_status(&pkg) },
                        Message::PgoStatusLoaded,
                    );
                }
            }
            Message::PgoStatusLoaded(Ok(status)) => {
                // For builds running in their own terminal window there is no in-app stream to tell
                // us the abs process exited. Detect it via the pipeline reaching a stage where abs
                // parks and returns control (a reboot gate, completion, or abort). A short grace
                // period avoids reacting to stale state from a just-finished previous run.
                if self.busy
                    && self
                        .external_run_since
                        .map(|t| t.elapsed() >= std::time::Duration::from_secs(6))
                        .unwrap_or(false)
                    && matches!(
                        status.stage.as_str(),
                        "wait_reboot1" | "wait_reboot2" | "stage2_build" | "stage3_build"
                            | "done" | "aborted"
                    )
                {
                    self.busy = false;
                    self.external_run_since = None;
                    self.external_pid_path = None;
                    self.append_log(format!(
                        "abs (terminal) reached: {}. {}",
                        status.stage_label, status.next_action
                    ));
                }
                self.sync_pgo_selected_stage_from_status(&status);
                self.pgo_status = Some(status);
                self.pgo_status_error = None;
            }
            Message::PgoStatusLoaded(Err(e)) => {
                self.pgo_status = None;
                self.pgo_status_error = Some(e);
            }
            Message::PgoSelectStage(stage) => {
                if self.busy {
                    return self.push_log(
                        "PGO is running — wait for it to finish before changing phase.",
                    );
                }
                if is_valid_pgo_phase_key(&stage) {
                    self.pgo_selected_stage = stage;
                }
            }
            Message::PgoRestartFromScratch => {
                return self.launch_pgo_run(
                    PgoAction::Restart,
                    None,
                    false,
                    &format!(
                        "Starting PGO from scratch for {}…",
                        self.selected_kernel.as_deref().unwrap_or("kernel")
                    ),
                );
            }
            Message::PgoStartFromPhase => {
                let selected = self.pgo_selected_stage.clone();
                let saved = self.effective_pgo_stage();
                let stage_arg = pgo_resume_stage_arg(&selected, saved);
                let label = if stage_arg.is_some() {
                    pgo_stage_label(&selected)
                } else {
                    pgo_stage_label(pgo_next_phase_after_wait(saved).unwrap_or("stage2_profile"))
                };
                return self.launch_pgo_run(
                    PgoAction::Resume,
                    stage_arg,
                    true,
                    &format!(
                        "PGO phase {label} for {}…",
                        self.selected_kernel.as_deref().unwrap_or("kernel")
                    ),
                );
            }
            Message::PgoContinueAfterReboot => {
                return self.launch_pgo_run(
                    PgoAction::Resume,
                    None,
                    true,
                    &format!(
                        "Continuing PGO after reboot for {}…",
                        self.selected_kernel.as_deref().unwrap_or("kernel")
                    ),
                );
            }
            Message::KernelBuildStart => {
                let Some(pkg) = self.selected_kernel.clone() else {
                    return Task::none();
                };
                if self.busy {
                    return self
                        .push_log("A build is already running — wait for it to finish or click Abort.");
                }
                self.list_editors.apply_all(&mut self.config);
                if !self.config.packages.contains_key(&pkg) {
                    let msg = format!(
                        "{pkg} is not saved in abs.toml yet — configure fields below and click Save config"
                    );
                    self.status_message = Some(msg.clone());
                    return self.push_log(format!("Cannot build: {msg}"));
                }
                if let Err(e) = abs_runner::verify_abs_binary() {
                    self.status_message = Some(e.clone());
                    return self.push_log(format!("Cannot build: {e}"));
                }
                self.busy = true;
                self.building_oneshot = true;
                self.pgo_run.reset();
                self.log_follow_tail = true;
                self.last_event_log_path = None;
                self.status_message = Some(format!("Building {pkg} (no PGO)…"));
                self.append_log(format!("One-shot kernel build for {pkg} (no PGO)…"));
                let path = self.config_path.clone();
                let doc = self.config.clone();
                if let Err(e) = save_config(&path, &doc) {
                    self.busy = false;
                    self.building_oneshot = false;
                    self.status_message = Some(e.clone());
                    return self.push_log(format!("Cannot build: failed to save config: {e}"));
                }
                self.append_log(format!("Saved {}", path.display()));
                let abs_cmd =
                    abs_runner::format_abs_pgo_command(PgoAction::KernelBuild, &pkg, None, None, false, false);
                self.append_log(format!("$ {abs_cmd}"));
                match abs_runner::launch_in_terminal(&abs_cmd, None) {
                    Ok(term) => {
                        // The build is interactive in its own window; nothing to track in-app.
                        self.busy = false;
                        self.building_oneshot = false;
                        self.append_log(format!(
                            "Launched abs in a new {term} window. Enter your sudo password and watch \
                             the build there. Press Ctrl+C in that window to stop it."
                        ));
                        return self.scroll_log_if_following();
                    }
                    Err(e) => {
                        self.append_log(format!(
                            "Could not open a terminal window ({e}). Running in-app instead."
                        ));
                        let handle = self.pgo_run.clone();
                        return Task::batch([
                            self.scroll_log_if_following(),
                            Task::stream(
                                stream_abs_pgo(PgoAction::KernelBuild, pkg, None, None, false, false, handle).map(
                                    |item| match item {
                                        AbsPgoStreamItem::Line(line) => Message::PgoLogLine(line),
                                        AbsPgoStreamItem::Finished(result) => {
                                            Message::PgoRunFinished(result)
                                        }
                                    },
                                ),
                            ),
                        ]);
                    }
                }
            }
            Message::PgoAbort => {
                let Some(pkg) = self.selected_kernel.clone() else {
                    return Task::none();
                };
                if !self.busy && self.external_run_since.is_none() {
                    return self.push_log("No PGO run is active.");
                }
                self.status_message = Some(format!("Aborting build for {pkg}…"));
                let handle = self.pgo_run.clone();
                let run_pgo_abort = !self.building_oneshot;
                let pid_path = self.external_pid_path.clone();
                handle.stop_running_build(pid_path.as_deref());
                let scroll = self.push_log(format!(
                    "Aborting build for {pkg} — sent stop signal to abs; running cleanup…"
                ));
                let abs_bin = abs_runner::abs_binary();
                let mut abort_tasks = vec![scroll];
                if run_pgo_abort {
                    abort_tasks.push(self.push_log(format!("$ {abs_bin} --pgo-abort {pkg}")));
                }
                abort_tasks.push(self.push_log(format!("$ {abs_bin} --ramdisk-shutdown")));
                abort_tasks.push(Task::perform(
                    async move { handle.abort(&pkg, run_pgo_abort, pid_path.as_deref()) },
                    Message::PgoAbortFinished,
                ));
                return Task::batch(abort_tasks);
            }
            Message::PgoLogLine(line) => {
                return self.push_log(line);
            }
            Message::PgoRunFinished(Ok(output)) => {
                self.busy = false;
                self.external_run_since = None;
                self.external_pid_path = None;
                let was_oneshot = self.building_oneshot;
                self.building_oneshot = false;
                let label = if was_oneshot { "Kernel build" } else { "PGO" };
                if output.user_aborted {
                    if was_oneshot {
                        self.status_message = Some("Build aborted.".into());
                        return Task::none();
                    }
                    return Task::done(Message::RefreshPgoStatus);
                } else if !output.success {
                    let code = output.exit_code.unwrap_or(-1);
                    let event_hint = output
                        .event_log
                        .as_ref()
                        .map(|p| format!(" Event log: {}", p.display()))
                        .unwrap_or_default();
                    self.status_message = Some(format!(
                        "{label} failed (exit {code}) — details are in the Build log panel below"
                    ));
                    return self.push_log(format!(
                        "--- abs failed with exit code {code}.{event_hint} Scroll to lines marked [stderr] for the error."
                    ));
                } else {
                    self.status_message = Some(format!("{label} finished successfully."));
                }
                if was_oneshot {
                    return Task::none();
                }
                return Task::done(Message::RefreshPgoStatus);
            }
            Message::PgoRunFinished(Err(e)) => {
                self.busy = false;
                self.building_oneshot = false;
                self.external_run_since = None;
                self.external_pid_path = None;
                self.status_message = Some(format!("Build error: {e}"));
                return self.push_log(format!("Error: {e}"));
            }
            Message::PgoAbortFinished(Ok(msg)) => {
                self.busy = false;
                self.building_oneshot = false;
                self.external_run_since = None;
                self.external_pid_path = None;
                if !msg.trim().is_empty() {
                    self.append_log(msg.trim().to_string());
                }
                self.append_log(
                    "Stopped. Pipeline state is preserved — use Resume or a stage button to continue.",
                );
                self.status_message = Some("Aborted.".into());
                return Task::batch([
                    self.scroll_log_if_following(),
                    Task::done(Message::RefreshPgoStatus),
                ]);
            }
            Message::PgoAbortFinished(Err(e)) => {
                self.busy = false;
                self.building_oneshot = false;
                self.external_run_since = None;
                self.external_pid_path = None;
                self.status_message = Some(format!("Abort failed: {e}"));
                return self.push_log(format!("Abort failed: {e}"));
            }
            Message::LogClear => {
                self.log_lines.clear();
                self.log_content = text_editor::Content::new();
                self.log_follow_tail = true;
            }
            Message::LogCopy => {
                return clipboard::write(self.log_lines.join("\n"));
            }
            Message::LogEdited(action) => {
                if matches!(&action, text_editor::Action::Scroll { .. }) {
                    self.log_follow_tail = false;
                }
                if !action.is_edit() {
                    self.log_content.perform(action);
                }
            }
            Message::LogFollowTail => {
                self.log_follow_tail = true;
                self.sync_log_content_from_lines();
                self.log_content
                    .perform(text_editor::Action::Move(text_editor::Motion::DocumentEnd));
            }
            Message::WindowResized(size) => {
                self.gui_settings.set_size(size);
            }
            Message::WindowMoved(point) => {
                self.gui_settings.set_position(point);
            }
            Message::WindowCloseRequested => {
                let _ = self.gui_settings.save();
                if self.external_run_since.is_some() {
                    // A build is running in its own terminal window; leave it (and its ramdisk)
                    // alone so closing the GUI doesn't kill an in-progress kernel compile.
                    return Task::done(Message::ExitAfterCleanup);
                }
                let pkg = self.selected_kernel.clone();
                let handle = self.pgo_run.clone();
                let busy = self.busy;
                let run_pgo_abort = !self.building_oneshot;
                let pid_path = self.external_pid_path.clone();
                return Task::perform(
                    async move {
                        if busy {
                            if let Some(p) = pkg {
                                let _ = handle.abort(&p, run_pgo_abort, pid_path.as_deref());
                            }
                        }
                        let _ = run_ramdisk_shutdown();
                    },
                    |_| Message::ExitAfterCleanup,
                );
            }
            Message::ExitAfterCleanup => {
                return iced::exit();
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let theme = self.app_theme();
        let content: Element<Message> = match self.page {
            Page::Kernels => self.view_kernels(theme),
            Page::DefaultKernelConfig => self.view_default_config(theme),
            Page::KernelConfig => self.view_kernel_config(theme),
            Page::Packages => self.view_packages(theme),
            Page::PackageConfig => self.view_package_config(theme),
            Page::AbsSettings => abs_settings::view(&self.config, &self.list_editors, theme),
            Page::AppSettings => self.view_app_settings(theme),
        };

        // Cap the form width so fields stay readable on wide/maximized windows.
        let body = row![
            self.view_sidebar(theme),
            scrollable(
                container(container(content).max_width(940).padding(24))
                    .width(Length::Fill)
                    .align_x(Alignment::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill),
        ]
        .height(Length::Fill);

        column![body, self.view_status_bar(theme)].into()
    }

    fn view_sidebar(&self, theme: AppTheme) -> Element<'_, Message> {
        let kernels_active =
            matches!(self.page, Page::Kernels | Page::KernelConfig | Page::DefaultKernelConfig);
        let nav = column![
            row![
                image(image::Handle::from_bytes(
                    bytes::Bytes::from_static(include_bytes!("../assets/icon.png")),
                ))
                .width(Length::Fixed(36.0))
                .height(Length::Fixed(36.0)),
                column![
                    text("ABS").size(24).color(style::iced_theme(theme).palette().primary),
                    text("kernel manager").size(11).color(style::muted(theme)),
                ],
            ]
            .spacing(10)
            .align_y(Alignment::Center),
            Space::new().height(Length::Fixed(12.0)),
            rule::horizontal(1),
            Space::new().height(Length::Fixed(12.0)),
            nav_btn("Kernels", kernels_active, theme, Message::OpenKernels),
            nav_btn(
                "Packages",
                matches!(self.page, Page::Packages | Page::PackageConfig),
                theme,
                Message::OpenPackages,
            ),
            nav_btn(
                "ABS settings",
                self.page == Page::AbsSettings,
                theme,
                Message::OpenAbsSettings,
            ),
            nav_btn(
                "App settings",
                self.page == Page::AppSettings,
                theme,
                Message::OpenAppSettings,
            ),
            Space::new().height(Length::Fill),
            text(concat!("absgui ", env!("CARGO_PKG_VERSION")))
                .size(11)
                .color(style::muted(theme)),
        ]
        .spacing(6)
        .padding(16)
        .height(Length::Fill);

        container(nav)
            .width(Length::Fixed(200.0))
            .height(Length::Fill)
            .style(style::sidebar(theme))
            .into()
    }

    fn view_status_bar(&self, theme: AppTheme) -> Element<'_, Message> {
        let palette = style::iced_theme(theme).palette();
        let left = if let Some(ref msg) = self.status_message {
            let lower = msg.to_ascii_lowercase();
            let is_error = ["fail", "error", "abort"]
                .iter()
                .any(|needle| lower.contains(needle));
            text(msg.clone()).size(12).color(if is_error {
                palette.danger
            } else {
                palette.primary
            })
        } else if let Some(ref err) = self.config_error {
            text(format!("Config error: {err}"))
                .size(12)
                .color(palette.danger)
        } else {
            text(self.config_path.display().to_string())
                .size(12)
                .color(style::muted(theme))
        };
        let right = if self.busy {
            text("running abs…").size(12).color(palette.primary)
        } else {
            text("").size(12)
        };
        container(
            row![left, Space::new().width(Length::Fill), right]
                .align_y(Alignment::Center)
                .padding(Padding::from([6.0, 16.0])),
        )
        .style(style::sidebar(theme))
        .width(Length::Fill)
        .into()
    }

    fn view_kernels(&self, theme: AppTheme) -> Element<'_, Message> {
        let mut col = column![page_title("Kernels", theme)].spacing(14);

        let default_card = card_section(
            "Default kernel configuration",
            theme,
            row![
                text("New kernels inherit these values the first time you configure them.")
                    .size(12)
                    .color(style::muted(theme))
                    .width(Length::Fill),
                button(text("Edit defaults").size(14))
                    .style(button::secondary)
                    .on_press(Message::OpenDefaultConfig),
            ]
            .align_y(Alignment::Center),
        );
        col = col.push(default_card);

        for (name, sched, desc) in style::KERNEL_CATALOG {
            let configured = self.config.packages.contains_key(*name);
            let status = if configured {
                container(text("Configured").size(11))
                    .padding(pill_padding())
                    .style(style::tag(theme))
            } else {
                container(text("Not configured").size(11))
                    .padding(pill_padding())
                    .style(style::tag_muted(theme))
            };
            col = col.push(card_section(
                name,
                theme,
                row![
                    column![
                        text(*desc).size(12).color(style::muted(theme)),
                    ]
                    .width(Length::Fill),
                    container(text(*sched).size(11))
                        .padding(pill_padding())
                        .style(style::tag(theme)),
                    status,
                    button(text("Configure").size(14))
                        .style(button::primary)
                        .on_press(Message::OpenKernel(name.to_string())),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            ));
        }

        let custom_name = self.custom_kernel.trim().to_string();
        let custom_btn = button(text("Configure custom").size(14)).style(button::secondary);
        let custom_btn = if custom_name.is_empty() {
            custom_btn
        } else {
            custom_btn.on_press(Message::OpenKernel(custom_name))
        };
        col = col.push(card_section(
            "Custom kernel package",
            theme,
            row![
                text_input("e.g. linux-custom-cachy", &self.custom_kernel)
                    .on_input(Message::CustomKernelChanged)
                    .padding(8)
                    .width(Length::Fill),
                custom_btn,
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        ));

        col.into()
    }

    fn view_default_config(&self, theme: AppTheme) -> Element<'_, Message> {
        let pkg = &self.config.kernel_defaults;
        column![
            row![
                button(text("← Kernels").size(14))
                    .style(button::secondary)
                    .on_press(Message::Back),
                page_title("Default kernel config", theme),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            text("These values seed a kernel the first time you configure it. Per-kernel edits stay independent.")
                .size(12)
                .color(style::muted(theme)),
            kernel_form(EditTarget::Default, pkg, theme),
            button(text("Save").size(14))
                .style(button::primary)
                .on_press(Message::SaveConfig),
        ]
        .spacing(16)
        .into()
    }

    fn view_kernel_config(&self, theme: AppTheme) -> Element<'_, Message> {
        let name = self
            .selected_kernel
            .clone()
            .unwrap_or_else(|| "—".to_string());
        let sched = style::KERNEL_CATALOG
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, s, _)| *s)
            .unwrap_or("custom");
        let Some(pkg) = self.target_pkg(EditTarget::Selected) else {
            return text("No kernel selected.").into();
        };

        column![
            row![
                button(text("← Kernels").size(14))
                    .style(button::secondary)
                    .on_press(Message::Back),
                text(name.clone()).size(22).width(Length::Fill),
                container(text(sched).size(11))
                    .padding(pill_padding())
                    .style(style::tag(theme)),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            kernel_form(EditTarget::Selected, pkg, theme),
            self.view_oneshot_build(theme),
            self.view_pgo_pipeline(theme),
            self.view_log(theme),
            button(text("Save config").size(14))
                .style(button::primary)
                .on_press(Message::SaveConfig),
        ]
        .spacing(16)
        .into()
    }

    fn view_packages(&self, theme: AppTheme) -> Element<'_, Message> {
        let mut col = column![
            page_title("Packages", theme),
            text(
                "Per-package overrides from [packages.*] in abs.toml. Any package ABS builds can \
                 be configured here — kernels too, but their kernel/PGO options live on the \
                 Kernels page. Changes are written when you click Save config."
            )
            .size(12)
            .color(style::muted(theme)),
        ]
        .spacing(14);

        let add_name = self.new_package_name.trim().to_string();
        let add_btn = button(text("+ Add package").size(14)).style(button::primary);
        let add_btn = if add_name.is_empty() {
            add_btn
        } else {
            add_btn.on_press(Message::PackageAdd)
        };
        col = col.push(card_section(
            "Add package configuration",
            theme,
            row![
                text_input("e.g. firefox, qemu-desktop, paru", &self.new_package_name)
                    .on_input(Message::NewPackageNameChanged)
                    .on_submit(Message::PackageAdd)
                    .padding(8)
                    .width(Length::Fill),
                add_btn,
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        ));

        let mut names: Vec<_> = self.config.packages.keys().cloned().collect();
        names.sort();
        if names.is_empty() {
            col = col.push(card_section(
                "Configured packages",
                theme,
                text("No per-package configuration yet — add one above.")
                    .size(12)
                    .color(style::muted(theme)),
            ));
            return col.into();
        }

        let mut rows = column![].spacing(8);
        for name in names {
            let pkg = &self.config.packages[&name];
            let mut tags: Vec<String> = Vec::new();
            if let Some(src) = &pkg.source {
                tags.push(src.clone());
            }
            if let Some(env) = &pkg.build_env {
                tags.push(env.clone());
            }
            if let Some(n) = pkg.compilation_threads {
                tags.push(format!("-j{n}"));
            }
            if pkg.compile_alone {
                tags.push("alone".into());
            }
            let is_kernel = pkg.kernel.is_some() || pkg.pgo.is_some();
            let mut item = row![text(name.clone()).size(14).width(Length::Fill)]
                .spacing(8)
                .align_y(Alignment::Center);
            if !tags.is_empty() {
                item = item.push(
                    container(text(tags.join(" · ")).size(11))
                        .padding(pill_padding())
                        .style(style::tag(theme)),
                );
            }
            if is_kernel {
                item = item.push(
                    container(text("kernel").size(11))
                        .padding(pill_padding())
                        .style(style::tag_muted(theme)),
                );
            }
            item = item.push(
                button(text("Edit").size(13))
                    .style(button::secondary)
                    .on_press(Message::OpenPackage(name.clone())),
            );
            item = item.push(
                button(text("Remove").size(13))
                    .style(button::danger)
                    .on_press(Message::PackageRemove(name)),
            );
            rows = rows.push(item);
        }
        col = col.push(card_section("Configured packages", theme, rows));
        col.into()
    }

    fn view_package_config(&self, theme: AppTheme) -> Element<'_, Message> {
        let name = self
            .selected_package
            .clone()
            .unwrap_or_else(|| "—".to_string());
        let Some(pkg) = self.target_pkg(EditTarget::Package) else {
            return text("No package selected.").into();
        };
        let is_kernel = pkg.kernel.is_some() || pkg.pgo.is_some();

        let mut col = column![row![
            button(text("← Packages").size(14))
                .style(button::secondary)
                .on_press(Message::Back),
            text(name.clone()).size(22).width(Length::Fill),
        ]
        .spacing(12)
        .align_y(Alignment::Center),]
        .spacing(16);

        if is_kernel {
            col = col.push(
                text("This package also has kernel/PGO options — edit those on the Kernels page.")
                    .size(12)
                    .color(style::muted(theme)),
            );
        }

        col = col.push(package_form(EditTarget::Package, pkg, theme));
        col = col.push(
            row![
                button(text("Save config").size(14))
                    .style(button::primary)
                    .on_press(Message::SaveConfig),
                button(text("Remove package").size(14))
                    .style(button::danger)
                    .on_press(Message::PackageRemove(name)),
            ]
            .spacing(8),
        );
        col.into()
    }

    fn view_app_settings(&self, theme: AppTheme) -> Element<'_, Message> {
        let current = match self.gui_settings.theme {
            AppTheme::Dark => "Dark",
            AppTheme::Light => "Light",
        };
        column![
            page_title("App settings", theme),
            card_section(
                "About",
                theme,
                column![
                    text(format!(
                        "absgui {} ({})",
                        env!("CARGO_PKG_VERSION"),
                        env!("ABSGUI_BUILD_ID")
                    ))
                    .size(14),
                    text(format!(
                        "Running: {}",
                        std::env::current_exe()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|_| "?".into())
                    ))
                    .size(12)
                    .color(style::muted(theme)),
                    text(format!(
                        "abs binary: {} (set ABS_BINARY to override)",
                        crate::abs_runner::abs_binary()
                    ))
                    .size(12)
                    .color(style::muted(theme)),
                    text(
                        "Kernel/PGO build output (including updpkgsums source prefetch at stage 1) \
                         appears in the terminal window opened by Start PGO — not in the build log below."
                    )
                    .size(12)
                    .color(style::muted(theme)),
                ]
                .spacing(8),
            ),
            card_section(
                "Appearance",
                theme,
                column![
                    field_pick(
                        "Theme",
                        None,
                        &["Dark", "Light"],
                        current,
                        theme,
                        |choice| Message::AppThemeSelected(if choice == "Light" {
                            AppTheme::Light
                        } else {
                            AppTheme::Dark
                        }),
                    ),
                    text("Window size and position are saved when you close the app.")
                        .size(12)
                        .color(style::muted(theme)),
                ]
                .spacing(10),
            ),
            button(text("Save app settings").size(14))
                .style(button::primary)
                .on_press(Message::SaveAppSettings),
        ]
        .spacing(16)
        .into()
    }

    fn view_oneshot_build(&self, theme: AppTheme) -> Element<'_, Message> {
        card_section(
            "One-shot build (no PGO)",
            theme,
            column![
                text(
                    "Compile and install this kernel once using the options above \
                     (scheduler, compiler/LTO, tick rate, etc.). No profiling or reboots. \
                     Opens in a new terminal window for sudo input and full output."
                )
                .size(12)
                .color(style::muted(theme)),
                row![
                    button(text("Build now").size(14))
                        .style(button::primary)
                        .on_press_maybe((!self.busy).then_some(Message::KernelBuildStart)),
                    button(text("Abort").size(14))
                        .style(button::danger)
                        .on_press_maybe(self.busy.then_some(Message::PgoAbort)),
                ]
                .spacing(8),
            ]
            .spacing(12),
        )
    }

    fn view_pgo_pipeline(&self, theme: AppTheme) -> Element<'_, Message> {
        let selected = self.pgo_selected_stage.as_str();
        let saved = self.effective_pgo_stage();
        let saved_idx = pgo_stage_index(saved);
        let at_wait_reboot = pgo_saved_at_wait_reboot(saved);
        let show_start_from_phase = !at_wait_reboot && self.pgo_selected_stage != pgo_first_phase_key();

        let mut timeline = row![].spacing(6);
        for (i, (label, key)) in PGO_STEPS.iter().copied().enumerate() {
            let is_selected = selected == key;
            let is_saved = saved == key
                || (saved == "done" && key == "stage3_build");
            let is_done = saved == "done" || saved_idx.is_some_and(|idx| i < idx);
            let pill_label = if is_done {
                format!("✓ {label}")
            } else if is_saved && !is_selected {
                format!("◦ {label}")
            } else if is_selected {
                format!("● {label}")
            } else {
                label.to_string()
            };
            let pill_text = text(pill_label).size(if is_selected { 12 } else { 11 });
            let pill: Element<'_, Message> = if !self.busy {
                button(pill_text)
                    .padding(pill_padding())
                    .style(if is_selected {
                        button::primary
                    } else {
                        button::secondary
                    })
                    .on_press(Message::PgoSelectStage(key.to_string()))
                    .into()
            } else if is_selected {
                container(pill_text)
                    .padding(pill_padding())
                    .style(style::pgo_stage_active(theme))
                    .into()
            } else if is_done {
                container(pill_text)
                    .padding(pill_padding())
                    .style(style::pgo_stage_done(theme))
                    .into()
            } else {
                container(pill_text)
                    .padding(pill_padding())
                    .style(style::tag_muted(theme))
                    .into()
            };
            timeline = timeline.push(pill);
        }

        let status_row: Element<'_, Message> = if let Some(ref s) = self.pgo_status {
            let mut detail = vec![format!(
                "selected: {}",
                pgo_stage_label(selected)
            )];
            if !s.stage.is_empty() && s.stage_label != "No pipeline" {
                detail.push(format!("saved: {}", s.stage_label));
            }
            if s.reboot_required {
                detail.push("Reboot required.".into());
            } else if s.boot_ready {
                detail.push("Boot verified — ready to continue.".into());
            }
            if let Some(ref uname) = s.expected_kernel_uname {
                detail.push(format!("expected kernel: {uname}"));
            }
            text(detail.join("  ·  "))
                .size(12)
                .color(style::muted(theme))
                .into()
        } else if self.busy {
            text("Running… — status updates every few seconds.")
                .size(12)
                .color(style::primary(theme))
                .into()
        } else if let Some(ref e) = self.pgo_status_error {
            text(format!("No saved pipeline ({e})."))
                .size(12)
                .color(style::muted(theme))
                .into()
        } else {
            text("No saved pipeline — choose a phase and use Start from scratch.")
                .size(12)
                .color(style::muted(theme))
                .into()
        };

        let mut action_row = row![
            button(text("Start from scratch").size(14))
                .style(button::primary)
                .on_press_maybe((!self.busy).then_some(Message::PgoRestartFromScratch)),
        ]
        .spacing(8);
        if at_wait_reboot {
            action_row = action_row.push(
                button(text("Continue after reboot").size(14))
                    .style(button::primary)
                    .on_press_maybe((!self.busy).then_some(Message::PgoContinueAfterReboot)),
            );
        }
        if show_start_from_phase {
            action_row = action_row.push(
                button(text("Start from current phase").size(14))
                    .style(button::secondary)
                    .on_press_maybe((!self.busy).then_some(Message::PgoStartFromPhase)),
            );
        }
        action_row = action_row.push(
            button(text("Abort").size(14))
                .style(button::danger)
                .on_press(Message::PgoAbort),
        );

        card_section(
            "PGO compilation",
            theme,
            column![
                text(
                    "Select a phase below (● = selected, ◦ = saved pipeline position). \
                     After rebooting, use Continue after reboot or select Profile AutoFDO and \
                     Start from current phase. Start from scratch clears saved state and runs stage 1."
                )
                .size(12)
                .color(style::muted(theme)),
                timeline,
                status_row,
                action_row,
            ]
            .spacing(12),
        )
    }

    fn view_log(&self, theme: AppTheme) -> Element<'_, Message> {
        let hint = if let Some(ref path) = self.last_event_log_path {
            format!(
                "Lines starting with $ are commands; everything below is their live output. \
                 Select text to copy, or use Copy all. JSON events: {}",
                path.display()
            )
        } else {
            "Lines starting with $ are commands; everything below is their live output. \
             Select text to copy, or use Copy all."
                .into()
        };
        let empty = self.log_lines.is_empty();
        let mut controls = row![
            button(text("Copy all").size(13))
                .style(button::secondary)
                .on_press(Message::LogCopy),
            button(text("Clear").size(13))
                .style(button::secondary)
                .on_press(Message::LogClear),
        ]
        .spacing(8);
        if !self.log_follow_tail && !empty {
            controls = controls.push(
                button(text("Follow output").size(13))
                    .style(button::secondary)
                    .on_press(Message::LogFollowTail),
            );
        }
        card_section(
            "Build log",
            theme,
            column![
                text(hint).size(11).color(style::log_hint(theme)),
                controls,
                container(
                    text_editor(&self.log_content)
                        .font(Font::MONOSPACE)
                        .size(14.0)
                        .placeholder(
                            "(no output yet — run Start PGO or Resume to capture abs output here)",
                        )
                        .height(Length::Fixed(260.0))
                        .padding(10)
                        .on_action(Message::LogEdited)
                        .style(style::log_editor(theme)),
                )
                .padding(4)
                .style(style::log_surface(theme))
                .height(Length::Fixed(280.0))
                .width(Length::Fill),
            ]
            .spacing(8),
        )
    }
}

fn kernel_form<'a>(
    target: EditTarget,
    pkg: &'a PackageSection,
    theme: AppTheme,
) -> Element<'a, Message> {
    let ramdisk_str = kstr_value(pkg, KStr::Ramdisk);
    let (ramdisk_w, ramdisk_c, ramdisk_p, ramdisk_r) = parse_ramdisk_flags(&ramdisk_str);
    let kernel = card_section(
        "Kernel options (CachyOS)",
        theme,
        column![
            row![
                field_pick(
                    "Scheduler (_cpusched)",
                    Some(field_help::CPUSCHED),
                    SCHED_OPTS,
                    &kstr_value(pkg, KStr::Cpusched),
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::Cpusched, v),
                ),
                field_pick(
                    "Processor opt (_processor_opt)",
                    Some(field_help::PROCESSOR_OPT),
                    &["native", "x86-64-v2", "x86-64-v3", "x86-64-v4"],
                    &kstr_value(pkg, KStr::ProcessorOpt),
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::ProcessorOpt, v),
                ),
            ]
            .spacing(12),
            row![
                field_pick(
                    "LLVM LTO (_use_llvm_lto)",
                    Some(field_help::LLVM_LTO),
                    LTO_OPTS,
                    &kstr_value(pkg, KStr::LlvmLto),
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::LlvmLto, v),
                ),
                field_pick(
                    "Tick rate Hz (_HZ_ticks)",
                    Some(field_help::HZ_TICKS),
                    HZ_OPTS,
                    &kstr_value(pkg, KStr::HzTicks),
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::HzTicks, v),
                ),
            ]
            .spacing(12),
            row![
                field_pick(
                    "Tickless (_tickrate)",
                    Some(field_help::TICKRATE),
                    TICK_OPTS,
                    &kstr_value(pkg, KStr::Tickrate),
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::Tickrate, v),
                ),
                field_pick(
                    "Preemption (_preempt)",
                    Some(field_help::PREEMPT),
                    PREEMPT_OPTS,
                    &kstr_value(pkg, KStr::Preempt),
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::Preempt, v),
                ),
            ]
            .spacing(12),
            field_pick(
                "Transparent hugepages (_hugepage)",
                Some(field_help::HUGEPAGE),
                HUGE_OPTS,
                &kstr_value(pkg, KStr::Hugepage),
                theme,
                move |v| Message::SetKernelStr(target, KStr::Hugepage, v),
            ),
            field_checkbox(
                "Harder compiler flags (_cc_harder)",
                Some(field_help::CC_HARDER),
                kbool_value(pkg, KBool::CcHarder),
                theme,
                move |v| Message::SetKernelBool(target, KBool::CcHarder, v),
            ),
            row![
                field_checkbox(
                    "-lto name suffix (_use_lto_suffix)",
                    Some(field_help::LTO_SUFFIX),
                    kbool_value(pkg, KBool::LtoSuffix),
                    theme,
                    move |v| Message::SetKernelBool(target, KBool::LtoSuffix, v),
                ),
                field_checkbox(
                    "-gcc name suffix (_use_gcc_suffix)",
                    Some(field_help::GCC_SUFFIX),
                    kbool_value(pkg, KBool::GccSuffix),
                    theme,
                    move |v| Message::SetKernelBool(target, KBool::GccSuffix, v),
                ),
                field_checkbox(
                    "Kernel CFI (_use_kcfi)",
                    Some(field_help::KCFI),
                    kbool_value(pkg, KBool::Kcfi),
                    theme,
                    move |v| Message::SetKernelBool(target, KBool::Kcfi, v),
                ),
            ]
            .spacing(16),
        ]
        .spacing(12),
    );

    let abs_card = card_section(
        "ABS build",
        theme,
        column![
            row![
                field_pick(
                    "Source repository",
                    Some(field_help::SOURCE),
                    SOURCE_OPTS,
                    &kstr_value(pkg, KStr::Source),
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::Source, v),
                ),
                field_pick(
                    "Build environment",
                    Some(field_help::BUILD_ENV),
                    ENV_OPTS,
                    &kstr_value(pkg, KStr::BuildEnv),
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::BuildEnv, v),
                ),
            ]
            .spacing(12),
            row![
                field_number(
                    "compilation_threads (optional)",
                    Some(field_help::PACKAGE_COMPILATION_THREADS),
                    &pkg
                        .compilation_threads
                        .map(|n| n.to_string())
                        .unwrap_or_default(),
                    theme,
                    move |v| Message::PackageCompilationThreads(target, v),
                ),
                field_number(
                    "compilation_priority",
                    Some(field_help::PACKAGE_COMPILATION_PRIORITY),
                    &pkg.compilation_priority.to_string(),
                    theme,
                    move |v| Message::PackageCompilationPriority(target, v),
                ),
            ]
            .spacing(12),
            field_checkbox(
                "compile_alone",
                Some(field_help::PACKAGE_COMPILE_ALONE),
                pkg.compile_alone,
                theme,
                move |v| Message::PackageCompileAlone(target, v),
            ),
            kernel_ramdisk_targets_field(target, ramdisk_w, ramdisk_c, ramdisk_p, ramdisk_r, theme),
        ]
        .spacing(12),
    );

    let benchmark_preset = {
        let v = kstr_value(pkg, KStr::BenchmarkPreset);
        if v.is_empty() {
            "fast".to_string()
        } else {
            v
        }
    };

    let profiling_quality = {
        let v = kstr_value(pkg, KStr::ProfilingQuality);
        if v.is_empty() {
            "maximum".to_string()
        } else {
            v
        }
    };

    let pgo = card_section(
        "PGO (AutoFDO + Propeller)",
        theme,
        column![
            row![
                field_checkbox(
                    "Enable multi-stage PGO",
                    Some(field_help::PGO_ENABLED),
                    kbool_value(pkg, KBool::PgoEnabled),
                    theme,
                    move |v| Message::SetKernelBool(target, KBool::PgoEnabled, v),
                ),
                field_checkbox(
                    "Auto-restart pipeline",
                    Some(field_help::PGO_AUTO_RESTART),
                    kbool_value(pkg, KBool::PgoAutoRestart),
                    theme,
                    move |v| Message::SetKernelBool(target, KBool::PgoAutoRestart, v),
                ),
            ]
            .spacing(16),
            row![
                field_checkbox(
                    "Verify boot after reboot",
                    Some(field_help::PGO_VERIFY_BOOT),
                    kbool_value(pkg, KBool::PgoVerifyBoot),
                    theme,
                    move |v| Message::SetKernelBool(target, KBool::PgoVerifyBoot, v),
                ),
                field_checkbox(
                    "Perf data on ramdisk",
                    Some(field_help::PGO_PERF_DATA_ON_RAM),
                    kbool_value(pkg, KBool::PgoPerfDataOnRam),
                    theme,
                    move |v| Message::SetKernelBool(target, KBool::PgoPerfDataOnRam, v),
                ),
            ]
            .spacing(16),
            field_text(
                "Pipeline preset",
                Some(field_help::PGO_PRESET),
                &kstr_value(pkg, KStr::PgoPreset),
                "cachyos-kernel",
                theme,
                move |v| Message::SetKernelStr(target, KStr::PgoPreset, v),
            ),
            field_path(
                "Profiles archive dir (required)",
                Some(field_help::PGO_ARCHIVE_DIR),
                &kstr_value(pkg, KStr::ArchiveDir),
                "/mnt/hdd/abs/pgo/profiles",
                WPathField::PgoArchiveDir,
                WPathKind::Folder,
                theme,
                move |v| Message::SetKernelStr(target, KStr::ArchiveDir, v),
            ),
            field_path(
                "Profile scratch dir",
                Some(field_help::PGO_PROFILE_SCRATCH),
                &kstr_value(pkg, KStr::ProfileScratchDir),
                "auto",
                WPathField::PgoProfileScratchDir,
                WPathKind::Folder,
                theme,
                move |v| Message::SetKernelStr(target, KStr::ProfileScratchDir, v),
            ),
            field_path(
                "PGO state file (optional)",
                Some(field_help::PGO_STATE_FILE),
                &kstr_value(pkg, KStr::StateFile),
                "(default: ~/.config/abs/pgo/PKG.json)",
                WPathField::PgoStateFile,
                WPathKind::File,
                theme,
                move |v| Message::SetKernelStr(target, KStr::StateFile, v),
            ),
            field_pick(
                "Profiling quality",
                Some(field_help::PGO_PROFILING_QUALITY),
                PGO_PROFILING_QUALITY_OPTS,
                &profiling_quality,
                theme,
                move |v| Message::SetKernelStr(target, KStr::ProfilingQuality, v),
            ),
            field_pick(
                "Benchmark preset",
                Some(field_help::PGO_BENCHMARK_PRESET),
                PGO_BENCHMARK_PRESET_OPTS,
                &benchmark_preset,
                theme,
                move |v| Message::SetKernelStr(target, KStr::BenchmarkPreset, v),
            ),
            field_path(
                "Benchmark command (optional)",
                Some(field_help::PGO_BENCHMARK),
                &kstr_value(pkg, KStr::Benchmark),
                "(bundled ABS benchmark if empty)",
                WPathField::PgoBenchmark,
                WPathKind::File,
                theme,
                move |v| Message::SetKernelStr(target, KStr::Benchmark, v),
            ),
            field_path(
                "Benchmark asset cache (optional)",
                Some(field_help::PGO_BENCHMARK_WORKDIR),
                &kstr_value(pkg, KStr::BenchmarkWorkdir),
                "(default: archive dir/benchmark-workdir)",
                WPathField::PgoBenchmarkWorkdir,
                WPathKind::Folder,
                theme,
                move |v| Message::SetKernelStr(target, KStr::BenchmarkWorkdir, v),
            ),
            row![
                field_text(
                    "Build user",
                    Some(field_help::PGO_BUILD_USER),
                    &kstr_value(pkg, KStr::BuildUser),
                    "john",
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::BuildUser, v),
                ),
                field_text(
                    "Sysctl command",
                    Some(field_help::PGO_SYSCTL),
                    &kstr_value(pkg, KStr::SysctlCommand),
                    "cachyos-perf-sysctl",
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::SysctlCommand, v),
                ),
            ]
            .spacing(12),
            field_text(
                "perf event args",
                Some(field_help::PGO_PERF_EVENT_ARGS),
                &kstr_value(pkg, KStr::PerfEventArgs),
                "auto",
                theme,
                move |v| Message::SetKernelStr(target, KStr::PerfEventArgs, v),
            ),
            field_text(
                "perf extra args",
                Some(field_help::PGO_PERF_EXTRA_ARGS),
                &kstr_value(pkg, KStr::PerfExtraArgs),
                "--mmap-pages 131072 -a -N -b -c 56000",
                theme,
                move |v| Message::SetKernelStr(target, KStr::PerfExtraArgs, v),
            ),
            field_path(
                "vmlinux path",
                Some(field_help::PGO_VMLINUX),
                &kstr_value(pkg, KStr::Vmlinux),
                "auto",
                WPathField::PgoVmlinux,
                WPathKind::File,
                theme,
                move |v| Message::SetKernelStr(target, KStr::Vmlinux, v),
            ),
            row![
                field_text(
                    "AutoFDO tool",
                    Some(field_help::PGO_AFDO_TOOL),
                    &kstr_value(pkg, KStr::AfdoTool),
                    "llvm-profgen",
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::AfdoTool, v),
                ),
                field_text(
                    "Propeller tool",
                    Some(field_help::PGO_PROPELLER_TOOL),
                    &kstr_value(pkg, KStr::PropellerTool),
                    "create_llvm_prof",
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::PropellerTool, v),
                ),
            ]
            .spacing(12),
            field_text(
                "AutoFDO profile filename",
                Some(field_help::PGO_AFDO_PROFILE_NAME),
                &kstr_value(pkg, KStr::AfdoProfileName),
                "kernel-compilation.afdo",
                theme,
                move |v| Message::SetKernelStr(target, KStr::AfdoProfileName, v),
            ),
        ]
        .spacing(12),
    );

    column![kernel, abs_card, pgo].spacing(16).into()
}

/// Full per-package editor (everything `[packages.NAME]` supports except kernel/PGO tables).
fn package_form<'a>(
    target: EditTarget,
    pkg: &'a PackageSection,
    theme: AppTheme,
) -> Element<'a, Message> {
    let ramdisk_str = kstr_value(pkg, KStr::Ramdisk);
    let (ramdisk_w, ramdisk_c, ramdisk_p, ramdisk_r) = parse_ramdisk_flags(&ramdisk_str);

    let source_build = card_section(
        "Source & build",
        theme,
        column![
            row![
                field_pick(
                    "source",
                    Some(field_help::SOURCE),
                    SOURCE_OPTS,
                    &kstr_value(pkg, KStr::Source),
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::Source, v),
                ),
                field_pick(
                    "build_env",
                    Some(field_help::BUILD_ENV),
                    ENV_OPTS,
                    &kstr_value(pkg, KStr::BuildEnv),
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::BuildEnv, v),
                ),
            ]
            .spacing(12),
            row![
                field_text(
                    "compiler (optional)",
                    Some(field_help::PACKAGE_COMPILER),
                    &kstr_value(pkg, KStr::Compiler),
                    "gcc14",
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::Compiler, v),
                ),
                field_text(
                    "alias (optional)",
                    Some(field_help::PACKAGE_ALIAS),
                    &kstr_value(pkg, KStr::Alias),
                    "upstream-package-name",
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::Alias, v),
                ),
            ]
            .spacing(12),
            optional_bool_field(
                "tests",
                Some(field_help::PACKAGE_TESTS),
                pkg.tests,
                "makepkg default",
                theme,
                move |v| Message::SetPackageOptBool(target, KOptBool::Tests, v),
            ),
            ramdisk_targets_field(target, ramdisk_w, ramdisk_c, ramdisk_p, ramdisk_r, theme),
        ]
        .spacing(12),
    );

    let scheduling = card_section(
        "Compilation scheduling",
        theme,
        column![
            row![
                field_number(
                    "compilation_threads (optional)",
                    Some(field_help::PACKAGE_COMPILATION_THREADS),
                    &pkg
                        .compilation_threads
                        .map(|n| n.to_string())
                        .unwrap_or_default(),
                    theme,
                    move |v| Message::PackageCompilationThreads(target, v),
                ),
                field_number(
                    "compilation_priority",
                    Some(field_help::PACKAGE_COMPILATION_PRIORITY),
                    &pkg.compilation_priority.to_string(),
                    theme,
                    move |v| Message::PackageCompilationPriority(target, v),
                ),
            ]
            .spacing(12),
            field_checkbox(
                "compile_alone",
                Some(field_help::PACKAGE_COMPILE_ALONE),
                pkg.compile_alone,
                theme,
                move |v| Message::PackageCompileAlone(target, v),
            ),
        ]
        .spacing(12),
    );

    let commands = card_section(
        "Commands & hooks",
        theme,
        column![
            field_text(
                "custom_local_build_command (optional)",
                Some(field_help::PACKAGE_CUSTOM_LOCAL_CMD),
                &kstr_value(pkg, KStr::CustomLocalBuildCommand),
                "makepkg -si --noconfirm",
                theme,
                move |v| Message::SetKernelStr(target, KStr::CustomLocalBuildCommand, v),
            ),
            field_text(
                "custom_chroot_build_command (optional)",
                Some(field_help::PACKAGE_CUSTOM_CHROOT_CMD),
                &kstr_value(pkg, KStr::CustomChrootBuildCommand),
                "makechrootpkg -r /path/to/chroot",
                theme,
                move |v| Message::SetKernelStr(target, KStr::CustomChrootBuildCommand, v),
            ),
            field_text(
                "pre_update_command (optional)",
                Some(field_help::PACKAGE_PRE_UPDATE_CMD),
                &kstr_value(pkg, KStr::PreUpdateCommand),
                "systemctl stop myservice",
                theme,
                move |v| Message::SetKernelStr(target, KStr::PreUpdateCommand, v),
            ),
            field_text(
                "post_update_command (optional)",
                Some(field_help::PACKAGE_POST_UPDATE_CMD),
                &kstr_value(pkg, KStr::PostUpdateCommand),
                "systemctl restart myservice",
                theme,
                move |v| Message::SetKernelStr(target, KStr::PostUpdateCommand, v),
            ),
        ]
        .spacing(12),
    );

    let upstream = card_section(
        "Upstream version checks",
        theme,
        column![
            field_text(
                "upstream_github (optional)",
                Some(field_help::PACKAGE_UPSTREAM_GITHUB),
                &kstr_value(pkg, KStr::UpstreamGithub),
                "owner/repo",
                theme,
                move |v| Message::SetKernelStr(target, KStr::UpstreamGithub, v),
            ),
            optional_bool_field(
                "upstream_prereleases",
                Some(field_help::PACKAGE_UPSTREAM_PRERELEASES),
                pkg.upstream_prereleases,
                "false",
                theme,
                move |v| Message::SetPackageOptBool(target, KOptBool::UpstreamPrereleases, v),
            ),
        ]
        .spacing(12),
    );

    column![source_build, scheduling, commands, upstream]
        .spacing(16)
        .into()
}

fn nav_btn(
    label: &'static str,
    active: bool,
    theme: AppTheme,
    msg: Message,
) -> Element<'static, Message> {
    if active {
        button(text(label).size(14))
            .width(Length::Fill)
            .padding(10)
            .style(style::nav_active(theme))
            .on_press(msg)
            .into()
    } else {
        button(text(label).size(14))
            .width(Length::Fill)
            .padding(10)
            .style(style::nav_inactive(theme))
            .on_press(msg)
            .into()
    }
}

fn pill_padding() -> Padding {
    Padding {
        top: 3.0,
        right: 10.0,
        bottom: 3.0,
        left: 10.0,
    }
}

fn opt_str(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

fn parse_opt_usize(value: &str) -> Option<usize> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        trimmed.parse().ok()
    }
}

fn validate_pgo_start(section: &PackageSection, package: &str) -> Result<(), String> {
    let pgo = section
        .pgo
        .as_ref()
        .filter(|p| p.enabled)
        .ok_or_else(|| {
            format!(
                "PGO is disabled for {package}. Enable «Enable multi-stage PGO» in the PGO section below."
            )
        })?;
    if pgo
        .profiles_archive_dir
        .as_ref()
        .is_none_or(|s| s.trim().is_empty())
    {
        return Err(format!(
            "Set «Profiles archive dir (required)» in the PGO section for {package}."
        ));
    }
    Ok(())
}

fn kstr_value(pkg: &PackageSection, field: KStr) -> String {
    let kernel = pkg.kernel.as_ref();
    let pgo = pkg.pgo.as_ref();
    match field {
        KStr::Source => pkg.source.clone(),
        KStr::BuildEnv => pkg.build_env.clone(),
        KStr::Ramdisk => pkg.ramdisk.clone(),
        KStr::Alias => pkg.alias.clone(),
        KStr::Compiler => pkg.compiler.clone(),
        KStr::UpstreamGithub => pkg.upstream_github.clone(),
        KStr::PreUpdateCommand => pkg.pre_update_command.clone(),
        KStr::PostUpdateCommand => pkg.post_update_command.clone(),
        KStr::CustomLocalBuildCommand => pkg.custom_local_build_command.clone(),
        KStr::CustomChrootBuildCommand => pkg.custom_chroot_build_command.clone(),
        KStr::Cpusched => kernel.and_then(|k| k.cpusched.clone()),
        KStr::ProcessorOpt => kernel.and_then(|k| k.processor_opt.clone()),
        KStr::LlvmLto => kernel.and_then(|k| k.use_llvm_lto.clone()),
        KStr::HzTicks => kernel.and_then(|k| k.hz_ticks.clone()),
        KStr::Tickrate => kernel.and_then(|k| k.tickrate.clone()),
        KStr::Preempt => kernel.and_then(|k| k.preempt.clone()),
        KStr::Hugepage => kernel.and_then(|k| k.hugepage.clone()),
        KStr::ArchiveDir => pgo.and_then(|p| p.profiles_archive_dir.clone()),
        KStr::Benchmark => pgo.and_then(|p| p.benchmark_command.clone()),
        KStr::BenchmarkWorkdir => pgo.and_then(|p| p.benchmark_workdir.clone()),
        KStr::BenchmarkPreset => pgo.map(|p| p.benchmark_preset.clone()),
        KStr::ProfilingQuality => pgo.map(|p| p.profiling_quality.clone()),
        KStr::BuildUser => pgo.and_then(|p| p.build_user.clone()),
        KStr::SysctlCommand => pgo.and_then(|p| p.sysctl_command.clone()),
        KStr::PgoPreset => pgo.map(|p| p.preset.clone()),
        KStr::ProfileScratchDir => pgo.map(|p| p.profile_scratch_dir.clone()),
        KStr::PerfEventArgs => pgo.map(|p| p.perf_event_args.clone()),
        KStr::PerfExtraArgs => pgo.map(|p| p.perf_extra_args.clone()),
        KStr::Vmlinux => pgo.map(|p| p.vmlinux.clone()),
        KStr::AfdoTool => pgo.map(|p| p.afdo_tool.clone()),
        KStr::PropellerTool => pgo.map(|p| p.propeller_tool.clone()),
        KStr::AfdoProfileName => pgo.map(|p| p.afdo_profile_name.clone()),
        KStr::StateFile => pgo.and_then(|p| p.state_file.clone()),
    }
    .unwrap_or_default()
}

fn set_kstr(pkg: &mut PackageSection, field: KStr, value: String) {
    if matches!(field, KStr::BenchmarkPreset) {
        let pgo = pkg.pgo.get_or_insert_with(Default::default);
        pgo.benchmark_preset = if value.trim().is_empty() {
            "fast".into()
        } else {
            value.trim().to_string()
        };
        return;
    }
    if matches!(field, KStr::ProfilingQuality) {
        let pgo = pkg.pgo.get_or_insert_with(Default::default);
        pgo.profiling_quality = if value.trim().is_empty() {
            "maximum".into()
        } else {
            value.trim().to_string()
        };
        return;
    }
    if matches!(
        field,
        KStr::PgoPreset
            | KStr::ProfileScratchDir
            | KStr::PerfEventArgs
            | KStr::PerfExtraArgs
            | KStr::Vmlinux
            | KStr::AfdoTool
            | KStr::PropellerTool
            | KStr::AfdoProfileName
    ) {
        let pgo = pkg.pgo.get_or_insert_with(Default::default);
        let trimmed = value.trim();
        match field {
            KStr::PgoPreset => {
                pgo.preset = if trimmed.is_empty() {
                    "cachyos-kernel".into()
                } else {
                    trimmed.to_string()
                };
            }
            KStr::ProfileScratchDir => {
                pgo.profile_scratch_dir = if trimmed.is_empty() {
                    "auto".into()
                } else {
                    trimmed.to_string()
                };
            }
            KStr::PerfEventArgs => {
                pgo.perf_event_args = if trimmed.is_empty() {
                    "auto".into()
                } else {
                    trimmed.to_string()
                };
            }
            KStr::PerfExtraArgs => pgo.perf_extra_args = trimmed.to_string(),
            KStr::Vmlinux => {
                pgo.vmlinux = if trimmed.is_empty() {
                    "auto".into()
                } else {
                    trimmed.to_string()
                };
            }
            KStr::AfdoTool => {
                pgo.afdo_tool = if trimmed.is_empty() {
                    "llvm-profgen".into()
                } else {
                    trimmed.to_string()
                };
            }
            KStr::PropellerTool => {
                pgo.propeller_tool = if trimmed.is_empty() {
                    "create_llvm_prof".into()
                } else {
                    trimmed.to_string()
                };
            }
            KStr::AfdoProfileName => {
                pgo.afdo_profile_name = if trimmed.is_empty() {
                    "kernel-compilation.afdo".into()
                } else {
                    trimmed.to_string()
                };
            }
            _ => unreachable!(),
        }
        return;
    }
    let opt = opt_str(value);
    match field {
        KStr::Source => pkg.source = opt,
        KStr::BuildEnv => pkg.build_env = opt,
        KStr::Ramdisk => pkg.ramdisk = opt,
        KStr::Alias => pkg.alias = opt,
        KStr::Compiler => pkg.compiler = opt,
        KStr::UpstreamGithub => pkg.upstream_github = opt,
        KStr::PreUpdateCommand => pkg.pre_update_command = opt,
        KStr::PostUpdateCommand => pkg.post_update_command = opt,
        KStr::CustomLocalBuildCommand => pkg.custom_local_build_command = opt,
        KStr::CustomChrootBuildCommand => pkg.custom_chroot_build_command = opt,
        KStr::Cpusched => pkg.kernel.get_or_insert_with(Default::default).cpusched = opt,
        KStr::ProcessorOpt => pkg.kernel.get_or_insert_with(Default::default).processor_opt = opt,
        KStr::LlvmLto => pkg.kernel.get_or_insert_with(Default::default).use_llvm_lto = opt,
        KStr::HzTicks => pkg.kernel.get_or_insert_with(Default::default).hz_ticks = opt,
        KStr::Tickrate => pkg.kernel.get_or_insert_with(Default::default).tickrate = opt,
        KStr::Preempt => pkg.kernel.get_or_insert_with(Default::default).preempt = opt,
        KStr::Hugepage => pkg.kernel.get_or_insert_with(Default::default).hugepage = opt,
        KStr::ArchiveDir => {
            pkg.pgo.get_or_insert_with(Default::default).profiles_archive_dir = opt
        }
        KStr::Benchmark => pkg.pgo.get_or_insert_with(Default::default).benchmark_command = opt,
        KStr::BenchmarkWorkdir => pkg.pgo.get_or_insert_with(Default::default).benchmark_workdir = opt,
        KStr::BenchmarkPreset | KStr::ProfilingQuality => unreachable!("handled above"),
        KStr::BuildUser => pkg.pgo.get_or_insert_with(Default::default).build_user = opt,
        KStr::SysctlCommand => pkg.pgo.get_or_insert_with(Default::default).sysctl_command = opt,
        KStr::PgoPreset
        | KStr::ProfileScratchDir
        | KStr::PerfEventArgs
        | KStr::PerfExtraArgs
        | KStr::Vmlinux
        | KStr::AfdoTool
        | KStr::PropellerTool
        | KStr::AfdoProfileName => unreachable!("handled above"),
        KStr::StateFile => pkg.pgo.get_or_insert_with(Default::default).state_file = opt,
    }
}

fn kbool_value(pkg: &PackageSection, field: KBool) -> bool {
    match field {
        KBool::PgoEnabled => pkg.pgo.as_ref().map(|p| p.enabled).unwrap_or(true),
        KBool::PgoAutoRestart => pkg.pgo.as_ref().map(|p| p.auto_restart).unwrap_or(false),
        KBool::PgoPerfDataOnRam => pkg.pgo.as_ref().map(|p| p.perf_data_on_ram).unwrap_or(true),
        KBool::PgoVerifyBoot => pkg.pgo.as_ref().map(|p| p.verify_boot).unwrap_or(true),
        KBool::CcHarder => pkg
            .kernel
            .as_ref()
            .and_then(|k| k.cc_harder.as_deref())
            .is_some_and(is_truthy),
        KBool::LtoSuffix => pkg
            .kernel
            .as_ref()
            .and_then(|k| k.use_lto_suffix.as_deref())
            .is_some_and(is_truthy),
        KBool::GccSuffix => pkg
            .kernel
            .as_ref()
            .and_then(|k| k.use_gcc_suffix.as_deref())
            .is_some_and(is_truthy),
        KBool::Kcfi => pkg
            .kernel
            .as_ref()
            .and_then(|k| k.use_kcfi.as_deref())
            .is_some_and(is_truthy),
    }
}

fn set_kbool(pkg: &mut PackageSection, field: KBool, value: bool) {
    match field {
        KBool::PgoEnabled => pkg.pgo.get_or_insert_with(Default::default).enabled = value,
        KBool::PgoAutoRestart => {
            pkg.pgo.get_or_insert_with(Default::default).auto_restart = value
        }
        KBool::PgoPerfDataOnRam => {
            pkg.pgo.get_or_insert_with(Default::default).perf_data_on_ram = value
        }
        KBool::PgoVerifyBoot => {
            pkg.pgo.get_or_insert_with(Default::default).verify_boot = value
        }
        KBool::CcHarder => {
            pkg.kernel.get_or_insert_with(Default::default).cc_harder =
                if value { Some("y".into()) } else { None }
        }
        KBool::LtoSuffix => {
            pkg.kernel.get_or_insert_with(Default::default).use_lto_suffix =
                if value { Some("y".into()) } else { None }
        }
        KBool::GccSuffix => {
            pkg.kernel.get_or_insert_with(Default::default).use_gcc_suffix =
                if value { Some("y".into()) } else { None }
        }
        KBool::Kcfi => {
            pkg.kernel.get_or_insert_with(Default::default).use_kcfi =
                if value { Some("y".into()) } else { None }
        }
    }
}

fn is_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "y" | "yes" | "true" | "1"
    )
}

#[cfg(test)]
mod pgo_validation_tests {
    use super::validate_pgo_start;
    use crate::config::{PackageSection, PgoSection};

    #[test]
    fn rejects_missing_archive_dir() {
        let section = PackageSection {
            pgo: Some(PgoSection {
                enabled: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        let err = validate_pgo_start(&section, "linux-cachyos").unwrap_err();
        assert!(err.contains("Profiles archive dir"));
    }

    #[test]
    fn accepts_minimal_pgo_config() {
        let section = PackageSection {
            pgo: Some(PgoSection {
                enabled: true,
                profiles_archive_dir: Some("/tmp/pgo".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        validate_pgo_start(&section, "linux-cachyos").unwrap();
    }
}
