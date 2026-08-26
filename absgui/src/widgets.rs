use crate::app_settings::{AppTheme, ThemePref};
use crate::field_help;
use crate::list_editors::PackageListField;
use crate::log_save::{LogSaveFormat, LogSaveTarget};
use crate::log_view::LogLines;
use crate::messages::{
    EditTarget, Message, PackageSortCol, PkgbuildPreview, RamdiskLetter, ViewportId,
};
use crate::ramdisk_size::{self, SizeVsRam};
use crate::style;
use crate::terminal_themes::{LogPalette, TerminalTheme};
use iced::widget::{
    button, checkbox, column, container, image, mouse_area, pick_list, rich_text, row, scrollable,
    span, text, text_editor, text_input, tooltip, Space,
};
use iced::{Alignment, Element, Font, Length, Padding, Theme};
use std::borrow::Borrow;
use std::collections::VecDeque;
use std::sync::OnceLock;
use std::time::Duration;

pub use crate::messages::{PathField, PathKind};

pub fn help_line<'a>(help: &'a str, app_theme: AppTheme) -> Element<'a, Message> {
    text(help)
        .size(style::TEXT_HELP)
        .color(style::muted(app_theme))
        .into()
}

pub fn field_label_column<'a>(
    label: &'a str,
    help: Option<&'a str>,
    app_theme: AppTheme,
    body: Element<'a, Message>,
) -> Element<'a, Message> {
    let mut col = column![text(label)
        .size(style::TEXT_LABEL)
        .font(Font {
            weight: iced::font::Weight::Medium,
            ..Font::DEFAULT
        })
        .color(style::muted(app_theme)),]
    .spacing(5);
    if let Some(h) = help {
        col = col.push(help_line(h, app_theme));
    }
    col.push(body).width(Length::Fill).into()
}

pub fn themed_pick_list<'a, T, L, V>(
    options: L,
    selected: Option<V>,
    on_select: impl Fn(T) -> Message + 'a,
    app_theme: AppTheme,
    width: Length,
) -> Element<'a, Message>
where
    T: ToString + PartialEq + Clone + 'a,
    L: Borrow<[T]> + 'a,
    V: Borrow<T> + 'a,
{
    pick_list(options, selected, on_select)
        .padding(8)
        .width(width)
        .style(style::select(app_theme))
        .menu_style(style::pick_menu(app_theme))
        .handle(pick_list::Handle::Arrow {
            size: Some(12.0.into()),
        })
        .into()
}

fn icon_control_btn<'a>(
    content: impl Into<Element<'a, Message>>,
    msg: Message,
    app_theme: AppTheme,
) -> iced::widget::Button<'a, Message> {
    button(content)
        .width(Length::Fixed(style::CONTROL_H))
        .height(Length::Fixed(style::CONTROL_H))
        .padding(0)
        .style(style::btn_icon(app_theme))
        .on_press(msg)
}

fn step_glyph<'a>(label: &'static str) -> Element<'a, Message> {
    container(text(label).size(16).font(Font {
        weight: iced::font::Weight::Bold,
        ..Font::DEFAULT
    }))
    .center(Length::Fill)
    .into()
}

pub fn path_browse_button<'a>(
    kind: PathKind,
    msg: Message,
    app_theme: AppTheme,
) -> Element<'a, Message> {
    let label = match kind {
        PathKind::Folder => abs_i18n::t("gui.common.browse"),
        PathKind::File => abs_i18n::t("gui.common.choose_file"),
    };
    let icon = image(path_kind_icon(kind, app_theme))
        .width(Length::Fixed(16.0))
        .height(Length::Fixed(16.0));
    tooltip(
        icon_control_btn(container(icon).center(Length::Fill), msg, app_theme),
        container(text(label).size(style::TEXT_HELP))
            .padding(Padding::from([4.0, 8.0]))
            .style(style::tooltip_box(app_theme)),
        tooltip::Position::Bottom,
    )
    .gap(6)
    .delay(Duration::from_millis(350))
    .into()
}

fn path_kind_icon(kind: PathKind, theme: AppTheme) -> iced::widget::image::Handle {
    static FOLDER_DARK: OnceLock<iced::widget::image::Handle> = OnceLock::new();
    static FOLDER_LIGHT: OnceLock<iced::widget::image::Handle> = OnceLock::new();
    static FILE_DARK: OnceLock<iced::widget::image::Handle> = OnceLock::new();
    static FILE_LIGHT: OnceLock<iced::widget::image::Handle> = OnceLock::new();
    let cell = match (kind, theme) {
        (PathKind::Folder, AppTheme::Dark) => &FOLDER_DARK,
        (PathKind::Folder, AppTheme::Light) => &FOLDER_LIGHT,
        (PathKind::File, AppTheme::Dark) => &FILE_DARK,
        (PathKind::File, AppTheme::Light) => &FILE_LIGHT,
    };
    cell.get_or_init(|| raster_path_icon(kind, style::muted(theme)))
        .clone()
}

fn raster_path_icon(kind: PathKind, color: iced::Color) -> iced::widget::image::Handle {
    let scale = 2u32;
    let size = 24 * scale;
    let mut img = ::image::RgbaImage::new(size, size);
    let c = ::image::Rgba([
        (color.r * 255.0).round() as u8,
        (color.g * 255.0).round() as u8,
        (color.b * 255.0).round() as u8,
        (color.a * 255.0).round() as u8,
    ]);
    match kind {
        PathKind::Folder => {
            fill_round_rect(&mut img, 8, 18, 40, 40, 4, c);
            fill_round_rect(&mut img, 8, 12, 22, 22, 3, c);
        }
        PathKind::File => {
            fill_round_rect(&mut img, 12, 6, 36, 42, 4, c);
            fill_round_rect(
                &mut img,
                24,
                6,
                36,
                18,
                2,
                ::image::Rgba([c.0[0], c.0[1], c.0[2], 180]),
            );
        }
    }
    let out = ::image::imageops::resize(&img, 24, 24, ::image::imageops::FilterType::Triangle);
    iced::widget::image::Handle::from_rgba(24, 24, out.into_raw())
}

fn fill_round_rect(
    img: &mut ::image::RgbaImage,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    radius: u32,
    color: ::image::Rgba<u8>,
) {
    let w = img.width();
    let h = img.height();
    let x0 = x0.min(w);
    let y0 = y0.min(h);
    let x1 = x1.min(w).max(x0);
    let y1 = y1.min(h).max(y0);
    let r = radius as f32;
    for y in y0..y1 {
        for x in x0..x1 {
            if round_rect_hit(
                x as f32 + 0.5,
                y as f32 + 0.5,
                x0 as f32,
                y0 as f32,
                x1 as f32,
                y1 as f32,
                r,
            ) {
                img.put_pixel(x, y, color);
            }
        }
    }
}

fn round_rect_hit(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> bool {
    if px < x0 || px >= x1 || py < y0 || py >= y1 {
        return false;
    }
    let r = r.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
    let in_x = px >= x0 + r && px < x1 - r;
    let in_y = py >= y0 + r && py < y1 - r;
    if in_x || in_y {
        return true;
    }
    let cx = if px < x0 + r { x0 + r } else { x1 - r };
    let cy = if py < y0 + r { y0 + r } else { y1 - r };
    let dx = px - cx;
    let dy = py - cy;
    dx * dx + dy * dy <= r * r
}

pub fn card_section<'a>(
    title: &'a str,
    app_theme: AppTheme,
    body: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    let mut col = column![];
    if !title.is_empty() {
        col = col.push(text(title).size(style::TEXT_CARD).font(Font {
            weight: iced::font::Weight::Bold,
            ..Font::DEFAULT
        }));
    }
    col = col.push(body.into());
    iced::widget::container(col.spacing(10))
        .padding(14)
        .width(Length::Fill)
        .style(style::card(app_theme))
        .into()
}

pub fn dense_header_cell<'a>(label: &'static str, theme: AppTheme) -> Element<'a, Message> {
    text(label)
        .size(10.5)
        .font(Font {
            weight: iced::font::Weight::Bold,
            ..Font::DEFAULT
        })
        .color(style::muted(theme))
        .into()
}

pub fn dense_sort_header_cell<'a>(
    label: &'static str,
    column: PackageSortCol,
    current: PackageSortCol,
    descending: bool,
    theme: AppTheme,
) -> Element<'a, Message> {
    let active = column == current;
    let marker = if !active {
        ""
    } else if descending {
        " ▼"
    } else {
        " ▲"
    };
    button(text(format!("{label}{marker}")).size(10.5).font(Font {
        weight: iced::font::Weight::Bold,
        ..Font::DEFAULT
    }))
    .padding(Padding::ZERO)
    .width(Length::Fill)
    .style(style::table_sort_header(theme, active))
    .on_press(Message::PackageSort(column))
    .into()
}

pub fn dense_table_row<'a>(
    cells: Vec<Element<'a, Message>>,
    portions: &[u16],
    header: bool,
    active: bool,
    hovered: bool,
    theme: AppTheme,
) -> Element<'a, Message> {
    let mut r = row![].spacing(8).align_y(Alignment::Center);
    for (cell, portion) in cells.into_iter().zip(portions.iter().copied()) {
        r = r.push(container(cell).width(Length::FillPortion(portion)));
    }
    container(r)
        .padding(Padding::from([7.0, 16.0]))
        .width(Length::Fill)
        .style(style::dense_row(
            theme,
            active && !header,
            hovered && !header,
        ))
        .into()
}

pub fn interactive_list_row<'a>(
    row: Element<'a, Message>,
    on_enter: Message,
    on_exit: Message,
    on_double_click: Message,
) -> Element<'a, Message> {
    mouse_area(row)
        .on_enter(on_enter)
        .on_exit(on_exit)
        .on_double_click(on_double_click)
        .interaction(iced::mouse::Interaction::Pointer)
        .into()
}

pub fn kernel_status_dot<'a>(
    configured: bool,
    hovered: bool,
    theme: AppTheme,
) -> Element<'a, Message> {
    let (glyph, color) = if configured {
        ("●", style::success(theme))
    } else if hovered {
        ("●", style::warning(theme))
    } else {
        ("○", style::muted(theme))
    };
    text(glyph).size(11).color(color).into()
}

pub fn dense_table<'a>(
    header: impl Into<Element<'a, Message>>,
    body: impl Into<Element<'a, Message>>,
    theme: AppTheme,
) -> Element<'a, Message> {
    container(column![header.into(), body.into()].spacing(0))
        .width(Length::Fill)
        .style(style::dense_table(theme))
        .into()
}

/// Start-anchored relative Y: 0 is the top, 1 is the bottom. `None` is the
/// hysteresis band so layout jitter while pinned does not pause follow.
pub fn log_scroll_at_bottom(relative_y: f32) -> Option<bool> {
    if !relative_y.is_finite() {
        return Some(true);
    }
    if relative_y >= 0.92 {
        Some(true)
    } else if relative_y < 0.80 {
        Some(false)
    } else {
        None
    }
}

fn log_scroll_pinned(relative_y: f32, currently_pinned: bool) -> bool {
    log_scroll_at_bottom(relative_y).unwrap_or(currently_pinned)
}

fn viewport_scrollbar() -> scrollable::Scrollbar {
    scrollable::Scrollbar::new()
        .width(12)
        .scroller_width(12)
        .spacing(4)
}

/// One scrollable viewport. `id` keeps page/list scroll state from leaking across widgets.
pub fn scroll_viewport<'a>(
    content: impl Into<Element<'a, Message>>,
    style: impl Fn(&Theme, scrollable::Status) -> scrollable::Style + 'a,
    width: Length,
    height: Length,
    id: &'static str,
) -> Element<'a, Message> {
    scrollable(content.into())
        .id(id)
        .direction(scrollable::Direction::Vertical(viewport_scrollbar()))
        .style(style)
        .width(width)
        .height(height)
        .into()
}

/// Terminal frame sized to its content (App Settings theme sample).
fn terminal_preview_panel<'a>(
    content: impl Into<Element<'a, Message>>,
    palette: LogPalette,
) -> Element<'a, Message> {
    container(content.into())
        .padding(4)
        .style(style::log_surface(palette))
        .width(Length::Fill)
        .into()
}

/// Height of a command log when the surrounding page scrolls (kernel config, system update).
pub const COMMAND_LOG_PAGE_HEIGHT: Length = Length::Fixed(360.0);

/// Monospace live-output panel used by PGO builds and `abs -RU`.
#[allow(clippy::too_many_arguments)]
pub fn command_log<'a>(
    title: &'a str,
    hint: String,
    placeholder: &'static str,
    lines: &'a VecDeque<String>,
    autoscroll: bool,
    pinned: bool,
    viewport: ViewportId,
    theme: AppTheme,
    palette: LogPalette,
    editor_height: Length,
    stdin_value: &'a str,
    stdin_enabled: bool,
) -> Element<'a, Message> {
    let empty = lines.is_empty();
    let following = autoscroll && pinned;
    let follow_icon = if following { "▶" } else { "⏸" };
    let follow_text = if following {
        abs_i18n::t("gui.common.autoscroll_on")
    } else {
        abs_i18n::t("gui.common.autoscroll_off")
    };
    let follow_label = row![text(follow_icon).size(13), text(follow_text).size(13),]
        .spacing(6)
        .align_y(Alignment::Center);
    let controls = row![
        button(follow_label)
            .style(style::theme_chip(theme, following))
            .on_press(Message::ViewportAutoscroll(viewport, !following)),
        button(text(abs_i18n::t("gui.common.copy_all")).size(13))
            .style(style::btn_secondary(theme))
            .on_press(Message::LogCopy),
        button(text(abs_i18n::t("gui.common.save_log")).size(13))
            .style(style::btn_secondary(theme))
            .on_press_maybe((!empty).then_some(Message::LogSave)),
        button(text(abs_i18n::t("gui.common.clear")).size(13))
            .style(style::btn_secondary(theme))
            .on_press(Message::LogClear),
        Space::new().width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let heading = text(title).size(style::TEXT_CARD).font(Font {
        weight: iced::font::Weight::Semibold,
        ..Font::DEFAULT
    });
    let fill = matches!(editor_height, Length::Fill);
    let editor_id = viewport.scroll_id();
    let body: Element<'a, Message> = LogLines::new(lines, palette)
        .placeholder(placeholder)
        .into();
    let mut log_scroll = scrollable(body)
        .id(editor_id)
        .direction(scrollable::Direction::Vertical(viewport_scrollbar()))
        .style(style::terminal_scroll(palette))
        .width(Length::Fill)
        .height(Length::Fill);
    log_scroll = log_scroll.on_scroll(move |vp| {
        Message::ViewportScrolled(viewport, log_scroll_pinned(vp.relative_offset().y, pinned))
    });
    let panel = container(log_scroll)
        .padding(4)
        .style(style::log_surface(palette))
        .width(Length::Fill)
        .height(if fill { Length::Fill } else { editor_height });
    let mut reply = text_input(abs_i18n::t("gui.log.stdin_placeholder"), stdin_value)
        .size(13)
        .font(Font::MONOSPACE)
        .padding(Padding::from([8.0, 10.0]))
        .width(Length::Fill);
    if stdin_enabled {
        reply = reply
            .on_input(Message::AbsStdinChanged)
            .on_submit(Message::AbsStdinSubmit);
    }
    let stdin_row = row![
        text(">")
            .size(14)
            .font(Font::MONOSPACE)
            .color(palette.green()),
        reply,
        button(text(abs_i18n::t("gui.log.stdin_send")).size(13))
            .style(style::btn_secondary(theme))
            .on_press_maybe(stdin_enabled.then_some(Message::AbsStdinSubmit)),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .width(Length::Fill);
    let mut body = column![
        heading,
        text(hint).size(11).color(palette.hint),
        controls,
        panel,
        stdin_row,
    ]
    .spacing(12);
    if fill {
        body = body.height(Length::Fill);
    }
    let mut card = container(body)
        .padding(16)
        .width(Length::Fill)
        .style(style::card(theme));
    if fill {
        card = card.height(Length::Fill);
    }
    card.into()
}

const ANSI_NAME_KEYS: [&str; 8] = [
    "gui.field.ansi_black",
    "gui.field.ansi_red",
    "gui.field.ansi_green",
    "gui.field.ansi_yellow",
    "gui.field.ansi_blue",
    "gui.field.ansi_magenta",
    "gui.field.ansi_cyan",
    "gui.field.ansi_white",
];

fn ansi_color_line<'a>(colors: [iced::Color; 8]) -> Element<'a, Message> {
    let mut spans: Vec<iced::widget::text::Span<'static, ()>> = Vec::with_capacity(15);
    for (i, key) in ANSI_NAME_KEYS.iter().enumerate() {
        if i > 0 {
            spans.push(span(" "));
        }
        spans.push(span(abs_i18n::t(key)).color(colors[i]));
    }
    rich_text(spans).font(Font::MONOSPACE).size(13).into()
}

fn fake_terminal_body<'a>(palette: LogPalette) -> Element<'a, Message> {
    let prompt = rich_text::<(), Message, _, _>([
        span("user@abs:~$ ").color(palette.green()),
        span("abs -RU").color(palette.fg),
        span(" "),
    ])
    .font(Font::MONOSPACE)
    .size(13);
    let cursor = container(
        Space::new()
            .width(Length::Fixed(8.0))
            .height(Length::Fixed(14.0)),
    )
    .style(style::cursor_block(palette));
    let build = rich_text::<(), Message, _, _>([
        span("==> ").color(palette.green()),
        span(abs_i18n::t("gui.field.preview_starting")).color(palette.fg),
    ])
    .font(Font::MONOSPACE)
    .size(13);
    let warning = rich_text::<(), Message, _, _>([
        span("warning: ").color(palette.yellow()),
        span(abs_i18n::t("gui.field.preview_fallback")).color(palette.fg),
    ])
    .font(Font::MONOSPACE)
    .size(13);
    let error = rich_text::<(), Message, _, _>([
        span("error: ").color(palette.red()),
        span(abs_i18n::t("gui.field.preview_failed")).color(palette.fg),
    ])
    .font(Font::MONOSPACE)
    .size(13);
    let paths = rich_text::<(), Message, _, _>([
        span("/var/cache/abs").color(palette.cyan()),
        span("  ").color(palette.fg),
        span("linux-cachyos").color(palette.magenta()),
        span("  ").color(palette.fg),
        span(abs_i18n::t("gui.field.preview_ok")).color(palette.blue()),
    ])
    .font(Font::MONOSPACE)
    .size(13);
    column![
        row![prompt, cursor]
            .spacing(2)
            .align_y(iced::Alignment::Center),
        text(abs_i18n::t("gui.field.preview_normal"))
            .size(11)
            .color(palette.hint)
            .font(Font::MONOSPACE),
        ansi_color_line(palette.ansi),
        text(abs_i18n::t("gui.field.preview_bright"))
            .size(11)
            .color(palette.hint)
            .font(Font::MONOSPACE),
        ansi_color_line(palette.bright),
        build,
        warning,
        error,
        paths,
    ]
    .spacing(6)
    .padding(10)
    .into()
}

/// Palette chips and preview for the current window theme only.
pub fn terminal_theme_picker<'a>(
    preview_dark: TerminalTheme,
    committed_dark: TerminalTheme,
    preview_light: TerminalTheme,
    committed_light: TerminalTheme,
    chrome: AppTheme,
) -> Element<'a, Message> {
    let (slot, preview, committed) = match chrome {
        AppTheme::Dark => (AppTheme::Dark, preview_dark, committed_dark),
        AppTheme::Light => (AppTheme::Light, preview_light, committed_light),
    };
    let dirty = preview != committed;
    let apply = button(text(abs_i18n::t("gui.settings.use_theme")).size(13))
        .style(style::btn_primary(chrome))
        .on_press_maybe(dirty.then_some(Message::TerminalThemeApply));
    column![
        help_line(field_help::terminal_colors(), chrome),
        terminal_theme_slot(slot, preview, chrome),
        apply,
    ]
    .spacing(12)
    .into()
}

fn terminal_theme_slot<'a>(
    slot: AppTheme,
    preview: TerminalTheme,
    chrome: AppTheme,
) -> Element<'a, Message> {
    let themes = TerminalTheme::choices_for(slot, preview);
    let mut chips = column![].spacing(6);
    for chunk in themes.chunks(4) {
        let mut r = row![].spacing(6).align_y(Alignment::Center);
        for theme in chunk {
            let selected = *theme == preview;
            let palette = style::log_palette(slot, *theme);
            r = r.push(
                button(
                    row![
                        viewport_bg_dot(palette),
                        text(theme.display_label()).size(style::TEXT_CHIP),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                )
                .padding(Padding::from([5.0, 10.0]))
                .style(style::theme_chip(chrome, selected))
                .on_press(Message::TerminalThemePreview(slot, *theme)),
            );
        }
        chips = chips.push(r);
    }
    let preview_palette = style::log_palette(slot, preview);
    column![
        chips,
        terminal_preview_panel(fake_terminal_body(preview_palette), preview_palette),
    ]
    .spacing(6)
    .into()
}

/// Dot fill is the scheme’s log viewport background; a 1px contrast ring keeps light palettes visible.
fn viewport_bg_dot<'a>(palette: LogPalette) -> Element<'a, Message> {
    container(
        container(Space::new())
            .width(Length::Fixed(12.0))
            .height(Length::Fixed(12.0))
            .style(style::color_swatch(palette.bg)),
    )
    .padding(1)
    .style(style::color_swatch(style::swatch_contrast_ring(palette.bg)))
    .into()
}

pub fn app_theme_toggle<'a>(current: ThemePref, theme: AppTheme) -> Element<'a, Message> {
    row![
        button(text(abs_i18n::t("gui.settings.theme_dark")).size(style::TEXT_BODY))
            .width(Length::Fill)
            .padding(Padding::from([8.0, 12.0]))
            .style(style::theme_chip(theme, current == ThemePref::Dark))
            .on_press(Message::AppThemeSelected(ThemePref::Dark)),
        button(text(abs_i18n::t("gui.settings.theme_light")).size(style::TEXT_BODY))
            .width(Length::Fill)
            .padding(Padding::from([8.0, 12.0]))
            .style(style::theme_chip(theme, current == ThemePref::Light))
            .on_press(Message::AppThemeSelected(ThemePref::Light)),
        button(text(abs_i18n::t("gui.settings.theme_system")).size(style::TEXT_BODY))
            .width(Length::Fill)
            .padding(Padding::from([8.0, 12.0]))
            .style(style::theme_chip(theme, current == ThemePref::System))
            .on_press(Message::AppThemeSelected(ThemePref::System)),
    ]
    .spacing(8)
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

/// Compact folder/file button beside a path field (tooltip keeps Browse / Choose file).
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
    field_label_column(
        label,
        help,
        app_theme,
        row![
            text_input(placeholder, value)
                .on_input(on_change)
                .padding(8)
                .width(Length::Fill),
            path_browse_button(kind, Message::BrowsePath(field, kind), app_theme),
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
        themed_pick_list(opts, selected, on_change, app_theme, Length::Fill),
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

/// Integer setting with a compact text field and − / + buttons.
pub fn stepper_number<'a>(
    label: &'a str,
    help: Option<&'a str>,
    value: &str,
    app_theme: AppTheme,
    on_input: impl Fn(String) -> Message + 'a,
    on_dec: Message,
    on_inc: Message,
) -> Element<'a, Message> {
    field_label_column(
        label,
        help,
        app_theme,
        row![
            icon_control_btn(step_glyph("−"), on_dec, app_theme),
            text_input("5000", value)
                .on_input(on_input)
                .padding(8)
                .width(Length::Fixed(100.0)),
            icon_control_btn(step_glyph("+"), on_inc, app_theme),
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center)
        .into(),
    )
}

pub fn log_save_row<'a>(
    target: LogSaveTarget,
    path: &'a str,
    dont_ask: bool,
    format: LogSaveFormat,
    app_theme: AppTheme,
) -> Element<'a, Message> {
    field_label_column(
        target.settings_label(),
        None,
        app_theme,
        row![
            text_input("%date%_%time%_%log_name%.%ext%", path)
                .on_input(move |s| Message::LogSavePath(target, s))
                .padding(8)
                .width(Length::Fill),
            path_browse_button(PathKind::Folder, Message::LogSaveBrowse(target), app_theme,),
            checkbox(dont_ask)
                .label(abs_i18n::t("gui.log.dont_ask"))
                .on_toggle(move |v| Message::LogSaveDontAsk(target, v)),
            themed_pick_list(
                LogSaveFormat::ALL,
                Some(format),
                move |f| Message::LogSaveFormat(target, f),
                app_theme,
                Length::Fixed(110.0),
            ),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .into(),
    )
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
    let mut col = column![checkbox(checked).label(label).on_toggle(on_toggle),].spacing(4);
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

pub fn ramdisk_targets_field(
    target: EditTarget,
    workdir: bool,
    chroot: bool,
    packages: bool,
    profiles: bool,
    app_theme: AppTheme,
) -> Element<'static, Message> {
    field_label_column(
        abs_i18n::t("gui.abs.ramdisk_targets"),
        Some(field_help::ramdisk_targets()),
        app_theme,
        column![
            field_checkbox(
                abs_i18n::t("gui.abs.ramdisk_w"),
                Some(field_help::ramdisk_w()),
                workdir,
                app_theme,
                move |v| Message::SetRamdiskTarget(target, RamdiskLetter::Workdir, v),
            ),
            field_checkbox(
                abs_i18n::t("gui.abs.ramdisk_c"),
                Some(field_help::ramdisk_c()),
                chroot,
                app_theme,
                move |v| Message::SetRamdiskTarget(target, RamdiskLetter::Chroot, v),
            ),
            field_checkbox(
                abs_i18n::t("gui.abs.ramdisk_p"),
                Some(field_help::ramdisk_p()),
                packages,
                app_theme,
                move |v| Message::SetRamdiskTarget(target, RamdiskLetter::Packages, v),
            ),
            field_checkbox(
                abs_i18n::t("gui.abs.ramdisk_r"),
                Some(field_help::ramdisk_r()),
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
        abs_i18n::t("gui.field.ramdisk_kernel"),
        Some(field_help::kernel_ramdisk_targets()),
        app_theme,
        column![
            field_checkbox(
                abs_i18n::t("gui.field.ramdisk_kernel_w"),
                Some(field_help::kernel_ramdisk_w()),
                workdir,
                app_theme,
                move |v| Message::SetRamdiskTarget(target, RamdiskLetter::Workdir, v),
            ),
            field_checkbox(
                abs_i18n::t("gui.field.ramdisk_kernel_p"),
                Some(field_help::kernel_ramdisk_p()),
                packages,
                app_theme,
                move |v| Message::SetRamdiskTarget(target, RamdiskLetter::Packages, v),
            ),
            field_checkbox(
                abs_i18n::t("gui.field.ramdisk_kernel_r"),
                Some(field_help::kernel_ramdisk_r()),
                profiles,
                app_theme,
                move |v| Message::SetRamdiskTarget(target, RamdiskLetter::Profiles, v),
            ),
            field_checkbox(
                abs_i18n::t("gui.field.ramdisk_kernel_c"),
                Some(field_help::ramdisk_c()),
                chroot,
                app_theme,
                move |v| Message::SetRamdiskTarget(target, RamdiskLetter::Chroot, v),
            ),
            text(field_help::kernel_ramdisk_hint())
                .size(style::TEXT_HELP)
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
        .id(field.editor_id())
        .font(Font::MONOSPACE)
        .size(14)
        .padding(10)
        .height(Length::Fixed(260.0));
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
    let default_choice = abs_i18n::tf("gui.field.default_choice", &[("label", default_label)]);
    let tri: String = match value {
        None => default_choice.clone(),
        Some(true) => "true".into(),
        Some(false) => "false".into(),
    };
    let options = vec![default_choice.clone(), "true".into(), "false".into()];
    field_label_column(
        label,
        help,
        app_theme,
        themed_pick_list(
            options,
            Some(tri),
            move |choice| {
                let v = if choice == default_choice {
                    None
                } else if choice == "true" {
                    Some(true)
                } else {
                    Some(false)
                };
                on_change(v)
            },
            app_theme,
            Length::Fill,
        ),
    )
}

pub fn settings_tab_bar<'a>(
    active_tab: crate::messages::SettingsTab,
    app_theme: AppTheme,
) -> Element<'a, Message> {
    use crate::messages::SettingsTab;
    let tabs = [
        (
            SettingsTab::GeneralPaths,
            abs_i18n::t("gui.abs.tab_general"),
        ),
        (SettingsTab::BuildChroot, abs_i18n::t("gui.abs.tab_build")),
        (SettingsTab::Ramdisk, abs_i18n::t("gui.abs.tab_ramdisk")),
        (SettingsTab::HeldPackages, abs_i18n::t("gui.abs.tab_held")),
        (SettingsTab::Repositories, abs_i18n::t("gui.abs.tab_repos")),
    ];
    let mut bar = row![]
        .spacing(3)
        .align_y(iced::Alignment::Center)
        .width(Length::Fill);
    for (tab, label) in tabs {
        let is_active = active_tab == tab;
        bar = bar.push(
            button(text(label).size(style::TEXT_CHIP).font(iced::Font {
                weight: if is_active {
                    iced::font::Weight::Bold
                } else {
                    iced::font::Weight::Medium
                },
                ..iced::Font::DEFAULT
            }))
            .padding(iced::Padding::from([4.0, 10.0]))
            .style(style::tab_button(app_theme, is_active))
            .on_press(Message::SettingsTabSelected(tab)),
        );
    }
    container(bar.wrap().vertical_spacing(3))
        .padding(3)
        .width(Length::Fill)
        .style(style::tab_bar_strip(app_theme))
        .into()
}

pub fn search_bar<'a, F>(
    query: &'a str,
    placeholder: &'static str,
    app_theme: AppTheme,
    on_input: F,
    input_id: Option<&'static str>,
) -> Element<'a, Message>
where
    F: Fn(String) -> Message + 'a,
{
    let mut input = text_input(placeholder, query)
        .on_input(on_input)
        .width(Length::Fill);
    if let Some(id) = input_id {
        input = input.id(id);
    }
    let mut inner = row![text("🔍").size(14).color(style::muted(app_theme)), input,]
        .spacing(8)
        .align_y(iced::Alignment::Center);
    if input_id.is_some() {
        inner = inner.push(
            container(
                text(abs_i18n::t("gui.chrome.search_shortcut"))
                    .size(10)
                    .font(Font::MONOSPACE)
                    .color(style::muted(app_theme)),
            )
            .padding(Padding::from([1.5, 6.0]))
            .style(style::kbd_hint(app_theme)),
        );
    }
    container(inner)
        .padding(Padding::from([4.0, 10.0]))
        .style(style::card(app_theme))
        .width(Length::Fill)
        .into()
}

pub fn ram_share_meter<'a>(
    label: &'a str,
    size: &'a str,
    ram_total: u64,
    ram_used: u64,
    app_theme: AppTheme,
) -> Element<'a, Message> {
    let (left_ratio, used_ratio, left_warn, caption) =
        ram_share_caption(size, ram_total, ram_used, app_theme);
    let (left_only, overlap, gap, right_only) =
        ramdisk_size::share_bar_segments(left_ratio, used_ratio);
    let heading = row![
        text(label)
            .size(style::TEXT_HELP)
            .color(style::muted(app_theme))
            .width(Length::Fill),
        caption,
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let mut fills = row![].height(Length::Fixed(10.0)).width(Length::Fill);
    fills = push_meter_segment(fills, left_only, style::meter_fill(app_theme, left_warn));
    fills = push_meter_segment(fills, overlap, style::meter_fill_overlap(app_theme));
    fills = push_meter_gap(fills, gap);
    fills = push_meter_segment(fills, right_only, style::meter_fill_used(app_theme));
    let track = container(fills)
        .width(Length::Fill)
        .height(Length::Fixed(10.0))
        .style(style::meter_track(app_theme));
    column![heading, track]
        .spacing(6)
        .width(Length::Fill)
        .into()
}

fn push_meter_segment<'a>(
    row: iced::widget::Row<'a, Message>,
    fraction: f32,
    style: impl Fn(&Theme) -> iced::widget::container::Style + 'a,
) -> iced::widget::Row<'a, Message> {
    let portion = meter_portion(fraction);
    if portion == 0 {
        return row;
    }
    row.push(
        container(Space::new())
            .width(Length::FillPortion(portion))
            .height(Length::Fixed(10.0))
            .style(style),
    )
}

fn push_meter_gap<'a>(
    row: iced::widget::Row<'a, Message>,
    fraction: f32,
) -> iced::widget::Row<'a, Message> {
    let portion = meter_portion(fraction);
    if portion == 0 {
        return row;
    }
    row.push(
        container(Space::new())
            .width(Length::FillPortion(portion))
            .height(Length::Fixed(10.0)),
    )
}

fn meter_portion(fraction: f32) -> u16 {
    if fraction <= 0.0 {
        0
    } else {
        ((fraction * 1000.0).round() as u16).max(1)
    }
}

fn ram_share_caption<'a>(
    size: &'a str,
    ram_total: u64,
    ram_used: u64,
    app_theme: AppTheme,
) -> (f32, f32, bool, Element<'a, Message>) {
    let muted = style::muted(app_theme);
    let used_ratio = if ram_total == 0 {
        0.0
    } else {
        ram_used as f32 / ram_total as f32
    };
    if ram_total == 0 {
        return (
            0.0,
            0.0,
            false,
            text(abs_i18n::t("gui.abs.ramdisk_vs_ram_unknown"))
                .size(style::TEXT_HELP)
                .color(muted)
                .into(),
        );
    }
    let ram = ramdisk_size::fmt_bytes(ram_total);
    let size = size.trim();
    let (left_ratio, left_warn, ramdisk_bytes, base) = match ramdisk_size::check(size, ram_total) {
        SizeVsRam::Invalid => (
            0.0,
            true,
            None,
            abs_i18n::t("gui.abs.ramdisk_vs_ram_invalid").to_string(),
        ),
        SizeVsRam::Fits { ratio, bytes } => {
            let pct = format!("{}", (ratio * 100.0).round().clamp(0.0, 100.0) as u32);
            (
                ratio,
                false,
                Some(bytes),
                abs_i18n::tf(
                    "gui.abs.ramdisk_vs_ram_of",
                    &[("size", size), ("ram", &ram), ("pct", &pct)],
                ),
            )
        }
        SizeVsRam::Exceeds { ratio, bytes } => (
            ratio,
            true,
            Some(bytes),
            abs_i18n::tf(
                "gui.abs.ramdisk_vs_ram_over",
                &[("size", size), ("ram", &ram)],
            ),
        ),
    };
    let lack = ramdisk_bytes.map(|disk| ramdisk_size::deficiency_bytes(disk, ram_used, ram_total));
    let bold = Font {
        weight: iced::font::Weight::Bold,
        ..Font::DEFAULT
    };
    let base_color = if left_warn {
        style::danger(app_theme)
    } else {
        muted
    };
    let mut spans: Vec<iced::widget::text::Span<'static, ()>> = vec![span(base).color(base_color)];
    if ram_used > 0 {
        let used_fmt = ramdisk_size::fmt_bytes(ram_used);
        spans.push(span(", ").color(muted));
        spans.push(span(used_fmt).font(bold).color(style::ram_used(app_theme)));
        spans.push(
            span(format!(
                " {}",
                abs_i18n::t("gui.abs.ramdisk_vs_ram_consumed")
            ))
            .color(muted),
        );
    }
    if let Some(lack) = lack {
        if lack > 0 {
            let lack_fmt = ramdisk_size::fmt_bytes(lack);
            spans.push(span(", ").color(muted));
            spans.push(
                span(abs_i18n::tf(
                    "gui.abs.ramdisk_vs_ram_lack",
                    &[("lack", &lack_fmt)],
                ))
                .color(style::danger(app_theme)),
            );
        }
    }
    (
        left_ratio,
        used_ratio,
        left_warn,
        rich_text(spans).size(style::TEXT_HELP).into(),
    )
}

pub fn unsaved_changes_dialog(app_theme: AppTheme) -> Element<'static, Message> {
    let body = column![
        text(abs_i18n::t("gui.common.unsaved")).size(20).font(Font {
            weight: iced::font::Weight::Semibold,
            ..Font::DEFAULT
        }),
        text(abs_i18n::t("gui.common.unsaved_body"))
            .size(14)
            .color(style::muted(app_theme)),
        row![
            button(text(abs_i18n::t("gui.common.save")).size(14))
                .style(style::btn_primary(app_theme))
                .padding(Padding::from([8.0, 16.0]))
                .on_press(Message::UnsavedSave),
            button(text(abs_i18n::t("gui.common.discard")).size(14))
                .style(style::btn_danger(app_theme))
                .padding(Padding::from([8.0, 16.0]))
                .on_press(Message::UnsavedDiscard),
            button(text(abs_i18n::t("gui.common.cancel")).size(14))
                .style(style::btn_secondary(app_theme))
                .padding(Padding::from([8.0, 16.0]))
                .on_press(Message::UnsavedCancel),
        ]
        .spacing(8),
    ]
    .spacing(14)
    .max_width(420);
    container(body)
        .padding(24)
        .style(style::card(app_theme))
        .into()
}

pub fn confirm_dialog<'a>(
    title: String,
    body: String,
    confirm_label: &'static str,
    confirm: Message,
    cancel: Message,
    app_theme: AppTheme,
) -> Element<'a, Message> {
    let content = column![
        text(title).size(20).font(Font {
            weight: iced::font::Weight::Semibold,
            ..Font::DEFAULT
        }),
        text(body).size(14).color(style::muted(app_theme)),
        row![
            button(text(confirm_label).size(14))
                .style(style::btn_danger(app_theme))
                .padding(Padding::from([8.0, 16.0]))
                .on_press(confirm),
            button(text(abs_i18n::t("gui.common.cancel")).size(14))
                .style(style::btn_secondary(app_theme))
                .padding(Padding::from([8.0, 16.0]))
                .on_press(cancel),
        ]
        .spacing(8),
    ]
    .spacing(14)
    .max_width(420);
    container(content)
        .padding(24)
        .style(style::card(app_theme))
        .into()
}

pub fn preview_pkgbuild_button<'a>(
    name: String,
    size: f32,
    app_theme: AppTheme,
) -> Element<'a, Message> {
    let padding = if size >= 13.0 {
        Padding::from([8.0, 16.0])
    } else {
        Padding::from([4.0, 10.0])
    };
    button(text(abs_i18n::t("gui.packages.preview_pkgbuild")).size(size))
        .padding(padding)
        .style(style::btn_secondary(app_theme))
        .on_press(Message::PreviewPkgbuild(name))
        .into()
}

pub fn pkgbuild_preview_dialog<'a>(
    preview: &'a PkgbuildPreview,
    app_theme: AppTheme,
) -> Element<'a, Message> {
    let title = match preview.version.as_deref().filter(|s| !s.is_empty()) {
        Some(version) => abs_i18n::tf(
            "gui.pkgbuild.title_version",
            &[("name", preview.name.as_str()), ("version", version)],
        ),
        None => abs_i18n::tf("gui.pkgbuild.title", &[("name", preview.name.as_str())]),
    };

    let body: Element<'a, Message> = if let Some(err) = preview.error.as_deref() {
        text(err).size(14).color(style::danger(app_theme)).into()
    } else if preview.text.is_some() {
        let colorize_delta = preview.show_delta
            && preview
                .delta
                .as_deref()
                .is_some_and(|diff| !diff.is_empty());
        let src: &str = if preview.show_delta {
            match preview.delta.as_deref() {
                Some(diff) if !diff.is_empty() => diff,
                Some(_) => abs_i18n::t("gui.pkgbuild.no_changes"),
                None => abs_i18n::t("gui.pkgbuild.no_previous"),
            }
        } else {
            preview.text.as_deref().unwrap_or("")
        };
        pkgbuild_source_scroll(src, colorize_delta, app_theme)
    } else {
        text(abs_i18n::t("gui.pkgbuild.loading"))
            .size(14)
            .color(style::muted(app_theme))
            .into()
    };

    let close = button(text(abs_i18n::t("gui.common.close")).size(14))
        .style(style::btn_secondary(app_theme))
        .padding(Padding::from([8.0, 16.0]))
        .on_press(Message::ClosePkgbuildPreview);
    let view_toggle = if preview.show_delta {
        button(text(abs_i18n::t("gui.pkgbuild.full_text")).size(14))
            .style(style::btn_secondary(app_theme))
            .padding(Padding::from([8.0, 16.0]))
            .on_press(Message::TogglePkgbuildDelta)
    } else {
        button(text(abs_i18n::t("gui.pkgbuild.delta")).size(14))
            .style(style::btn_secondary(app_theme))
            .padding(Padding::from([8.0, 16.0]))
            .on_press(Message::TogglePkgbuildDelta)
    };
    let buttons = if preview.text.is_some() {
        row![
            view_toggle,
            button(text(abs_i18n::t("gui.common.copy")).size(14))
                .style(style::btn_primary(app_theme))
                .padding(Padding::from([8.0, 16.0]))
                .on_press(Message::CopyPkgbuild),
            close,
        ]
        .spacing(8)
        .align_y(Alignment::Center)
    } else {
        row![close].spacing(8).align_y(Alignment::Center)
    };

    let header = row![
        text(title).size(18).font(Font {
            weight: iced::font::Weight::Semibold,
            ..Font::DEFAULT
        }),
        Space::new().width(Length::Fill),
        buttons,
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    let filled = preview.text.is_some();
    let col_height = if filled { Length::Fill } else { Length::Shrink };
    container(
        column![header, body]
            .spacing(12)
            .width(Length::Fill)
            .height(col_height),
    )
    .padding(20)
    .width(Length::Fill)
    .height(if filled { Length::Fill } else { Length::Shrink })
    .max_width(960)
    .max_height(720)
    .style(style::card(app_theme))
    .into()
}

fn pkgbuild_source_scroll<'a>(
    src: &'a str,
    colorize_delta: bool,
    app_theme: AppTheme,
) -> Element<'a, Message> {
    let inner: Element<'a, Message> = if colorize_delta {
        pkgbuild_delta_lines(src, app_theme)
    } else {
        text(src)
            .size(12.5)
            .font(Font::MONOSPACE)
            .wrapping(iced::widget::text::Wrapping::None)
            .into()
    };
    scrollable(
        container(inner)
            .padding(12)
            .width(Length::Fill)
            .style(style::code_well(app_theme)),
    )
    .id("pkgbuild-preview")
    .direction(scrollable::Direction::Both {
        vertical: viewport_scrollbar(),
        horizontal: viewport_scrollbar(),
    })
    .style(style::page_scroll(app_theme))
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn pkgbuild_delta_lines<'a>(src: &'a str, app_theme: AppTheme) -> Element<'a, Message> {
    let mut col = column![].spacing(0);
    for line in src.lines() {
        let kind = crate::pkgbuild_diff::classify_diff_line(line);
        let (fg, bg, bold) = style::diff_line_style(app_theme, kind);
        let shown = if line.is_empty() { " " } else { line };
        col = col.push(
            container(
                text(shown)
                    .size(12.5)
                    .font(Font {
                        weight: if bold {
                            iced::font::Weight::Bold
                        } else {
                            iced::font::Weight::Normal
                        },
                        ..Font::MONOSPACE
                    })
                    .color(fg)
                    .wrapping(iced::widget::text::Wrapping::None),
            )
            .width(Length::Fill)
            .padding(Padding::from([1.0, 6.0]))
            .style(style::diff_line_fill(bg)),
        );
    }
    col.into()
}

#[derive(Clone, Copy)]
pub struct PgoRoundStep {
    pub key: &'static str,
    pub label: &'static str,
    pub done: bool,
    pub active: bool,
    pub selected: bool,
}

fn pgo_node_kind(step: PgoRoundStep) -> style::PgoNodeKind {
    if step.done {
        style::PgoNodeKind::Done
    } else if step.active {
        style::PgoNodeKind::Active
    } else if step.selected {
        style::PgoNodeKind::Selected
    } else {
        style::PgoNodeKind::Pending
    }
}

fn pgo_node_glyph(step: PgoRoundStep, index: usize) -> String {
    if step.done {
        "✓".into()
    } else if step.key.starts_with("wait_reboot") {
        "↻".into()
    } else {
        (index + 1).to_string()
    }
}

fn pgo_node_label_color(step: PgoRoundStep, app_theme: AppTheme) -> iced::Color {
    if step.done {
        match app_theme {
            AppTheme::Dark => iced::Color::from_rgb8(0xf1, 0xf5, 0xf9),
            AppTheme::Light => iced::Color::from_rgb8(0x0f, 0x17, 0x2a),
        }
    } else if step.active || step.selected {
        style::primary(app_theme)
    } else {
        style::muted(app_theme)
    }
}

fn pgo_node_glyph_color(kind: style::PgoNodeKind, app_theme: AppTheme) -> iced::Color {
    match kind {
        style::PgoNodeKind::Done => match app_theme {
            AppTheme::Dark => iced::Color::from_rgb8(0x06, 0x4e, 0x3b),
            AppTheme::Light => iced::Color::WHITE,
        },
        style::PgoNodeKind::Active => iced::Color::WHITE,
        style::PgoNodeKind::Selected => style::primary_soft(app_theme),
        style::PgoNodeKind::Pending => style::muted(app_theme),
    }
}

/// Round PGO stepper plus a bar that fills for finished steps and the current step.
pub fn pgo_round_pipeline<'a>(
    steps: Vec<PgoRoundStep>,
    done_n: u16,
    active_n: u16,
    clickable: bool,
    app_theme: AppTheme,
) -> Element<'a, Message> {
    let n = steps.len().max(1) as u16;
    let rest_n = n.saturating_sub(done_n.saturating_add(active_n));
    let mut track = row![]
        .spacing(0)
        .align_y(Alignment::Start)
        .width(Length::Fill);
    for (i, step) in steps.iter().copied().enumerate() {
        if i > 0 {
            let kind = if (i as u16) <= done_n {
                style::PgoTrackKind::Done
            } else if (i as u16) == done_n + 1 && active_n > 0 {
                style::PgoTrackKind::Active
            } else {
                style::PgoTrackKind::Pending
            };
            let line = container(Space::new())
                .width(Length::Fill)
                .height(Length::Fixed(3.0))
                .style(style::pgo_connector_line(app_theme, kind));
            track = track.push(
                container(line)
                    .width(Length::FillPortion(2))
                    .padding(Padding {
                        top: (style::PGO_NODE_SIZE - 3.0) / 2.0,
                        right: 0.0,
                        bottom: 0.0,
                        left: 0.0,
                    }),
            );
        }
        let kind = pgo_node_kind(step);
        let glyph = pgo_node_glyph(step, i);
        let circle = container(
            text(glyph)
                .size(18)
                .font(Font {
                    weight: iced::font::Weight::Bold,
                    ..Font::DEFAULT
                })
                .color(pgo_node_glyph_color(kind, app_theme)),
        )
        .width(Length::Fixed(style::PGO_NODE_SIZE))
        .height(Length::Fixed(style::PGO_NODE_SIZE))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(style::pgo_node_circle(app_theme, kind));
        let label = text(step.label)
            .size(12.5)
            .font(Font {
                weight: iced::font::Weight::Bold,
                ..Font::DEFAULT
            })
            .color(pgo_node_label_color(step, app_theme))
            .align_x(Alignment::Center);
        let node = column![circle, label]
            .spacing(10)
            .align_x(Alignment::Center)
            .width(Length::Fill);
        let node: Element<'a, Message> = if clickable {
            button(node)
                .padding(Padding::from([2.0, 0.0]))
                .style(style::pgo_node_hit())
                .on_press(Message::PgoSelectStage(step.key.to_string()))
                .into()
        } else {
            node.into()
        };
        track = track.push(container(node).width(Length::FillPortion(3)));
    }

    let mut bar = row![].height(Length::Fill);
    if done_n > 0 {
        bar = bar.push(
            container(Space::new())
                .width(Length::FillPortion(done_n))
                .height(Length::Fill)
                .style(style::pgo_progress_fill(
                    app_theme,
                    style::PgoTrackKind::Done,
                )),
        );
    }
    if active_n > 0 {
        bar = bar.push(
            container(Space::new())
                .width(Length::FillPortion(active_n))
                .height(Length::Fill)
                .style(style::pgo_progress_fill(
                    app_theme,
                    style::PgoTrackKind::Active,
                )),
        );
    }
    if rest_n > 0 {
        bar = bar.push(
            container(Space::new())
                .width(Length::FillPortion(rest_n))
                .height(Length::Fill),
        );
    }
    let progress = container(bar)
        .width(Length::Fill)
        .height(Length::Fixed(style::PGO_BAR_HEIGHT))
        .style(style::pgo_progress_track(app_theme));

    column![track, progress]
        .spacing(16)
        .width(Length::Fill)
        .into()
}

pub const SEARCH_KERNELS: &str = "search-kernels";
pub const SEARCH_PACKAGES: &str = "search-packages";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavChrome {
    pub labels: bool,
    pub brand_text: bool,
    pub metrics: NavMetrics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavMetrics {
    Full,
    Compact,
    Hidden,
}

/// Inner width of the top nav after left/right chrome padding.
pub fn nav_inner_width(window_width: f32) -> f32 {
    (window_width - 2.0 * style::shell_pad_x(window_width)).max(0.0)
}

/// Inner width needed for labeled tabs + brand + metrics to fit on one row.
const NAV_LABELS_MIN: f32 = 1400.0;
const NAV_BRAND_TEXT_MIN: f32 = 900.0;
const NAV_METRICS_FULL_MIN: f32 = 1280.0;
const NAV_METRICS_COMPACT_MIN: f32 = 640.0;

/// Density of the top nav so every page stays reachable on small displays.
pub fn nav_chrome(inner_width: f32) -> NavChrome {
    NavChrome {
        labels: inner_width >= NAV_LABELS_MIN,
        brand_text: inner_width >= NAV_BRAND_TEXT_MIN,
        metrics: if inner_width >= NAV_METRICS_FULL_MIN {
            NavMetrics::Full
        } else if inner_width >= NAV_METRICS_COMPACT_MIN {
            NavMetrics::Compact
        } else {
            NavMetrics::Hidden
        },
    }
}

pub fn top_nav_tab<'a>(
    icon: &'static str,
    label: &'static str,
    badge: Option<String>,
    active: bool,
    show_label: bool,
    theme: AppTheme,
    msg: Message,
) -> Element<'a, Message> {
    let mut content = row![text(icon).size(15)]
        .spacing(8)
        .align_y(Alignment::Center);
    if show_label {
        content = content.push(text(label).size(13).font(Font {
            weight: if active {
                iced::font::Weight::Bold
            } else {
                iced::font::Weight::Semibold
            },
            ..Font::DEFAULT
        }));
    }
    if let Some(badge) = badge {
        content = content.push(
            container(text(badge).size(10.5).font(Font {
                weight: iced::font::Weight::Bold,
                ..Font::DEFAULT
            }))
            .padding(Padding::from([1.0, 6.0]))
            .style(style::nav_badge(theme, active)),
        );
    }
    let pad = if show_label {
        Padding::from([7.0, 14.0])
    } else {
        Padding::from([7.0, 10.0])
    };
    let tab = button(content)
        .padding(pad)
        .style(style::tab_button(theme, active))
        .on_press(msg);
    if show_label {
        tab.into()
    } else {
        tooltip(
            tab,
            container(text(label).size(style::TEXT_HELP))
                .padding(Padding::from([4.0, 8.0]))
                .style(style::tooltip_box(theme)),
            tooltip::Position::Bottom,
        )
        .gap(6)
        .delay(Duration::from_millis(350))
        .into()
    }
}

pub fn breadcrumb_row<'a>(
    parent: &'static str,
    current: String,
    extra_right: Option<Element<'a, Message>>,
    theme: AppTheme,
) -> Element<'a, Message> {
    let crumbs = row![
        text(parent)
            .size(16)
            .font(Font {
                weight: iced::font::Weight::Semibold,
                ..Font::DEFAULT
            })
            .color(style::muted(theme)),
        text(">").size(16).color(style::muted(theme)),
        text(current).size(20).font(Font {
            weight: iced::font::Weight::Bold,
            ..Font::DEFAULT
        }),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let mut row = row![crumbs, Space::new().width(Length::Fill)]
        .spacing(12)
        .align_y(Alignment::Center);
    if let Some(right) = extra_right {
        row = row.push(right);
    }
    row.into()
}

pub fn filter_chip<'a>(
    label: &'static str,
    count: usize,
    active: bool,
    theme: AppTheme,
    msg: Message,
) -> Element<'a, Message> {
    button(
        row![
            text(label).size(style::TEXT_CHIP).font(Font {
                weight: iced::font::Weight::Semibold,
                ..Font::DEFAULT
            }),
            container(text(count.to_string()).size(11).font(Font {
                weight: iced::font::Weight::Bold,
                ..Font::DEFAULT
            }),)
            .padding(Padding::from([1.0, 5.0]))
            .style(style::nav_badge(theme, active)),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding(Padding::from([4.0, 10.0]))
    .style(style::quick_filter_btn(theme, active))
    .on_press(msg)
    .into()
}

pub fn wizard_stepper<'a>(current: usize, total: usize, theme: AppTheme) -> Element<'a, Message> {
    let mut bar = row![].spacing(6).align_y(Alignment::Center);
    for i in 0..total {
        let done = i < current;
        let active = i == current;
        bar = bar.push(
            container(
                text((i + 1).to_string())
                    .size(11)
                    .font(Font {
                        weight: iced::font::Weight::Bold,
                        ..Font::DEFAULT
                    })
                    .color(if active || done {
                        style::primary_soft(theme)
                    } else {
                        style::muted(theme)
                    }),
            )
            .padding(Padding::from([4.0, 9.0]))
            .style(style::wizard_step_tag(theme, active, done)),
        );
    }
    bar.into()
}

#[cfg(test)]
mod tests {
    use super::{encode_ramdisk_flags, parse_ramdisk_flags};

    #[test]
    fn log_scroll_hysteresis() {
        assert_eq!(super::log_scroll_at_bottom(1.0), Some(true));
        assert_eq!(super::log_scroll_at_bottom(0.0), Some(false));
        assert_eq!(super::log_scroll_at_bottom(0.85), None);
        assert_eq!(super::log_scroll_at_bottom(f32::NAN), Some(true));
    }

    #[test]
    fn ramdisk_flags_roundtrip() {
        assert_eq!(parse_ramdisk_flags("wcp"), (true, true, true, false));
        assert_eq!(parse_ramdisk_flags("wcr"), (true, true, false, true));
        assert_eq!(parse_ramdisk_flags("wc"), (true, true, false, false));
        assert_eq!(encode_ramdisk_flags(true, false, true, true), "wpr");
    }

    #[test]
    fn settings_tab_default() {
        assert_eq!(
            crate::messages::SettingsTab::default(),
            crate::messages::SettingsTab::GeneralPaths
        );
    }

    #[test]
    fn round_rect_corners_are_clipped() {
        assert!(super::round_rect_hit(10.0, 10.0, 0.0, 0.0, 20.0, 20.0, 4.0));
        assert!(!super::round_rect_hit(0.5, 0.5, 0.0, 0.0, 20.0, 20.0, 4.0));
        let _ = super::raster_path_icon(super::PathKind::Folder, iced::Color::WHITE);
        let _ = super::raster_path_icon(super::PathKind::File, iced::Color::BLACK);
    }

    #[test]
    fn nav_inner_width_subtracts_horizontal_chrome_padding() {
        let w = 1920.0;
        let pad = crate::style::shell_pad_x(w);
        assert!((super::nav_inner_width(w) - (w - 2.0 * pad)).abs() < f32::EPSILON);
    }

    #[test]
    fn nav_chrome_full_on_1080p() {
        let chrome = super::nav_chrome(super::nav_inner_width(1920.0));
        assert!(chrome.labels);
        assert!(chrome.brand_text);
        assert_eq!(chrome.metrics, super::NavMetrics::Full);
    }

    #[test]
    fn nav_chrome_drops_labels_on_1366_laptop() {
        // Brand + 6 labeled tabs + metrics pill is ~1500px; a 1366×768 screen clips pages.
        let chrome = super::nav_chrome(super::nav_inner_width(1366.0));
        assert!(!chrome.labels);
    }

    #[test]
    fn nav_chrome_compacts_at_minimum_window() {
        let chrome = super::nav_chrome(super::nav_inner_width(
            crate::app_settings::WINDOW_MIN_WIDTH,
        ));
        assert!(!chrome.labels);
        assert!(!chrome.brand_text);
        assert_ne!(chrome.metrics, super::NavMetrics::Full);
    }
}
