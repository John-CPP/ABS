use iced::window::{self, Icon};
use iced::{Point, Size};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AppTheme {
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiSettings {
    #[serde(default)]
    pub theme: AppTheme,
    #[serde(default = "default_width")]
    pub window_width: f32,
    #[serde(default = "default_height")]
    pub window_height: f32,
    #[serde(default)]
    pub window_x: Option<f32>,
    #[serde(default)]
    pub window_y: Option<f32>,
}

fn default_width() -> f32 {
    1060.0
}
fn default_height() -> f32 {
    760.0
}

impl Default for GuiSettings {
    fn default() -> Self {
        Self {
            theme: AppTheme::Dark,
            window_width: default_width(),
            window_height: default_height(),
            window_x: None,
            window_y: None,
        }
    }
}

impl GuiSettings {
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .map(|d| d.join("abs").join("absgui-settings.toml"))
            .unwrap_or_else(|| PathBuf::from("absgui-settings.toml"))
    }

    pub fn load() -> Self {
        let path = Self::path();
        fs::read_to_string(&path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create dir: {e}"))?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| format!("serialize: {e}"))?;
        fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))
    }

    pub fn window_settings(&self, icon: Option<Icon>) -> window::Settings {
        window::Settings {
            size: Size::new(self.window_width, self.window_height),
            min_size: Some(Size::new(860.0, 560.0)),
            position: match (self.window_x, self.window_y) {
                (Some(x), Some(y)) => window::Position::Specific(Point::new(x, y)),
                _ => window::Position::default(),
            },
            icon,
            ..Default::default()
        }
    }

    pub fn set_size(&mut self, size: Size) {
        self.window_width = size.width;
        self.window_height = size.height;
    }

    pub fn set_position(&mut self, point: Point) {
        self.window_x = Some(point.x);
        self.window_y = Some(point.y);
    }
}

pub fn load_window_icon() -> Option<Icon> {
    iced::window::icon::from_file_data(include_bytes!("../assets/icon.png"), None).ok()
}
