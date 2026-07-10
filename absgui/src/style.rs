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

// Palette: slate neutrals with a calm blue accent.
// Dark: soft graphite surfaces, no pure black; light: airy slate with white cards.

/// Accent color (buttons, active nav, highlights).
/// Light uses a muted denim blue: enough presence for buttons without glaring contrast.
pub fn primary(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0x60, 0xa5, 0xfa),
        AppTheme::Light => Color::from_rgb8(0x4a, 0x72, 0xb4),
    }
}

/// Softer accent for tinted text on accent-tinted backgrounds.
fn primary_soft(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0x93, 0xc5, 0xfd),
        AppTheme::Light => Color::from_rgb8(0x3c, 0x5f, 0x9a),
    }
}

/// Translucent accent for tag/selection backgrounds.
fn primary_tint(theme: AppTheme, alpha: f32) -> Color {
    let p = primary(theme);
    Color { a: alpha, ..p }
}

pub fn muted(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0x94, 0xa3, 0xb8),
        AppTheme::Light => Color::from_rgb8(0x5d, 0x6b, 0x7e),
    }
}

pub fn iced_theme(theme: AppTheme) -> Theme {
    match theme {
        AppTheme::Dark => Theme::custom(
            "ABS Dark",
            iced::theme::Palette {
                background: Color::from_rgb8(0x13, 0x17, 0x1d),
                text: Color::from_rgb8(0xe6, 0xea, 0xf0),
                primary: primary(AppTheme::Dark),
                success: Color::from_rgb8(0x34, 0xd3, 0x99),
                warning: Color::from_rgb8(0xfb, 0xbf, 0x24),
                danger: Color::from_rgb8(0xf8, 0x71, 0x71),
            },
        ),
        AppTheme::Light => Theme::custom(
            "ABS Light",
            iced::theme::Palette {
                background: Color::from_rgb8(0xf3, 0xf4, 0xf7),
                text: Color::from_rgb8(0x2a, 0x33, 0x40),
                primary: primary(AppTheme::Light),
                success: Color::from_rgb8(0x2e, 0x8b, 0x6b),
                warning: Color::from_rgb8(0xc2, 0x76, 0x1c),
                danger: Color::from_rgb8(0xc4, 0x4a, 0x4a),
            },
        ),
    }
}

fn surface(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0x1a, 0x20, 0x28),
        // Slightly off-white: keeps cards visible on the gray background without glare.
        AppTheme::Light => Color::from_rgb8(0xfc, 0xfc, 0xfd),
    }
}

fn surface_border(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0x2b, 0x33, 0x3f),
        AppTheme::Light => Color::from_rgb8(0xe1, 0xe4, 0xea),
    }
}

fn sidebar_bg(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0x0e, 0x11, 0x16),
        AppTheme::Light => Color::from_rgb8(0xea, 0xec, 0xf1),
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
        background: Some(Background::Color(primary_tint(app_theme, match app_theme {
            AppTheme::Dark => 0.16,
            AppTheme::Light => 0.12,
        }))),
        text_color: Some(primary_soft(app_theme)),
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
            AppTheme::Dark => Color::from_rgb8(0x25, 0x2c, 0x36),
            AppTheme::Light => Color::from_rgb8(0xed, 0xf0, 0xf5),
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
        background: Some(Background::Color(primary_tint(app_theme, match app_theme {
            AppTheme::Dark => 0.24,
            AppTheme::Light => 0.16,
        }))),
        text_color: Some(primary_soft(app_theme)),
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
        background: Some(Background::Color(primary_tint(app_theme, match app_theme {
            AppTheme::Dark => 0.08,
            AppTheme::Light => 0.07,
        }))),
        text_color: Some(muted(app_theme)),
        border: Border {
            color: primary_tint(app_theme, 0.35),
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
            AppTheme::Dark => Color::from_rgb8(0x0d, 0x14, 0x20),
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
                    AppTheme::Dark => Color::from_rgb8(0x20, 0x27, 0x31),
                    AppTheme::Light => Color::from_rgb8(0xdd, 0xe1, 0xe8),
                }
            } else {
                Color::TRANSPARENT
            })),
            text_color: match app_theme {
                AppTheme::Dark => Color::from_rgb8(0xc9, 0xd1, 0xdb),
                AppTheme::Light => Color::from_rgb8(0x39, 0x46, 0x58),
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
            AppTheme::Dark => Color::from_rgb8(0x0e, 0x11, 0x16),
            AppTheme::Light => Color::from_rgb8(0xee, 0xf1, 0xf6),
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
        AppTheme::Dark => Color::from_rgb8(0xdd, 0xe4, 0xec),
        AppTheme::Light => Color::from_rgb8(0x1c, 0x24, 0x30),
    }
}

pub fn log_hint(app_theme: AppTheme) -> Color {
    match app_theme {
        AppTheme::Dark => Color::from_rgb8(0x93, 0x9e, 0xac),
        AppTheme::Light => Color::from_rgb8(0x4c, 0x59, 0x6c),
    }
}

pub fn log_editor(app_theme: AppTheme) -> impl Fn(&Theme, text_editor::Status) -> text_editor::Style {
    let bg = match app_theme {
        AppTheme::Dark => Color::from_rgb8(0x0e, 0x11, 0x16),
        AppTheme::Light => Color::from_rgb8(0xee, 0xf1, 0xf6),
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
        selection: primary_tint(app_theme, match app_theme {
            AppTheme::Dark => 0.35,
            AppTheme::Light => 0.25,
        }),
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
