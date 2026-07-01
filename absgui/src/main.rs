mod list_editors;
mod abs_runner;
mod app;
mod app_settings;
mod config;
mod dialog;
mod field_help;
mod messages;
mod style;
mod widgets;
mod views;

fn main() -> iced::Result {
    app::run()
}
