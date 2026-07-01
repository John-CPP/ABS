use crate::field_help;
use crate::list_editors::PackageListField;
use crate::messages::{EditTarget, Message, RamdiskLetter};
use crate::app_settings::AppTheme;
use crate::style;
use iced::widget::{button, checkbox, column, pick_list, row, text, text_editor, text_input, Space};
use iced::{Element, Length, Font};

pub use crate::messages::{PathField, PathKind};

pub fn help_line<'a>(help: &'a str, app_theme: AppTheme) -> Element<'a, Message> {
    text(help).size(11).color(style::muted(app_theme)).into()
}

pub fn field_label_column<'a>(
    label: &'a str,
    help: Option<&'a str>,
    app_theme: AppTheme,
    body: Element<'a, Message>,
) -> Element<'a, Message> {
    let mut col = column![text(label).size(12).color(style::muted(app_theme)),].spacing(4);
    if let Some(h) = help {
        col = col.push(help_line(h, app_theme));
    }
    col.push(body).width(Length::Fill).into()
}

pub fn page_title(title: &str, app_theme: AppTheme) -> Element<'_, Message> {
    column![
        text(title).size(26),
        iced::widget::container(Space::with_height(Length::Fixed(3.0)))
            .width(Length::Fixed(48.0))
            .height(Length::Fixed(3.0))
            .style(style::accent_bar(app_theme)),
    ]
    .spacing(8)
    .into()
}

pub fn card_section<'a>(
    title: &'a str,
    app_theme: AppTheme,
    body: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    iced::widget::container(column![text(title).size(16), body.into()].spacing(12))
        .padding(16)
        .width(Length::Fill)
        .style(style::card(app_theme))
        .into()
}

pub fn field_text<'a, F>(
    label: &'a str,
    help: Option<&'a str>,
    value: &str,
    placeholder: &'a str,
    app_theme: AppTheme,
    on_change: F,
) -> Element<'a, Message>
where
    F: Fn(String) -> Message + 'a,
{
    field_label_column(
        label,
        help,
        app_theme,
        text_input(placeholder, value)
            .on_input(on_change)
            .padding(8)
            .width(Length::Fill)
            .into(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn field_path<'a, F>(
    label: &'a str,
    help: Option<&'a str>,
    value: &str,
    placeholder: &'a str,
    field: PathField,
    kind: PathKind,
    app_theme: AppTheme,
    on_change: F,
) -> Element<'a, Message>
where
    F: Fn(String) -> Message + 'a,
{
    let browse_label = match kind {
        PathKind::Folder => "Browse…",
        PathKind::File => "Choose file…",
    };
    field_label_column(
        label,
        help,
        app_theme,
        row![
            text_input(placeholder, value)
                .on_input(on_change)
                .padding(8)
                .width(Length::Fill),
            button(text(browse_label).size(13))
                .style(iced::widget::button::secondary)
                .on_press(Message::BrowsePath(field, kind)),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .into(),
    )
}

pub fn field_pick<'a, F>(
    label: &'a str,
    help: Option<&'a str>,
    options: &[&'static str],
    value: &str,
    app_theme: AppTheme,
    on_change: F,
) -> Element<'a, Message>
where
    F: Fn(String) -> Message + 'a,
{
    let mut opts: Vec<String> = options.iter().map(|s| (*s).to_string()).collect();
    if !value.is_empty() && !opts.contains(&value.to_string()) {
        opts.insert(0, value.to_string());
    }
    let selected = if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    };
    field_label_column(
        label,
        help,
        app_theme,
        pick_list(opts, selected, on_change)
            .padding(8)
            .width(Length::Fill)
            .into(),
    )
}

pub fn field_number<'a, F>(
    label: &'a str,
    help: Option<&'a str>,
    value: &str,
    app_theme: AppTheme,
    on_change: F,
) -> Element<'a, Message>
where
    F: Fn(String) -> Message + 'a,
{
    field_text(label, help, value, "0", app_theme, on_change)
}

pub fn field_checkbox<'a, F>(
    label: &'a str,
    help: Option<&'a str>,
    checked: bool,
    app_theme: AppTheme,
    on_toggle: F,
) -> Element<'a, Message>
where
    F: Fn(bool) -> Message + 'a,
{
    let mut col = column![checkbox(label, checked).on_toggle(on_toggle),].spacing(4);
    if let Some(h) = help {
        col = col.push(help_line(h, app_theme));
    }
    col.width(Length::Fill).into()
}

pub fn parse_ramdisk_flags(value: &str) -> (bool, bool, bool, bool) {
    let lower = value.to_ascii_lowercase();
    (
        lower.contains('w'),
        lower.contains('c'),
        lower.contains('p'),
        lower.contains('r'),
    )
}

pub fn encode_ramdisk_flags(workdir: bool, chroot: bool, packages: bool, profiles: bool) -> String {
    let mut s = String::new();
    if workdir {
        s.push('w');
    }
    if chroot {
        s.push('c');
    }
    if packages {
        s.push('p');
    }
    if profiles {
        s.push('r');
    }
    s
}

#[allow(dead_code)]
pub fn ramdisk_targets_field(
    target: EditTarget,
    workdir: bool,
    chroot: bool,
    packages: bool,
    profiles: bool,
    app_theme: AppTheme,
) -> Element<'static, Message> {
    field_label_column(
        "Ramdisk targets",
        Some(field_help::RAMDISK_TARGETS),
        app_theme,
        column![
            field_checkbox(
                "Build workdir (w)",
                Some(field_help::RAMDISK_W),
                workdir,
                app_theme,
                move |v| Message::SetRamdiskTarget(target, RamdiskLetter::Workdir, v),
            ),
            field_checkbox(
                "Chroot (c)",
                Some(field_help::RAMDISK_C),
                chroot,
                app_theme,
                move |v| Message::SetRamdiskTarget(target, RamdiskLetter::Chroot, v),
            ),
            field_checkbox(
                "Packages (p)",
                Some(field_help::RAMDISK_P),
                packages,
                app_theme,
                move |v| Message::SetRamdiskTarget(target, RamdiskLetter::Packages, v),
            ),
            field_checkbox(
                "Profile scratch (r)",
                Some(field_help::RAMDISK_R),
                profiles,
                app_theme,
                move |v| Message::SetRamdiskTarget(target, RamdiskLetter::Profiles, v),
            ),
        ]
        .spacing(8)
        .into(),
    )
}

/// Kernel-specific ramdisk labels: downloads stay on disk unless repo-on-ramdisk is enabled.
pub fn kernel_ramdisk_targets_field(
    target: EditTarget,
    workdir: bool,
    chroot: bool,
    packages: bool,
    profiles: bool,
    app_theme: AppTheme,
) -> Element<'static, Message> {
    field_label_column(
        "Ramdisk (kernel)",
        Some(field_help::KERNEL_RAMDISK_TARGETS),
        app_theme,
        column![
            field_checkbox(
                "Compilation on ramdisk (w)",
                Some(field_help::KERNEL_RAMDISK_W),
                workdir,
                app_theme,
                move |v| Message::SetRamdiskTarget(target, RamdiskLetter::Workdir, v),
            ),
            field_checkbox(
                "Repo on ramdisk (p)",
                Some(field_help::KERNEL_RAMDISK_P),
                packages,
                app_theme,
                move |v| Message::SetRamdiskTarget(target, RamdiskLetter::Packages, v),
            ),
            field_checkbox(
                "Profile scratch on ramdisk (r)",
                Some(field_help::KERNEL_RAMDISK_R),
                profiles,
                app_theme,
                move |v| Message::SetRamdiskTarget(target, RamdiskLetter::Profiles, v),
            ),
            field_checkbox(
                "Chroot on ramdisk (c)",
                Some(field_help::RAMDISK_C),
                chroot,
                app_theme,
                move |v| Message::SetRamdiskTarget(target, RamdiskLetter::Chroot, v),
            ),
            text(
                "Recommended: w + r only (compile and profile scratch on tmpfs; git repo and kernel \
                 tarballs stay on disk). Stage 1 runs updpkgsums once before copying to ramdisk."
            )
            .size(11)
            .color(crate::style::muted(app_theme)),
        ]
        .spacing(8)
        .into(),
    )
}

pub fn packages_list_editor<'a>(
    label: &'a str,
    help: Option<&'a str>,
    content: &'a text_editor::Content,
    field: PackageListField,
    app_theme: AppTheme,
    enabled: bool,
) -> Element<'a, Message> {
    let editor = text_editor(content)
        .font(Font::MONOSPACE)
        .padding(8)
        .height(Length::Fixed(120.0));
    let editor = if enabled {
        editor.on_action(move |action| Message::PackageListEdited(field, action))
    } else {
        editor
    };
    field_label_column(label, help, app_theme, editor.into())
}

pub fn optional_bool_field<'a, F>(
    label: &'a str,
    help: Option<&'a str>,
    value: Option<bool>,
    default_label: &str,
    app_theme: AppTheme,
    on_change: F,
) -> Element<'a, Message>
where
    F: Fn(Option<bool>) -> Message + 'a,
{
    let tri: String = match value {
        None => "default".into(),
        Some(true) => "true".into(),
        Some(false) => "false".into(),
    };
    let options = vec![
        format!("Default ({default_label})"),
        "true".into(),
        "false".into(),
    ];
    field_label_column(
        label,
        help,
        app_theme,
        pick_list(options, Some(tri), move |choice| {
            let v = if choice.starts_with("Default") {
                None
            } else if choice == "true" {
                Some(true)
            } else {
                Some(false)
            };
            on_change(v)
        })
        .padding(8)
        .width(Length::Fill)
        .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::{encode_ramdisk_flags, parse_ramdisk_flags};

    #[test]
    fn ramdisk_flags_roundtrip() {
        assert_eq!(parse_ramdisk_flags("wcp"), (true, true, true, false));
        assert_eq!(parse_ramdisk_flags("wcr"), (true, true, false, true));
        assert_eq!(parse_ramdisk_flags("wc"), (true, true, false, false));
        assert_eq!(encode_ramdisk_flags(true, false, true, true), "wpr");
    }
}
