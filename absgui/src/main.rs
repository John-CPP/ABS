mod abs_runner;
mod app;
mod app_settings;
mod config;
mod dialog;
mod field_help;
mod list_editors;
mod log_save;
mod log_view;
mod messages;
mod metrics;
mod pkgbuild_diff;
mod ramdisk_size;
mod style;
mod system_theme;
mod terminal_themes;
mod views;
mod widgets;

fn main() -> iced::Result {
    app::run()
}
