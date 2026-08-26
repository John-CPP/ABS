use crate::abs_runner::{
    self, fetch_pending_updates, fetch_pgo_status, run_ramdisk_shutdown, stream_abs_command,
    stream_abs_pgo, AbsPgoStreamItem, PendingUpdates, PgoAction, PgoRunHandle, PgoStatus,
};
use crate::app_settings::{
    clamp_terminal_lines_limit, clamp_window_geometry, load_rgba_png, load_window_icon, AppTheme,
    GuiSettings, ThemePref, TERMINAL_LINES_STEP,
};
use crate::config::{config_path, load_config, save_config, ConfigDocument, PackageSection};
use crate::dialog;
use crate::field_help;
use crate::list_editors::{self, ListEditors, PackageListField};
use crate::log_save::{
    self, apply_folder, format_from_path, remember_save_dir, replace_known_extension,
    suggested_save_path, ExpandCtx, LogSaveTarget, DEFAULT_FILENAME,
};
use crate::messages::{
    EditTarget, KBool, KOptBool, KStr, Message, PackageConfirm, PackageListFilter, PackageSortCol,
    Page, PathField, PkgbuildPreview, RamdiskLetter, ViewportId, WindowCloseSnapshot,
};
use crate::ramdisk_size;
use crate::style;
use crate::system_theme::{self, ColorScheme};
use crate::terminal_themes::TerminalTheme;
use crate::views::{abs_settings, config_wizard, system_update};
use crate::widgets::{
    self, app_theme_toggle, card_section, command_log, confirm_dialog, dense_header_cell,
    dense_sort_header_cell, dense_table, dense_table_row, encode_ramdisk_flags, field_checkbox,
    field_label_column, field_number, field_path, field_pick, field_text, help_line,
    interactive_list_row, kernel_ramdisk_targets_field, kernel_status_dot, log_save_row,
    optional_bool_field, parse_ramdisk_flags, pgo_round_pipeline, pkgbuild_preview_dialog,
    preview_pkgbuild_button, ramdisk_targets_field, scroll_viewport, stepper_number,
    terminal_theme_picker, unsaved_changes_dialog, PathField as WPathField, PathKind as WPathKind,
    COMMAND_LOG_PAGE_HEIGHT,
};
use iced::event;
use iced::futures::{Stream, StreamExt};
use iced::keyboard;
use iced::widget::operation;
use iced::widget::{
    button, center, column, container, image, opaque, row, stack, text, text_input, Space,
};
use iced::{clipboard, time, window, Point, Size};
use iced::{Alignment, Element, Font, Length, Padding, Subscription, Task, Theme};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PGO_STATUS_POLL_INTERVAL: Duration = Duration::from_secs(2);
const LOG_SCROLL_IGNORE: Duration = Duration::from_millis(120);

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

fn pgo_stage_label(key: &str) -> &'static str {
    match key {
        "stage1_build" => abs_i18n::t("gui.pgo.stage1_build"),
        "wait_reboot1" => abs_i18n::t("gui.pgo.wait_reboot1"),
        "stage2_profile" => abs_i18n::t("gui.pgo.stage2_profile"),
        "stage2_build" => abs_i18n::t("gui.pgo.stage2_build"),
        "wait_reboot2" => abs_i18n::t("gui.pgo.wait_reboot2"),
        "stage3_profile" => abs_i18n::t("gui.pgo.stage3_profile"),
        "stage3_build" => abs_i18n::t("gui.pgo.stage3_build"),
        _ => abs_i18n::t_or(key, "?"),
    }
}

fn pgo_auto_restart_enabled(pkg: &PackageSection) -> bool {
    pkg.pgo.as_ref().map(|p| p.auto_restart).unwrap_or(false)
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

fn join_log_lines(lines: &VecDeque<String>) -> String {
    let cap: usize = lines.iter().map(|s| s.len() + 1).sum();
    let mut buf = String::with_capacity(cap);
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            buf.push('\n');
        }
        buf.push_str(&abs_runner::strip_ansi(line));
    }
    buf
}

fn page_scroll_id(page: Page) -> &'static str {
    match page {
        Page::Kernels => "page-kernels",
        Page::DefaultKernelConfig => "page-default-kernel-config",
        Page::KernelConfig => "page-kernel-config",
        Page::Packages => "page-packages",
        Page::PackageConfig => "page-package-config",
        Page::SystemUpdate => "page-system-update",
        Page::AbsSettings => "page-abs-settings",
        Page::AppSettings => "page-app-settings",
        Page::ConfigWizard => "page-config-wizard",
    }
}

/// One live-output pane. Build and system-update logs are kept separate.
/// `autoscroll` is the Follow-tail play/pause control. `pinned` is whether we are at the tail.
/// Both must be true for the button to show as activated.
struct LogPane {
    lines: VecDeque<String>,
    autoscroll: bool,
    pinned: bool,
    ignore_scroll_until: Instant,
}

impl LogPane {
    fn new() -> Self {
        Self {
            lines: VecDeque::new(),
            autoscroll: true,
            pinned: true,
            ignore_scroll_until: Instant::now(),
        }
    }

    fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    fn text(&self) -> String {
        join_log_lines(&self.lines)
    }

    fn following_tail(&self) -> bool {
        self.autoscroll && self.pinned
    }

    fn clear(&mut self) {
        self.lines.clear();
        self.autoscroll = true;
        self.pinned = true;
        self.ignore_scroll_until = Instant::now();
    }

    fn trim(&mut self, limit: usize) {
        if self.lines.len() > limit {
            let overflow = self.lines.len() - limit;
            self.lines.drain(..overflow);
        }
    }

    fn append(&mut self, line: String) {
        self.lines.push_back(line);
    }

    fn ignore_next_scrolls(&mut self) {
        self.ignore_scroll_until = Instant::now() + LOG_SCROLL_IGNORE;
    }
}

const SCHED_OPTS: &[&str] = &[
    "cachyos",
    "bore",
    "eevdf",
    "rt",
    "rt-bore",
    "hardened",
    "bmq",
    "sched-ext",
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
    apply_effective_lang(&gui_settings);
    system_theme::cleanup_legacy_icon_overlays();
    let icon = load_window_icon();
    let window = gui_settings.window_settings(icon);
    let boot_settings = gui_settings.clone();
    iced::application(
        move || App::new(boot_settings.clone()),
        App::update,
        App::view,
    )
    .settings(iced::Settings {
        id: Some("absgui".into()),
        ..Default::default()
    })
    .title(App::title)
    .theme(App::theme)
    .subscription(App::subscription)
    .window(window)
    .exit_on_close_request(false)
    .run()
}

fn apply_effective_lang(gui: &GuiSettings) {
    if let Some(lang) = gui.lang.as_deref().and_then(abs_i18n::Lang::parse) {
        abs_i18n::set_lang(lang);
        return;
    }
    let abs_lang = std::fs::read_to_string(config_path())
        .ok()
        .and_then(|t| abs_i18n::peek_lang_toml(&t));
    abs_i18n::set_lang(
        abs_lang
            .or_else(abs_i18n::Lang::from_system)
            .unwrap_or(abs_i18n::Lang::En),
    );
}

pub struct App {
    page: Page,
    gui_settings: GuiSettings,
    /// Palette drafts in App Settings; discarded if the user leaves without applying.
    terminal_preview_dark: TerminalTheme,
    terminal_preview_light: TerminalTheme,
    terminal_lines_limit_input: String,
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
    build_log: LogPane,
    update_log: LogPane,
    log_inbox: Arc<Mutex<Vec<String>>>,
    log_flush_scheduled: Arc<AtomicBool>,
    last_event_log_path: Option<std::path::PathBuf>,
    list_editors: ListEditors,
    /// Last `abs --hold-check` output shown on ABS settings.
    hold_check_report: Option<String>,
    busy: bool,
    /// True while a one-shot (non-PGO) kernel build is running; suppresses PGO status polling.
    building_oneshot: bool,
    /// True while `abs -RU` or a targeted repo/AUR install is streaming on the System update page.
    running_system_update: bool,
    pending_updates: Option<PendingUpdates>,
    pending_updates_error: Option<String>,
    pending_updates_loading: bool,
    wizard: config_wizard::WizardSession,
    pgo_run: PgoRunHandle,
    last_log_target: LogSaveTarget,
    /// Set when the active build runs in a separate terminal window (not streamed in-app).
    /// Holds the launch time so status polling can ignore stale state during a short grace period.
    external_run_since: Option<std::time::Instant>,
    /// PID file for the abs process running in the external terminal (see [`abs_runner::external_run_pid_path`]).
    external_pid_path: Option<std::path::PathBuf>,
    settings_tab: crate::messages::SettingsTab,
    kernel_filter: String,
    package_filter: String,
    package_list_filter: PackageListFilter,
    package_sort: PackageSortCol,
    package_sort_desc: bool,
    pending_package_confirm: Option<PackageConfirm>,
    hovered_kernel: Option<String>,
    hovered_package: Option<String>,
    metrics_sampler: crate::metrics::MetricsSampler,
    dark_icon_handle: iced::widget::image::Handle,
    light_icon_handle: iced::widget::image::Handle,
    saved_config: ConfigDocument,
    saved_gui: GuiSettings,
    pending_leave: Option<Message>,
    pkgbuild_preview: Option<PkgbuildPreview>,
    abs_stdin_draft: String,
    taskbar_scheme: ColorScheme,
    /// Live window width used for chrome insets (updated on every resize, including maximized).
    viewport_width: f32,
}

impl App {
    fn title(_state: &Self) -> String {
        abs_i18n::t("gui.chrome.app_name").to_string()
    }

    fn theme(state: &Self) -> Theme {
        style::iced_theme(state.app_theme())
    }

    fn subscription(state: &Self) -> Subscription<Message> {
        let mut subs: Vec<Subscription<Message>> = vec![
            window::resize_events().map(|(_id, size)| Message::WindowResized(size)),
            event::listen_with(|event, _status, _id| match event {
                iced::Event::Window(window::Event::Moved(point)) => {
                    Some(Message::WindowMoved(point))
                }
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                    if modifiers.control() {
                        if let keyboard::Key::Character(c) = key.as_ref() {
                            if c.eq_ignore_ascii_case("f") {
                                return Some(Message::FocusSearch);
                            }
                        }
                        return None;
                    }
                    if matches!(key, keyboard::Key::Named(keyboard::key::Named::Escape)) {
                        return Some(Message::ClosePkgbuildPreview);
                    }
                    None
                }
                _ => None,
            }),
            window::close_requests().map(|_| Message::WindowCloseRequested),
            time::every(std::time::Duration::from_secs(1)).map(|_| Message::SystemMetricsTick),
        ];
        if state.pgo_status_poll_active() {
            subs.push(time::every(PGO_STATUS_POLL_INTERVAL).map(|_| Message::RefreshPgoStatus));
        }
        if state.page == Page::ConfigWizard && state.wizard.needs_timer() {
            subs.push(time::every(Duration::from_millis(33)).map(|_| Message::WizardTimer));
        }
        Subscription::batch(subs)
    }

    fn new(gui_settings: GuiSettings) -> (Self, Task<Message>) {
        let path = config_path();
        let mut page = Page::Kernels;
        let mut selected_kernel = None;
        let mut selected_package = None;
        let mut settings_tab = crate::messages::SettingsTab::default();
        let mut initial_wizard_step: usize = 0;
        let mut gui_settings = gui_settings;

        let args: Vec<String> = std::env::args().collect();
        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--theme" if i + 1 < args.len() => {
                    if args[i + 1] == "dark" {
                        gui_settings.theme = ThemePref::Dark;
                    } else if args[i + 1] == "light" {
                        gui_settings.theme = ThemePref::Light;
                    } else if args[i + 1] == "system" {
                        gui_settings.theme = ThemePref::System;
                    }
                    i += 1;
                }
                "--page" if i + 1 < args.len() => {
                    match args[i + 1].as_str() {
                        "kernel-config" => page = Page::KernelConfig,
                        "default-config" => page = Page::DefaultKernelConfig,
                        "packages" => page = Page::Packages,
                        "package-config" => page = Page::PackageConfig,
                        "update" => page = Page::SystemUpdate,
                        "abs-settings" => page = Page::AbsSettings,
                        "app-settings" => page = Page::AppSettings,
                        "config-wizard" => page = Page::ConfigWizard,
                        _ => {}
                    }
                    i += 1;
                }
                "--settings-tab" if i + 1 < args.len() => {
                    match args[i + 1].as_str() {
                        "general-paths" => {
                            settings_tab = crate::messages::SettingsTab::GeneralPaths
                        }
                        "build-chroot" => settings_tab = crate::messages::SettingsTab::BuildChroot,
                        "ramdisk" => settings_tab = crate::messages::SettingsTab::Ramdisk,
                        "held-packages" => {
                            settings_tab = crate::messages::SettingsTab::HeldPackages
                        }
                        "repositories" => settings_tab = crate::messages::SettingsTab::Repositories,
                        _ => {}
                    }
                    i += 1;
                }
                "--package" if i + 1 < args.len() => {
                    selected_package = Some(args[i + 1].clone());
                    i += 1;
                }
                "--kernel" if i + 1 < args.len() => {
                    selected_kernel = Some(args[i + 1].clone());
                    i += 1;
                }
                "--wizard-step" if i + 1 < args.len() => {
                    if let Ok(st) = args[i + 1].parse::<usize>() {
                        initial_wizard_step = st;
                    }
                    i += 1;
                }
                _ => {}
            }
            i += 1;
        }

        let saved_gui = gui_settings.clone();
        let missing_config = !path.exists();
        if missing_config && page == Page::Kernels {
            page = Page::ConfigWizard;
        }
        let load_path = path.clone();
        let mut boot_tasks = vec![
            Task::perform(async move { load_config(&load_path) }, |r| {
                Message::ConfigLoaded(Box::new(r))
            }),
            Self::clamp_window_to_monitor_task(),
        ];
        if page == Page::ConfigWizard {
            boot_tasks.push(config_wizard::WizardSession::load_task());
        }
        let viewport_width = gui_settings.window_width;
        (
            Self {
                page,
                terminal_preview_dark: gui_settings.terminal_theme_dark,
                terminal_preview_light: gui_settings.terminal_theme_light,
                terminal_lines_limit_input: gui_settings.terminal_lines_limit.to_string(),
                gui_settings,
                config_path: path.clone(),
                config: ConfigDocument::default(),
                config_error: None,
                status_message: None,
                selected_kernel,
                custom_kernel: String::new(),
                selected_package,
                new_package_name: String::new(),
                pgo_status: None,
                pgo_status_error: None,
                pgo_selected_stage: pgo_first_phase_key().to_string(),
                build_log: LogPane::new(),
                update_log: LogPane::new(),
                log_inbox: Arc::new(Mutex::new(Vec::new())),
                log_flush_scheduled: Arc::new(AtomicBool::new(false)),
                last_event_log_path: None,
                list_editors: ListEditors::from_config(&ConfigDocument::default()),
                hold_check_report: None,
                busy: false,
                building_oneshot: false,
                running_system_update: false,
                pending_updates: None,
                pending_updates_error: None,
                pending_updates_loading: false,
                wizard: {
                    let mut w = config_wizard::WizardSession::default();
                    w.step = initial_wizard_step;
                    if page == Page::ConfigWizard {
                        w.loading = true;
                    }
                    w
                },
                pgo_run: PgoRunHandle::new(),
                last_log_target: LogSaveTarget::Build,
                external_run_since: None,
                external_pid_path: None,
                settings_tab,
                kernel_filter: String::new(),
                package_filter: String::new(),
                package_list_filter: PackageListFilter::All,
                package_sort: PackageSortCol::Name,
                package_sort_desc: false,
                pending_package_confirm: None,
                hovered_kernel: None,
                hovered_package: None,
                metrics_sampler: crate::metrics::MetricsSampler::new(),
                dark_icon_handle: load_rgba_png(include_bytes!("../assets/icon_dark.png"), 64),
                light_icon_handle: load_rgba_png(include_bytes!("../assets/icon_light.png"), 64),
                saved_config: ConfigDocument::default(),
                saved_gui,
                pending_leave: None,
                pkgbuild_preview: None,
                abs_stdin_draft: String::new(),
                taskbar_scheme: system_theme::detect(),
                viewport_width,
            },
            Task::batch(boot_tasks),
        )
    }

    fn app_theme(&self) -> AppTheme {
        self.gui_settings.theme.resolve(self.taskbar_scheme)
    }

    fn chrome_pad_x(&self) -> f32 {
        style::shell_pad_x(self.viewport_width)
    }

    fn ramdisk_size_save_error(&self) -> Option<String> {
        let total = ramdisk_size::mem_total_bytes()?;
        ramdisk_size::ensure_fits(&self.config.ramdisk.size, total).err()
    }

    fn log_palette(&self) -> crate::terminal_themes::LogPalette {
        style::log_palette(
            self.app_theme(),
            self.gui_settings.viewport_theme(self.app_theme()),
        )
    }

    fn log_pane(&self, id: ViewportId) -> &LogPane {
        match id {
            ViewportId::BuildLog => &self.build_log,
            ViewportId::UpdateLog => &self.update_log,
        }
    }

    fn log_pane_mut(&mut self, id: ViewportId) -> &mut LogPane {
        match id {
            ViewportId::BuildLog => &mut self.build_log,
            ViewportId::UpdateLog => &mut self.update_log,
        }
    }

    fn active_log_viewport(&self) -> ViewportId {
        if self.page == Page::SystemUpdate || self.running_system_update {
            ViewportId::UpdateLog
        } else {
            ViewportId::BuildLog
        }
    }

    fn visible_log_viewport(&self) -> ViewportId {
        if self.page == Page::SystemUpdate {
            ViewportId::UpdateLog
        } else {
            ViewportId::BuildLog
        }
    }

    fn visible_log(&self) -> &LogPane {
        self.log_pane(self.visible_log_viewport())
    }

    fn visible_log_mut(&mut self) -> &mut LogPane {
        let id = self.visible_log_viewport();
        self.log_pane_mut(id)
    }

    fn active_log_mut(&mut self) -> &mut LogPane {
        let id = self.active_log_viewport();
        self.log_pane_mut(id)
    }

    fn live_config(&self) -> ConfigDocument {
        let mut doc = self.config.clone();
        self.list_editors.apply_all(&mut doc);
        doc
    }

    fn config_dirty(&self) -> bool {
        self.live_config() != self.saved_config
    }

    fn app_settings_dirty(&self) -> bool {
        self.terminal_preview_dark != self.gui_settings.terminal_theme_dark
            || self.terminal_preview_light != self.gui_settings.terminal_theme_light
            || self.parsed_or_current_limit() != self.saved_gui.terminal_lines_limit
            || !self.gui_settings.content_eq(&self.saved_gui)
    }

    fn is_dirty(&self) -> bool {
        self.config_dirty() || self.app_settings_dirty()
    }

    fn mark_saved(&mut self) {
        self.list_editors.apply_all(&mut self.config);
        self.saved_config = self.config.clone();
        self.saved_gui = self.gui_settings.clone();
    }

    fn restore_saved(&mut self) {
        let window_width = self.gui_settings.window_width;
        let window_height = self.gui_settings.window_height;
        let window_x = self.gui_settings.window_x;
        let window_y = self.gui_settings.window_y;
        let window_fullscreen = self.gui_settings.window_fullscreen;
        let window_maximized = self.gui_settings.window_maximized;
        self.config = self.saved_config.clone();
        self.list_editors = ListEditors::from_config(&self.config);
        self.gui_settings = self.saved_gui.clone();
        self.gui_settings.window_width = window_width;
        self.gui_settings.window_height = window_height;
        self.gui_settings.window_x = window_x;
        self.gui_settings.window_y = window_y;
        self.gui_settings.window_fullscreen = window_fullscreen;
        self.gui_settings.window_maximized = window_maximized;
        self.sync_terminal_previews();
        self.terminal_lines_limit_input = self.gui_settings.terminal_lines_limit.to_string();
    }

    fn overlay_message_allowed(message: &Message) -> bool {
        matches!(
            message,
            Message::UnsavedSave
                | Message::UnsavedDiscard
                | Message::UnsavedCancel
                | Message::PackageConfirmAccept
                | Message::PackageConfirmCancel
                | Message::ConfigSaved(_)
                | Message::AppSettingsSaved(_)
                | Message::SystemMetricsTick
                | Message::WindowResized(_)
                | Message::WindowMoved(_)
                | Message::WindowSizeCommitted { .. }
                | Message::WindowPositionCommitted { .. }
                | Message::WindowClampToMonitor { .. }
                | Message::WindowCloseSnapshot(_)
                | Message::LogFlush
                | Message::PgoRunFinished(_)
                | Message::PgoAbortFinished(_)
                | Message::RefreshPgoStatus
                | Message::PgoStatusLoaded(_)
                | Message::WizardTimer
                | Message::WizardCheckResult(_, _, _)
                | Message::WizardFormLoaded(_)
                | Message::WizardApplyDone(_)
                | Message::WizardStepChecked(_)
                | Message::PkgbuildLoaded { .. }
                | Message::CopyPkgbuild
                | Message::TogglePkgbuildDelta
                | Message::ClosePkgbuildPreview
                | Message::AbsStdinChanged(_)
                | Message::AbsStdinSubmit
        )
    }

    fn is_leave_page(message: &Message) -> bool {
        matches!(
            message,
            Message::OpenKernels
                | Message::OpenDefaultConfig
                | Message::OpenPackages
                | Message::OpenPackage(_)
                | Message::PackageAdd
                | Message::OpenSystemUpdate
                | Message::OpenAbsSettings
                | Message::OpenConfigWizard
                | Message::OpenAppSettings
                | Message::Back
                | Message::OpenKernel(_)
                | Message::ReloadConfig
                | Message::WindowCloseRequested
        )
    }

    fn finish_pending_leave(&mut self) -> Task<Message> {
        match self.pending_leave.take() {
            Some(next) => self.handle_message(next),
            None => Task::none(),
        }
    }

    fn remove_configured_package(&mut self, name: &str) {
        self.config.packages.remove(name);
        if self.selected_package.as_deref() == Some(name) {
            self.selected_package = None;
            self.navigate(Page::Packages);
        }
        self.status_message = Some(abs_i18n::tf("gui.msg.removed_package", &[("name", name)]));
    }

    fn purge_configured_packages(&mut self) {
        let count = self.config.packages.len();
        self.config.packages.clear();
        self.selected_package = None;
        self.hovered_package = None;
        if self.page == Page::PackageConfig {
            self.navigate(Page::Packages);
        }
        self.status_message = Some(abs_i18n::tf(
            "gui.msg.purged_packages",
            &[("count", &count.to_string())],
        ));
    }

    fn save_unsaved_then_leave(&mut self) -> Task<Message> {
        self.list_editors.apply_all(&mut self.config);
        self.config
            .held_packages
            .retain(|h| !h.name.trim().is_empty());
        for h in &mut self.config.held_packages {
            h.name = h.name.trim().to_string();
            h.version = h.version.trim().to_string();
        }
        if let Ok(n) = self.terminal_lines_limit_input.trim().parse::<usize>() {
            self.gui_settings.terminal_lines_limit = clamp_terminal_lines_limit(n);
            self.terminal_lines_limit_input = self.gui_settings.terminal_lines_limit.to_string();
            self.trim_log_lines();
        }
        self.commit_terminal_preview();
        let _ = self.gui_settings.save();
        let config_dirty = self.config != self.saved_config;
        if config_dirty {
            if let Some(e) = self.ramdisk_size_save_error() {
                self.status_message = Some(e.clone());
                return self.push_log(e);
            }
            let path = self.config_path.clone();
            let doc = self.config.clone();
            return Task::perform(
                async move { save_config(&path, &doc) },
                Message::ConfigSaved,
            );
        }
        self.mark_saved();
        self.finish_pending_leave()
    }

    fn navigate(&mut self, page: Page) {
        if self.page == Page::AppSettings && page != Page::AppSettings {
            self.sync_terminal_previews();
        }
        if page == Page::AppSettings {
            self.sync_terminal_previews();
        }
        self.hovered_kernel = None;
        self.hovered_package = None;
        self.page = page;
    }

    fn start_abs_on_update_page(
        &mut self,
        abs_cmd: String,
        status: String,
        save_config_first: bool,
    ) -> Task<Message> {
        if self.busy {
            return self.push_log(abs_i18n::t("gui.msg.busy_build"));
        }
        if let Err(e) = abs_runner::verify_abs_binary() {
            self.status_message = Some(e.clone());
            return self.push_log(abs_i18n::tf("gui.msg.cannot_start_update", &[("e", &e)]));
        }
        if let Err(e) = abs_runner::require_gui_askpass() {
            self.status_message = Some(e.clone());
            return self.push_log(abs_i18n::tf("gui.msg.cannot_start_update", &[("e", &e)]));
        }
        if save_config_first {
            self.list_editors.apply_all(&mut self.config);
            if let Some(e) = self.ramdisk_size_save_error() {
                self.status_message = Some(e.clone());
                return self.push_log(abs_i18n::tf(
                    "gui.msg.cannot_start_update_save",
                    &[("e", &e)],
                ));
            }
            let path = self.config_path.clone();
            let doc = self.config.clone();
            if let Err(e) = save_config(&path, &doc) {
                self.status_message = Some(e.clone());
                return self.push_log(abs_i18n::tf(
                    "gui.msg.cannot_start_update_save",
                    &[("e", &e)],
                ));
            }
            self.append_log(abs_i18n::tf(
                "gui.msg.saved_path",
                &[("path", &path.display().to_string())],
            ));
        }
        self.busy = true;
        self.running_system_update = true;
        self.pgo_run.reset();
        self.update_log.clear();
        self.log_inbox.lock().unwrap().clear();
        self.log_flush_scheduled.store(false, Ordering::Release);
        self.last_log_target = LogSaveTarget::Update;
        self.last_event_log_path = None;
        self.status_message = Some(status);
        self.append_log(format!("$ {abs_cmd}"));
        let handle = self.pgo_run.clone();
        Task::stream(Self::absorb_abs_stream(
            self.log_inbox.clone(),
            self.log_flush_scheduled.clone(),
            stream_abs_command(abs_cmd, handle, None),
        ))
    }

    fn sync_terminal_previews(&mut self) {
        self.terminal_preview_dark = self.gui_settings.terminal_theme_dark;
        self.terminal_preview_light = self.gui_settings.terminal_theme_light;
    }

    fn commit_terminal_preview(&mut self) {
        self.gui_settings.terminal_theme_dark = self.terminal_preview_dark;
        self.gui_settings.terminal_theme_light = self.terminal_preview_light;
        self.gui_settings.terminal_theme = self.terminal_preview_dark;
    }

    fn log_text(&self) -> String {
        self.visible_log().text()
    }

    fn trim_log_lines(&mut self) {
        let limit = self.gui_settings.terminal_lines_limit;
        self.build_log.trim(limit);
        self.update_log.trim(limit);
    }

    fn drain_log_inbox(&mut self) -> bool {
        self.log_flush_scheduled.store(false, Ordering::Release);
        let lines = {
            let mut inbox = self.log_inbox.lock().unwrap();
            if inbox.is_empty() {
                return false;
            }
            std::mem::take(&mut *inbox)
        };
        for line in lines {
            self.append_log(line);
        }
        true
    }

    fn log_pane_visible(&self, id: ViewportId) -> bool {
        match id {
            ViewportId::BuildLog => self.page == Page::KernelConfig,
            ViewportId::UpdateLog => self.page == Page::SystemUpdate,
        }
    }

    fn snap_log_if_following(&mut self) -> Task<Message> {
        let id = self.active_log_viewport();
        if self.log_pane(id).following_tail() && self.log_pane_visible(id) {
            self.log_pane_mut(id).ignore_next_scrolls();
            operation::snap_to_end(id.scroll_id())
        } else {
            Task::none()
        }
    }

    fn absorb_abs_stream(
        inbox: Arc<Mutex<Vec<String>>>,
        flush_scheduled: Arc<AtomicBool>,
        stream: impl Stream<Item = AbsPgoStreamItem> + Send + 'static,
    ) -> impl Stream<Item = Message> {
        stream.filter_map(move |item| {
            let inbox = inbox.clone();
            let flush_scheduled = flush_scheduled.clone();
            async move {
                match item {
                    AbsPgoStreamItem::Lines(lines) => {
                        inbox.lock().unwrap().extend(lines);
                        if flush_scheduled.swap(true, Ordering::AcqRel) {
                            None
                        } else {
                            Some(Message::LogFlush)
                        }
                    }
                    AbsPgoStreamItem::Finished(result) => Some(Message::PgoRunFinished(result)),
                }
            }
        })
    }

    fn apply_terminal_lines_limit(&mut self, n: usize) {
        let n = clamp_terminal_lines_limit(n);
        self.gui_settings.terminal_lines_limit = n;
        self.terminal_lines_limit_input = n.to_string();
        self.trim_log_lines();
        let _ = self.gui_settings.save();
    }

    fn parsed_or_current_limit(&self) -> usize {
        self.terminal_lines_limit_input
            .trim()
            .parse::<usize>()
            .map(clamp_terminal_lines_limit)
            .unwrap_or(self.gui_settings.terminal_lines_limit)
    }

    fn log_save_target(&self) -> LogSaveTarget {
        if self.page == Page::SystemUpdate {
            LogSaveTarget::Update
        } else {
            self.last_log_target
        }
    }

    fn persist_gui_settings(&mut self) {
        let _ = self.gui_settings.save();
        self.saved_gui = self.gui_settings.clone();
    }

    fn query_size_placement(size: Size) -> Task<Message> {
        window::latest().then(move |id| {
            let Some(id) = id else {
                return Task::done(Message::WindowSizeCommitted {
                    size,
                    fullscreen: false,
                    maximized: false,
                });
            };
            window::mode(id).then(move |mode| {
                window::is_maximized(id).map(move |maximized| Message::WindowSizeCommitted {
                    size,
                    fullscreen: mode == window::Mode::Fullscreen,
                    maximized,
                })
            })
        })
    }

    fn query_position_placement(point: Point) -> Task<Message> {
        window::latest().then(move |id| {
            let Some(id) = id else {
                return Task::done(Message::WindowPositionCommitted {
                    point,
                    fullscreen: false,
                    maximized: false,
                });
            };
            window::mode(id).then(move |mode| {
                window::is_maximized(id).map(move |maximized| Message::WindowPositionCommitted {
                    point,
                    fullscreen: mode == window::Mode::Fullscreen,
                    maximized,
                })
            })
        })
    }

    fn clamp_window_to_monitor_task() -> Task<Message> {
        window::latest().then(|id| {
            let Some(id) = id else {
                return Task::none();
            };
            window::monitor_size(id).then(move |monitor| {
                window::size(id).then(move |size| {
                    window::position(id).map(move |position| Message::WindowClampToMonitor {
                        id,
                        monitor,
                        size,
                        position,
                    })
                })
            })
        })
    }

    fn snapshot_window_then_close() -> Task<Message> {
        window::latest().then(|id| {
            let Some(id) = id else {
                return Task::done(Message::WindowCloseSnapshot(None));
            };
            window::mode(id).then(move |mode| {
                window::is_maximized(id).then(move |maximized| {
                    window::size(id).then(move |size| {
                        window::position(id).map(move |position| {
                            Message::WindowCloseSnapshot(Some(WindowCloseSnapshot {
                                fullscreen: mode == window::Mode::Fullscreen,
                                maximized,
                                size,
                                position,
                            }))
                        })
                    })
                })
            })
        })
    }

    fn begin_exit(&mut self) -> Task<Message> {
        if self.external_run_since.is_some() {
            // A build is running in its own terminal window; leave it (and its ramdisk)
            // alone so closing the GUI doesn't kill an in-progress kernel compile.
            return Task::done(Message::ExitAfterCleanup);
        }
        // Idle close: do not unmount the ramdisk. Leaving it mounted is intentional
        // (reclaim on next build); forcing `abs --ramdisk-shutdown` always prompted sudo.
        if !self.busy {
            return Task::done(Message::ExitAfterCleanup);
        }
        if self.running_system_update {
            let handle = self.pgo_run.clone();
            return Task::perform(
                async move {
                    handle.stop_running_build(None);
                },
                |_| Message::ExitAfterCleanup,
            );
        }
        let pkg = self.selected_kernel.clone();
        let handle = self.pgo_run.clone();
        let run_pgo_abort = !self.building_oneshot;
        let pid_path = self.external_pid_path.clone();
        Task::perform(
            async move {
                if let Some(p) = pkg {
                    // abort() already runs --ramdisk-shutdown after stopping the build.
                    let _ = handle.abort(&p, run_pgo_abort, pid_path.as_deref());
                } else {
                    let _ = run_ramdisk_shutdown();
                }
            },
            |_| Message::ExitAfterCleanup,
        )
    }

    fn append_log(&mut self, line: impl Into<String>) {
        let Some(line) = abs_runner::sanitize_log_line(&line.into()) else {
            return;
        };
        self.active_log_mut().append(line);
        self.trim_log_lines();
    }

    fn push_log(&mut self, line: impl Into<String>) -> Task<Message> {
        self.append_log(line);
        self.snap_log_if_following()
    }

    fn sync_pgo_selected_stage_from_status(&mut self, status: &PgoStatus) {
        let saved = status.stage.as_str();
        let stage_changed = self.pgo_status.as_ref().map(|s| s.stage.as_str()) != Some(saved);

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
            return self.push_log(abs_i18n::t("gui.msg.busy_pgo"));
        }
        self.list_editors.apply_all(&mut self.config);
        let Some(section) = self.config.packages.get(&pkg).cloned() else {
            let msg = abs_i18n::tf("gui.msg.pkg_not_saved", &[("pkg", &pkg)]);
            self.status_message = Some(msg.clone());
            return self.push_log(abs_i18n::tf("gui.msg.cannot_start_pgo", &[("e", &msg)]));
        };
        if let Err(msg) = validate_pgo_start(&section, &pkg) {
            self.status_message = Some(msg.clone());
            return self.push_log(abs_i18n::tf("gui.msg.cannot_start_pgo", &[("e", &msg)]));
        }
        if let Err(e) = abs_runner::verify_abs_binary() {
            self.status_message = Some(e.clone());
            return self.push_log(abs_i18n::tf("gui.msg.cannot_start_pgo", &[("e", &e)]));
        }
        let event_log = abs_runner::default_event_log_path(&pkg);
        if let Err(e) = abs_runner::ensure_event_log_path(&event_log) {
            self.status_message = Some(e.clone());
            return self.push_log(abs_i18n::tf("gui.msg.cannot_start_pgo", &[("e", &e)]));
        }
        if let Some(e) = self.ramdisk_size_save_error() {
            self.status_message = Some(e.clone());
            return self.push_log(abs_i18n::tf("gui.msg.cannot_start_pgo_save", &[("e", &e)]));
        }
        self.busy = true;
        self.pgo_run.reset();
        self.build_log.autoscroll = true;
        self.build_log.pinned = true;
        self.log_inbox.lock().unwrap().clear();
        self.log_flush_scheduled.store(false, Ordering::Release);
        self.last_log_target = LogSaveTarget::Build;
        self.last_event_log_path = Some(event_log.clone());
        self.status_message = Some(status_msg.to_string());
        self.append_log(status_msg);
        self.append_log(abs_i18n::tf(
            "gui.msg.detailed_events",
            &[("path", &event_log.display().to_string())],
        ));
        let path = self.config_path.clone();
        let doc = self.config.clone();
        if let Err(e) = save_config(&path, &doc) {
            self.busy = false;
            self.status_message = Some(e.clone());
            return self.push_log(abs_i18n::tf("gui.msg.cannot_start_pgo_save", &[("e", &e)]));
        }
        self.append_log(abs_i18n::tf(
            "gui.msg.saved_path",
            &[("path", &path.display().to_string())],
        ));
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
                self.append_log(abs_i18n::tf("gui.msg.launched_pgo", &[("term", &term)]));
                Task::done(Message::RefreshPgoStatus)
            }
            Err(e) => {
                self.append_log(abs_i18n::tf("gui.msg.terminal_fallback", &[("e", &e)]));
                let handle = self.pgo_run.clone();
                Task::batch([
                    Task::done(Message::RefreshPgoStatus),
                    Task::stream(Self::absorb_abs_stream(
                        self.log_inbox.clone(),
                        self.log_flush_scheduled.clone(),
                        stream_abs_pgo(
                            action,
                            pkg,
                            Some(event_log),
                            stage_owned,
                            once,
                            pgo_auto,
                            handle,
                        ),
                    )),
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
        let opt = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        };
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
        self.handle_message(message)
    }

    fn handle_message(&mut self, message: Message) -> Task<Message> {
        if (self.pending_leave.is_some() || self.pending_package_confirm.is_some())
            && !Self::overlay_message_allowed(&message)
        {
            return Task::none();
        }
        if Self::is_leave_page(&message) && self.is_dirty() {
            self.pending_leave = Some(message);
            return Task::none();
        }
        match message {
            Message::OpenKernels => self.navigate(Page::Kernels),
            Message::OpenDefaultConfig => self.navigate(Page::DefaultKernelConfig),
            Message::OpenPackages => self.navigate(Page::Packages),
            Message::OpenPackage(name) => {
                if !self.config.packages.contains_key(&name) && !name.is_empty() {
                    self.config
                        .packages
                        .insert(name.clone(), PackageSection::default());
                    self.status_message =
                        Some(abs_i18n::tf("gui.msg.created_package", &[("name", &name)]));
                }
                self.selected_package = Some(name);
                self.navigate(Page::PackageConfig);
            }
            Message::NewPackageNameChanged(v) => self.new_package_name = v,
            Message::PackageAdd => {
                let name = self.new_package_name.trim().to_string();
                if name.is_empty() {
                    return Task::none();
                }
                if self.config.packages.contains_key(&name) {
                    self.status_message =
                        Some(abs_i18n::tf("gui.msg.package_exists", &[("name", &name)]));
                } else {
                    self.config
                        .packages
                        .insert(name.clone(), PackageSection::default());
                    self.status_message =
                        Some(abs_i18n::tf("gui.msg.added_package", &[("name", &name)]));
                }
                self.new_package_name.clear();
                self.selected_package = Some(name);
                self.navigate(Page::PackageConfig);
            }
            Message::PackageRemove(name) => {
                self.pending_package_confirm = Some(PackageConfirm::Remove(name));
            }
            Message::PackagePurgeAll => {
                if !self.config.packages.is_empty() {
                    self.pending_package_confirm = Some(PackageConfirm::PurgeAll);
                }
            }
            Message::PackageSort(col) => {
                if self.package_sort == col {
                    self.package_sort_desc = !self.package_sort_desc;
                } else {
                    self.package_sort = col;
                    self.package_sort_desc = false;
                }
            }
            Message::PackageConfirmAccept => match self.pending_package_confirm.take() {
                Some(PackageConfirm::Remove(name)) => self.remove_configured_package(&name),
                Some(PackageConfirm::PurgeAll) => self.purge_configured_packages(),
                None => {}
            },
            Message::PackageConfirmCancel => {
                self.pending_package_confirm = None;
            }
            Message::OpenSystemUpdate => {
                self.navigate(Page::SystemUpdate);
                return Task::done(Message::PendingUpdatesRefresh);
            }
            Message::OpenAbsSettings => self.navigate(Page::AbsSettings),
            Message::OpenConfigWizard => {
                self.navigate(Page::ConfigWizard);
                self.wizard = config_wizard::WizardSession::default();
                self.wizard.loading = true;
                return config_wizard::WizardSession::load_task();
            }
            Message::SettingsTabSelected(tab) => self.settings_tab = tab,
            Message::KernelFilter(q) => self.kernel_filter = q,
            Message::PackageFilter(q) => self.package_filter = q,
            Message::KernelRowEnter(name) => {
                if self.hovered_kernel.as_ref() != Some(&name) {
                    self.hovered_kernel = Some(name);
                }
            }
            Message::KernelRowExit(name) => {
                if self.hovered_kernel.as_ref() == Some(&name) {
                    self.hovered_kernel = None;
                }
            }
            Message::PackageRowEnter(name) => {
                if self.hovered_package.as_ref() != Some(&name) {
                    self.hovered_package = Some(name);
                }
            }
            Message::PackageRowExit(name) => {
                if self.hovered_package.as_ref() == Some(&name) {
                    self.hovered_package = None;
                }
            }
            Message::PackageListFilter(f) => self.package_list_filter = f,
            Message::FocusSearch => {
                let id = match self.page {
                    Page::Kernels => widgets::SEARCH_KERNELS,
                    Page::Packages => widgets::SEARCH_PACKAGES,
                    _ => return Task::none(),
                };
                return operation::focus(id);
            }
            Message::OpenAppSettings => self.navigate(Page::AppSettings),
            Message::Back => {
                let next = match self.page {
                    Page::PackageConfig => Page::Packages,
                    _ => Page::Kernels,
                };
                self.navigate(next);
            }
            Message::OpenKernel(name) => {
                self.config.ensure_kernel_from_defaults(&name);
                self.selected_kernel = Some(name);
                self.pgo_selected_stage = pgo_first_phase_key().to_string();
                self.navigate(Page::KernelConfig);
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
                self.config
                    .held_packages
                    .retain(|h| !h.name.trim().is_empty());
                for h in &mut self.config.held_packages {
                    h.name = h.name.trim().to_string();
                    h.version = h.version.trim().to_string();
                }
                if let Some(e) = self.ramdisk_size_save_error() {
                    self.status_message = Some(e.clone());
                    return self.push_log(e);
                }
                let path = self.config_path.clone();
                let doc = self.config.clone();
                return Task::perform(
                    async move { save_config(&path, &doc) },
                    Message::ConfigSaved,
                );
            }
            Message::SaveAppSettings => {
                if let Ok(n) = self.terminal_lines_limit_input.trim().parse::<usize>() {
                    self.gui_settings.terminal_lines_limit = clamp_terminal_lines_limit(n);
                    self.terminal_lines_limit_input =
                        self.gui_settings.terminal_lines_limit.to_string();
                    self.trim_log_lines();
                }
                self.commit_terminal_preview();
                let settings = self.gui_settings.clone();
                return Task::perform(async move { settings.save() }, Message::AppSettingsSaved);
            }
            Message::AppThemeSelected(theme) => {
                self.gui_settings.theme = theme;
                self.persist_gui_settings();
            }
            Message::GuiLangSelected(code) => {
                self.gui_settings.lang = code;
                apply_effective_lang(&self.gui_settings);
                self.persist_gui_settings();
            }
            Message::AbsLangSelected(code) => {
                self.config.lang = code.clone();
                if self.gui_settings.lang.is_none() {
                    abs_i18n::set_lang(
                        code.as_deref()
                            .and_then(abs_i18n::Lang::parse)
                            .or_else(abs_i18n::Lang::from_system)
                            .unwrap_or(abs_i18n::Lang::En),
                    );
                }
            }
            Message::TerminalThemePreview(slot, theme) => match slot {
                AppTheme::Dark => self.terminal_preview_dark = theme,
                AppTheme::Light => self.terminal_preview_light = theme,
            },
            Message::TerminalThemeApply => {
                self.commit_terminal_preview();
                self.persist_gui_settings();
                self.status_message = Some(abs_i18n::t("gui.status.terminal_theme_saved").into());
            }
            Message::TerminalLinesLimitInput(v) => {
                self.terminal_lines_limit_input = v;
            }
            Message::TerminalLinesLimitDec => {
                let n = self
                    .parsed_or_current_limit()
                    .saturating_sub(TERMINAL_LINES_STEP);
                self.apply_terminal_lines_limit(n);
            }
            Message::TerminalLinesLimitInc => {
                let n = self
                    .parsed_or_current_limit()
                    .saturating_add(TERMINAL_LINES_STEP);
                self.apply_terminal_lines_limit(n);
            }
            Message::ConfigLoaded(result) => match *result {
                Ok(doc) => {
                    self.config = doc;
                    if let Some(ref k) = self.selected_kernel {
                        self.config.ensure_kernel_from_defaults(k);
                    }
                    self.list_editors = ListEditors::from_config(&self.config);
                    self.config_error = None;
                    self.status_message = Some(abs_i18n::t("gui.status.config_loaded").into());
                    self.mark_saved();
                    apply_effective_lang(&self.gui_settings);
                    if self.page == Page::SystemUpdate {
                        return Task::done(Message::PendingUpdatesRefresh);
                    }
                }
                Err(e) => {
                    if self.page == Page::ConfigWizard && !self.config_path.exists() {
                        self.config_error = None;
                    } else {
                        self.config_error = Some(e);
                    }
                }
            },
            Message::ConfigSaved(Ok(())) => {
                self.mark_saved();
                self.status_message = Some(abs_i18n::tf(
                    "gui.status.config_saved",
                    &[("path", &self.config_path.display().to_string())],
                ));
                let log = self.push_log(abs_i18n::tf(
                    "gui.status.config_saved",
                    &[("path", &self.config_path.display().to_string())],
                ));
                if self.pending_leave.is_some() {
                    return Task::batch([log, self.finish_pending_leave()]);
                }
                return log;
            }
            Message::ConfigSaved(Err(e)) => {
                self.status_message = Some(abs_i18n::tf("gui.status.save_failed", &[("e", &e)]));
            }
            Message::AppSettingsSaved(Ok(())) => {
                self.mark_saved();
                self.status_message = Some(abs_i18n::t("gui.status.app_settings_saved").into());
                if self.pending_leave.is_some() {
                    return self.finish_pending_leave();
                }
            }
            Message::AppSettingsSaved(Err(e)) => {
                self.status_message = Some(abs_i18n::tf("gui.status.save_failed", &[("e", &e)]));
            }
            Message::UnsavedSave => {
                return self.save_unsaved_then_leave();
            }
            Message::UnsavedDiscard => {
                self.restore_saved();
                return self.finish_pending_leave();
            }
            Message::UnsavedCancel => {
                self.pending_leave = None;
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
            Message::SelfUpdateInstallPath(v) => {
                self.config.self_update_install_path = opt_str(v);
            }
            Message::SelfUpdateUsePacman(v) => self.config.self_update_use_pacman = v,
            Message::InstallAbsGui(v) => self.config.install_absgui = v,
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
                    if self
                        .list_editors
                        .skip_install_after
                        .text()
                        .trim()
                        .is_empty()
                    {
                        self.list_editors.skip_install_after =
                            iced::widget::text_editor::Content::with_text(
                                &list_editors::lines_to_text(&self.config.skip_install_packages),
                            );
                        self.list_editors
                            .apply_field(PackageListField::SkipInstallAfter, &mut self.config);
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
                self.config
                    .system_update
                    .command_to_perform_system_update_no_refresh = opt_str(v);
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
            Message::HeldNameChanged(idx, name) => {
                if let Some(h) = self.config.held_packages.get_mut(idx) {
                    h.name = name;
                }
            }
            Message::HeldVersionChanged(idx, version) => {
                if let Some(h) = self.config.held_packages.get_mut(idx) {
                    h.version = version;
                }
            }
            Message::HeldTriggersChanged(idx, text) => {
                if let Some(h) = self.config.held_packages.get_mut(idx) {
                    h.set_triggers_from_text(&text);
                }
            }
            Message::HeldAdd => {
                let version = String::new();
                self.config.held_packages.push(crate::config::HeldPackage {
                    name: String::new(),
                    version,
                    auto_recompile_trigger: crate::config::AutoRecompileTrigger::default(),
                });
                self.status_message = Some(abs_i18n::t("gui.msg.held_added").into());
            }
            Message::HeldRemove(idx) => {
                if idx < self.config.held_packages.len() {
                    let removed = self.config.held_packages.remove(idx);
                    self.status_message = Some(abs_i18n::tf(
                        "gui.msg.held_removed",
                        &[(
                            "name",
                            if removed.name.is_empty() {
                                abs_i18n::t("gui.msg.unnamed")
                            } else {
                                removed.name.as_str()
                            },
                        )],
                    ));
                }
            }
            Message::HeldSnapshotTriggers(idx) => {
                if let Some(h) = self.config.held_packages.get_mut(idx) {
                    let mut filled = 0usize;
                    for (pkg, ver) in h.auto_recompile_trigger.on_packages_updated.iter_mut() {
                        if ver.is_empty() {
                            if let Some(installed) = abs_runner::pacman_query_version(pkg) {
                                *ver = installed;
                                filled += 1;
                            }
                        }
                    }
                    // Also prefill held version from pacman when empty.
                    if h.version.trim().is_empty() && !h.name.trim().is_empty() {
                        if let Some(installed) = abs_runner::pacman_query_version(&h.name) {
                            h.version = installed;
                            filled += 1;
                        }
                    }
                    self.status_message = Some(abs_i18n::tf(
                        "gui.msg.snapshot_filled",
                        &[("filled", &filled.to_string())],
                    ));
                }
            }
            Message::HeldCheck => {
                let pkgs: Vec<String> = self
                    .config
                    .held_packages
                    .iter()
                    .map(|h| h.name.clone())
                    .filter(|n| !n.is_empty())
                    .collect();
                self.status_message = Some(abs_i18n::t("gui.msg.hold_check_running").into());
                return Task::perform(
                    async move { abs_runner::run_hold_check(&pkgs) },
                    Message::HeldCheckDone,
                );
            }
            Message::HeldCheckDone(result) => match result {
                Ok(text) => {
                    self.hold_check_report = Some(text);
                    self.status_message = Some(abs_i18n::t("gui.msg.hold_check_done").into());
                }
                Err(e) => {
                    self.hold_check_report = Some(abs_i18n::tf("gui.msg.error", &[("e", &e)]));
                    self.status_message =
                        Some(abs_i18n::tf("gui.msg.hold_check_failed", &[("e", &e)]));
                }
            },
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
                // us the abs process exited. Prefer PID-file liveness; only fall back to "parked"
                // stages (reboot gates / done / aborted) when the PID file is already gone.
                // Never treat live compile stages (`stage2_build` / `stage3_build`) as parked —
                // those are written for the entire build and previously cleared busy mid-compile.
                if self.busy
                    && self
                        .external_run_since
                        .map(|t| t.elapsed() >= std::time::Duration::from_secs(6))
                        .unwrap_or(false)
                {
                    let pid_alive =
                        abs_runner::pid_file_process_alive(self.external_pid_path.as_deref());
                    // Reboot gates / done / aborted mean abs returned control (the terminal shell
                    // may still be alive waiting for Enter — that must not keep us "busy").
                    let parked = matches!(
                        status.stage.as_str(),
                        "wait_reboot1" | "wait_reboot2" | "done" | "aborted"
                    );
                    if parked || !pid_alive {
                        self.busy = false;
                        self.external_run_since = None;
                        self.external_pid_path = None;
                        self.append_log(abs_i18n::tf(
                            "gui.msg.abs_reached",
                            &[
                                ("stage", &status.stage_label),
                                ("next", &status.next_action),
                            ],
                        ));
                    }
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
                    return self.push_log(abs_i18n::t("gui.msg.pgo_wait_phase"));
                }
                if is_valid_pgo_phase_key(&stage) {
                    self.pgo_selected_stage = stage;
                }
            }
            Message::PgoRestartFromScratch => {
                let pkg = self
                    .selected_kernel
                    .as_deref()
                    .unwrap_or_else(|| abs_i18n::t("gui.msg.kernel_fallback"));
                let status = abs_i18n::tf("gui.msg.pgo_from_scratch", &[("pkg", pkg)]);
                return self.launch_pgo_run(PgoAction::Restart, None, false, &status);
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
                let pkg = self
                    .selected_kernel
                    .as_deref()
                    .unwrap_or_else(|| abs_i18n::t("gui.msg.kernel_fallback"));
                let status =
                    abs_i18n::tf("gui.msg.pgo_phase_for", &[("label", label), ("pkg", pkg)]);
                return self.launch_pgo_run(PgoAction::Resume, stage_arg, true, &status);
            }
            Message::PgoContinueAfterReboot => {
                let pkg = self
                    .selected_kernel
                    .as_deref()
                    .unwrap_or_else(|| abs_i18n::t("gui.msg.kernel_fallback"));
                let status = abs_i18n::tf("gui.msg.pgo_continue_reboot", &[("pkg", pkg)]);
                return self.launch_pgo_run(PgoAction::Resume, None, true, &status);
            }
            Message::KernelBuildStart => {
                let Some(pkg) = self.selected_kernel.clone() else {
                    return Task::none();
                };
                if self.busy {
                    return self.push_log(abs_i18n::t("gui.msg.busy_build"));
                }
                self.list_editors.apply_all(&mut self.config);
                if !self.config.packages.contains_key(&pkg) {
                    let msg = abs_i18n::tf("gui.msg.pkg_not_saved", &[("pkg", &pkg)]);
                    self.status_message = Some(msg.clone());
                    return self.push_log(abs_i18n::tf("gui.msg.cannot_build", &[("e", &msg)]));
                }
                if let Err(e) = abs_runner::verify_abs_binary() {
                    self.status_message = Some(e.clone());
                    return self.push_log(abs_i18n::tf("gui.msg.cannot_build", &[("e", &e)]));
                }
                if let Some(e) = self.ramdisk_size_save_error() {
                    self.status_message = Some(e.clone());
                    return self.push_log(abs_i18n::tf("gui.msg.cannot_build_save", &[("e", &e)]));
                }
                self.busy = true;
                self.building_oneshot = true;
                self.pgo_run.reset();
                self.build_log.autoscroll = true;
                self.build_log.pinned = true;
                self.log_inbox.lock().unwrap().clear();
                self.log_flush_scheduled.store(false, Ordering::Release);
                self.last_log_target = LogSaveTarget::Build;
                self.last_event_log_path = None;
                self.status_message =
                    Some(abs_i18n::tf("gui.msg.oneshot_status", &[("pkg", &pkg)]));
                self.append_log(abs_i18n::tf("gui.msg.oneshot_log", &[("pkg", &pkg)]));
                let path = self.config_path.clone();
                let doc = self.config.clone();
                if let Err(e) = save_config(&path, &doc) {
                    self.busy = false;
                    self.building_oneshot = false;
                    self.status_message = Some(e.clone());
                    return self.push_log(abs_i18n::tf("gui.msg.cannot_build_save", &[("e", &e)]));
                }
                self.append_log(abs_i18n::tf(
                    "gui.msg.saved_path",
                    &[("path", &path.display().to_string())],
                ));
                let abs_cmd = abs_runner::format_abs_pgo_command(
                    PgoAction::KernelBuild,
                    &pkg,
                    None,
                    None,
                    false,
                    false,
                );
                self.append_log(format!("$ {abs_cmd}"));
                match abs_runner::launch_in_terminal(&abs_cmd, None) {
                    Ok(term) => {
                        // The build is interactive in its own window; nothing to track in-app.
                        self.busy = false;
                        self.building_oneshot = false;
                        self.append_log(abs_i18n::tf(
                            "gui.msg.launched_oneshot",
                            &[("term", &term)],
                        ));
                        return Task::none();
                    }
                    Err(e) => {
                        self.append_log(abs_i18n::tf("gui.msg.terminal_fallback", &[("e", &e)]));
                        let handle = self.pgo_run.clone();
                        return Task::stream(Self::absorb_abs_stream(
                            self.log_inbox.clone(),
                            self.log_flush_scheduled.clone(),
                            stream_abs_pgo(
                                PgoAction::KernelBuild,
                                pkg,
                                None,
                                None,
                                false,
                                false,
                                handle,
                            ),
                        ));
                    }
                }
            }
            Message::SystemUpdateStart => {
                let has_work = self.pending_updates.as_ref().is_some_and(|p| p.has_work());
                if !has_work {
                    return self.push_log(abs_i18n::t("gui.msg.no_updates"));
                }
                return self.start_abs_on_update_page(
                    abs_runner::format_abs_system_update_command(),
                    "Running abs -RU…".into(),
                    true,
                );
            }
            Message::PendingUpdatesRefresh => {
                if self.pending_updates_loading {
                    return Task::none();
                }
                if let Err(e) = abs_runner::verify_abs_binary() {
                    self.pending_updates_error = Some(e.clone());
                    self.status_message = Some(e);
                    return Task::none();
                }
                self.pending_updates_loading = true;
                self.pending_updates_error = None;
                return Task::perform(
                    async { fetch_pending_updates() },
                    Message::PendingUpdatesLoaded,
                );
            }
            Message::PendingUpdatesLoaded(result) => {
                self.pending_updates_loading = false;
                match result {
                    Ok(data) => {
                        self.pending_updates_error = None;
                        self.pending_updates = Some(data);
                    }
                    Err(e) => {
                        self.pending_updates_error = Some(e);
                    }
                }
            }
            Message::InstallRepoUpdates => {
                let names: Vec<String> = self
                    .pending_updates
                    .as_ref()
                    .map(|p| p.repo.iter().map(|pkg| pkg.name.clone()).collect())
                    .unwrap_or_default();
                if names.is_empty() {
                    return self.push_log(abs_i18n::t("gui.msg.no_repo_updates"));
                }
                return self.start_abs_on_update_page(
                    abs_runner::format_install_repo_updates(&names),
                    abs_i18n::t("gui.msg.installing_repo").into(),
                    false,
                );
            }
            Message::InstallAur(pkg) => {
                if pkg.trim().is_empty() {
                    return self.push_log(abs_i18n::t("gui.msg.no_aur_selected"));
                }
                return self.start_abs_on_update_page(
                    abs_runner::format_install_aur(&pkg),
                    abs_i18n::tf("gui.msg.installing_aur", &[("pkg", &pkg)]),
                    false,
                );
            }
            Message::PreviewPkgbuild(name) => {
                let name = name.trim().to_string();
                let packages_path = self.config.paths.packages_path.clone();
                self.pkgbuild_preview = Some(PkgbuildPreview::loading(name.clone()));
                return Task::perform(
                    async move {
                        let result = abs_runner::fetch_aur_pkgbuild_preview(&name, &packages_path);
                        (name, result)
                    },
                    |(requested, result)| Message::PkgbuildLoaded { requested, result },
                );
            }
            Message::PkgbuildLoaded { requested, result } => {
                let Some(preview) = self.pkgbuild_preview.as_mut() else {
                    return Task::none();
                };
                if preview.name != requested {
                    return Task::none();
                }
                match result {
                    Ok(pkg) => {
                        preview.name = pkg.name;
                        preview.version = Some(pkg.version);
                        preview.text = Some(pkg.text);
                        preview.delta = pkg.delta;
                        preview.show_delta = false;
                        preview.error = None;
                    }
                    Err(e) => {
                        preview.error = Some(e);
                        preview.text = None;
                        preview.delta = None;
                    }
                }
            }
            Message::ClosePkgbuildPreview => {
                if self.pending_package_confirm.take().is_some() {
                    return Task::none();
                }
                self.pkgbuild_preview = None;
            }
            Message::CopyPkgbuild => {
                if let Some(text) = self.pkgbuild_preview.as_ref().and_then(|p| p.copy_text()) {
                    return clipboard::write(text);
                }
            }
            Message::TogglePkgbuildDelta => {
                if let Some(preview) = self.pkgbuild_preview.as_mut() {
                    if preview.text.is_some() {
                        preview.show_delta = !preview.show_delta;
                    }
                }
            }
            Message::SystemUpdateAbort => {
                if !self.running_system_update || !self.busy {
                    return self.push_log(abs_i18n::t("gui.msg.no_update_running"));
                }
                self.status_message = Some(abs_i18n::t("gui.msg.aborting_update").into());
                self.pgo_run.stop_running_build(None);
                return self.push_log(abs_i18n::t("gui.msg.aborting_update_log"));
            }
            Message::PgoAbort => {
                if self.running_system_update {
                    return Task::done(Message::SystemUpdateAbort);
                }
                let Some(pkg) = self.selected_kernel.clone() else {
                    return Task::none();
                };
                if !self.busy && self.external_run_since.is_none() {
                    return self.push_log(abs_i18n::t("gui.msg.no_pgo_active"));
                }
                self.status_message =
                    Some(abs_i18n::tf("gui.msg.aborting_build", &[("pkg", &pkg)]));
                let handle = self.pgo_run.clone();
                let run_pgo_abort = !self.building_oneshot;
                let pid_path = self.external_pid_path.clone();
                handle.stop_running_build(pid_path.as_deref());
                let scroll = self.push_log(abs_i18n::tf(
                    "gui.msg.aborting_build_cleanup",
                    &[("pkg", &pkg)],
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
            Message::LogFlush => {
                self.log_flush_scheduled.store(false, Ordering::Release);
                if !self.drain_log_inbox() {
                    return Task::none();
                }
                return self.snap_log_if_following();
            }
            Message::PgoRunFinished(Ok(output)) => {
                let _ = self.drain_log_inbox();
                self.busy = false;
                self.external_run_since = None;
                self.external_pid_path = None;
                let was_oneshot = self.building_oneshot;
                let was_sys_update = self.running_system_update;
                self.building_oneshot = false;
                self.running_system_update = false;
                let label = if was_sys_update {
                    abs_i18n::t("gui.msg.label_system_update")
                } else if was_oneshot {
                    abs_i18n::t("gui.msg.label_kernel_build")
                } else {
                    abs_i18n::t("gui.msg.label_pgo")
                };
                if output.user_aborted {
                    if was_oneshot || was_sys_update {
                        self.status_message =
                            Some(abs_i18n::tf("gui.msg.run_aborted", &[("label", label)]));
                        if was_sys_update {
                            return Task::done(Message::PendingUpdatesRefresh);
                        }
                        return Task::none();
                    }
                    return Task::done(Message::RefreshPgoStatus);
                } else if !output.success {
                    let code = output.exit_code.unwrap_or(-1);
                    let event_hint = output
                        .event_log
                        .as_ref()
                        .map(|p| {
                            abs_i18n::tf("gui.msg.event_log", &[("path", &p.display().to_string())])
                        })
                        .unwrap_or_default();
                    let panel = if was_sys_update {
                        abs_i18n::t("gui.msg.panel_update")
                    } else {
                        abs_i18n::t("gui.msg.panel_build")
                    };
                    let code_s = code.to_string();
                    self.status_message = Some(abs_i18n::tf(
                        "gui.msg.run_failed",
                        &[("label", label), ("code", &code_s), ("panel", panel)],
                    ));
                    let log = self.push_log(abs_i18n::tf(
                        "gui.msg.abs_failed",
                        &[("code", &code_s), ("event_hint", &event_hint)],
                    ));
                    if was_sys_update {
                        return Task::batch([log, Task::done(Message::PendingUpdatesRefresh)]);
                    }
                    return log;
                } else {
                    self.status_message = Some(abs_i18n::tf("gui.msg.run_ok", &[("label", label)]));
                }
                if was_sys_update {
                    return Task::done(Message::PendingUpdatesRefresh);
                }
                if was_oneshot {
                    return Task::none();
                }
                return Task::done(Message::RefreshPgoStatus);
            }
            Message::PgoRunFinished(Err(e)) => {
                let _ = self.drain_log_inbox();
                let was_sys_update = self.running_system_update;
                self.busy = false;
                self.building_oneshot = false;
                self.running_system_update = false;
                self.external_run_since = None;
                self.external_pid_path = None;
                self.status_message = Some(abs_i18n::tf("gui.msg.build_error", &[("e", &e)]));
                let log = self.push_log(abs_i18n::tf("gui.msg.error", &[("e", &e)]));
                if was_sys_update {
                    return Task::batch([log, Task::done(Message::PendingUpdatesRefresh)]);
                }
                return log;
            }
            Message::PgoAbortFinished(Ok(msg)) => {
                let _ = self.drain_log_inbox();
                self.busy = false;
                self.building_oneshot = false;
                self.running_system_update = false;
                self.external_run_since = None;
                self.external_pid_path = None;
                if !msg.trim().is_empty() {
                    self.append_log(msg.trim().to_string());
                }
                self.append_log(abs_i18n::t("gui.msg.pipeline_stopped"));
                self.status_message = Some(abs_i18n::t("gui.msg.aborted").into());
                return Task::done(Message::RefreshPgoStatus);
            }
            Message::PgoAbortFinished(Err(e)) => {
                let _ = self.drain_log_inbox();
                self.busy = false;
                self.building_oneshot = false;
                self.running_system_update = false;
                self.external_run_since = None;
                self.external_pid_path = None;
                self.status_message = Some(abs_i18n::tf("gui.msg.abort_failed", &[("e", &e)]));
                return self.push_log(abs_i18n::tf("gui.msg.abort_failed", &[("e", &e)]));
            }
            Message::LogClear => {
                self.log_inbox.lock().unwrap().clear();
                self.log_flush_scheduled.store(false, Ordering::Release);
                self.visible_log_mut().clear();
            }
            Message::AbsStdinChanged(value) => {
                self.abs_stdin_draft = value;
            }
            Message::AbsStdinSubmit => {
                let line = std::mem::take(&mut self.abs_stdin_draft);
                match self.pgo_run.write_stdin(&format!("{line}\n")) {
                    Ok(()) => {}
                    Err(e) => {
                        self.status_message = Some(e.clone());
                        return self.push_log(e);
                    }
                }
            }
            Message::LogCopy => {
                let _ = self.drain_log_inbox();
                return clipboard::write(self.log_text());
            }
            Message::LogSave => {
                let _ = self.drain_log_inbox();
                if self.visible_log().is_empty() {
                    return Task::none();
                }
                let target = self.log_save_target();
                let template = self.gui_settings.log_save_path(target).to_string();
                let format = self.gui_settings.log_save_format(target);
                let dont_ask = self.gui_settings.log_save_dont_ask(target);
                let ctx = ExpandCtx::now(target, format);
                let suggested = suggested_save_path(&template, &ctx);
                let text = self.log_text();
                if dont_ask && !template.trim().is_empty() {
                    return Task::perform(
                        async move {
                            log_save::write_log(&suggested, format, &text)
                                .map(|()| (suggested, None))
                        },
                        Message::LogSaveFinished,
                    );
                }
                let title = target.dialog_title();
                return Task::perform(
                    async move { dialog::save_file(title, &suggested, format) },
                    Message::LogSavePicked,
                );
            }
            Message::LogSavePicked(None) => {}
            Message::LogSavePicked(Some(path)) => {
                let target = self.log_save_target();
                let format = format_from_path(&path)
                    .unwrap_or_else(|| self.gui_settings.log_save_format(target));
                let template = self.gui_settings.log_save_path(target).to_string();
                let new_template = remember_save_dir(&template, &path);
                let text = self.log_text();
                return Task::perform(
                    async move {
                        log_save::write_log(&path, format, &text)
                            .map(|()| (path, Some(new_template)))
                    },
                    Message::LogSaveFinished,
                );
            }
            Message::LogSaveFinished(Ok((path, new_template))) => {
                if let Some(template) = new_template {
                    self.gui_settings
                        .set_log_save_path(self.log_save_target(), template);
                }
                self.persist_gui_settings();
                self.status_message = Some(abs_i18n::tf("gui.msg.saved_log", &[("path", &path)]));
            }
            Message::LogSaveFinished(Err(e)) => {
                self.status_message = Some(abs_i18n::tf("gui.msg.save_log_failed", &[("e", &e)]));
            }
            Message::LogSavePath(target, path) => {
                self.gui_settings.set_log_save_path(target, path);
            }
            Message::LogSaveBrowse(target) => {
                let current = self.gui_settings.log_save_path(target).to_string();
                return Task::perform(
                    async move { dialog::pick_log_folder(target, &current) },
                    move |picked| Message::LogSaveFolderPicked(target, picked),
                );
            }
            Message::LogSaveFolderPicked(_, None) => {}
            Message::LogSaveFolderPicked(target, Some(folder)) => {
                let existing = self.gui_settings.log_save_path(target).to_string();
                let joined = apply_folder(&existing, &folder, DEFAULT_FILENAME);
                self.gui_settings.set_log_save_path(target, joined);
                self.persist_gui_settings();
            }
            Message::LogSaveDontAsk(target, v) => {
                self.gui_settings.set_log_save_dont_ask(target, v);
                self.persist_gui_settings();
            }
            Message::LogSaveFormat(target, format) => {
                let path = self.gui_settings.log_save_path(target).to_string();
                self.gui_settings
                    .set_log_save_path(target, replace_known_extension(&path, format));
                self.gui_settings.set_log_save_format(target, format);
                self.persist_gui_settings();
            }
            Message::ViewportScrolled(id, at_bottom) => {
                {
                    let pane = self.log_pane_mut(id);
                    if Instant::now() < pane.ignore_scroll_until {
                        return Task::none();
                    }
                    if !pane.autoscroll {
                        pane.pinned = false;
                        return Task::none();
                    }
                    if pane.pinned == at_bottom {
                        return Task::none();
                    }
                    pane.pinned = at_bottom;
                    if !at_bottom {
                        // Scrolling away pauses follow so the button matches the viewport.
                        pane.autoscroll = false;
                        return Task::none();
                    }
                    pane.ignore_next_scrolls();
                }
                let _ = self.drain_log_inbox();
                return operation::snap_to_end(id.scroll_id());
            }
            Message::ViewportAutoscroll(id, enabled) => {
                {
                    let pane = self.log_pane_mut(id);
                    pane.autoscroll = enabled;
                    pane.pinned = enabled;
                    if !enabled {
                        return Task::none();
                    }
                    pane.ignore_next_scrolls();
                }
                let _ = self.drain_log_inbox();
                return operation::snap_to_end(id.scroll_id());
            }
            Message::SystemMetricsTick => {
                self.metrics_sampler.sample();
                let scheme = system_theme::detect();
                if scheme != self.taskbar_scheme {
                    self.taskbar_scheme = scheme;
                }
            }
            Message::WizardFormLoaded(result) => {
                self.wizard.on_form_loaded(result);
            }
            Message::WizardFieldChanged(id, value, immediate) => {
                return self.wizard.set_value(id, value, immediate);
            }
            Message::WizardCheckResult(gen, id, result) => {
                self.wizard.on_check_result(gen, id, result);
            }
            Message::WizardStepChecked(errors) => {
                if self.wizard.on_step_checked(errors) {
                    if let Some(form) = self.wizard.form.as_ref() {
                        if self.wizard.step + 1 < form.steps.len() {
                            self.wizard.step += 1;
                        }
                    }
                }
            }
            Message::WizardBrowse(id, kind) => {
                let current = self
                    .wizard
                    .answers
                    .get(&id)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                return Task::perform(
                    async move { dialog::pick_path_generic(kind, &current) },
                    move |picked| Message::WizardPathPicked(id, picked),
                );
            }
            Message::WizardPathPicked(id, picked) => {
                if let Some(path) = picked {
                    return self.wizard.set_value(id, serde_json::json!(path), true);
                }
            }
            Message::WizardUseSuggested(id) => {
                return self.wizard.use_suggested(id);
            }
            Message::WizardListDraft(id, draft) => {
                self.wizard.list_draft.insert(id, draft);
            }
            Message::WizardListAdd(id) => {
                let draft = self.wizard.list_draft.remove(&id).unwrap_or_default();
                let name = draft.trim();
                if name.is_empty() {
                    return Task::none();
                }
                let mut items: Vec<String> = self
                    .wizard
                    .answers
                    .get(&id)
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                if !items.iter().any(|x| x == name) {
                    items.push(name.to_string());
                }
                return self.wizard.set_value(id, serde_json::json!(items), true);
            }
            Message::WizardRepoDraftName(s) => self.wizard.repo_draft_name = s,
            Message::WizardRepoDraftUrl(s) => self.wizard.repo_draft_url = s,
            Message::WizardRepoAdd(id) => {
                let name = self.wizard.repo_draft_name.trim().to_string();
                let url = self.wizard.repo_draft_url.trim().to_string();
                if name.is_empty() || url.is_empty() {
                    return Task::none();
                }
                let mut obj = self
                    .wizard
                    .answers
                    .get(&id)
                    .and_then(|v| v.as_object())
                    .cloned()
                    .unwrap_or_default();
                let mut entries = obj
                    .get("entries")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                entries.push(serde_json::json!({"name": name, "url": url}));
                obj.insert("entries".into(), serde_json::Value::Array(entries));
                if obj
                    .get("default")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .is_empty()
                {
                    obj.insert(
                        "default".into(),
                        serde_json::json!(self.wizard.repo_draft_name.trim()),
                    );
                }
                self.wizard.repo_draft_name.clear();
                self.wizard.repo_draft_url.clear();
                return self
                    .wizard
                    .set_value(id, serde_json::Value::Object(obj), true);
            }
            Message::WizardNext => {
                return self.wizard.validate_current_step();
            }
            Message::WizardBack => {
                if self.wizard.step > 0 {
                    self.wizard.step -= 1;
                }
            }
            Message::WizardApply => {
                self.wizard.applying = true;
                return self.wizard.apply_task();
            }
            Message::WizardApplyDone(result) => {
                self.wizard.applying = false;
                match &result {
                    Ok(path) => {
                        self.status_message =
                            Some(abs_i18n::tf("gui.wizard.saved", &[("path", path.as_str())]));
                        self.wizard.apply_result = Some(result);
                        let path = self.config_path.clone();
                        return Task::perform(async move { load_config(&path) }, |r| {
                            Message::ConfigLoaded(Box::new(r))
                        });
                    }
                    Err(_) => {
                        self.wizard.apply_result = Some(result);
                    }
                }
            }
            Message::WizardCancel => {
                let next = if self.config_path.exists() {
                    Page::AbsSettings
                } else {
                    Page::Kernels
                };
                self.navigate(next);
            }
            Message::WizardTimer => {
                return self.wizard.on_timer();
            }
            Message::WindowResized(size) => {
                self.viewport_width = size.width;
                return Self::query_size_placement(size);
            }
            Message::WindowMoved(point) => {
                return Self::query_position_placement(point);
            }
            Message::WindowSizeCommitted {
                size,
                fullscreen,
                maximized,
            } => {
                self.viewport_width = size.width;
                self.gui_settings
                    .apply_live_size(size, fullscreen, maximized);
            }
            Message::WindowPositionCommitted {
                point,
                fullscreen,
                maximized,
            } => {
                self.gui_settings
                    .apply_live_position(point, fullscreen, maximized);
            }
            Message::WindowClampToMonitor {
                id,
                monitor,
                size,
                position,
            } => {
                if self.gui_settings.window_fullscreen {
                    return Task::none();
                }
                let source_size = if self.gui_settings.window_maximized {
                    Size::new(
                        self.gui_settings.window_width,
                        self.gui_settings.window_height,
                    )
                } else {
                    size
                };
                let source_pos = if self.gui_settings.window_maximized {
                    match (self.gui_settings.window_x, self.gui_settings.window_y) {
                        (Some(x), Some(y)) => Some(Point::new(x, y)),
                        _ => position,
                    }
                } else {
                    position
                };
                let (clamped_size, clamped_pos) =
                    clamp_window_geometry(source_size, source_pos, monitor);
                self.gui_settings.set_size(clamped_size);
                if let Some(point) = clamped_pos {
                    self.gui_settings.set_position(point);
                }
                if self.gui_settings.window_maximized {
                    return Task::none();
                }
                let mut tasks = Vec::new();
                if clamped_size != size {
                    tasks.push(window::resize(id, clamped_size));
                }
                match (clamped_pos, position) {
                    (Some(new), Some(old)) if new != old => {
                        tasks.push(window::move_to(id, new));
                    }
                    (Some(new), None) => {
                        tasks.push(window::move_to(id, new));
                    }
                    _ => {}
                }
                return Task::batch(tasks);
            }
            Message::WindowCloseSnapshot(snapshot) => {
                if let Some(snap) = snapshot {
                    self.gui_settings.apply_close_snapshot(
                        snap.fullscreen,
                        snap.maximized,
                        snap.size,
                        snap.position,
                    );
                }
                let _ = self.gui_settings.save();
                return self.begin_exit();
            }
            Message::WindowCloseRequested => {
                return Self::snapshot_window_then_close();
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
            Page::SystemUpdate => system_update::view(
                self.busy,
                self.running_system_update,
                self.pending_updates.as_ref(),
                self.pending_updates_error.as_deref(),
                self.pending_updates_loading,
                self.update_log.autoscroll,
                self.update_log.pinned,
                &self.update_log.lines,
                theme,
                self.log_palette(),
                &self.abs_stdin_draft,
                self.pgo_run.stdin_open(),
            ),
            Page::AbsSettings => {
                let (ram_total, ram_used) = ramdisk_size::mem_total_and_used().unwrap_or((0, 0));
                abs_settings::view(
                    &self.config,
                    &self.list_editors,
                    self.hold_check_report.as_deref(),
                    self.settings_tab,
                    ram_total,
                    ram_used,
                    theme,
                )
            }
            Page::ConfigWizard => config_wizard::view(&self.wizard, theme),
            Page::AppSettings => self.view_app_settings(theme),
        };

        // Window-wide page: same left/right inset as the top nav and status bar.
        let pad_x = self.chrome_pad_x();
        let page_pad = Padding {
            top: 16.0,
            right: pad_x,
            bottom: 16.0,
            left: pad_x,
        };
        let main: Element<Message> = scroll_viewport(
            container(content).padding(page_pad).width(Length::Fill),
            style::page_scroll(theme),
            Length::Fill,
            Length::Fill,
            page_scroll_id(self.page),
        );
        let ui: Element<Message> =
            column![self.view_top_nav(theme), main, self.view_status_bar(theme)]
                .height(Length::Fill)
                .into();
        if self.pkgbuild_preview.is_none()
            && self.pending_leave.is_none()
            && self.pending_package_confirm.is_none()
        {
            return ui;
        }
        let mut layers = vec![ui];
        if let Some(preview) = &self.pkgbuild_preview {
            layers.push(
                opaque(
                    container(center(pkgbuild_preview_dialog(preview, theme)))
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .style(style::modal_scrim(theme)),
                )
                .into(),
            );
        }
        if self.pending_leave.is_some() {
            layers.push(
                opaque(
                    container(center(unsaved_changes_dialog(theme)))
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .style(style::modal_scrim(theme)),
                )
                .into(),
            );
        }
        if let Some(confirm) = &self.pending_package_confirm {
            let (title, body, confirm_label) = match confirm {
                PackageConfirm::Remove(name) => (
                    abs_i18n::t("gui.packages.confirm_remove_title").to_string(),
                    abs_i18n::tf("gui.packages.confirm_remove_body", &[("name", name)]),
                    abs_i18n::t("gui.common.remove"),
                ),
                PackageConfirm::PurgeAll => (
                    abs_i18n::t("gui.packages.confirm_purge_title").to_string(),
                    abs_i18n::tf(
                        "gui.packages.confirm_purge_body",
                        &[("count", &self.config.packages.len().to_string())],
                    ),
                    abs_i18n::t("gui.packages.purge_all"),
                ),
            };
            layers.push(
                opaque(
                    container(center(confirm_dialog(
                        title,
                        body,
                        confirm_label,
                        Message::PackageConfirmAccept,
                        Message::PackageConfirmCancel,
                        theme,
                    )))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(style::modal_scrim(theme)),
                )
                .into(),
            );
        }
        stack(layers)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn view_top_nav(&self, theme: AppTheme) -> Element<'_, Message> {
        let kernels_active = matches!(
            self.page,
            Page::Kernels | Page::KernelConfig | Page::DefaultKernelConfig
        );
        let logo_handle = match theme {
            AppTheme::Dark => self.dark_icon_handle.clone(),
            AppTheme::Light => self.light_icon_handle.clone(),
        };
        let logo_img = image(logo_handle)
            .width(Length::Fixed(28.0))
            .height(Length::Fixed(28.0));
        let brand = row![
            logo_img,
            column![
                text(abs_i18n::t("gui.chrome.app_name"))
                    .size(15)
                    .font(Font {
                        weight: iced::font::Weight::Bold,
                        ..Font::DEFAULT
                    })
                    .color(style::primary(theme)),
                text(concat!("v", env!("CARGO_PKG_VERSION")))
                    .size(style::TEXT_CHIP)
                    .color(style::muted(theme)),
            ],
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        let kernel_n = style::KERNEL_CATALOG.len().to_string();
        let pkg_n = self.config.packages.len().to_string();
        let update_badge = self
            .pending_updates
            .as_ref()
            .map(|p| (p.repo.len() + p.aur.len() + p.manual.len()).to_string());

        let tabs = row![
            widgets::top_nav_tab(
                "⬡",
                abs_i18n::t("gui.nav.kernels"),
                Some(kernel_n),
                kernels_active,
                theme,
                Message::OpenKernels,
            ),
            widgets::top_nav_tab(
                "📦",
                abs_i18n::t("gui.nav.packages"),
                Some(pkg_n),
                matches!(self.page, Page::Packages | Page::PackageConfig),
                theme,
                Message::OpenPackages,
            ),
            widgets::top_nav_tab(
                "🔄",
                abs_i18n::t("gui.nav.system_update"),
                update_badge,
                self.page == Page::SystemUpdate,
                theme,
                Message::OpenSystemUpdate,
            ),
            widgets::top_nav_tab(
                "⚙",
                abs_i18n::t("gui.nav.abs_settings"),
                None,
                self.page == Page::AbsSettings,
                theme,
                Message::OpenAbsSettings,
            ),
            widgets::top_nav_tab(
                "🧙",
                abs_i18n::t("gui.nav.config_wizard"),
                None,
                self.page == Page::ConfigWizard,
                theme,
                Message::OpenConfigWizard,
            ),
            widgets::top_nav_tab(
                "🎨",
                abs_i18n::t("gui.nav.app_settings"),
                None,
                self.page == Page::AppSettings,
                theme,
                Message::OpenAppSettings,
            ),
        ]
        .spacing(4)
        .align_y(Alignment::Center);

        container(
            row![
                brand,
                tabs,
                Space::new().width(Length::Fill),
                crate::metrics::hardware_pill_widget(&self.metrics_sampler.current, theme),
            ]
            .spacing(16)
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .height(Length::Fixed(48.0)),
        )
        .padding(Padding::from([6.0, self.chrome_pad_x()]))
        .width(Length::Fill)
        .style(style::top_nav(theme))
        .into()
    }

    fn view_status_bar(&self, theme: AppTheme) -> Element<'_, Message> {
        let palette = style::iced_theme(theme).palette();
        let left = if let Some(ref msg) = self.status_message {
            let lower = msg.to_ascii_lowercase();
            let is_error = ["fail", "error", "abort"]
                .iter()
                .any(|needle| lower.contains(needle));
            text(msg.clone()).size(style::TEXT_BODY).color(if is_error {
                palette.danger
            } else {
                palette.primary
            })
        } else if let Some(ref err) = self.config_error {
            text(abs_i18n::tf("gui.status.config_error", &[("err", err)]))
                .size(style::TEXT_BODY)
                .color(palette.danger)
        } else {
            text(self.config_path.display().to_string())
                .size(style::TEXT_BODY)
                .color(style::muted(theme))
        };
        let status_label = if self.running_system_update {
            abs_i18n::t("gui.nav.updating")
        } else if self.busy || self.building_oneshot {
            abs_i18n::t("gui.nav.running")
        } else {
            abs_i18n::t("gui.nav.ready")
        };
        let right = row![
            container(Space::new())
                .width(Length::Fixed(6.0))
                .height(Length::Fixed(6.0))
                .style(style::status_dot(theme)),
            text(format!(
                "absgui v{} · {status_label}",
                env!("CARGO_PKG_VERSION")
            ))
            .size(style::TEXT_CHIP)
            .font(Font {
                weight: iced::font::Weight::Semibold,
                ..Font::DEFAULT
            })
            .color(style::muted(theme)),
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        container(
            row![left, Space::new().width(Length::Fill), right]
                .align_y(Alignment::Center)
                .width(Length::Fill),
        )
        .padding(Padding::from([4.0, self.chrome_pad_x()]))
        .style(style::status_bar(theme))
        .width(Length::Fill)
        .into()
    }

    fn boot_indicator(&self, theme: AppTheme) -> Element<'_, Message> {
        let release = self
            .metrics_sampler
            .current
            .boot_release
            .as_deref()
            .unwrap_or("—");
        let mut inner = row![
            container(Space::new())
                .width(Length::Fixed(7.0))
                .height(Length::Fixed(7.0))
                .style(style::status_dot(theme)),
            text(abs_i18n::t("gui.chrome.current_boot"))
                .size(style::TEXT_CHIP)
                .color(style::muted(theme)),
            text(release).size(style::TEXT_CHIP).font(Font {
                weight: iced::font::Weight::Bold,
                family: iced::font::Family::Monospace,
                ..Font::DEFAULT
            }),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        if let Some(ref scx) = self.metrics_sampler.current.sched_ext {
            inner = inner.push(text("|").size(11).color(style::surface_border(theme)));
            inner = inner.push(
                text(scx.clone())
                    .size(style::TEXT_CHIP)
                    .font(Font {
                        family: iced::font::Family::Monospace,
                        ..Font::DEFAULT
                    })
                    .color(style::primary(theme)),
            );
        }
        container(inner)
            .padding(Padding::from([4.0, 12.0]))
            .style(style::boot_pill(theme))
            .into()
    }

    fn view_kernels(&self, theme: AppTheme) -> Element<'_, Message> {
        let filter_lower = self.kernel_filter.trim().to_ascii_lowercase();
        let catalog_count = |needle: &str| {
            style::KERNEL_CATALOG
                .iter()
                .filter(|(name, sched, desc)| kernel_catalog_match(name, sched, desc, needle))
                .count()
        };

        let quick_filters = row![
            widgets::filter_chip(
                abs_i18n::t("gui.kernels.all"),
                catalog_count(""),
                filter_lower.is_empty(),
                theme,
                Message::KernelFilter(String::new()),
            ),
            widgets::filter_chip(
                abs_i18n::t("gui.kernels.filter_bore"),
                catalog_count("bore"),
                filter_lower == "bore",
                theme,
                Message::KernelFilter("bore".to_string()),
            ),
            widgets::filter_chip(
                abs_i18n::t("gui.kernels.filter_eevdf"),
                catalog_count("eevdf"),
                filter_lower == "eevdf",
                theme,
                Message::KernelFilter("eevdf".to_string()),
            ),
            widgets::filter_chip(
                abs_i18n::t("gui.kernels.filter_lto"),
                catalog_count("lto"),
                filter_lower == "lto",
                theme,
                Message::KernelFilter("lto".to_string()),
            ),
            widgets::filter_chip(
                abs_i18n::t("gui.kernels.filter_rt"),
                catalog_count("rt"),
                filter_lower.contains("rt"),
                theme,
                Message::KernelFilter("rt".to_string()),
            ),
        ]
        .spacing(5)
        .align_y(Alignment::Center);

        let boot = self.boot_indicator(theme);
        let mut col = column![
            widgets::breadcrumb_row(
                abs_i18n::t("gui.nav.kernels"),
                abs_i18n::t("gui.kernels.title").to_string(),
                Some(boot),
                theme,
            ),
            text(abs_i18n::t("gui.kernels.subtitle"))
                .size(style::TEXT_HELP)
                .color(style::muted(theme)),
            row![
                widgets::search_bar(
                    &self.kernel_filter,
                    abs_i18n::t("gui.kernels.search"),
                    theme,
                    Message::KernelFilter,
                    Some(widgets::SEARCH_KERNELS),
                ),
                quick_filters,
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        ]
        .spacing(10);

        let lto_val = self
            .config
            .kernel_defaults
            .kernel
            .as_ref()
            .and_then(|k| k.use_llvm_lto.as_deref())
            .unwrap_or("none");
        let pgo_on = self
            .config
            .kernel_defaults
            .pgo
            .as_ref()
            .map(|p| p.enabled)
            .unwrap_or(false);
        let default_card = container(
            row![
                container(Space::new())
                    .width(Length::Fixed(4.0))
                    .height(Length::Fill)
                    .style(style::accent_bar(theme)),
                column![
                    row![
                        text(abs_i18n::t("gui.kernels.default_config"))
                            .size(13)
                            .font(Font {
                                weight: iced::font::Weight::Bold,
                                ..Font::DEFAULT
                            }),
                        container(
                            text(abs_i18n::tf("gui.kernels.lto_pill", &[("value", lto_val)]))
                                .size(10)
                                .font(Font::MONOSPACE)
                        )
                        .padding(Padding::from([1.0, 6.0]))
                        .style(style::tag(theme)),
                        container(
                            text(if pgo_on {
                                abs_i18n::t("gui.kernels.autofdo_on")
                            } else {
                                abs_i18n::t("gui.kernels.autofdo_off")
                            })
                            .size(10)
                            .font(Font::MONOSPACE)
                        )
                        .padding(Padding::from([1.0, 6.0]))
                        .style(style::tag_status_style(theme, pgo_on)),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                    text(abs_i18n::t("gui.kernels.default_config_help"))
                        .size(11)
                        .color(style::muted(theme)),
                ]
                .spacing(2)
                .width(Length::Fill),
                button(text(abs_i18n::t("gui.kernels.quick_edit")).size(12))
                    .padding(Padding::from([6.0, 12.0]))
                    .style(style::btn_secondary(theme))
                    .on_press(Message::OpenDefaultConfig),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([8.0, 14.0]))
        .width(Length::Fill)
        .style(style::card_banner(theme));
        col = col.push(default_card);

        let matching_kernels: Vec<_> = style::KERNEL_CATALOG
            .iter()
            .copied()
            .filter(|(name, sched, desc)| kernel_catalog_match(name, sched, desc, &filter_lower))
            .collect();

        if matching_kernels.is_empty() {
            col = col.push(card_section(
                abs_i18n::t("gui.kernels.no_matching"),
                theme,
                text(abs_i18n::tf(
                    "gui.kernels.no_matching_body",
                    &[("filter", &filter_lower)],
                ))
                .size(13)
                .color(style::muted(theme)),
            ));
        } else {
            const KERNEL_COLS: &[u16] = &[3, 2, 2, 4, 2, 2];
            let header = container(dense_table_row(
                vec![
                    dense_header_cell(abs_i18n::t("gui.kernels.col_package"), theme),
                    dense_header_cell(abs_i18n::t("gui.kernels.col_scheduler"), theme),
                    dense_header_cell(abs_i18n::t("gui.kernels.col_spec"), theme),
                    dense_header_cell(abs_i18n::t("gui.kernels.col_description"), theme),
                    dense_header_cell(abs_i18n::t("gui.kernels.col_status"), theme),
                    dense_header_cell(abs_i18n::t("gui.kernels.col_action"), theme),
                ],
                KERNEL_COLS,
                true,
                false,
                false,
                theme,
            ))
            .style(style::dense_table_head(theme));
            let mut body = column![].spacing(0);
            let boot_rel = self.metrics_sampler.current.boot_release.as_deref();
            for (name, sched, desc) in matching_kernels {
                let configured = self.config.packages.contains_key(name);
                let current = boot_matches(boot_rel, name);
                let hovered = self.hovered_kernel.as_deref() == Some(name);
                let (spec_label, spec_kind) = kernel_spec_tag(name, sched);
                let status =
                    if configured {
                        container(text(abs_i18n::t("gui.common.configured")).size(10.5).font(
                            Font {
                                weight: iced::font::Weight::Bold,
                                ..Font::DEFAULT
                            },
                        ))
                        .padding(Padding::from([2.0, 8.0]))
                        .style(style::tag_success(theme))
                    } else {
                        container(text(abs_i18n::t("gui.kernels.tag_ready")).size(10.5))
                            .padding(Padding::from([2.0, 8.0]))
                            .style(style::tag_muted(theme))
                    };
                let name_cell = row![
                    kernel_status_dot(configured, hovered, theme),
                    text(name).size(12.5).font(Font {
                        weight: iced::font::Weight::Bold,
                        family: iced::font::Family::Monospace,
                        ..Font::DEFAULT
                    }),
                ]
                .spacing(8)
                .align_y(Alignment::Center);
                let action = button(
                    text(if configured {
                        abs_i18n::t("gui.common.manage")
                    } else {
                        abs_i18n::t("gui.common.configure")
                    })
                    .size(11),
                )
                .padding(Padding::from([4.0, 12.0]))
                .style(style::catalog_btn_style(theme, configured))
                .on_press(Message::OpenKernel(name.to_string()));
                body = body.push(interactive_list_row(
                    dense_table_row(
                        vec![
                            name_cell.into(),
                            container(text(sched).size(10.5).font(Font {
                                weight: iced::font::Weight::Bold,
                                family: iced::font::Family::Monospace,
                                ..Font::DEFAULT
                            }))
                            .padding(Padding::from([2.0, 7.0]))
                            .style(style::tag_sched(theme, sched))
                            .into(),
                            container(text(spec_label).size(10.5).font(Font {
                                weight: iced::font::Weight::Semibold,
                                ..Font::DEFAULT
                            }))
                            .padding(Padding::from([2.0, 7.0]))
                            .style(style::tag_spec(theme, spec_kind))
                            .into(),
                            text(desc).size(11.5).color(style::muted(theme)).into(),
                            status.into(),
                            action.into(),
                        ],
                        KERNEL_COLS,
                        false,
                        current,
                        hovered,
                        theme,
                    ),
                    Message::KernelRowEnter(name.to_string()),
                    Message::KernelRowExit(name.to_string()),
                    Message::OpenKernel(name.to_string()),
                ));
            }
            col = col.push(dense_table(header, body, theme));
        }

        let custom_name = self.custom_kernel.trim().to_string();
        let custom_btn = button(text(abs_i18n::t("gui.kernels.custom_add")).size(12))
            .style(style::btn_secondary(theme));
        let custom_btn = if custom_name.is_empty() {
            custom_btn
        } else {
            custom_btn.on_press(Message::OpenKernel(custom_name))
        };
        col = col.push(
            container(
                row![
                    text(abs_i18n::t("gui.kernels.custom")).size(12).font(Font {
                        weight: iced::font::Weight::Bold,
                        ..Font::DEFAULT
                    }),
                    text_input(
                        abs_i18n::t("gui.kernels.custom_placeholder"),
                        &self.custom_kernel
                    )
                    .on_input(Message::CustomKernelChanged)
                    .padding(6)
                    .width(Length::Fill),
                    custom_btn,
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            )
            .padding(Padding::from([7.0, 14.0]))
            .width(Length::Fill)
            .style(style::card(theme)),
        );

        col.into()
    }

    fn view_default_config(&self, theme: AppTheme) -> Element<'_, Message> {
        let pkg = &self.config.kernel_defaults;
        column![
            widgets::breadcrumb_row(
                abs_i18n::t("gui.nav.kernels"),
                abs_i18n::t("gui.kernels.default_title").to_string(),
                Some(
                    button(text(abs_i18n::t("gui.kernels.back_kernels")).size(13))
                        .style(style::btn_secondary(theme))
                        .on_press(Message::Back)
                        .into(),
                ),
                theme,
            ),
            text(abs_i18n::t("gui.kernels.default_seed"))
                .size(style::TEXT_HELP)
                .color(style::muted(theme)),
            kernel_form(EditTarget::Default, pkg, theme),
            button(text(abs_i18n::t("gui.kernels.save_default")).size(14))
                .style(style::btn_primary(theme))
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
            return text(abs_i18n::t("gui.kernels.no_selected")).into();
        };

        let is_ramdisk_active = pkg
            .ramdisk
            .as_deref()
            .map(|r| !r.is_empty())
            .unwrap_or(self.config.ramdisk.enabled);

        let lto_str = pkg
            .kernel
            .as_ref()
            .and_then(|k| k.use_llvm_lto.as_deref())
            .or_else(|| {
                self.config
                    .kernel_defaults
                    .kernel
                    .as_ref()
                    .and_then(|k| k.use_llvm_lto.as_deref())
            })
            .unwrap_or("none");

        let hz_str = pkg
            .kernel
            .as_ref()
            .and_then(|k| k.hz_ticks.as_deref().or(k.tickrate.as_deref()))
            .or_else(|| {
                self.config
                    .kernel_defaults
                    .kernel
                    .as_ref()
                    .and_then(|k| k.hz_ticks.as_deref().or(k.tickrate.as_deref()))
            })
            .unwrap_or("1000");

        let header_banner = container(
            row![
                container(
                    text(abs_i18n::tf(
                        "gui.kernels.scheduler_tag",
                        &[("sched", sched)]
                    ))
                    .size(12)
                    .font(Font {
                        weight: iced::font::Weight::Bold,
                        ..Font::DEFAULT
                    })
                )
                .padding(Padding::from([4.0, 12.0]))
                .style(style::tag_sched(theme, sched)),
                container(
                    text(abs_i18n::tf("gui.kernels.lto_tag", &[("lto", lto_str)]))
                        .size(12)
                        .font(Font {
                            weight: iced::font::Weight::Medium,
                            ..Font::DEFAULT
                        })
                )
                .padding(Padding::from([4.0, 12.0]))
                .style(style::tag(theme)),
                container(
                    text(abs_i18n::tf("gui.kernels.tick_tag", &[("hz", hz_str)]))
                        .size(12)
                        .font(Font {
                            weight: iced::font::Weight::Medium,
                            ..Font::DEFAULT
                        })
                )
                .padding(Padding::from([4.0, 12.0]))
                .style(style::tag_muted(theme)),
                container(
                    text(if is_ramdisk_active {
                        abs_i18n::t("gui.kernels.ramdisk_on")
                    } else {
                        abs_i18n::t("gui.kernels.ramdisk_off")
                    })
                    .size(12)
                    .font(Font {
                        weight: iced::font::Weight::Medium,
                        ..Font::DEFAULT
                    })
                )
                .padding(Padding::from([4.0, 12.0]))
                .style(style::tag_status_style(theme, is_ramdisk_active)),
                Space::new().width(Length::Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([10.0, 14.0]))
        .width(Length::Fill)
        .style(style::card_banner(theme));

        let actions = row![
            button(text(abs_i18n::t("gui.kernels.back_kernels")).size(13))
                .style(style::btn_secondary(theme))
                .on_press(Message::Back),
            button(text(abs_i18n::t("gui.common.save_config")).size(13))
                .style(style::btn_primary(theme))
                .on_press(Message::SaveConfig),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        column![
            widgets::breadcrumb_row(
                abs_i18n::t("gui.nav.kernels"),
                name.clone(),
                Some(actions.into()),
                theme,
            ),
            header_banner,
            self.view_pgo_pipeline(theme),
            self.view_oneshot_build(theme),
            self.view_log(theme),
            kernel_form(EditTarget::Selected, pkg, theme),
        ]
        .spacing(16)
        .into()
    }

    fn view_packages(&self, theme: AppTheme) -> Element<'_, Message> {
        let names: Vec<_> = self.config.packages.keys().cloned().collect();
        let chip_count = |f: PackageListFilter| {
            names
                .iter()
                .filter(|n| package_matches_chip(n, &self.config.packages[*n], f))
                .count()
        };
        let search = self.package_filter.trim().to_ascii_lowercase();
        let chips = row![
            widgets::filter_chip(
                abs_i18n::t("gui.packages.filter_all"),
                chip_count(PackageListFilter::All),
                self.package_list_filter == PackageListFilter::All,
                theme,
                Message::PackageListFilter(PackageListFilter::All),
            ),
            widgets::filter_chip(
                abs_i18n::t("gui.packages.filter_kernels"),
                chip_count(PackageListFilter::Kernels),
                self.package_list_filter == PackageListFilter::Kernels,
                theme,
                Message::PackageListFilter(PackageListFilter::Kernels),
            ),
            widgets::filter_chip(
                abs_i18n::t("gui.packages.filter_pgo"),
                chip_count(PackageListFilter::PgoLto),
                self.package_list_filter == PackageListFilter::PgoLto,
                theme,
                Message::PackageListFilter(PackageListFilter::PgoLto),
            ),
            widgets::filter_chip(
                abs_i18n::t("gui.packages.filter_aur"),
                chip_count(PackageListFilter::Aur),
                self.package_list_filter == PackageListFilter::Aur,
                theme,
                Message::PackageListFilter(PackageListFilter::Aur),
            ),
            widgets::filter_chip(
                abs_i18n::t("gui.packages.filter_official"),
                chip_count(PackageListFilter::Official),
                self.package_list_filter == PackageListFilter::Official,
                theme,
                Message::PackageListFilter(PackageListFilter::Official),
            ),
        ]
        .spacing(5)
        .align_y(Alignment::Center);

        let mut col = column![
            widgets::breadcrumb_row(
                abs_i18n::t("gui.nav.packages"),
                abs_i18n::t("gui.packages.hub").to_string(),
                None,
                theme,
            ),
            text(abs_i18n::t("gui.packages.subtitle"))
                .size(style::TEXT_HELP)
                .color(style::muted(theme)),
            row![
                widgets::search_bar(
                    &self.package_filter,
                    abs_i18n::t("gui.packages.search"),
                    theme,
                    Message::PackageFilter,
                    Some(widgets::SEARCH_PACKAGES),
                ),
                chips,
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        ]
        .spacing(10);

        let add_name = self.new_package_name.trim().to_string();
        let add_btn =
            button(text(abs_i18n::t("gui.packages.add")).size(13)).style(style::btn_primary(theme));
        let add_btn = if add_name.is_empty() {
            add_btn
        } else {
            add_btn.on_press(Message::PackageAdd)
        };
        col = col.push(
            container(
                row![
                    text(abs_i18n::t("gui.packages.add_card"))
                        .size(style::TEXT_LABEL)
                        .font(Font {
                            weight: iced::font::Weight::Bold,
                            ..Font::DEFAULT
                        }),
                    text_input(
                        abs_i18n::t("gui.packages.add_placeholder"),
                        &self.new_package_name
                    )
                    .on_input(Message::NewPackageNameChanged)
                    .on_submit(Message::PackageAdd)
                    .padding(6)
                    .width(Length::Fill),
                    add_btn,
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            )
            .padding(Padding::from([8.0, 14.0]))
            .width(Length::Fill)
            .style(style::card(theme)),
        );

        let mut filtered_names: Vec<_> = names
            .into_iter()
            .filter(|n| {
                let pkg = &self.config.packages[n];
                (search.is_empty() || n.to_ascii_lowercase().contains(&search))
                    && package_matches_chip(n, pkg, self.package_list_filter)
            })
            .collect();
        sort_package_names(
            &mut filtered_names,
            &self.config.packages,
            self.package_sort,
            self.package_sort_desc,
        );

        if filtered_names.is_empty() {
            col = col.push(card_section(
                abs_i18n::t("gui.packages.configured"),
                theme,
                text(if self.config.packages.is_empty() {
                    abs_i18n::t("gui.packages.none_yet")
                } else {
                    abs_i18n::t("gui.packages.none_match")
                })
                .size(style::TEXT_HELP)
                .color(style::muted(theme)),
            ));
        } else {
            const PKG_COLS: &[u16] = &[3, 2, 3, 1, 1, 4];
            let header = container(dense_table_row(
                vec![
                    dense_sort_header_cell(
                        abs_i18n::t("gui.packages.col_name"),
                        PackageSortCol::Name,
                        self.package_sort,
                        self.package_sort_desc,
                        theme,
                    ),
                    dense_sort_header_cell(
                        abs_i18n::t("gui.packages.col_source"),
                        PackageSortCol::Source,
                        self.package_sort,
                        self.package_sort_desc,
                        theme,
                    ),
                    dense_sort_header_cell(
                        abs_i18n::t("gui.packages.col_flags"),
                        PackageSortCol::Flags,
                        self.package_sort,
                        self.package_sort_desc,
                        theme,
                    ),
                    dense_sort_header_cell(
                        abs_i18n::t("gui.packages.col_threads"),
                        PackageSortCol::Threads,
                        self.package_sort,
                        self.package_sort_desc,
                        theme,
                    ),
                    dense_sort_header_cell(
                        abs_i18n::t("gui.packages.col_isolation"),
                        PackageSortCol::Isolation,
                        self.package_sort,
                        self.package_sort_desc,
                        theme,
                    ),
                    dense_header_cell(abs_i18n::t("gui.packages.col_action"), theme),
                ],
                PKG_COLS,
                true,
                false,
                false,
                theme,
            ))
            .style(style::dense_table_head(theme));
            let mut body = column![].spacing(0);
            for name in &filtered_names {
                let pkg = &self.config.packages[name];
                let hovered = self.hovered_package.as_deref() == Some(name.as_str());
                let is_kernel = pkg.kernel.is_some() || pkg.pgo.is_some();
                let mut name_row = row![text(name.clone()).size(12.5).font(Font {
                    weight: iced::font::Weight::Bold,
                    family: iced::font::Family::Monospace,
                    ..Font::DEFAULT
                })]
                .spacing(6)
                .align_y(Alignment::Center);
                if is_kernel {
                    name_row = name_row.push(
                        container(text(abs_i18n::t("gui.kernels.tag_kernel")).size(9.5))
                            .padding(Padding::from([1.0, 6.0]))
                            .style(style::tag_info(theme)),
                    );
                }
                let source = package_source_label(pkg);
                let mut flags = row![].spacing(4).align_y(Alignment::Center);
                let mut any_flag = false;
                if let Some(c) = &pkg.compiler {
                    any_flag = true;
                    flags = flags.push(
                        container(text(c.as_str()).size(10))
                            .padding(Padding::from([1.0, 6.0]))
                            .style(style::tag(theme)),
                    );
                }
                if let Some(lto) = pkg.kernel.as_ref().and_then(|k| k.use_llvm_lto.as_deref()) {
                    if lto != "none" {
                        any_flag = true;
                        flags = flags.push(
                            container(text(format!("LTO:{lto}")).size(10))
                                .padding(Padding::from([1.0, 6.0]))
                                .style(style::tag_spec(theme, "lto")),
                        );
                    }
                }
                if pkg.pgo.as_ref().is_some_and(|p| p.enabled) {
                    any_flag = true;
                    let preset = pkg.pgo.as_ref().map(|p| p.preset.as_str()).unwrap_or("pgo");
                    flags = flags.push(
                        container(text(format!("PGO:{preset}")).size(10))
                            .padding(Padding::from([1.0, 6.0]))
                            .style(style::tag_success(theme)),
                    );
                }
                if pkg.ramdisk.as_ref().is_some_and(|r| !r.is_empty()) {
                    any_flag = true;
                    flags = flags.push(
                        container(text("ramdisk").size(10))
                            .padding(Padding::from([1.0, 6.0]))
                            .style(style::tag_muted(theme)),
                    );
                }
                if pkg.tests == Some(true) {
                    any_flag = true;
                    flags = flags.push(
                        container(text("tests").size(10))
                            .padding(Padding::from([1.0, 6.0]))
                            .style(style::tag_muted(theme)),
                    );
                }
                let flags_el: Element<'_, Message> = if any_flag {
                    flags.into()
                } else {
                    text(abs_i18n::t("gui.packages.unset"))
                        .size(11)
                        .color(style::muted(theme))
                        .into()
                };
                let threads = pkg
                    .compilation_threads
                    .map(|n| format!("-j{n}"))
                    .unwrap_or_else(|| abs_i18n::t("gui.packages.unset").to_string());
                let isolation = package_isolation_label(pkg);
                let mut actions = row![].spacing(6).align_y(Alignment::Center);
                if package_is_aur(pkg) {
                    actions = actions.push(preview_pkgbuild_button(name.clone(), 11.0, theme));
                }
                actions = actions.push(
                    button(text(abs_i18n::t("gui.common.configure")).size(11))
                        .padding(Padding::from([4.0, 10.0]))
                        .style(style::btn_secondary(theme))
                        .on_press(Message::OpenPackage(name.clone())),
                );
                actions = actions.push(
                    button(text(abs_i18n::t("gui.common.remove")).size(11))
                        .padding(Padding::from([4.0, 10.0]))
                        .style(style::btn_danger(theme))
                        .on_press(Message::PackageRemove(name.clone())),
                );
                body = body.push(interactive_list_row(
                    dense_table_row(
                        vec![
                            name_row.into(),
                            text(source).size(11.5).color(style::muted(theme)).into(),
                            flags_el,
                            text(threads).size(11.5).font(Font::MONOSPACE).into(),
                            container(text(isolation).size(10.5))
                                .padding(Padding::from([2.0, 7.0]))
                                .style(style::isolation_tag(theme, pkg.compile_alone))
                                .into(),
                            actions.into(),
                        ],
                        PKG_COLS,
                        false,
                        false,
                        hovered,
                        theme,
                    ),
                    Message::PackageRowEnter(name.clone()),
                    Message::PackageRowExit(name.clone()),
                    Message::OpenPackage(name.clone()),
                ));
            }
            col = col.push(dense_table(header, body, theme));
        }
        let purge = button(text(abs_i18n::t("gui.packages.purge_all")).size(13))
            .padding(Padding::from([8.0, 16.0]))
            .style(style::btn_danger(theme));
        let purge = if self.config.packages.is_empty() {
            purge
        } else {
            purge.on_press(Message::PackagePurgeAll)
        };
        col = col.push(purge);
        col.into()
    }

    fn view_package_config(&self, theme: AppTheme) -> Element<'_, Message> {
        let name = self
            .selected_package
            .clone()
            .unwrap_or_else(|| "—".to_string());
        let Some(pkg) = self.target_pkg(EditTarget::Package) else {
            return text(abs_i18n::t("gui.packages.no_selected")).into();
        };
        let is_kernel = pkg.kernel.is_some() || pkg.pgo.is_some();
        let show_pkgbuild = package_is_aur(pkg);

        let mut col = column![widgets::breadcrumb_row(
            abs_i18n::t("gui.nav.packages"),
            name.clone(),
            Some(
                button(text(abs_i18n::t("gui.packages.back")).size(13))
                    .style(style::btn_secondary(theme))
                    .on_press(Message::Back)
                    .into(),
            ),
            theme,
        ),]
        .spacing(12);

        if is_kernel {
            col = col.push(
                text(abs_i18n::t("gui.packages.kernel_note"))
                    .size(style::TEXT_HELP)
                    .color(style::muted(theme)),
            );
        }

        col = col.push(package_form(EditTarget::Package, pkg, theme));
        let mut actions = row![button(text(abs_i18n::t("gui.packages.save")).size(14))
            .style(style::btn_primary(theme))
            .on_press(Message::SaveConfig),]
        .spacing(8)
        .align_y(Alignment::Center);
        if show_pkgbuild {
            actions = actions.push(preview_pkgbuild_button(name.clone(), 14.0, theme));
        }
        actions = actions.push(
            button(text(abs_i18n::t("gui.packages.exit")).size(14))
                .style(style::btn_secondary(theme))
                .on_press(Message::Back),
        );
        actions = actions.push(
            button(text(abs_i18n::t("gui.packages.remove")).size(14))
                .style(style::btn_danger(theme))
                .on_press(Message::PackageRemove(name)),
        );
        col = col.push(actions);
        col.into()
    }

    fn view_app_settings(&self, theme: AppTheme) -> Element<'_, Message> {
        let exe_path = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "?".into());
        let abs_bin = crate::abs_runner::abs_binary();
        let about = card_section(
            abs_i18n::t("gui.settings.about"),
            theme,
            column![
                text(format!(
                    "absgui {} ({})",
                    env!("CARGO_PKG_VERSION"),
                    env!("ABSGUI_BUILD_ID")
                ))
                .size(16)
                .font(Font {
                    weight: iced::font::Weight::Semibold,
                    ..Font::DEFAULT
                }),
                text(abs_i18n::tf(
                    "gui.settings.running_exe",
                    &[("path", exe_path.as_str())],
                ))
                .size(style::TEXT_BODY),
                text(abs_i18n::tf(
                    "gui.settings.abs_binary",
                    &[("path", abs_bin.as_str())],
                ))
                .size(style::TEXT_BODY),
                text(abs_i18n::t("gui.settings.pgo_note"))
                    .size(style::TEXT_BODY)
                    .color(style::muted(theme)),
            ]
            .spacing(8),
        );
        let appearance = card_section(
            abs_i18n::t("gui.settings.appearance"),
            theme,
            column![
                field_label_column(
                    abs_i18n::t("gui.settings.theme"),
                    Some(abs_i18n::t("gui.settings.theme_help")),
                    theme,
                    app_theme_toggle(self.gui_settings.theme, theme),
                ),
                language_picker_field(&self.gui_settings, theme),
                stepper_number(
                    abs_i18n::t("gui.settings.terminal_lines"),
                    Some(field_help::terminal_lines_limit()),
                    &self.terminal_lines_limit_input,
                    theme,
                    Message::TerminalLinesLimitInput,
                    Message::TerminalLinesLimitDec,
                    Message::TerminalLinesLimitInc,
                ),
                text(abs_i18n::t("gui.settings.window_geom"))
                    .size(style::TEXT_BODY)
                    .color(style::muted(theme)),
            ]
            .spacing(10),
        );
        let terminal = card_section(
            abs_i18n::t("gui.settings.terminal_colors"),
            theme,
            terminal_theme_picker(
                self.terminal_preview_dark,
                self.gui_settings.terminal_theme_dark,
                self.terminal_preview_light,
                self.gui_settings.terminal_theme_light,
                theme,
            ),
        );
        let logs = card_section(
            abs_i18n::t("gui.settings.logs"),
            theme,
            column![
                help_line(field_help::log_save(), theme),
                log_save_row(
                    LogSaveTarget::Build,
                    &self.gui_settings.log_save_build,
                    self.gui_settings.log_save_build_dont_ask,
                    self.gui_settings.log_save_build_format,
                    theme,
                ),
                log_save_row(
                    LogSaveTarget::Update,
                    &self.gui_settings.log_save_update,
                    self.gui_settings.log_save_update_dont_ask,
                    self.gui_settings.log_save_update_format,
                    theme,
                ),
            ]
            .spacing(12),
        );
        column![
            widgets::breadcrumb_row(
                abs_i18n::t("gui.nav.app_settings"),
                abs_i18n::t("gui.settings.hub").to_string(),
                None,
                theme,
            ),
            row![
                column![about, appearance, logs]
                    .spacing(12)
                    .width(Length::Fill),
                column![terminal].spacing(12).width(Length::Fill),
            ]
            .spacing(12),
            button(text(abs_i18n::t("gui.settings.save")).size(14))
                .style(style::btn_primary(theme))
                .on_press(Message::SaveAppSettings),
        ]
        .spacing(12)
        .into()
    }

    fn view_oneshot_build(&self, theme: AppTheme) -> Element<'_, Message> {
        card_section(
            abs_i18n::t("gui.kernels.oneshot"),
            theme,
            column![
                text(abs_i18n::t("gui.kernels.oneshot_help"))
                    .size(style::TEXT_HELP)
                    .color(style::muted(theme)),
                row![
                    button(text(abs_i18n::t("gui.kernels.build_now")).size(13))
                        .style(style::btn_primary(theme))
                        .on_press_maybe((!self.busy).then_some(Message::KernelBuildStart)),
                    button(text(abs_i18n::t("gui.common.abort")).size(13))
                        .style(style::btn_danger(theme))
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
        let show_start_from_phase =
            !at_wait_reboot && self.pgo_selected_stage != pgo_first_phase_key();

        let n = PGO_STEPS.len() as u16;
        let (done_n, active_n) = if saved == "done" {
            (n, 0)
        } else if let Some(idx) = saved_idx {
            (idx as u16, 1)
        } else if self.busy {
            (pgo_stage_index(selected).unwrap_or(0) as u16, 1)
        } else {
            (0, 0)
        };

        let steps: Vec<crate::widgets::PgoRoundStep> = PGO_STEPS
            .iter()
            .copied()
            .enumerate()
            .map(|(i, (_, key))| {
                let is_selected = selected == key;
                let is_saved = saved == key || (saved == "done" && key == "stage3_build");
                let is_done = saved == "done" || saved_idx.is_some_and(|idx| i < idx);
                let is_active = if self.busy {
                    is_selected && !is_done
                } else {
                    is_saved && !is_done
                };
                crate::widgets::PgoRoundStep {
                    key,
                    label: pgo_stage_label(key),
                    done: is_done,
                    active: is_active,
                    selected: is_selected && !is_done && !is_active,
                }
            })
            .collect();
        let timeline = pgo_round_pipeline(steps, done_n, active_n, !self.busy, theme);

        let status_row: Element<'_, Message> = if let Some(ref s) = self.pgo_status {
            let mut badges = row![].spacing(8).align_y(Alignment::Center);
            badges = badges.push(
                container(
                    text(abs_i18n::tf(
                        "gui.pgo.phase",
                        &[("label", pgo_stage_label(selected))],
                    ))
                    .size(11),
                )
                .padding(pill_padding())
                .style(style::tag_info(theme)),
            );
            if !s.stage.is_empty() && s.stage_label != "No pipeline" {
                badges = badges.push(
                    container(
                        text(abs_i18n::tf(
                            "gui.pgo.saved",
                            &[("label", s.stage_label.as_str())],
                        ))
                        .size(11),
                    )
                    .padding(pill_padding())
                    .style(style::tag_muted(theme)),
                );
            }
            if s.reboot_required {
                badges = badges.push(
                    container(text(abs_i18n::t("gui.pgo.reboot")).size(11))
                        .padding(pill_padding())
                        .style(style::tag_warning(theme)),
                );
            } else if s.boot_ready {
                badges = badges.push(
                    container(text(abs_i18n::t("gui.pgo.boot_ready")).size(11))
                        .padding(pill_padding())
                        .style(style::tag_success(theme)),
                );
            }
            if let Some(ref uname) = s.expected_kernel_uname {
                badges = badges.push(
                    container(
                        text(abs_i18n::tf(
                            "gui.pgo.expected",
                            &[("uname", uname.as_str())],
                        ))
                        .size(11),
                    )
                    .padding(pill_padding())
                    .style(style::tag_muted(theme)),
                );
            }
            badges.into()
        } else if self.busy {
            row![container(text(abs_i18n::t("gui.pgo.active")).size(11))
                .padding(pill_padding())
                .style(style::tag_info(theme)),]
            .into()
        } else if let Some(ref e) = self.pgo_status_error {
            text(abs_i18n::tf(
                "gui.pgo.no_pipeline_err",
                &[("e", e.as_str())],
            ))
            .size(style::TEXT_HELP)
            .color(style::muted(theme))
            .into()
        } else {
            text(abs_i18n::t("gui.pgo.no_pipeline"))
                .size(style::TEXT_HELP)
                .color(style::muted(theme))
                .into()
        };

        let mut action_row = row![button(text(abs_i18n::t("gui.pgo.start_scratch")).size(13))
            .style(style::btn_primary(theme))
            .on_press_maybe((!self.busy).then_some(Message::PgoRestartFromScratch)),]
        .spacing(8);
        if at_wait_reboot {
            action_row = action_row.push(
                button(text(abs_i18n::t("gui.pgo.continue_reboot")).size(13))
                    .style(style::btn_primary(theme))
                    .on_press_maybe((!self.busy).then_some(Message::PgoContinueAfterReboot)),
            );
        }
        if show_start_from_phase {
            action_row = action_row.push(
                button(text(abs_i18n::t("gui.pgo.start_phase")).size(13))
                    .style(style::btn_secondary(theme))
                    .on_press_maybe((!self.busy).then_some(Message::PgoStartFromPhase)),
            );
        }
        action_row = action_row.push(
            button(text(abs_i18n::t("gui.common.abort")).size(13))
                .style(style::btn_danger(theme))
                .on_press(Message::PgoAbort),
        );

        card_section(
            abs_i18n::t("gui.pgo.title"),
            theme,
            column![
                text(abs_i18n::t("gui.pgo.help"))
                    .size(style::TEXT_HELP)
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
            abs_i18n::tf(
                "gui.log.hint_json",
                &[("path", &path.display().to_string())],
            )
        } else {
            abs_i18n::t("gui.log.hint").to_string()
        };
        command_log(
            abs_i18n::t("gui.log.build"),
            hint,
            abs_i18n::t("gui.log.empty_build"),
            &self.build_log.lines,
            self.build_log.autoscroll,
            self.build_log.pinned,
            ViewportId::BuildLog,
            theme,
            self.log_palette(),
            COMMAND_LOG_PAGE_HEIGHT,
            &self.abs_stdin_draft,
            self.pgo_run.stdin_open(),
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
        abs_i18n::t("gui.kernels.options"),
        theme,
        column![
            row![
                field_pick(
                    abs_i18n::t("gui.field.scheduler"),
                    Some(field_help::cpusched()),
                    SCHED_OPTS,
                    &kstr_value(pkg, KStr::Cpusched),
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::Cpusched, v),
                ),
                field_pick(
                    abs_i18n::t("gui.field.processor_opt"),
                    Some(field_help::processor_opt()),
                    &["native", "x86-64-v2", "x86-64-v3", "x86-64-v4"],
                    &kstr_value(pkg, KStr::ProcessorOpt),
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::ProcessorOpt, v),
                ),
            ]
            .spacing(12),
            row![
                field_pick(
                    abs_i18n::t("gui.field.llvm_lto"),
                    Some(field_help::llvm_lto()),
                    LTO_OPTS,
                    &kstr_value(pkg, KStr::LlvmLto),
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::LlvmLto, v),
                ),
                field_pick(
                    abs_i18n::t("gui.field.hz_ticks"),
                    Some(field_help::hz_ticks()),
                    HZ_OPTS,
                    &kstr_value(pkg, KStr::HzTicks),
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::HzTicks, v),
                ),
            ]
            .spacing(12),
            row![
                field_pick(
                    abs_i18n::t("gui.field.tickrate"),
                    Some(field_help::tickrate()),
                    TICK_OPTS,
                    &kstr_value(pkg, KStr::Tickrate),
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::Tickrate, v),
                ),
                field_pick(
                    abs_i18n::t("gui.field.preempt"),
                    Some(field_help::preempt()),
                    PREEMPT_OPTS,
                    &kstr_value(pkg, KStr::Preempt),
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::Preempt, v),
                ),
            ]
            .spacing(12),
            field_pick(
                abs_i18n::t("gui.field.hugepage"),
                Some(field_help::hugepage()),
                HUGE_OPTS,
                &kstr_value(pkg, KStr::Hugepage),
                theme,
                move |v| Message::SetKernelStr(target, KStr::Hugepage, v),
            ),
            field_checkbox(
                abs_i18n::t("gui.field.cc_harder"),
                Some(field_help::cc_harder()),
                kbool_value(pkg, KBool::CcHarder),
                theme,
                move |v| Message::SetKernelBool(target, KBool::CcHarder, v),
            ),
            row![
                field_checkbox(
                    abs_i18n::t("gui.field.lto_suffix"),
                    Some(field_help::lto_suffix()),
                    kbool_value(pkg, KBool::LtoSuffix),
                    theme,
                    move |v| Message::SetKernelBool(target, KBool::LtoSuffix, v),
                ),
                field_checkbox(
                    abs_i18n::t("gui.field.gcc_suffix"),
                    Some(field_help::gcc_suffix()),
                    kbool_value(pkg, KBool::GccSuffix),
                    theme,
                    move |v| Message::SetKernelBool(target, KBool::GccSuffix, v),
                ),
                field_checkbox(
                    abs_i18n::t("gui.field.kcfi"),
                    Some(field_help::kcfi()),
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
        abs_i18n::t("gui.kernels.abs_build"),
        theme,
        column![
            row![
                field_pick(
                    abs_i18n::t("gui.field.source_repo"),
                    Some(field_help::source()),
                    SOURCE_OPTS,
                    &kstr_value(pkg, KStr::Source),
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::Source, v),
                ),
                field_pick(
                    abs_i18n::t("gui.field.build_env"),
                    Some(field_help::build_env()),
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
                    Some(field_help::package_compilation_threads()),
                    &pkg.compilation_threads
                        .map(|n| n.to_string())
                        .unwrap_or_default(),
                    theme,
                    move |v| Message::PackageCompilationThreads(target, v),
                ),
                field_number(
                    "compilation_priority",
                    Some(field_help::package_compilation_priority()),
                    &pkg.compilation_priority.to_string(),
                    theme,
                    move |v| Message::PackageCompilationPriority(target, v),
                ),
            ]
            .spacing(12),
            field_checkbox(
                "compile_alone",
                Some(field_help::package_compile_alone()),
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
        abs_i18n::t("gui.pgo.title"),
        theme,
        column![
            row![
                field_checkbox(
                    abs_i18n::t("gui.field.pgo_enabled"),
                    Some(field_help::pgo_enabled()),
                    kbool_value(pkg, KBool::PgoEnabled),
                    theme,
                    move |v| Message::SetKernelBool(target, KBool::PgoEnabled, v),
                ),
                field_checkbox(
                    abs_i18n::t("gui.field.pgo_auto_restart"),
                    Some(field_help::pgo_auto_restart()),
                    kbool_value(pkg, KBool::PgoAutoRestart),
                    theme,
                    move |v| Message::SetKernelBool(target, KBool::PgoAutoRestart, v),
                ),
            ]
            .spacing(16),
            row![
                field_checkbox(
                    abs_i18n::t("gui.field.pgo_verify_boot"),
                    Some(field_help::pgo_verify_boot()),
                    kbool_value(pkg, KBool::PgoVerifyBoot),
                    theme,
                    move |v| Message::SetKernelBool(target, KBool::PgoVerifyBoot, v),
                ),
                field_checkbox(
                    abs_i18n::t("gui.field.pgo_perf_data_on_ram"),
                    Some(field_help::pgo_perf_data_on_ram()),
                    kbool_value(pkg, KBool::PgoPerfDataOnRam),
                    theme,
                    move |v| Message::SetKernelBool(target, KBool::PgoPerfDataOnRam, v),
                ),
            ]
            .spacing(16),
            field_text(
                abs_i18n::t("gui.field.pgo_preset"),
                Some(field_help::pgo_preset()),
                &kstr_value(pkg, KStr::PgoPreset),
                "cachyos-kernel",
                theme,
                move |v| Message::SetKernelStr(target, KStr::PgoPreset, v),
            ),
            field_path(
                abs_i18n::t("gui.field.pgo_archive_dir"),
                Some(field_help::pgo_archive_dir()),
                &kstr_value(pkg, KStr::ArchiveDir),
                "/mnt/hdd/abs/pgo/profiles",
                WPathField::PgoArchiveDir,
                WPathKind::Folder,
                theme,
                move |v| Message::SetKernelStr(target, KStr::ArchiveDir, v),
            ),
            field_path(
                abs_i18n::t("gui.field.pgo_profile_scratch"),
                Some(field_help::pgo_profile_scratch()),
                &kstr_value(pkg, KStr::ProfileScratchDir),
                "auto",
                WPathField::PgoProfileScratchDir,
                WPathKind::Folder,
                theme,
                move |v| Message::SetKernelStr(target, KStr::ProfileScratchDir, v),
            ),
            field_path(
                abs_i18n::t("gui.field.pgo_state_file"),
                Some(field_help::pgo_state_file()),
                &kstr_value(pkg, KStr::StateFile),
                "(default: ~/.config/abs/pgo/PKG.json)",
                WPathField::PgoStateFile,
                WPathKind::File,
                theme,
                move |v| Message::SetKernelStr(target, KStr::StateFile, v),
            ),
            field_pick(
                abs_i18n::t("gui.field.pgo_profiling_quality"),
                Some(field_help::pgo_profiling_quality()),
                PGO_PROFILING_QUALITY_OPTS,
                &profiling_quality,
                theme,
                move |v| Message::SetKernelStr(target, KStr::ProfilingQuality, v),
            ),
            field_pick(
                abs_i18n::t("gui.field.pgo_benchmark_preset"),
                Some(field_help::pgo_benchmark_preset()),
                PGO_BENCHMARK_PRESET_OPTS,
                &benchmark_preset,
                theme,
                move |v| Message::SetKernelStr(target, KStr::BenchmarkPreset, v),
            ),
            field_path(
                abs_i18n::t("gui.field.pgo_benchmark"),
                Some(field_help::pgo_benchmark()),
                &kstr_value(pkg, KStr::Benchmark),
                "(bundled ABS benchmark if empty)",
                WPathField::PgoBenchmark,
                WPathKind::File,
                theme,
                move |v| Message::SetKernelStr(target, KStr::Benchmark, v),
            ),
            field_path(
                abs_i18n::t("gui.field.pgo_benchmark_workdir"),
                Some(field_help::pgo_benchmark_workdir()),
                &kstr_value(pkg, KStr::BenchmarkWorkdir),
                "(default: archive dir/benchmark-workdir)",
                WPathField::PgoBenchmarkWorkdir,
                WPathKind::Folder,
                theme,
                move |v| Message::SetKernelStr(target, KStr::BenchmarkWorkdir, v),
            ),
            row![
                field_text(
                    abs_i18n::t("gui.field.pgo_build_user"),
                    Some(field_help::pgo_build_user()),
                    &kstr_value(pkg, KStr::BuildUser),
                    "john",
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::BuildUser, v),
                ),
                field_text(
                    abs_i18n::t("gui.field.pgo_sysctl"),
                    Some(field_help::pgo_sysctl()),
                    &kstr_value(pkg, KStr::SysctlCommand),
                    "cachyos-perf-sysctl",
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::SysctlCommand, v),
                ),
            ]
            .spacing(12),
            field_text(
                abs_i18n::t("gui.field.pgo_perf_event_args"),
                Some(field_help::pgo_perf_event_args()),
                &kstr_value(pkg, KStr::PerfEventArgs),
                "auto",
                theme,
                move |v| Message::SetKernelStr(target, KStr::PerfEventArgs, v),
            ),
            field_text(
                abs_i18n::t("gui.field.pgo_perf_extra_args"),
                Some(field_help::pgo_perf_extra_args()),
                &kstr_value(pkg, KStr::PerfExtraArgs),
                "--mmap-pages 131072 -a -N -b -c 56000",
                theme,
                move |v| Message::SetKernelStr(target, KStr::PerfExtraArgs, v),
            ),
            field_path(
                abs_i18n::t("gui.field.pgo_vmlinux"),
                Some(field_help::pgo_vmlinux()),
                &kstr_value(pkg, KStr::Vmlinux),
                "auto",
                WPathField::PgoVmlinux,
                WPathKind::File,
                theme,
                move |v| Message::SetKernelStr(target, KStr::Vmlinux, v),
            ),
            row![
                field_text(
                    abs_i18n::t("gui.field.pgo_afdo_tool"),
                    Some(field_help::pgo_afdo_tool()),
                    &kstr_value(pkg, KStr::AfdoTool),
                    "llvm-profgen",
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::AfdoTool, v),
                ),
                field_text(
                    abs_i18n::t("gui.field.pgo_propeller_tool"),
                    Some(field_help::pgo_propeller_tool()),
                    &kstr_value(pkg, KStr::PropellerTool),
                    "auto",
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::PropellerTool, v),
                ),
            ]
            .spacing(12),
            field_text(
                abs_i18n::t("gui.field.pgo_afdo_profile_name"),
                Some(field_help::pgo_afdo_profile_name()),
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
        abs_i18n::t("gui.packages.source_build"),
        theme,
        column![
            row![
                field_pick(
                    "source",
                    Some(field_help::source()),
                    SOURCE_OPTS,
                    &kstr_value(pkg, KStr::Source),
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::Source, v),
                ),
                field_pick(
                    "build_env",
                    Some(field_help::build_env()),
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
                    Some(field_help::package_compiler()),
                    &kstr_value(pkg, KStr::Compiler),
                    "gcc14",
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::Compiler, v),
                ),
                field_text(
                    "alias (optional)",
                    Some(field_help::package_alias()),
                    &kstr_value(pkg, KStr::Alias),
                    "upstream-package-name",
                    theme,
                    move |v| Message::SetKernelStr(target, KStr::Alias, v),
                ),
            ]
            .spacing(12),
            optional_bool_field(
                "tests",
                Some(field_help::package_tests()),
                pkg.tests,
                abs_i18n::t("gui.field.tests_default"),
                theme,
                move |v| Message::SetPackageOptBool(target, KOptBool::Tests, v),
            ),
            ramdisk_targets_field(target, ramdisk_w, ramdisk_c, ramdisk_p, ramdisk_r, theme),
        ]
        .spacing(12),
    );

    let scheduling = card_section(
        abs_i18n::t("gui.packages.scheduling"),
        theme,
        column![
            row![
                field_number(
                    "compilation_threads (optional)",
                    Some(field_help::package_compilation_threads()),
                    &pkg.compilation_threads
                        .map(|n| n.to_string())
                        .unwrap_or_default(),
                    theme,
                    move |v| Message::PackageCompilationThreads(target, v),
                ),
                field_number(
                    "compilation_priority",
                    Some(field_help::package_compilation_priority()),
                    &pkg.compilation_priority.to_string(),
                    theme,
                    move |v| Message::PackageCompilationPriority(target, v),
                ),
            ]
            .spacing(12),
            field_checkbox(
                "compile_alone",
                Some(field_help::package_compile_alone()),
                pkg.compile_alone,
                theme,
                move |v| Message::PackageCompileAlone(target, v),
            ),
        ]
        .spacing(12),
    );

    let commands = card_section(
        abs_i18n::t("gui.packages.commands"),
        theme,
        column![
            field_text(
                "custom_local_build_command (optional)",
                Some(field_help::package_custom_local_cmd()),
                &kstr_value(pkg, KStr::CustomLocalBuildCommand),
                "makepkg -si --noconfirm",
                theme,
                move |v| Message::SetKernelStr(target, KStr::CustomLocalBuildCommand, v),
            ),
            field_text(
                "custom_chroot_build_command (optional)",
                Some(field_help::package_custom_chroot_cmd()),
                &kstr_value(pkg, KStr::CustomChrootBuildCommand),
                "makechrootpkg -r /path/to/chroot",
                theme,
                move |v| Message::SetKernelStr(target, KStr::CustomChrootBuildCommand, v),
            ),
            field_text(
                "pre_update_command (optional)",
                Some(field_help::package_pre_update_cmd()),
                &kstr_value(pkg, KStr::PreUpdateCommand),
                "systemctl stop myservice",
                theme,
                move |v| Message::SetKernelStr(target, KStr::PreUpdateCommand, v),
            ),
            field_text(
                "post_update_command (optional)",
                Some(field_help::package_post_update_cmd()),
                &kstr_value(pkg, KStr::PostUpdateCommand),
                "systemctl restart myservice",
                theme,
                move |v| Message::SetKernelStr(target, KStr::PostUpdateCommand, v),
            ),
        ]
        .spacing(12),
    );

    let upstream = card_section(
        abs_i18n::t("gui.packages.upstream"),
        theme,
        column![
            field_text(
                "upstream_github (optional)",
                Some(field_help::package_upstream_github()),
                &kstr_value(pkg, KStr::UpstreamGithub),
                "owner/repo",
                theme,
                move |v| Message::SetKernelStr(target, KStr::UpstreamGithub, v),
            ),
            optional_bool_field(
                "upstream_prereleases",
                Some(field_help::package_upstream_prereleases()),
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

fn language_picker_field(gui: &GuiSettings, theme: AppTheme) -> Element<'_, Message> {
    let inherit = abs_i18n::t("gui.settings.inherit");
    let mut opts: Vec<String> = vec![inherit.to_string()];
    opts.extend(abs_i18n::Lang::ALL.iter().map(|l| l.picker_label()));
    let selected = gui
        .lang
        .as_deref()
        .and_then(abs_i18n::Lang::parse)
        .map(|l| l.picker_label())
        .unwrap_or_else(|| inherit.to_string());
    field_label_column(
        abs_i18n::t("gui.settings.language"),
        Some(abs_i18n::t("gui.settings.language_help")),
        theme,
        crate::widgets::themed_pick_list(
            opts,
            Some(selected),
            |choice| {
                if choice == abs_i18n::t("gui.settings.inherit") {
                    Message::GuiLangSelected(None)
                } else {
                    Message::GuiLangSelected(
                        abs_i18n::Lang::ALL
                            .iter()
                            .find(|l| l.picker_label() == choice)
                            .map(|l| l.code().to_string()),
                    )
                }
            },
            theme,
            Length::Fill,
        ),
    )
}

fn kernel_catalog_match(name: &str, sched: &str, desc: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    name.to_ascii_lowercase().contains(needle)
        || sched.to_ascii_lowercase().contains(needle)
        || desc.to_ascii_lowercase().contains(needle)
}

fn kernel_spec_tag(name: &str, sched: &str) -> (&'static str, &'static str) {
    if sched == "BORE" || sched == "RT+BORE" {
        (abs_i18n::t("gui.kernels.spec_gaming"), "gaming")
    } else if name.contains("lto") {
        (abs_i18n::t("gui.kernels.spec_lto"), "lto")
    } else if name.contains("hardened") {
        (abs_i18n::t("gui.kernels.spec_security"), "security")
    } else if name.contains("server") {
        (abs_i18n::t("gui.kernels.spec_server"), "server")
    } else {
        (abs_i18n::t("gui.kernels.spec_general"), "general")
    }
}

fn boot_matches(release: Option<&str>, pkg: &str) -> bool {
    let Some(release) = release else {
        return false;
    };
    let flavor = pkg.strip_prefix("linux-").unwrap_or(pkg);
    release.ends_with(flavor)
}

fn package_is_aur(pkg: &PackageSection) -> bool {
    pkg.source
        .as_deref()
        .is_some_and(|s| s.eq_ignore_ascii_case("aur"))
}

fn package_source_label(pkg: &PackageSection) -> String {
    let mut parts = Vec::new();
    if let Some(src) = &pkg.source {
        parts.push(src.clone());
    }
    if let Some(up) = &pkg.upstream_github {
        parts.push(up.clone());
    }
    if parts.is_empty() {
        abs_i18n::t("gui.packages.unset").to_string()
    } else {
        parts.join(" · ")
    }
}

fn package_flags_sort_key(pkg: &PackageSection) -> String {
    let mut parts = Vec::new();
    if let Some(c) = &pkg.compiler {
        parts.push(c.clone());
    }
    if let Some(lto) = pkg.kernel.as_ref().and_then(|k| k.use_llvm_lto.as_deref()) {
        if lto != "none" {
            parts.push(format!("LTO:{lto}"));
        }
    }
    if pkg.pgo.as_ref().is_some_and(|p| p.enabled) {
        let preset = pkg.pgo.as_ref().map(|p| p.preset.as_str()).unwrap_or("pgo");
        parts.push(format!("PGO:{preset}"));
    }
    if pkg.ramdisk.as_ref().is_some_and(|r| !r.is_empty()) {
        parts.push("ramdisk".into());
    }
    if pkg.tests == Some(true) {
        parts.push("tests".into());
    }
    parts.join(" ").to_ascii_lowercase()
}

fn package_isolation_label(pkg: &PackageSection) -> &'static str {
    if pkg.compile_alone {
        abs_i18n::t("gui.packages.isolation_alone")
    } else {
        abs_i18n::t("gui.packages.isolation_parallel")
    }
}

fn sort_package_names(
    names: &mut [String],
    packages: &std::collections::HashMap<String, PackageSection>,
    col: PackageSortCol,
    descending: bool,
) {
    names.sort_by(|a, b| {
        let pa = &packages[a];
        let pb = &packages[b];
        let ord = match col {
            PackageSortCol::Name => a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase()),
            PackageSortCol::Source => package_source_label(pa)
                .to_ascii_lowercase()
                .cmp(&package_source_label(pb).to_ascii_lowercase()),
            PackageSortCol::Flags => package_flags_sort_key(pa).cmp(&package_flags_sort_key(pb)),
            PackageSortCol::Threads => pa.compilation_threads.cmp(&pb.compilation_threads),
            PackageSortCol::Isolation => package_isolation_label(pa)
                .to_ascii_lowercase()
                .cmp(&package_isolation_label(pb).to_ascii_lowercase()),
        };
        let ord = if ord == std::cmp::Ordering::Equal {
            a.cmp(b)
        } else {
            ord
        };
        if descending {
            ord.reverse()
        } else {
            ord
        }
    });
}

fn package_matches_chip(name: &str, pkg: &PackageSection, filter: PackageListFilter) -> bool {
    match filter {
        PackageListFilter::All => true,
        PackageListFilter::Kernels => {
            pkg.kernel.is_some() || pkg.pgo.is_some() || name.starts_with("linux-")
        }
        PackageListFilter::PgoLto => {
            pkg.pgo.as_ref().is_some_and(|p| p.enabled)
                || pkg
                    .kernel
                    .as_ref()
                    .and_then(|k| k.use_llvm_lto.as_deref())
                    .is_some_and(|v| v != "none")
        }
        PackageListFilter::Aur => package_is_aur(pkg),
        PackageListFilter::Official => pkg
            .source
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case("arch") || s.eq_ignore_ascii_case("cachyos")),
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
    let pgo = section.pgo.as_ref().filter(|p| p.enabled).ok_or_else(|| {
        abs_i18n::tf(
            "gui.msg.pgo_disabled",
            &[
                ("package", package),
                ("field", abs_i18n::t("gui.field.pgo_enabled")),
            ],
        )
    })?;
    if pgo
        .profiles_archive_dir
        .as_ref()
        .is_none_or(|s| s.trim().is_empty())
    {
        return Err(abs_i18n::tf(
            "gui.msg.pgo_need_archive",
            &[
                ("package", package),
                ("field", abs_i18n::t("gui.field.pgo_archive_dir")),
            ],
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
                    "auto".into()
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
        KStr::ProcessorOpt => {
            pkg.kernel
                .get_or_insert_with(Default::default)
                .processor_opt = opt
        }
        KStr::LlvmLto => pkg.kernel.get_or_insert_with(Default::default).use_llvm_lto = opt,
        KStr::HzTicks => pkg.kernel.get_or_insert_with(Default::default).hz_ticks = opt,
        KStr::Tickrate => pkg.kernel.get_or_insert_with(Default::default).tickrate = opt,
        KStr::Preempt => pkg.kernel.get_or_insert_with(Default::default).preempt = opt,
        KStr::Hugepage => pkg.kernel.get_or_insert_with(Default::default).hugepage = opt,
        KStr::ArchiveDir => {
            pkg.pgo
                .get_or_insert_with(Default::default)
                .profiles_archive_dir = opt
        }
        KStr::Benchmark => {
            pkg.pgo
                .get_or_insert_with(Default::default)
                .benchmark_command = opt
        }
        KStr::BenchmarkWorkdir => {
            pkg.pgo
                .get_or_insert_with(Default::default)
                .benchmark_workdir = opt
        }
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
        KBool::PgoAutoRestart => pkg.pgo.get_or_insert_with(Default::default).auto_restart = value,
        KBool::PgoPerfDataOnRam => {
            pkg.pgo
                .get_or_insert_with(Default::default)
                .perf_data_on_ram = value
        }
        KBool::PgoVerifyBoot => pkg.pgo.get_or_insert_with(Default::default).verify_boot = value,
        KBool::CcHarder => {
            pkg.kernel.get_or_insert_with(Default::default).cc_harder =
                if value { Some("y".into()) } else { None }
        }
        KBool::LtoSuffix => {
            pkg.kernel
                .get_or_insert_with(Default::default)
                .use_lto_suffix = if value { Some("y".into()) } else { None }
        }
        KBool::GccSuffix => {
            pkg.kernel
                .get_or_insert_with(Default::default)
                .use_gcc_suffix = if value { Some("y".into()) } else { None }
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
        assert!(err.contains("linux-cachyos"));
        assert_eq!(
            err,
            abs_i18n::tf(
                "gui.msg.pgo_need_archive",
                &[
                    ("package", "linux-cachyos"),
                    ("field", abs_i18n::t("gui.field.pgo_archive_dir")),
                ],
            )
        );
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

#[cfg(test)]
mod chrome_version_tests {
    #[test]
    fn corners_follow_workspace_cargo_toml() {
        let root = include_str!("../../Cargo.toml");
        let quoted = format!("version = \"{}\"", env!("CARGO_PKG_VERSION"));
        assert!(
            root.contains("[workspace.package]") && root.contains(&quoted),
            "top-left and bottom-right chrome use CARGO_PKG_VERSION; it must match [workspace.package] in Cargo.toml"
        );
    }
}

#[cfg(test)]
mod package_list_sort_tests {
    use super::sort_package_names;
    use crate::config::PackageSection;
    use crate::messages::PackageSortCol;
    use std::collections::HashMap;

    fn pkgs(pairs: &[(&str, PackageSection)]) -> HashMap<String, PackageSection> {
        pairs
            .iter()
            .map(|(n, p)| ((*n).to_string(), p.clone()))
            .collect()
    }

    #[test]
    fn sorts_by_name_then_toggles_desc() {
        let map = pkgs(&[
            ("zeta", PackageSection::default()),
            ("alpha", PackageSection::default()),
        ]);
        let mut names = vec!["zeta".into(), "alpha".into()];
        sort_package_names(&mut names, &map, PackageSortCol::Name, false);
        assert_eq!(names, ["alpha", "zeta"]);
        sort_package_names(&mut names, &map, PackageSortCol::Name, true);
        assert_eq!(names, ["zeta", "alpha"]);
    }

    #[test]
    fn sorts_threads_with_unset_first() {
        let map = pkgs(&[
            (
                "eight",
                PackageSection {
                    compilation_threads: Some(8),
                    ..Default::default()
                },
            ),
            ("none", PackageSection::default()),
            (
                "two",
                PackageSection {
                    compilation_threads: Some(2),
                    ..Default::default()
                },
            ),
        ]);
        let mut names = vec!["eight".into(), "none".into(), "two".into()];
        sort_package_names(&mut names, &map, PackageSortCol::Threads, false);
        assert_eq!(names, ["none", "two", "eight"]);
    }

    #[test]
    fn sorts_isolation_by_label() {
        let map = pkgs(&[
            (
                "solo",
                PackageSection {
                    compile_alone: true,
                    ..Default::default()
                },
            ),
            ("shared", PackageSection::default()),
        ]);
        let mut names = vec!["solo".into(), "shared".into()];
        sort_package_names(&mut names, &map, PackageSortCol::Isolation, false);
        assert_eq!(names, ["solo", "shared"]);
    }
}
