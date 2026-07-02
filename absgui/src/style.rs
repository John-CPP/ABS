use crate::app_settings::AppTheme;
use iced::widget::{button, container, text_editor};
use iced::{Background, Border, Color, Theme};

/// A known CachyOS kernel package: (package name, scheduler tag, description).
pub const KERNEL_CATALOG: &[(&str, &str, &str)] = &[
    (
        "linux-cachyos",
        "EEVDF",
        "Default general-purpose desktop and developer kernel.",
    ),
    (
        "linux-cachyos-bore",
        "BORE",
        "Interactive workloads and gaming; favours I/O-bound tasks.",
    ),
    (
        "linux-cachyos-lto",
        "EEVDF",
        "Clang ThinLTO + AutoFDO + Propeller. Most aggressively optimised.",
    ),
    (
        "linux-cachyos-eevdf",
        "EEVDF",
        "Explicit EEVDF build pinned alongside the default.",
    ),
    (
        "linux-cachyos-lts",
        "EEVDF",
        "Long-term support kernel; good fallback / second kernel.",
    ),
    (
        "linux-cachyos-hardened",
        "EEVDF",
        "Security-focused kernel with linux-hardened patches.",
    ),
    (
        "linux-cachyos-rt-bore",
        "RT+BORE",
        "Real-time workloads needing bounded latency (pro audio).",
    ),
    (
        "linux-cachyos-server",
        "EEVDF",
        "Server-tuned config; longer timeslices, different IO defaults.",
    ),
    (
        "linux-cachyos-deckify",
        "BORE",
        "Steam Deck-style tuning and gaming hardware patches.",
    ),
    (
        "linux-cachyos-bmq",
        "BMQ",
        "BitMap Queue scheduler. Niche; specific workloads only.",
    ),
];

pub fn primary(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0x46, 0xe6, 0xa0),
        AppTheme::Light => Color::from_rgb8(0x05, 0x96, 0x69),
    }
}

pub fn muted(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0xa8, 0xb0, 0xba),
        AppTheme::Light => Color::from_rgb8(0x64, 0x74, 0x8b),
    }
}

pub fn iced_theme(theme: AppTheme) -> Theme {
    match theme {
        AppTheme::Dark => Theme::custom(
            "ABS Dark",
            iced::theme::Palette {
                background: Color::from_rgb8(0x0f, 0x12, 0x16),
                text: Color::from_rgb8(0xee, 0xf1, 0xf5),
                primary: primary(AppTheme::Dark),
                success: primary(AppTheme::Dark),
                warning: Color::from_rgb8(0xff, 0xc1, 0x4e),
                danger: Color::from_rgb8(0xf0, 0x5d, 0x5d),
            },
        ),
        AppTheme::Light => Theme::custom(
            "ABS Light",
            iced::theme::Palette {
                background: Color::from_rgb8(0xec, 0xf0, 0xf4),
                text: Color::from_rgb8(0x1e, 0x29, 0x3b),
                primary: primary(AppTheme::Light),
                success: primary(AppTheme::Light),
                warning: Color::from_rgb8(0xd9, 0x77, 0x06),
                danger: Color::from_rgb8(0xdc, 0x26, 0x26),
            },
        ),
    }
}

fn surface(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0x16, 0x1b, 0x22),
        AppTheme::Light => Color::from_rgb8(0xff, 0xff, 0xff),
    }
}

fn surface_border(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0x28, 0x30, 0x3a),
        AppTheme::Light => Color::from_rgb8(0xcb, 0xd5, 0xe1),
    }
}

fn sidebar_bg(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0x0a, 0x0d, 0x11),
        AppTheme::Light => Color::from_rgb8(0xe2, 0xe8, 0xf0),
    }
}

pub fn card(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(surface(app_theme))),
        border: Border {
            color: surface_border(app_theme),
            width: 1.0,
            radius: 12.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn sidebar(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(sidebar_bg(app_theme))),
        border: Border {
            color: surface_border(app_theme),
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn tag(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgba(0.27, 0.9, 0.63, 0.18),
            AppTheme::Light => Color::from_rgb8(0xd1, 0xfa, 0xe5),
        })),
        text_color: Some(match app_theme {
            AppTheme::Dark => Color::from_rgb8(0x46, 0xe6, 0xa0),
            AppTheme::Light => Color::from_rgb8(0x04, 0x78, 0x57),
        }),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn tag_muted(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgb8(0x22, 0x28, 0x30),
            AppTheme::Light => Color::from_rgb8(0xf1, 0xf5, 0xf9),
        })),
        text_color: Some(muted(app_theme)),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
}

/// Current step in the PGO timeline — strong accent so it stays visible at a glance.
pub fn pgo_stage_active(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgba(0.27, 0.9, 0.63, 0.28),
            AppTheme::Light => Color::from_rgb8(0xa7, 0xf3, 0xd0),
        })),
        text_color: Some(match app_theme {
            AppTheme::Dark => Color::from_rgb8(0x6e, 0xff, 0xb8),
            AppTheme::Light => Color::from_rgb8(0x02, 0x5f, 0x45),
        }),
        border: Border {
            color: primary(app_theme),
            width: 2.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}

/// Completed PGO timeline step.
pub fn pgo_stage_done(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgba(0.27, 0.9, 0.63, 0.10),
            AppTheme::Light => Color::from_rgb8(0xe6, 0xf9, 0xf0),
        })),
        text_color: Some(match app_theme {
            AppTheme::Dark => Color::from_rgb8(0x8b, 0x9a, 0xa8),
            AppTheme::Light => Color::from_rgb8(0x4b, 0x5e, 0x70),
        }),
        border: Border {
            color: match app_theme {
                AppTheme::Dark => Color::from_rgba(0.27, 0.9, 0.63, 0.35),
                AppTheme::Light => Color::from_rgb8(0x6e, 0xe7, 0xb7),
            },
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn nav_active(app_theme: AppTheme) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, _status| button::Style {
        background: Some(Background::Color(primary(app_theme))),
        text_color: match app_theme {
            AppTheme::Dark => Color::from_rgb8(0x0c, 0x10, 0x0e),
            AppTheme::Light => Color::from_rgb8(0xff, 0xff, 0xff),
        },
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 8.0.into(),
        },
        ..button::Style::default()
    }
}

pub fn nav_inactive(app_theme: AppTheme) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: Some(Background::Color(if hovered {
                match app_theme {
                    AppTheme::Dark => Color::from_rgb8(0x1e, 0x24, 0x2c),
                    AppTheme::Light => Color::from_rgb8(0xcb, 0xd5, 0xe1),
                }
            } else {
                Color::TRANSPARENT
            })),
            text_color: match app_theme {
                AppTheme::Dark => Color::from_rgb8(0xd2, 0xd7, 0xdd),
                AppTheme::Light => Color::from_rgb8(0x33, 0x41, 0x55),
            },
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 8.0.into(),
            },
            ..button::Style::default()
        }
    }
}

pub fn log_surface(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgb8(0x0c, 0x0f, 0x14),
            AppTheme::Light => Color::from_rgb8(0xf1, 0xf5, 0xf9),
        })),
        border: Border {
            color: surface_border(app_theme),
            width: 1.0,
            radius: 8.0.into(),
        },
        text_color: Some(log_text(app_theme)),
        ..container::Style::default()
    }
}

pub fn log_text(app_theme: AppTheme) -> Color {
    match app_theme {
        AppTheme::Dark => Color::from_rgb8(0xe6, 0xed, 0xf3),
        AppTheme::Light => Color::from_rgb8(0x1e, 0x29, 0x3b),
    }
}

pub fn log_hint(app_theme: AppTheme) -> Color {
    match app_theme {
        AppTheme::Dark => Color::from_rgb8(0x9d, 0xa7, 0xb3),
        AppTheme::Light => Color::from_rgb8(0x47, 0x55, 0x69),
    }
}

pub fn log_editor(app_theme: AppTheme) -> impl Fn(&Theme, text_editor::Status) -> text_editor::Style {
    let bg = match app_theme {
        AppTheme::Dark => Color::from_rgb8(0x0c, 0x0f, 0x14),
        AppTheme::Light => Color::from_rgb8(0xf1, 0xf5, 0xf9),
    };
    let fg = log_text(app_theme);
    move |_theme, _status| text_editor::Style {
        background: Background::Color(bg),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 0.0.into(),
        },
        placeholder: log_hint(app_theme),
        value: fg,
        selection: match app_theme {
            AppTheme::Dark => Color::from_rgba(0.27, 0.9, 0.63, 0.35),
            AppTheme::Light => Color::from_rgb8(0xbb, 0xf7, 0xd0),
        },
    }
}

pub fn accent_bar(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(primary(app_theme))),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 2.0.into(),
        },
        ..container::Style::default()
    }
}
