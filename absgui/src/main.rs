mod abs_runner;
mod app;
mod app_settings;
mod config;
mod dialog;
mod field_help;
mod list_editors;
mod messages;
mod style;
mod views;
mod widgets;

fn main() -> iced::Result {
    app::run()
}
