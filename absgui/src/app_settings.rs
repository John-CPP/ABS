use crate::log_save::LogSaveFormat;
use crate::system_theme::ColorScheme;
use crate::terminal_themes::TerminalTheme;
use iced::window::{self, Icon};
use iced::{Point, Size};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemePref {
    #[default]
    Dark,
    Light,
    System,
}

impl ThemePref {
    pub fn resolve(self, system: ColorScheme) -> AppTheme {
        match self {
            Self::Dark => AppTheme::Dark,
            Self::Light => AppTheme::Light,
            Self::System => match system {
                ColorScheme::Dark => AppTheme::Dark,
                ColorScheme::Light => AppTheme::Light,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppTheme {
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiSettings {
    #[serde(default)]
    pub theme: ThemePref,
    /// Legacy single viewport palette. Used when Dark/Light keys are missing in old files.
    #[serde(default)]
    pub terminal_theme: TerminalTheme,
    /// Log viewport palette while AbsGUI is in Dark.
    #[serde(default)]
    pub terminal_theme_dark: TerminalTheme,
    /// Log viewport palette while AbsGUI is in Light.
    #[serde(default)]
    pub terminal_theme_light: TerminalTheme,
    #[serde(default = "default_terminal_lines_limit")]
    pub terminal_lines_limit: usize,
    /// Save Log path template on the kernel/build page (directory + filename pattern).
    #[serde(default)]
    pub log_save_build: String,
    #[serde(default)]
    pub log_save_build_dont_ask: bool,
    #[serde(default)]
    pub log_save_build_format: LogSaveFormat,
    /// Save Log path template on the System update page.
    #[serde(default)]
    pub log_save_update: String,
    #[serde(default)]
    pub log_save_update_dont_ask: bool,
    #[serde(default)]
    pub log_save_update_format: LogSaveFormat,
    #[serde(default = "default_width")]
    pub window_width: f32,
    #[serde(default = "default_height")]
    pub window_height: f32,
    #[serde(default)]
    pub window_x: Option<f32>,
    #[serde(default)]
    pub window_y: Option<f32>,
    /// True when the window was closed in fullscreen. Size/position stay the last windowed rect.
    #[serde(default)]
    pub window_fullscreen: bool,
    /// True when the window was closed maximized (not fullscreen).
    #[serde(default)]
    pub window_maximized: bool,
    /// AbsGui language override (`en`, `de`, …). None = inherit abs.toml / system.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
}

pub const TERMINAL_LINES_MIN: usize = 1;
pub const TERMINAL_LINES_MAX: usize = 5_000_000;
pub const TERMINAL_LINES_STEP: usize = 100;
pub const WINDOW_MIN_WIDTH: f32 = 860.0;
pub const WINDOW_MIN_HEIGHT: f32 = 560.0;

fn default_width() -> f32 {
    1060.0
}
fn default_height() -> f32 {
    760.0
}

pub fn window_min_size() -> Size {
    Size::new(WINDOW_MIN_WIDTH, WINDOW_MIN_HEIGHT)
}

/// Keep a windowed rectangle on-screen after the display layout changes.
///
/// When `monitor` is `None`, the window is not on any display: snap to the origin.
/// When the top-left is inside `[0, mon_w) × [0, mon_h)`, treat that as the origin
/// monitor and shift so the window stays fully on it. Otherwise leave global `x/y`
/// (another monitor). `position == None` (Wayland) is left unchanged.
pub fn clamp_window_geometry(
    size: Size,
    position: Option<Point>,
    monitor: Option<Size>,
) -> (Size, Option<Point>) {
    let min = window_min_size();
    let Some(mon) = monitor else {
        return (
            Size::new(size.width.max(min.width), size.height.max(min.height)),
            Some(Point::ORIGIN),
        );
    };

    let width = size.width.clamp(min.width, mon.width.max(min.width));
    let height = size.height.clamp(min.height, mon.height.max(min.height));
    let size = Size::new(width, height);

    let position = match position {
        None => None,
        Some(pos) => {
            let on_origin = pos.x >= 0.0 && pos.x < mon.width && pos.y >= 0.0 && pos.y < mon.height;
            if on_origin {
                let max_x = (mon.width - width).max(0.0);
                let max_y = (mon.height - height).max(0.0);
                Some(Point::new(pos.x.clamp(0.0, max_x), pos.y.clamp(0.0, max_y)))
            } else {
                Some(pos)
            }
        }
    };
    (size, position)
}
fn default_terminal_lines_limit() -> usize {
    5_000
}

pub fn clamp_terminal_lines_limit(n: usize) -> usize {
    n.clamp(TERMINAL_LINES_MIN, TERMINAL_LINES_MAX)
}

impl Default for GuiSettings {
    fn default() -> Self {
        Self {
            theme: ThemePref::Dark,
            terminal_theme: TerminalTheme::MatchApp,
            terminal_theme_dark: TerminalTheme::MatchApp,
            terminal_theme_light: TerminalTheme::MatchApp,
            terminal_lines_limit: default_terminal_lines_limit(),
            log_save_build: String::new(),
            log_save_build_dont_ask: false,
            log_save_build_format: LogSaveFormat::Txt,
            log_save_update: String::new(),
            log_save_update_dont_ask: false,
            log_save_update_format: LogSaveFormat::Txt,
            window_width: default_width(),
            window_height: default_height(),
            window_x: None,
            window_y: None,
            window_fullscreen: false,
            window_maximized: false,
            lang: None,
        }
    }
}

impl GuiSettings {
    /// Settings the user edits in App settings, excluding window geometry.
    pub fn content_eq(&self, other: &Self) -> bool {
        self.theme == other.theme
            && self.terminal_theme == other.terminal_theme
            && self.terminal_theme_dark == other.terminal_theme_dark
            && self.terminal_theme_light == other.terminal_theme_light
            && self.terminal_lines_limit == other.terminal_lines_limit
            && self.log_save_build == other.log_save_build
            && self.log_save_build_dont_ask == other.log_save_build_dont_ask
            && self.log_save_build_format == other.log_save_build_format
            && self.log_save_update == other.log_save_update
            && self.log_save_update_dont_ask == other.log_save_update_dont_ask
            && self.log_save_update_format == other.log_save_update_format
            && self.lang == other.lang
    }
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .map(|d| d.join("abs").join("absgui-settings.toml"))
            .unwrap_or_else(|| PathBuf::from("absgui-settings.toml"))
    }

    pub fn load() -> Self {
        let path = Self::path();
        fs::read_to_string(&path)
            .ok()
            .and_then(|text| Self::from_toml(&text))
            .unwrap_or_default()
    }

    pub(crate) fn from_toml(text: &str) -> Option<Self> {
        let value: toml::Value = toml::from_str(text).ok()?;
        let mut settings: Self = toml::from_str(text).ok()?;
        settings.migrate_viewport_themes(&value);
        Some(settings)
    }

    fn migrate_viewport_themes(&mut self, value: &toml::Value) {
        let Some(table) = value.as_table() else {
            return;
        };
        if !table.contains_key("terminal_theme_dark") {
            self.terminal_theme_dark = self.terminal_theme;
        }
        if !table.contains_key("terminal_theme_light") {
            self.terminal_theme_light = self.terminal_theme;
        }
    }

    pub fn viewport_theme(&self, app: AppTheme) -> TerminalTheme {
        match app {
            AppTheme::Dark => self.terminal_theme_dark,
            AppTheme::Light => self.terminal_theme_light,
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create dir: {e}"))?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| format!("serialize: {e}"))?;
        write_mode_0600(&path, &text)
    }

    pub fn window_settings(&self, icon: Option<Icon>) -> window::Settings {
        let mut settings = window::Settings {
            size: Size::new(self.window_width, self.window_height),
            min_size: Some(window_min_size()),
            position: match (self.window_x, self.window_y) {
                (Some(x), Some(y)) => window::Position::Specific(Point::new(x, y)),
                _ => window::Position::default(),
            },
            fullscreen: self.window_fullscreen,
            maximized: self.window_maximized && !self.window_fullscreen,
            icon,
            ..Default::default()
        };
        // Wayland compositors match this to absgui.desktop (and thus the installed icon).
        #[cfg(target_os = "linux")]
        {
            settings.platform_specific.application_id = "absgui".into();
        }
        settings
    }

    pub fn set_size(&mut self, size: Size) {
        self.window_width = size.width;
        self.window_height = size.height;
    }

    pub fn set_position(&mut self, point: Point) {
        self.window_x = Some(point.x);
        self.window_y = Some(point.y);
    }

    /// Update live size only while the window is in a normal (not filled-screen) state.
    pub fn apply_live_size(&mut self, size: Size, fullscreen: bool, maximized: bool) {
        self.window_fullscreen = fullscreen;
        self.window_maximized = maximized && !fullscreen;
        if !self.window_fullscreen && !self.window_maximized {
            self.set_size(size);
        }
    }

    /// Update live position only while the window is in a normal (not filled-screen) state.
    pub fn apply_live_position(&mut self, point: Point, fullscreen: bool, maximized: bool) {
        self.window_fullscreen = fullscreen;
        self.window_maximized = maximized && !fullscreen;
        if !self.window_fullscreen && !self.window_maximized {
            self.set_position(point);
        }
    }

    /// Persist flags from a close-time query. Fullscreen/maximized keep the last windowed rect.
    pub fn apply_close_snapshot(
        &mut self,
        fullscreen: bool,
        maximized: bool,
        size: Size,
        position: Option<Point>,
    ) {
        if fullscreen {
            self.window_fullscreen = true;
            self.window_maximized = false;
            return;
        }
        if maximized {
            self.window_fullscreen = false;
            self.window_maximized = true;
            return;
        }
        self.window_fullscreen = false;
        self.window_maximized = false;
        self.set_size(size);
        if let Some(point) = position {
            self.set_position(point);
        }
    }

    pub fn log_save_path(&self, target: crate::log_save::LogSaveTarget) -> &str {
        match target {
            crate::log_save::LogSaveTarget::Build => &self.log_save_build,
            crate::log_save::LogSaveTarget::Update => &self.log_save_update,
        }
    }

    pub fn set_log_save_path(&mut self, target: crate::log_save::LogSaveTarget, path: String) {
        match target {
            crate::log_save::LogSaveTarget::Build => self.log_save_build = path,
            crate::log_save::LogSaveTarget::Update => self.log_save_update = path,
        }
    }

    pub fn log_save_dont_ask(&self, target: crate::log_save::LogSaveTarget) -> bool {
        match target {
            crate::log_save::LogSaveTarget::Build => self.log_save_build_dont_ask,
            crate::log_save::LogSaveTarget::Update => self.log_save_update_dont_ask,
        }
    }

    pub fn set_log_save_dont_ask(&mut self, target: crate::log_save::LogSaveTarget, v: bool) {
        match target {
            crate::log_save::LogSaveTarget::Build => self.log_save_build_dont_ask = v,
            crate::log_save::LogSaveTarget::Update => self.log_save_update_dont_ask = v,
        }
    }

    pub fn log_save_format(&self, target: crate::log_save::LogSaveTarget) -> LogSaveFormat {
        match target {
            crate::log_save::LogSaveTarget::Build => self.log_save_build_format,
            crate::log_save::LogSaveTarget::Update => self.log_save_update_format,
        }
    }

    pub fn set_log_save_format(
        &mut self,
        target: crate::log_save::LogSaveTarget,
        format: LogSaveFormat,
    ) {
        match target {
            crate::log_save::LogSaveTarget::Build => self.log_save_build_format = format,
            crate::log_save::LogSaveTarget::Update => self.log_save_update_format = format,
        }
    }
}

pub fn load_window_icon() -> Option<Icon> {
    iced::window::icon::from_file_data(include_bytes!("../assets/icon_light.png"), None).ok()
}

/// Decode a PNG once into RGBA so iced never flashes an empty image on redraw.
pub fn load_rgba_png(bytes: &[u8], max_side: u32) -> iced::widget::image::Handle {
    let Ok(dynamic) = image::load_from_memory(bytes) else {
        return iced::widget::image::Handle::from_rgba(1, 1, vec![0, 0, 0, 0]);
    };
    let rgba = dynamic.into_rgba8();
    let (w, h) = rgba.dimensions();
    let rgba = if w > max_side || h > max_side {
        image::imageops::resize(
            &rgba,
            max_side,
            max_side,
            image::imageops::FilterType::Triangle,
        )
    } else {
        rgba
    };
    let (w, h) = rgba.dimensions();
    iced::widget::image::Handle::from_rgba(w, h, rgba.into_raw())
}

fn write_mode_0600(path: &Path, text: &str) -> Result<(), String> {
    use std::fs::OpenOptions;
    use std::io::Write as _;
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    file.write_all(text.as_bytes())
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        clamp_terminal_lines_limit, default_terminal_lines_limit, AppTheme, GuiSettings,
        TERMINAL_LINES_MAX, TERMINAL_LINES_MIN,
    };
    use crate::terminal_themes::TerminalTheme;
    use iced::{Point, Size};

    #[test]
    fn test_load_window_icon_succeeds() {
        assert!(super::load_window_icon().is_some());
    }

    #[test]
    fn test_load_rgba_png_sidebar_logo() {
        let handle = super::load_rgba_png(include_bytes!("../assets/icon_dark.png"), 64);
        let _ = handle;
    }

    #[test]
    fn clamps_terminal_line_limit() {
        assert_eq!(default_terminal_lines_limit(), 5_000);
        assert_eq!(GuiSettings::default().terminal_lines_limit, 5_000);
        assert_eq!(clamp_terminal_lines_limit(0), TERMINAL_LINES_MIN);
        assert_eq!(clamp_terminal_lines_limit(1), 1);
        assert_eq!(clamp_terminal_lines_limit(5_000), 5_000);
        assert_eq!(TERMINAL_LINES_MAX, 5_000_000);
        assert_eq!(
            clamp_terminal_lines_limit(TERMINAL_LINES_MAX + 10),
            TERMINAL_LINES_MAX
        );
    }

    #[test]
    fn content_eq_ignores_window_geometry() {
        let mut a = GuiSettings::default();
        let mut b = a.clone();
        b.window_width = 2000.0;
        b.window_x = Some(10.0);
        b.window_fullscreen = true;
        b.window_maximized = true;
        assert!(a.content_eq(&b));
        b.theme = super::ThemePref::Light;
        assert!(!a.content_eq(&b));
        a.theme = super::ThemePref::Light;
        assert!(a.content_eq(&b));
    }

    #[test]
    fn theme_pref_system_follows_desktop_scheme() {
        assert_eq!(
            super::ThemePref::System.resolve(crate::system_theme::ColorScheme::Light),
            super::AppTheme::Light
        );
        assert_eq!(
            super::ThemePref::System.resolve(crate::system_theme::ColorScheme::Dark),
            super::AppTheme::Dark
        );
        assert_eq!(
            super::ThemePref::Dark.resolve(crate::system_theme::ColorScheme::Light),
            super::AppTheme::Dark
        );
    }

    #[test]
    fn theme_pref_system_roundtrips_toml() {
        let loaded: GuiSettings = toml::from_str("theme = \"system\"\n").expect("system theme");
        assert_eq!(loaded.theme, super::ThemePref::System);
    }

    #[test]
    fn old_toml_defaults_fullscreen_flags() {
        let old = r#"
theme = "dark"
window_width = 1060.0
window_height = 760.0
"#;
        let loaded: GuiSettings = toml::from_str(old).expect("old settings still load");
        assert_eq!(loaded.theme, super::ThemePref::Dark);
        assert!(!loaded.window_fullscreen);
        assert!(!loaded.window_maximized);
    }

    #[test]
    fn clamp_shrinks_to_monitor_and_shifts_on_origin() {
        let size = Size::new(2000.0, 1200.0);
        let pos = Some(Point::new(100.0, 80.0));
        let mon = Some(Size::new(1280.0, 720.0));
        let (out, pos) = super::clamp_window_geometry(size, pos, mon);
        assert_eq!(out, Size::new(1280.0, 720.0));
        assert_eq!(pos, Some(Point::new(0.0, 0.0)));
    }

    #[test]
    fn clamp_leaves_second_monitor_position() {
        let size = Size::new(900.0, 600.0);
        let pos = Some(Point::new(1920.0, 100.0));
        let mon = Some(Size::new(1920.0, 1080.0));
        let (out, pos) = super::clamp_window_geometry(size, pos, mon);
        assert_eq!(out, size);
        assert_eq!(pos, Some(Point::new(1920.0, 100.0)));
    }

    #[test]
    fn clamp_missing_monitor_snaps_origin() {
        let size = Size::new(400.0, 300.0);
        let (out, pos) = super::clamp_window_geometry(size, Some(Point::new(50.0, 50.0)), None);
        assert_eq!(out, super::window_min_size());
        assert_eq!(pos, Some(Point::ORIGIN));
    }

    #[test]
    fn clamp_wayland_keeps_missing_position() {
        let size = Size::new(2000.0, 800.0);
        let (out, pos) = super::clamp_window_geometry(size, None, Some(Size::new(1280.0, 720.0)));
        assert_eq!(out, Size::new(1280.0, 720.0));
        assert_eq!(pos, None);
    }

    #[test]
    fn apply_close_snapshot_fullscreen_keeps_windowed_rect() {
        let mut s = GuiSettings::default();
        s.set_size(Size::new(900.0, 700.0));
        s.set_position(Point::new(40.0, 50.0));
        s.apply_close_snapshot(
            true,
            false,
            Size::new(1920.0, 1080.0),
            Some(Point::new(0.0, 0.0)),
        );
        assert!(s.window_fullscreen);
        assert!(!s.window_maximized);
        assert_eq!(s.window_width, 900.0);
        assert_eq!(s.window_height, 700.0);
        assert_eq!(s.window_x, Some(40.0));
        assert_eq!(s.window_y, Some(50.0));
    }

    #[test]
    fn apply_live_size_skips_when_maximized() {
        let mut s = GuiSettings::default();
        s.set_size(Size::new(900.0, 700.0));
        s.apply_live_size(Size::new(1920.0, 1080.0), false, true);
        assert!(s.window_maximized);
        assert_eq!(s.window_width, 900.0);
    }

    #[test]
    fn log_save_fields_default_and_old_toml() {
        let s = GuiSettings::default();
        assert!(!s.log_save_build_dont_ask);
        assert_eq!(s.log_save_build_format, crate::log_save::LogSaveFormat::Txt);
        let old = r#"
theme = "dark"
terminal_theme = "match-app"
log_save_build = "/tmp/old-build.log"
log_save_update = "/tmp/old-update.log"
window_width = 1060.0
window_height = 760.0
"#;
        let loaded: GuiSettings = toml::from_str(old).expect("old settings still load");
        assert_eq!(loaded.log_save_build, "/tmp/old-build.log");
        assert!(!loaded.log_save_update_dont_ask);
        assert_eq!(
            loaded.log_save_update_format,
            crate::log_save::LogSaveFormat::Txt
        );
    }

    #[test]
    fn viewport_themes_migrate_from_single_key() {
        let loaded = GuiSettings::from_toml(
            r#"
theme = "dark"
terminal_theme = "catppuccin-mocha"
window_width = 1060.0
window_height = 760.0
"#,
        )
        .expect("legacy settings load");
        assert_eq!(loaded.terminal_theme, TerminalTheme::CatppuccinMocha);
        assert_eq!(loaded.terminal_theme_dark, TerminalTheme::CatppuccinMocha);
        assert_eq!(loaded.terminal_theme_light, TerminalTheme::CatppuccinMocha);
        assert_eq!(
            loaded.viewport_theme(AppTheme::Dark),
            TerminalTheme::CatppuccinMocha
        );
        assert_eq!(
            loaded.viewport_theme(AppTheme::Light),
            TerminalTheme::CatppuccinMocha
        );
    }

    #[test]
    fn viewport_themes_keep_independent_slots() {
        let loaded = GuiSettings::from_toml(
            r#"
theme = "light"
terminal_theme = "nord"
terminal_theme_dark = "dracula"
terminal_theme_light = "catppuccin-latte"
"#,
        )
        .expect("split settings load");
        assert_eq!(loaded.terminal_theme_dark, TerminalTheme::Dracula);
        assert_eq!(loaded.terminal_theme_light, TerminalTheme::CatppuccinLatte);
        assert_eq!(
            loaded.viewport_theme(AppTheme::Dark),
            TerminalTheme::Dracula
        );
        assert_eq!(
            loaded.viewport_theme(AppTheme::Light),
            TerminalTheme::CatppuccinLatte
        );
    }
}
