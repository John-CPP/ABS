use crate::abs_runner::PendingUpdates;
use crate::app_settings::AppTheme;
use crate::messages::Message;
use crate::style;
use crate::terminal_themes::LogPalette;
use crate::widgets::{command_log, dense_header_cell, dense_table, dense_table_row};
use iced::widget::{button, column, container, row, text, Space};
use iced::{Alignment, Color, Element, Font, Length, Padding};
use std::collections::VecDeque;

#[derive(Clone, Copy)]
enum PendingSource {
    Official,
    Aur,
    Abs,
}

#[allow(clippy::too_many_arguments)]
pub fn view<'a>(
    busy: bool,
    running: bool,
    pending: Option<&'a PendingUpdates>,
    pending_error: Option<&'a str>,
    pending_loading: bool,
    autoscroll: bool,
    pinned: bool,
    log_lines: &'a VecDeque<String>,
    theme: AppTheme,
    palette: LogPalette,
    stdin_value: &'a str,
    stdin_enabled: bool,
) -> Element<'a, Message> {
    let can_act = !busy && !pending_loading;
    let can_update_all = can_act && pending.is_some_and(|p| p.has_work());

    let mut col = column![crate::widgets::breadcrumb_row(
        abs_i18n::t("gui.nav.system_update"),
        abs_i18n::t("gui.system_update.hub").to_string(),
        Some(status_label(pending, pending_loading, theme)),
        theme,
    )]
    .spacing(10);

    if let Some(data) = pending {
        let n = data.repo.len() + data.aur.len() + data.manual.len();
        col = col.push(
            container(
                row![
                    text(abs_i18n::tf(
                        "gui.system_update.pending_banner",
                        &[("n", &n.to_string())],
                    ))
                    .size(13)
                    .font(Font {
                        weight: iced::font::Weight::Bold,
                        ..Font::DEFAULT
                    }),
                    Space::new().width(Length::Fill),
                    text(abs_i18n::tf(
                        "gui.system_update.helper",
                        &[(
                            "name",
                            if data.helper.is_empty() {
                                "?"
                            } else {
                                data.helper.as_str()
                            },
                        )],
                    ))
                    .size(style::TEXT_HELP)
                    .color(style::muted(theme)),
                ]
                .align_y(Alignment::Center),
            )
            .padding(Padding::from([8.0, 14.0]))
            .width(Length::Fill)
            .style(style::card_banner(theme)),
        );
    }

    col = col.push(
        row![
            button(text(abs_i18n::t("gui.common.refresh")).size(13))
                .padding(Padding::from([6.0, 12.0]))
                .style(style::btn_secondary(theme))
                .on_press_maybe(can_act.then_some(Message::PendingUpdatesRefresh)),
            button(text(abs_i18n::t("gui.system_update.update_all")).size(13))
                .padding(Padding::from([6.0, 12.0]))
                .style(style::btn_primary(theme))
                .on_press_maybe(can_update_all.then_some(Message::SystemUpdateStart)),
            button(text(abs_i18n::t("gui.system_update.install_repo")).size(13))
                .padding(Padding::from([6.0, 12.0]))
                .style(style::btn_secondary(theme))
                .on_press_maybe(
                    (can_act && pending.is_some_and(|p| !p.repo.is_empty()))
                        .then_some(Message::InstallRepoUpdates),
                ),
            button(text(abs_i18n::t("gui.common.abort")).size(13))
                .padding(Padding::from([6.0, 12.0]))
                .style(style::btn_danger(theme))
                .on_press_maybe(running.then_some(Message::SystemUpdateAbort)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    );

    if let Some(err) = pending_error {
        col = col.push(text(err).size(13).color(Color::from_rgb8(0xf8, 0x71, 0x71)));
    }

    if pending_loading && pending.is_none() {
        col = col.push(
            text(abs_i18n::t("gui.system_update.checking"))
                .size(13)
                .color(style::muted(theme)),
        );
    } else if let Some(data) = pending {
        col = col.push(pending_dense_table(data, can_act, theme));
        if !data.skipped.is_empty() {
            col = col.push(skipped_table(data, theme));
        }
    }

    col = col.push(command_log(
        abs_i18n::t("gui.system_update.log_title"),
        abs_i18n::t("gui.system_update.log_help").to_string(),
        abs_i18n::t("gui.system_update.log_empty"),
        log_lines,
        autoscroll,
        pinned,
        crate::messages::ViewportId::UpdateLog,
        theme,
        palette,
        Length::Fill,
        stdin_value,
        stdin_enabled,
    ));
    col.spacing(12).height(Length::Fill).into()
}

fn pending_dense_table<'a>(
    data: &'a PendingUpdates,
    can_act: bool,
    theme: AppTheme,
) -> Element<'a, Message> {
    const COLS: &[u16] = &[3, 2, 3, 3];
    let header = container(dense_table_row(
        vec![
            dense_header_cell(abs_i18n::t("gui.system_update.col_package"), theme),
            dense_header_cell(abs_i18n::t("gui.system_update.col_repo"), theme),
            dense_header_cell(abs_i18n::t("gui.system_update.col_version"), theme),
            dense_header_cell(abs_i18n::t("gui.system_update.col_action"), theme),
        ],
        COLS,
        true,
        false,
        false,
        theme,
    ))
    .style(style::dense_table_head(theme));

    let mut body = column![].spacing(0);
    let mut rows: Vec<(PendingSource, &crate::abs_runner::PendingPkg)> = Vec::new();
    for pkg in &data.repo {
        rows.push((PendingSource::Official, pkg));
    }
    for pkg in &data.aur {
        rows.push((PendingSource::Aur, pkg));
    }
    for pkg in &data.manual {
        rows.push((PendingSource::Abs, pkg));
    }

    if rows.is_empty() {
        return text(abs_i18n::t("gui.common.up_to_date"))
            .size(13)
            .color(style::muted(theme))
            .into();
    }

    for (source, pkg) in rows {
        let source_label = match source {
            PendingSource::Aur => abs_i18n::t("gui.system_update.source_aur"),
            PendingSource::Official => abs_i18n::t("gui.system_update.source_official"),
            PendingSource::Abs => abs_i18n::t("gui.system_update.source_abs"),
        };
        let action: Element<'a, Message> = match source {
            PendingSource::Aur => {
                let name = pkg.name.clone();
                row![
                    crate::widgets::preview_pkgbuild_button(name.clone(), 11.0, theme),
                    button(text(abs_i18n::t("gui.common.install")).size(11))
                        .padding(Padding::from([4.0, 10.0]))
                        .style(style::btn_secondary(theme))
                        .on_press_maybe(can_act.then_some(Message::InstallAur(name))),
                ]
                .spacing(6)
                .align_y(Alignment::Center)
                .into()
            }
            PendingSource::Official => container(
                text(abs_i18n::t("gui.system_update.status_pending"))
                    .size(10.5)
                    .font(Font {
                        weight: iced::font::Weight::Bold,
                        ..Font::DEFAULT
                    }),
            )
            .padding(Padding::from([2.0, 8.0]))
            .style(style::tag_muted(theme))
            .into(),
            PendingSource::Abs => container(
                text(abs_i18n::t("gui.system_update.status_abs"))
                    .size(10.5)
                    .font(Font {
                        weight: iced::font::Weight::Bold,
                        ..Font::DEFAULT
                    }),
            )
            .padding(Padding::from([2.0, 8.0]))
            .style(style::tag_success(theme))
            .into(),
        };
        body = body.push(dense_table_row(
            vec![
                text(pkg.name.clone())
                    .size(12.5)
                    .font(Font {
                        weight: iced::font::Weight::Bold,
                        family: iced::font::Family::Monospace,
                        ..Font::DEFAULT
                    })
                    .into(),
                container(text(source_label).size(10.5))
                    .padding(Padding::from([2.0, 7.0]))
                    .style(style::source_tag(
                        theme,
                        match source {
                            PendingSource::Aur => style::PkgSourceKind::Aur,
                            PendingSource::Official => style::PkgSourceKind::Official,
                            PendingSource::Abs => style::PkgSourceKind::Abs,
                        },
                    ))
                    .into(),
                text(abs_i18n::tf(
                    "gui.system_update.old_to_new",
                    &[("old", pkg.old.as_str()), ("new", pkg.new.as_str())],
                ))
                .size(12)
                .color(style::muted(theme))
                .into(),
                action,
            ],
            COLS,
            false,
            false,
            false,
            theme,
        ));
    }

    dense_table(header, body, theme)
}

fn status_label<'a>(
    pending: Option<&'a PendingUpdates>,
    loading: bool,
    theme: AppTheme,
) -> Element<'a, Message> {
    let label = if loading {
        abs_i18n::t("gui.system_update.checking_short").to_string()
    } else if let Some(p) = pending {
        let repo_n = p.repo.len().to_string();
        let aur_n = p.aur.len().to_string();
        let manual_n = p.manual.len().to_string();
        abs_i18n::tf(
            "gui.system_update.status",
            &[
                (
                    "helper",
                    if p.helper.is_empty() {
                        "?"
                    } else {
                        p.helper.as_str()
                    },
                ),
                ("repo", repo_n.as_str()),
                ("aur", aur_n.as_str()),
                ("manual", manual_n.as_str()),
            ],
        )
    } else {
        abs_i18n::t("gui.system_update.not_checked").into()
    };
    text(label)
        .size(style::TEXT_HELP)
        .color(style::muted(theme))
        .into()
}

fn skipped_table<'a>(data: &'a PendingUpdates, theme: AppTheme) -> Element<'a, Message> {
    const COLS: &[u16] = &[3, 4, 3];
    let header = container(dense_table_row(
        vec![
            dense_header_cell(abs_i18n::t("gui.system_update.col_package"), theme),
            dense_header_cell(abs_i18n::t("gui.system_update.col_version"), theme),
            dense_header_cell(abs_i18n::t("gui.system_update.skipped"), theme),
        ],
        COLS,
        true,
        false,
        false,
        theme,
    ))
    .style(style::dense_table_head(theme));
    let mut body = column![].spacing(0);
    for pkg in &data.skipped {
        body = body.push(dense_table_row(
            vec![
                text(pkg.name.clone()).size(12).font(Font::MONOSPACE).into(),
                text(abs_i18n::tf(
                    "gui.system_update.old_to_new",
                    &[("old", pkg.old.as_str()), ("new", pkg.new.as_str())],
                ))
                .size(12)
                .color(style::muted(theme))
                .into(),
                text(pkg.reason.clone())
                    .size(11)
                    .color(style::muted(theme))
                    .into(),
            ],
            COLS,
            false,
            false,
            false,
            theme,
        ));
    }
    dense_table(header, body, theme)
}
