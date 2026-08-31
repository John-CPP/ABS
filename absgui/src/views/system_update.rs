use crate::abs_runner::PendingUpdates;
use crate::app_settings::AppTheme;
use crate::messages::Message;
use crate::style;
use crate::terminal_themes::LogPalette;
use crate::widgets::{
    command_log, dense_header_cell, dense_table, dense_table_row, COMMAND_LOG_PAGE_HEIGHT,
};
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::widget::Tree;
use iced::advanced::{Renderer as _, Widget};
use iced::widget::{button, column, container, row, text, Space};
use iced::{
    Alignment, Background, Border, Color, Element, Font, Length, Padding, Rectangle, Size, Theme,
};
use std::collections::VecDeque;
use std::f32::consts::TAU;

/// One full gear turn, in seconds.
pub const GEAR_TURN_SECS: f32 = 1.2;
const GEAR_SIZE: f32 = 56.0;

pub fn show_fetch_overlay(pending_loading: bool) -> bool {
    pending_loading
}

pub fn gear_angle(elapsed_secs: f32) -> f32 {
    elapsed_secs * TAU / GEAR_TURN_SECS
}

struct SpinningGear {
    angle: f32,
    color: Color,
    hole: Color,
}

impl<Message> Widget<Message, Theme, iced::Renderer> for SpinningGear {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(GEAR_SIZE), Length::Fixed(GEAR_SIZE))
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        _limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(Size::new(GEAR_SIZE, GEAR_SIZE))
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: iced::mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let cx = bounds.center_x();
        let cy = bounds.center_y();
        let outer = GEAR_SIZE * 0.42;
        let hub = GEAR_SIZE * 0.22;
        let hole = GEAR_SIZE * 0.10;
        let tooth = GEAR_SIZE * 0.16;
        let ring = outer - tooth * 0.45;
        const TEETH: u32 = 8;

        let mut circle = |x: f32, y: f32, r: f32, color: Color| {
            renderer.fill_quad(
                Quad {
                    bounds: Rectangle {
                        x: x - r,
                        y: y - r,
                        width: r * 2.0,
                        height: r * 2.0,
                    },
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: r.into(),
                    },
                    shadow: iced::Shadow::default(),
                    snap: true,
                },
                Background::Color(color),
            );
        };

        circle(cx, cy, hub, self.color);
        for i in 0..TEETH {
            let a = self.angle + i as f32 * TAU / TEETH as f32;
            circle(
                cx + a.cos() * ring,
                cy + a.sin() * ring,
                tooth * 0.55,
                self.color,
            );
        }
        circle(cx, cy, hole, self.hole);
    }
}

impl<'a, Message: 'a> From<SpinningGear> for Element<'a, Message> {
    fn from(gear: SpinningGear) -> Self {
        Element::new(gear)
    }
}

pub fn fetching_overlay<'a>(theme: AppTheme, angle: f32) -> Element<'a, Message> {
    let gear = SpinningGear {
        angle,
        color: style::primary(theme),
        hole: style::surface(theme),
    };
    container(
        column![
            gear,
            text(abs_i18n::t("gui.system_update.fetching"))
                .size(16)
                .font(Font {
                    weight: iced::font::Weight::Semibold,
                    ..Font::DEFAULT
                }),
        ]
        .spacing(14)
        .align_x(Alignment::Center),
    )
    .padding(Padding::from([28.0, 36.0]))
    .style(style::card(theme))
    .into()
}

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
    let pgo_blocked = pending.is_some_and(pgo_is_blocking);
    let can_update_all = can_act && !pgo_blocked && pending.is_some_and(|p| p.has_work());

    let mut col = column![crate::widgets::breadcrumb_row(
        abs_i18n::t("gui.nav.system_update"),
        abs_i18n::t("gui.system_update.hub").to_string(),
        Some(status_label(pending, pending_loading, theme)),
        theme,
    )]
    .spacing(10);

    if let Some(data) = pending {
        if pgo_is_blocking(data) {
            col = col.push(pgo_paused_banner(data, theme));
        }
        let n = data.repo.len() + data.aur.len() + data.manual.len();
        if n > 0 || !pgo_is_blocking(data) {
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
                            ),],
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
                    (can_act && !pgo_blocked && pending.is_some_and(|p| !p.repo.is_empty()))
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

    if let Some(data) = pending {
        col = col.push(pending_dense_table(data, can_act && !pgo_blocked, theme));
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
        COMMAND_LOG_PAGE_HEIGHT,
        stdin_value,
        stdin_enabled,
    ));
    col.spacing(12).into()
}

fn pending_repo_label(source: PendingSource, pkg: &crate::abs_runner::PendingPkg) -> String {
    if let Some(repo) = pkg
        .repository
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return repo.to_string();
    }
    match source {
        PendingSource::Aur => abs_i18n::t("gui.system_update.source_aur").to_string(),
        PendingSource::Official => abs_i18n::t("gui.system_update.source_official").to_string(),
        PendingSource::Abs => abs_i18n::t("gui.system_update.source_abs").to_string(),
    }
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
        return text(empty_pending_copy(data))
            .size(13)
            .color(style::muted(theme))
            .into();
    }

    for (source, pkg) in rows {
        let source_label = pending_repo_label(source, pkg);
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

pub fn pgo_is_blocking(data: &PendingUpdates) -> bool {
    !data.pgo_pipelines.is_empty()
}

pub fn empty_pending_copy(data: &PendingUpdates) -> String {
    if pgo_is_blocking(data) {
        abs_i18n::t("gui.system_update.pgo_paused_empty").to_string()
    } else {
        abs_i18n::t("gui.common.up_to_date").to_string()
    }
}

pub fn skip_reason_label(reason: &str) -> String {
    match reason {
        "pgo_pipeline" => abs_i18n::t("gui.system_update.skip_pgo_pipeline").to_string(),
        other => other.to_string(),
    }
}

pub fn no_updates_message(pending: Option<&PendingUpdates>) -> String {
    if pending.is_some_and(pgo_is_blocking) {
        abs_i18n::t("gui.msg.no_updates_pgo").to_string()
    } else {
        abs_i18n::t("gui.msg.no_updates").to_string()
    }
}

fn pgo_paused_banner<'a>(data: &'a PendingUpdates, theme: AppTheme) -> Element<'a, Message> {
    let mut body = column![text(abs_i18n::t("gui.system_update.pgo_paused"))
        .size(13)
        .font(Font {
            weight: iced::font::Weight::Bold,
            ..Font::DEFAULT
        })
        .color(style::warning(theme)),]
    .spacing(6);
    for pipeline in &data.pgo_pipelines {
        body = body.push(
            text(abs_i18n::tf(
                "gui.system_update.pgo_paused_pkg",
                &[
                    ("package", pipeline.package.as_str()),
                    ("stage", pipeline.stage_label.as_str()),
                ],
            ))
            .size(12)
            .color(style::warning(theme)),
        );
    }
    body = body.push(
        text(abs_i18n::t("gui.system_update.pgo_paused_hint"))
            .size(12)
            .color(style::muted(theme)),
    );
    container(body)
        .padding(Padding::from([10.0, 14.0]))
        .width(Length::Fill)
        .style(style::warning_banner(theme))
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
                text(skip_reason_label(&pkg.reason))
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

#[cfg(test)]
mod tests {
    fn pkg(name: &str, repository: Option<&str>) -> crate::abs_runner::PendingPkg {
        crate::abs_runner::PendingPkg {
            name: name.into(),
            old: "1".into(),
            new: "2".into(),
            repository: repository.map(str::to_string),
        }
    }

    #[test]
    fn repo_column_shows_sync_repository_not_official() {
        assert_eq!(
            super::pending_repo_label(
                super::PendingSource::Official,
                &pkg("firefox", Some("cachyos-extra"))
            ),
            "cachyos-extra"
        );
        assert_eq!(
            super::pending_repo_label(super::PendingSource::Aur, &pkg("yay", Some("aur"))),
            "aur"
        );
        assert_eq!(
            super::pending_repo_label(super::PendingSource::Abs, &pkg("linux-cachyos", None)),
            abs_i18n::t("gui.system_update.source_abs")
        );
    }

    #[test]
    fn fetch_overlay_follows_loading_flag() {
        assert!(super::show_fetch_overlay(true));
        assert!(!super::show_fetch_overlay(false));
    }

    #[test]
    fn gear_angle_completes_a_turn_each_period() {
        assert!(super::gear_angle(0.0).abs() < 1e-5);
        assert!((super::gear_angle(super::GEAR_TURN_SECS) - std::f32::consts::TAU).abs() < 1e-4);
        assert!(super::gear_angle(super::GEAR_TURN_SECS / 2.0) > 0.0);
    }

    fn empty_pending() -> crate::abs_runner::PendingUpdates {
        crate::abs_runner::PendingUpdates::default()
    }

    fn pending_with_pgo() -> crate::abs_runner::PendingUpdates {
        crate::abs_runner::PendingUpdates {
            pgo_pipelines: vec![crate::abs_runner::PgoPipelineHold {
                package: "linux-cachyos".into(),
                stage_label: "Waiting for reboot (boot stage-2 kernel)".into(),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn empty_list_without_pgo_is_up_to_date() {
        assert_eq!(
            super::empty_pending_copy(&empty_pending()),
            abs_i18n::t("gui.common.up_to_date")
        );
        assert!(!super::pgo_is_blocking(&empty_pending()));
    }

    #[test]
    fn empty_list_with_unfinished_pgo_explains_pause() {
        let copy = super::empty_pending_copy(&pending_with_pgo());
        assert_ne!(copy, abs_i18n::t("gui.common.up_to_date"));
        assert!(
            copy.contains("PGO") || copy.to_lowercase().contains("pgo"),
            "empty system-update list must say PGO is why nothing is installable: {copy}"
        );
        assert!(super::pgo_is_blocking(&pending_with_pgo()));
    }

    #[test]
    fn skip_reason_pgo_pipeline_is_human() {
        let label = super::skip_reason_label("pgo_pipeline");
        assert_ne!(label, "pgo_pipeline");
        assert!(
            label.contains("PGO") || label.to_lowercase().contains("pgo"),
            "{label}"
        );
    }

    #[test]
    fn no_updates_message_mentions_pgo_when_pipeline_is_active() {
        assert_eq!(
            super::no_updates_message(Some(&empty_pending())),
            abs_i18n::t("gui.msg.no_updates")
        );
        let msg = super::no_updates_message(Some(&pending_with_pgo()));
        assert_ne!(msg, abs_i18n::t("gui.msg.no_updates"));
        assert!(
            msg.contains("PGO") || msg.to_lowercase().contains("pgo"),
            "{msg}"
        );
    }
}
