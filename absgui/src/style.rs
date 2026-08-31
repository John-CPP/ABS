use crate::app_settings::AppTheme;
use crate::pkgbuild_diff::DiffLineKind;
use crate::terminal_themes::{LogPalette, TerminalTheme};
use iced::widget::{button, container, overlay, pick_list, scrollable, text_editor, text_input};
use iced::{Background, Border, Color, Shadow, Theme, Vector};

/// Minimum left/right inset shared by the top nav, page body, and status bar.
pub const SHELL_PAD_X: f32 = 24.0;
/// Fraction of chrome width used as left *and* right inset.
pub const SHELL_PAD_X_RATIO: f32 = 0.10;
/// Cap so a 1440p/ultrawide window does not get 10% × width (256px+) gutters.
pub const SHELL_PAD_X_MAX: f32 = 96.0;

/// Same horizontal inset for the top bar, page content, and status bar.
pub fn shell_pad_x(window_width: f32) -> f32 {
    (window_width * SHELL_PAD_X_RATIO).clamp(SHELL_PAD_X, SHELL_PAD_X_MAX)
}

/// Card headings (About, Appearance, …).
pub const TEXT_CARD: f32 = 15.0;
/// Primary body copy (paths, notes, status).
pub const TEXT_BODY: f32 = 14.0;
/// Field names.
pub const TEXT_LABEL: f32 = 14.0;
/// Help under fields and secondary captions.
pub const TEXT_HELP: f32 = 13.5;
/// Compact controls that still need to be readable (chips, tabs, meters).
pub const TEXT_CHIP: f32 = 13.0;
/// Height of text fields, steppers, browse icons, and selects.
pub const CONTROL_H: f32 = 32.0;

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

// High-fidelity Palette:
// Dark: Deep Obsidian & Slate (#090d13 / #121822 / #161e2a) with vibrant Cyan/Sky (#38bdf8) & Emerald (#34d399).
// Light: Crisp Pure White & Slate (#f8fafc / #ffffff / #f1f5f9) with Sky Blue (#0284c7).

/// Primary accent color.
pub fn primary(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0x38, 0xbd, 0xf8),
        AppTheme::Light => Color::from_rgb8(0x02, 0x84, 0xc7),
    }
}

/// Softer accent for tinted text on accent-tinted backgrounds.
pub fn primary_soft(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0x7d, 0xd3, 0xfc),
        AppTheme::Light => Color::from_rgb8(0x03, 0x69, 0xa1),
    }
}

/// Translucent accent for tag/selection backgrounds.
pub fn primary_tint(theme: AppTheme, alpha: f32) -> Color {
    let p = primary(theme);
    Color { a: alpha, ..p }
}

pub fn muted(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0x8b, 0x9b, 0xb0),
        AppTheme::Light => Color::from_rgb8(0x65, 0x6d, 0x76),
    }
}

/// Idle (unselected) label on tabs, chips, and similar buttons.
/// Brighter than [`muted`] so dark-theme controls stay readable.
pub fn button_idle(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0xcb, 0xd5, 0xe1),
        AppTheme::Light => Color::from_rgb8(0x33, 0x41, 0x55),
    }
}

pub fn button_idle_hover(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::WHITE,
        AppTheme::Light => Color::from_rgb8(0x0f, 0x17, 0x2a),
    }
}

pub fn iced_theme(theme: AppTheme) -> Theme {
    match theme {
        AppTheme::Dark => Theme::custom(
            "ABS Dark",
            iced::theme::Palette {
                background: Color::from_rgb8(0x09, 0x0d, 0x13),
                text: Color::from_rgb8(0xec, 0xf2, 0xf8),
                primary: primary(AppTheme::Dark),
                success: Color::from_rgb8(0x34, 0xd3, 0x99),
                warning: Color::from_rgb8(0xfb, 0xbf, 0x24),
                danger: Color::from_rgb8(0xf8, 0x71, 0x71),
            },
        ),
        AppTheme::Light => Theme::custom(
            "ABS Light",
            iced::theme::Palette {
                background: Color::from_rgb8(0xf8, 0xfa, 0xfc),
                text: Color::from_rgb8(0x1f, 0x23, 0x28),
                primary: primary(AppTheme::Light),
                success: Color::from_rgb8(0x1a, 0x7f, 0x37),
                warning: Color::from_rgb8(0x9a, 0x67, 0x00),
                danger: Color::from_rgb8(0xcf, 0x22, 0x2e),
            },
        ),
    }
}

pub fn surface(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0x16, 0x1b, 0x22),
        AppTheme::Light => Color::from_rgb8(0xff, 0xff, 0xff),
    }
}

pub fn surface_elevated(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0x1c, 0x22, 0x2d),
        AppTheme::Light => Color::from_rgb8(0xf0, 0xf4, 0xf8),
    }
}

pub fn surface_border(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0x2d, 0x36, 0x44),
        AppTheme::Light => Color::from_rgb8(0xd0, 0xd7, 0xde),
    }
}

pub fn sidebar_bg(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0x10, 0x16, 0x20),
        AppTheme::Light => Color::from_rgb8(0xf8, 0xfa, 0xfc),
    }
}

/// Filled circle with no inset border, so the color is exactly `fill`.
pub fn color_swatch(fill: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(fill)),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 16.0.into(),
        },
        ..container::Style::default()
    }
}

/// Hairline around a viewport-bg swatch so a light fill stays visible on a light chip.
pub fn swatch_contrast_ring(bg: Color) -> Color {
    let lum = 0.2126 * bg.r + 0.7152 * bg.g + 0.0722 * bg.b;
    if lum > 0.55 {
        Color::from_rgba(0.12, 0.14, 0.18, 0.5)
    } else {
        Color::from_rgba(1.0, 1.0, 1.0, 0.28)
    }
}

pub fn theme_chip(
    app_theme: AppTheme,
    selected: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered);
        if selected {
            button::Style {
                background: Some(Background::Color(primary_tint(app_theme, 0.16))),
                text_color: primary_soft(app_theme),
                border: Border {
                    color: primary(app_theme),
                    width: 1.2,
                    radius: 8.0.into(),
                },
                ..button::Style::default()
            }
        } else {
            button::Style {
                background: Some(Background::Color(if hovered {
                    surface_elevated(app_theme)
                } else {
                    surface(app_theme)
                })),
                text_color: if hovered {
                    button_idle_hover(app_theme)
                } else {
                    button_idle(app_theme)
                },
                border: Border {
                    color: surface_border(app_theme),
                    width: 1.0,
                    radius: 8.0.into(),
                },
                ..button::Style::default()
            }
        }
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

pub fn card_banner(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgb8(0x13, 0x1d, 0x29),
            AppTheme::Light => Color::from_rgb8(0xeb, 0xf3, 0xfd),
        })),
        border: Border {
            color: match app_theme {
                AppTheme::Dark => Color::from_rgba(0.22, 0.74, 0.97, 0.4),
                AppTheme::Light => Color::from_rgba(0.14, 0.45, 0.82, 0.3),
            },
            width: 1.0,
            radius: 12.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn tag(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(primary_tint(
            app_theme,
            match app_theme {
                AppTheme::Dark => 0.18,
                AppTheme::Light => 0.12,
            },
        ))),
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
            AppTheme::Dark => Color::from_rgb8(0x21, 0x28, 0x33),
            AppTheme::Light => Color::from_rgb8(0xe5, 0xe9, 0xf0),
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

pub fn tag_success(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgba(0.2, 0.83, 0.6, 0.18),
            AppTheme::Light => Color::from_rgba(0.1, 0.6, 0.3, 0.14),
        })),
        text_color: Some(match app_theme {
            AppTheme::Dark => Color::from_rgb8(0x34, 0xd3, 0x99),
            AppTheme::Light => Color::from_rgb8(0x1a, 0x7f, 0x37),
        }),
        border: Border {
            color: match app_theme {
                AppTheme::Dark => Color::from_rgba(0.2, 0.83, 0.6, 0.3),
                AppTheme::Light => Color::from_rgba(0.1, 0.6, 0.3, 0.25),
            },
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn tag_warning(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgba(0.98, 0.75, 0.14, 0.18),
            AppTheme::Light => Color::from_rgba(0.76, 0.46, 0.11, 0.14),
        })),
        text_color: Some(match app_theme {
            AppTheme::Dark => Color::from_rgb8(0xfb, 0xbf, 0x24),
            AppTheme::Light => Color::from_rgb8(0x9a, 0x67, 0x00),
        }),
        border: Border {
            color: match app_theme {
                AppTheme::Dark => Color::from_rgba(0.98, 0.75, 0.14, 0.35),
                AppTheme::Light => Color::from_rgba(0.76, 0.46, 0.11, 0.3),
            },
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn tag_info(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgba(0.22, 0.74, 0.97, 0.18),
            AppTheme::Light => Color::from_rgba(0.14, 0.45, 0.82, 0.14),
        })),
        text_color: Some(primary_soft(app_theme)),
        border: Border {
            color: match app_theme {
                AppTheme::Dark => Color::from_rgba(0.22, 0.74, 0.97, 0.35),
                AppTheme::Light => Color::from_rgba(0.14, 0.45, 0.82, 0.3),
            },
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn tag_sched(app_theme: AppTheme, sched: &str) -> impl Fn(&Theme) -> container::Style {
    let sched = sched.to_string();
    move |_theme| {
        let (bg, fg, border) = match sched.as_str() {
            "BORE" => match app_theme {
                AppTheme::Dark => (
                    Color::from_rgb8(0x02, 0x84, 0xc7),
                    Color::from_rgb8(0xff, 0xff, 0xff),
                    Color::from_rgb8(0x38, 0xbd, 0xf8),
                ),
                AppTheme::Light => (
                    Color::from_rgb8(0x02, 0x84, 0xc7),
                    Color::from_rgb8(0xff, 0xff, 0xff),
                    Color::from_rgb8(0x03, 0x69, 0xa1),
                ),
            },
            "RT+BORE" => match app_theme {
                AppTheme::Dark => (
                    Color::from_rgb8(0xd9, 0x77, 0x06),
                    Color::from_rgb8(0xff, 0xff, 0xff),
                    Color::from_rgb8(0xfb, 0xbf, 0x24),
                ),
                AppTheme::Light => (
                    Color::from_rgb8(0xd9, 0x77, 0x06),
                    Color::from_rgb8(0xff, 0xff, 0xff),
                    Color::from_rgb8(0xb4, 0x53, 0x09),
                ),
            },
            "BMQ" => match app_theme {
                AppTheme::Dark => (
                    Color::from_rgb8(0x0d, 0x94, 0x88),
                    Color::from_rgb8(0xff, 0xff, 0xff),
                    Color::from_rgb8(0x2d, 0xd4, 0xbf),
                ),
                AppTheme::Light => (
                    Color::from_rgb8(0x0d, 0x94, 0x88),
                    Color::from_rgb8(0xff, 0xff, 0xff),
                    Color::from_rgb8(0x0f, 0x76, 0x6e),
                ),
            },
            "AutoFDO" => match app_theme {
                AppTheme::Dark => (
                    Color::from_rgb8(0x43, 0x38, 0xca),
                    Color::from_rgb8(0xff, 0xff, 0xff),
                    Color::from_rgb8(0x81, 0x8c, 0xf8),
                ),
                AppTheme::Light => (
                    Color::from_rgb8(0x43, 0x38, 0xca),
                    Color::from_rgb8(0xff, 0xff, 0xff),
                    Color::from_rgb8(0x37, 0x30, 0xa3),
                ),
            },
            _ => match app_theme {
                AppTheme::Dark => (
                    Color::from_rgb8(0x33, 0x41, 0x55),
                    Color::from_rgb8(0xf1, 0xf5, 0xf9),
                    Color::from_rgb8(0x47, 0x55, 0x69),
                ),
                AppTheme::Light => (
                    Color::from_rgb8(0x47, 0x55, 0x69),
                    Color::from_rgb8(0xff, 0xff, 0xff),
                    Color::from_rgb8(0x33, 0x41, 0x55),
                ),
            },
        };
        container::Style {
            background: Some(Background::Color(bg)),
            text_color: Some(fg),
            border: Border {
                color: border,
                width: 1.0,
                radius: 6.0.into(),
            },
            ..container::Style::default()
        }
    }
}

/// Custom button: Primary (Solid Accent)
pub fn btn_primary(app_theme: AppTheme) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let (bg, text_color) = match status {
            button::Status::Hovered => (
                match app_theme {
                    AppTheme::Dark => Color::from_rgb8(0x0e, 0xa5, 0xe9),
                    AppTheme::Light => Color::from_rgb8(0x03, 0x69, 0xa1),
                },
                Color::from_rgb8(0xff, 0xff, 0xff),
            ),
            button::Status::Pressed => (
                match app_theme {
                    AppTheme::Dark => Color::from_rgb8(0x03, 0x69, 0xa1),
                    AppTheme::Light => Color::from_rgb8(0x07, 0x59, 0x85),
                },
                Color::from_rgb8(0xff, 0xff, 0xff),
            ),
            button::Status::Disabled => (
                match app_theme {
                    AppTheme::Dark => Color::from_rgb8(0x21, 0x27, 0x33),
                    AppTheme::Light => Color::from_rgb8(0xe2, 0xe8, 0xf0),
                },
                muted(app_theme),
            ),
            _ => (
                match app_theme {
                    AppTheme::Dark => Color::from_rgb8(0x02, 0x84, 0xc7),
                    AppTheme::Light => Color::from_rgb8(0x02, 0x84, 0xc7),
                },
                Color::from_rgb8(0xff, 0xff, 0xff),
            ),
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 8.0.into(),
            },
            ..button::Style::default()
        }
    }
}

/// Custom button: Secondary (Elevated Surface)
pub fn btn_secondary(app_theme: AppTheme) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered);
        let pressed = matches!(status, button::Status::Pressed);
        let disabled = matches!(status, button::Status::Disabled);
        button::Style {
            background: Some(Background::Color(if disabled {
                Color::TRANSPARENT
            } else if pressed {
                match app_theme {
                    AppTheme::Dark => Color::from_rgb8(0x28, 0x33, 0x44),
                    AppTheme::Light => Color::from_rgb8(0xd6, 0xdc, 0xe5),
                }
            } else if hovered {
                match app_theme {
                    AppTheme::Dark => Color::from_rgb8(0x23, 0x2c, 0x3a),
                    AppTheme::Light => Color::from_rgb8(0xe2, 0xe7, 0xf0),
                }
            } else {
                surface_elevated(app_theme)
            })),
            text_color: if disabled {
                muted(app_theme)
            } else {
                match app_theme {
                    AppTheme::Dark => Color::from_rgb8(0xec, 0xf2, 0xf8),
                    AppTheme::Light => Color::from_rgb8(0x1f, 0x23, 0x28),
                }
            },
            border: Border {
                color: surface_border(app_theme),
                width: 1.0,
                radius: 8.0.into(),
            },
            ..button::Style::default()
        }
    }
}

/// Custom button: Danger (Soft Red Tint)
pub fn btn_danger(app_theme: AppTheme) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered);
        let pressed = matches!(status, button::Status::Pressed);
        button::Style {
            background: Some(Background::Color(if pressed {
                Color::from_rgb8(0x99, 0x1b, 0x1b)
            } else if hovered {
                Color::from_rgb8(0xdc, 0x26, 0x26)
            } else {
                match app_theme {
                    AppTheme::Dark => Color::from_rgba(0.97, 0.44, 0.44, 0.15),
                    AppTheme::Light => Color::from_rgba(0.81, 0.13, 0.18, 0.12),
                }
            })),
            text_color: if hovered || pressed {
                Color::WHITE
            } else {
                match app_theme {
                    AppTheme::Dark => Color::from_rgb8(0xf8, 0x71, 0x71),
                    AppTheme::Light => Color::from_rgb8(0xcf, 0x22, 0x2e),
                }
            },
            border: Border {
                color: if hovered || pressed {
                    Color::TRANSPARENT
                } else {
                    match app_theme {
                        AppTheme::Dark => Color::from_rgba(0.97, 0.44, 0.44, 0.35),
                        AppTheme::Light => Color::from_rgba(0.81, 0.13, 0.18, 0.3),
                    }
                },
                width: 1.0,
                radius: 8.0.into(),
            },
            ..button::Style::default()
        }
    }
}

/// Compact square control (+/−, folder browse) with an accent border on hover.
pub fn btn_icon(app_theme: AppTheme) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered);
        let pressed = matches!(status, button::Status::Pressed);
        let disabled = matches!(status, button::Status::Disabled);
        button::Style {
            background: Some(Background::Color(if disabled {
                Color::TRANSPARENT
            } else if pressed {
                match app_theme {
                    AppTheme::Dark => Color::from_rgb8(0x28, 0x37, 0x4d),
                    AppTheme::Light => Color::from_rgb8(0xd6, 0xdc, 0xe5),
                }
            } else if hovered {
                match app_theme {
                    AppTheme::Dark => Color::from_rgb8(0x28, 0x37, 0x4d),
                    AppTheme::Light => Color::from_rgb8(0xe2, 0xe8, 0xf0),
                }
            } else {
                match app_theme {
                    AppTheme::Dark => Color::from_rgb8(0x1c, 0x26, 0x36),
                    AppTheme::Light => Color::from_rgb8(0xf8, 0xfa, 0xfc),
                }
            })),
            text_color: if disabled {
                muted(app_theme)
            } else if hovered || pressed {
                match app_theme {
                    AppTheme::Dark => Color::from_rgb8(0xff, 0xff, 0xff),
                    AppTheme::Light => Color::from_rgb8(0x0f, 0x17, 0x2a),
                }
            } else {
                match app_theme {
                    AppTheme::Dark => Color::from_rgb8(0xcb, 0xd5, 0xe1),
                    AppTheme::Light => Color::from_rgb8(0x33, 0x41, 0x55),
                }
            },
            border: Border {
                color: if hovered || pressed {
                    primary(app_theme)
                } else {
                    match app_theme {
                        AppTheme::Dark => Color::from_rgb8(0x2e, 0x3e, 0x56),
                        AppTheme::Light => surface_border(app_theme),
                    }
                },
                width: 1.0,
                radius: 8.0.into(),
            },
            ..button::Style::default()
        }
    }
}

pub fn select(app_theme: AppTheme) -> impl Fn(&Theme, pick_list::Status) -> pick_list::Style {
    move |_theme, status| {
        let accent = matches!(
            status,
            pick_list::Status::Hovered | pick_list::Status::Opened { .. }
        );
        pick_list::Style {
            text_color: match app_theme {
                AppTheme::Dark => Color::from_rgb8(0xec, 0xf2, 0xf8),
                AppTheme::Light => Color::from_rgb8(0x1f, 0x23, 0x28),
            },
            placeholder_color: muted(app_theme),
            handle_color: button_idle(app_theme),
            background: Background::Color(if accent {
                surface_elevated(app_theme)
            } else {
                match app_theme {
                    AppTheme::Dark => Color::from_rgb8(0x0b, 0x0f, 0x16),
                    AppTheme::Light => Color::from_rgb8(0xf8, 0xfa, 0xfc),
                }
            }),
            border: Border {
                color: if accent {
                    primary(app_theme)
                } else {
                    surface_border(app_theme)
                },
                width: 1.0,
                radius: 8.0.into(),
            },
        }
    }
}

pub fn pick_menu(app_theme: AppTheme) -> impl Fn(&Theme) -> overlay::menu::Style {
    move |_theme| overlay::menu::Style {
        background: Background::Color(surface(app_theme)),
        border: Border {
            color: surface_border(app_theme),
            width: 1.0,
            radius: 8.0.into(),
        },
        text_color: match app_theme {
            AppTheme::Dark => Color::from_rgb8(0xec, 0xf2, 0xf8),
            AppTheme::Light => Color::from_rgb8(0x1f, 0x23, 0x28),
        },
        selected_text_color: primary_soft(app_theme),
        selected_background: Background::Color(primary_tint(app_theme, 0.16)),
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.28),
            offset: Vector::new(0.0, 4.0),
            blur_radius: 16.0,
        },
    }
}

pub fn tooltip_box(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(surface_elevated(app_theme))),
        text_color: Some(match app_theme {
            AppTheme::Dark => Color::from_rgb8(0xec, 0xf2, 0xf8),
            AppTheme::Light => Color::from_rgb8(0x1f, 0x23, 0x28),
        }),
        border: Border {
            color: surface_border(app_theme),
            width: 1.0,
            radius: 8.0.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.22),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 10.0,
        },
        ..container::Style::default()
    }
}

pub fn catalog_btn_style(
    app_theme: AppTheme,
    configured: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        if configured {
            btn_primary(app_theme)(theme, status)
        } else {
            btn_secondary(app_theme)(theme, status)
        }
    }
}

pub fn tag_status_style(app_theme: AppTheme, active: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        if active {
            tag_success(app_theme)(theme)
        } else {
            tag_muted(app_theme)(theme)
        }
    }
}

pub fn wizard_step_tag(
    app_theme: AppTheme,
    active: bool,
    done: bool,
) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        if active {
            tag_info(app_theme)(theme)
        } else if done {
            tag_success(app_theme)(theme)
        } else {
            tag_muted(app_theme)(theme)
        }
    }
}

pub fn quick_filter_btn(
    app_theme: AppTheme,
    active: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered);
        if active {
            button::Style {
                background: Some(Background::Color(match app_theme {
                    AppTheme::Dark => Color::from_rgb8(0x02, 0x84, 0xc7),
                    AppTheme::Light => Color::from_rgb8(0x02, 0x84, 0xc7),
                })),
                text_color: Color::from_rgb8(0xff, 0xff, 0xff),
                border: Border {
                    color: primary(app_theme),
                    width: 1.0,
                    radius: 6.0.into(),
                },
                ..button::Style::default()
            }
        } else {
            button::Style {
                background: Some(Background::Color(if hovered {
                    match app_theme {
                        AppTheme::Dark => Color::from_rgb8(0x18, 0x22, 0x30),
                        AppTheme::Light => Color::from_rgb8(0xe2, 0xe8, 0xf0),
                    }
                } else {
                    match app_theme {
                        AppTheme::Dark => Color::from_rgb8(0x0b, 0x0f, 0x16),
                        AppTheme::Light => Color::from_rgb8(0xf8, 0xfa, 0xfc),
                    }
                })),
                text_color: if hovered {
                    button_idle_hover(app_theme)
                } else {
                    button_idle(app_theme)
                },
                border: Border {
                    color: surface_border(app_theme),
                    width: 1.0,
                    radius: 6.0.into(),
                },
                ..button::Style::default()
            }
        }
    }
}

pub fn log_palette(app_theme: AppTheme, terminal: TerminalTheme) -> LogPalette {
    terminal.palette(matches!(app_theme, AppTheme::Dark))
}

pub fn log_surface(palette: LogPalette) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(palette.bg)),
        border: Border {
            color: palette.border,
            width: 1.0,
            radius: 10.0.into(),
        },
        text_color: Some(palette.fg),
        ..container::Style::default()
    }
}

#[allow(dead_code)]
pub fn log_editor(
    palette: LogPalette,
) -> impl Fn(&Theme, text_editor::Status) -> text_editor::Style {
    move |_theme, _status| text_editor::Style {
        background: Background::Color(palette.bg),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 0.0.into(),
        },
        placeholder: palette.hint,
        value: palette.fg,
        selection: palette.selection,
    }
}

pub fn cursor_block(palette: LogPalette) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(palette.cursor)),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 1.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn viewport_scroll(
    fg: Color,
    hint: Color,
    bg: Option<Color>,
) -> impl Fn(&Theme, scrollable::Status) -> scrollable::Style {
    move |_theme, status| {
        let hovered = matches!(
            status,
            scrollable::Status::Hovered {
                is_vertical_scrollbar_hovered: true,
                ..
            } | scrollable::Status::Dragged {
                is_vertical_scrollbar_dragged: true,
                ..
            }
        );
        let rail = scrollable::Rail {
            background: Some(Background::Color(Color { a: 0.18, ..hint })),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 4.0.into(),
            },
            scroller: scrollable::Scroller {
                background: Background::Color(if hovered {
                    fg
                } else {
                    Color { a: 0.8, ..hint }
                }),
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: 4.0.into(),
                },
            },
        };
        let fill = bg.unwrap_or(Color::TRANSPARENT);
        scrollable::Style {
            container: container::Style {
                background: bg.map(Background::Color),
                ..container::Style::default()
            },
            vertical_rail: rail,
            horizontal_rail: rail,
            gap: None,
            auto_scroll: scrollable::AutoScroll {
                background: Background::Color(Color { a: 0.9, ..fill }),
                border: Border {
                    color: hint,
                    width: 1.0,
                    radius: 16.0.into(),
                },
                shadow: Shadow {
                    color: Color::BLACK.scale_alpha(0.5),
                    offset: Vector::ZERO,
                    blur_radius: 2.0,
                },
                icon: fg,
            },
        }
    }
}

pub fn terminal_scroll(
    palette: LogPalette,
) -> impl Fn(&Theme, scrollable::Status) -> scrollable::Style {
    viewport_scroll(palette.fg, palette.hint, Some(palette.bg))
}

pub fn page_scroll(
    app_theme: AppTheme,
) -> impl Fn(&Theme, scrollable::Status) -> scrollable::Style {
    viewport_scroll(muted(app_theme), muted(app_theme), None)
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

pub fn tab_button(
    app_theme: AppTheme,
    active: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered);
        if active {
            button::Style {
                background: Some(Background::Color(match app_theme {
                    AppTheme::Dark => Color::from_rgb8(0x23, 0x2d, 0x3d),
                    AppTheme::Light => Color::from_rgb8(0xe6, 0xeb, 0xf2),
                })),
                text_color: primary_soft(app_theme),
                border: Border {
                    color: primary(app_theme),
                    width: 1.5,
                    radius: 8.0.into(),
                },
                ..button::Style::default()
            }
        } else {
            button::Style {
                background: Some(Background::Color(if hovered {
                    match app_theme {
                        AppTheme::Dark => Color::from_rgb8(0x1d, 0x24, 0x30),
                        AppTheme::Light => Color::from_rgb8(0xee, 0xf1, 0xf6),
                    }
                } else {
                    Color::TRANSPARENT
                })),
                text_color: if hovered {
                    button_idle_hover(app_theme)
                } else {
                    button_idle(app_theme)
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
}

pub fn tab_bar_strip(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgb8(0x11, 0x16, 0x1e),
            AppTheme::Light => Color::from_rgb8(0xf0, 0xf2, 0xf6),
        })),
        border: Border {
            color: surface_border(app_theme),
            width: 1.0,
            radius: 10.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn meter_track(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgb8(0x22, 0x29, 0x35),
            AppTheme::Light => Color::from_rgb8(0xe4, 0xe8, 0xee),
        })),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 4.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn meter_fill(app_theme: AppTheme, warn: bool) -> impl Fn(&Theme) -> container::Style {
    meter_fill_color(if warn {
        danger(app_theme)
    } else {
        primary(app_theme)
    })
}

/// Current RAM use (right-hand fill on the ramdisk share bar).
pub fn ram_used(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0xfb, 0xbf, 0x24),
        AppTheme::Light => Color::from_rgb8(0xb4, 0x53, 0x09),
    }
}

/// Where ramdisk (left) and current use (right) overlap.
pub fn ram_overlap(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0xfb, 0x71, 0x85),
        AppTheme::Light => Color::from_rgb8(0xbe, 0x12, 0x3c),
    }
}

pub fn meter_fill_used(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    meter_fill_color(ram_used(app_theme))
}

pub fn meter_fill_overlap(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    meter_fill_color(ram_overlap(app_theme))
}

fn meter_fill_color(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(color)),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 4.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn pgo_connector_line(
    app_theme: AppTheme,
    kind: PgoTrackKind,
) -> impl Fn(&Theme) -> container::Style {
    move |_theme| {
        let color = match kind {
            PgoTrackKind::Done => match app_theme {
                AppTheme::Dark => Color::from_rgb8(0x34, 0xd3, 0x99),
                AppTheme::Light => Color::from_rgb8(0x05, 0x96, 0x69),
            },
            PgoTrackKind::Active => match app_theme {
                AppTheme::Dark => Color::from_rgb8(0x38, 0xbd, 0xf8),
                AppTheme::Light => Color::from_rgb8(0x02, 0x84, 0xc7),
            },
            PgoTrackKind::Pending => match app_theme {
                AppTheme::Dark => Color::from_rgb8(0x1e, 0x29, 0x3b),
                AppTheme::Light => Color::from_rgb8(0xe2, 0xe8, 0xf0),
            },
        };
        container::Style {
            background: Some(Background::Color(color)),
            shadow: match kind {
                PgoTrackKind::Done => Shadow {
                    color: Color {
                        a: 0.45,
                        ..Color::from_rgb8(0x34, 0xd3, 0x99)
                    },
                    offset: Vector::ZERO,
                    blur_radius: 6.0,
                },
                PgoTrackKind::Active => Shadow {
                    color: Color {
                        a: 0.4,
                        ..primary(app_theme)
                    },
                    offset: Vector::ZERO,
                    blur_radius: 6.0,
                },
                PgoTrackKind::Pending => Shadow::default(),
            },
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 2.0.into(),
            },
            ..container::Style::default()
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PgoNodeKind {
    Pending,
    Selected,
    Active,
    Done,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PgoTrackKind {
    Pending,
    Active,
    Done,
}

fn pgo_emerald(app_theme: AppTheme) -> Color {
    match app_theme {
        AppTheme::Dark => Color::from_rgb8(0x34, 0xd3, 0x99),
        AppTheme::Light => Color::from_rgb8(0x05, 0x96, 0x69),
    }
}

pub const PGO_NODE_SIZE: f32 = 46.0;
pub const PGO_BAR_HEIGHT: f32 = 8.0;

pub fn pgo_node_circle(
    app_theme: AppTheme,
    kind: PgoNodeKind,
) -> impl Fn(&Theme) -> container::Style {
    move |_theme| {
        let radius = (PGO_NODE_SIZE / 2.0).into();
        match kind {
            PgoNodeKind::Done => container::Style {
                background: Some(Background::Color(pgo_emerald(app_theme))),
                text_color: Some(match app_theme {
                    AppTheme::Dark => Color::from_rgb8(0x06, 0x4e, 0x3b),
                    AppTheme::Light => Color::WHITE,
                }),
                border: Border {
                    color: match app_theme {
                        AppTheme::Dark => Color::from_rgb8(0x6e, 0xe7, 0xb7),
                        AppTheme::Light => Color::from_rgb8(0x10, 0xb9, 0x81),
                    },
                    width: 2.0,
                    radius,
                },
                shadow: Shadow {
                    color: Color {
                        a: 0.45,
                        ..pgo_emerald(app_theme)
                    },
                    offset: Vector::ZERO,
                    blur_radius: 14.0,
                },
                ..container::Style::default()
            },
            PgoNodeKind::Active => container::Style {
                background: Some(Background::Color(match app_theme {
                    AppTheme::Dark => Color::from_rgb8(0x02, 0x84, 0xc7),
                    AppTheme::Light => Color::from_rgb8(0x02, 0x84, 0xc7),
                })),
                text_color: Some(Color::WHITE),
                border: Border {
                    color: match app_theme {
                        AppTheme::Dark => Color::from_rgb8(0x38, 0xbd, 0xf8),
                        AppTheme::Light => Color::from_rgb8(0x38, 0xbd, 0xf8),
                    },
                    width: 2.0,
                    radius,
                },
                shadow: Shadow {
                    color: Color {
                        a: 0.7,
                        ..primary(app_theme)
                    },
                    offset: Vector::ZERO,
                    blur_radius: 18.0,
                },
                ..container::Style::default()
            },
            PgoNodeKind::Selected => container::Style {
                background: Some(Background::Color(primary_tint(app_theme, 0.18))),
                text_color: Some(primary_soft(app_theme)),
                border: Border {
                    color: primary(app_theme),
                    width: 2.0,
                    radius,
                },
                ..container::Style::default()
            },
            PgoNodeKind::Pending => container::Style {
                background: Some(Background::Color(match app_theme {
                    AppTheme::Dark => Color::from_rgb8(0x18, 0x22, 0x30),
                    AppTheme::Light => Color::from_rgb8(0xf1, 0xf5, 0xf9),
                })),
                text_color: Some(muted(app_theme)),
                border: Border {
                    color: match app_theme {
                        AppTheme::Dark => Color::from_rgb8(0x28, 0x37, 0x4d),
                        AppTheme::Light => Color::from_rgb8(0xcb, 0xd5, 0xe1),
                    },
                    width: 1.0,
                    radius,
                },
                ..container::Style::default()
            },
        }
    }
}

pub fn pgo_node_hit() -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: Some(Background::Color(if hovered {
                Color::from_rgba(1.0, 1.0, 1.0, 0.04)
            } else {
                Color::TRANSPARENT
            })),
            text_color: Color::WHITE,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 10.0.into(),
            },
            ..button::Style::default()
        }
    }
}

pub fn pgo_progress_track(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgb8(0x14, 0x1c, 0x28),
            AppTheme::Light => Color::from_rgb8(0xe2, 0xe8, 0xf0),
        })),
        border: Border {
            color: match app_theme {
                AppTheme::Dark => Color::from_rgb8(0x1f, 0x2b, 0x3e),
                AppTheme::Light => Color::from_rgb8(0xcb, 0xd5, 0xe1),
            },
            width: 1.0,
            radius: 4.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn pgo_progress_fill(
    app_theme: AppTheme,
    kind: PgoTrackKind,
) -> impl Fn(&Theme) -> container::Style {
    move |_theme| {
        let color = match kind {
            PgoTrackKind::Done => pgo_emerald(app_theme),
            PgoTrackKind::Active => match app_theme {
                AppTheme::Dark => Color::from_rgb8(0x02, 0x84, 0xc7),
                AppTheme::Light => Color::from_rgb8(0x02, 0x84, 0xc7),
            },
            PgoTrackKind::Pending => Color::TRANSPARENT,
        };
        container::Style {
            background: Some(Background::Color(color)),
            shadow: match kind {
                PgoTrackKind::Done => Shadow {
                    color: Color {
                        a: 0.55,
                        ..pgo_emerald(app_theme)
                    },
                    offset: Vector::ZERO,
                    blur_radius: 10.0,
                },
                PgoTrackKind::Active => Shadow {
                    color: Color {
                        a: 0.75,
                        ..primary(app_theme)
                    },
                    offset: Vector::ZERO,
                    blur_radius: 12.0,
                },
                PgoTrackKind::Pending => Shadow::default(),
            },
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 4.0.into(),
            },
            ..container::Style::default()
        }
    }
}

pub fn modal_scrim(_app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.5))),
        ..container::Style::default()
    }
}

/// Dims page content under a blocking overlay (system-update fetch).
pub fn page_scrim(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgba(0.04, 0.06, 0.09, 0.72),
            AppTheme::Light => Color::from_rgba(0.96, 0.97, 0.98, 0.78),
        })),
        ..container::Style::default()
    }
}

pub fn code_well(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgb8(0x0b, 0x10, 0x18),
            AppTheme::Light => Color::from_rgb8(0xf1, 0xf5, 0xf9),
        })),
        text_color: Some(match app_theme {
            AppTheme::Dark => Color::from_rgb8(0xe2, 0xe8, 0xf0),
            AppTheme::Light => Color::from_rgb8(0x0f, 0x17, 0x2a),
        }),
        border: Border {
            color: match app_theme {
                AppTheme::Dark => Color::from_rgb8(0x1e, 0x29, 0x3b),
                AppTheme::Light => Color::from_rgb8(0xcb, 0xd5, 0xe1),
            },
            width: 1.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}

/// Foreground, row tint, and weight for one unified-diff line.
pub fn diff_line_style(theme: AppTheme, kind: DiffLineKind) -> (Color, Color, bool) {
    match (theme, kind) {
        (AppTheme::Dark, DiffLineKind::HeaderOld) => (
            Color::from_rgb8(0xfe, 0xca, 0xca),
            Color::from_rgba(0.98, 0.25, 0.25, 0.34),
            true,
        ),
        (AppTheme::Dark, DiffLineKind::HeaderNew) => (
            Color::from_rgb8(0xbb, 0xf7, 0xd0),
            Color::from_rgba(0.16, 0.83, 0.44, 0.34),
            true,
        ),
        (AppTheme::Dark, DiffLineKind::Hunk) => (
            Color::from_rgb8(0x7d, 0xd3, 0xfc),
            Color::from_rgba(0.22, 0.74, 0.97, 0.16),
            true,
        ),
        (AppTheme::Dark, DiffLineKind::Delete) => (
            Color::from_rgb8(0xfc, 0xa5, 0xa5),
            Color::from_rgba(0.97, 0.44, 0.44, 0.16),
            false,
        ),
        (AppTheme::Dark, DiffLineKind::Insert) => (
            Color::from_rgb8(0x6e, 0xe7, 0xb7),
            Color::from_rgba(0.20, 0.83, 0.60, 0.16),
            false,
        ),
        (AppTheme::Dark, DiffLineKind::Context) => (
            Color::from_rgb8(0xe2, 0xe8, 0xf0),
            Color::TRANSPARENT,
            false,
        ),
        (AppTheme::Light, DiffLineKind::HeaderOld) => (
            Color::from_rgb8(0x9f, 0x12, 0x39),
            Color::from_rgba(0.81, 0.13, 0.18, 0.20),
            true,
        ),
        (AppTheme::Light, DiffLineKind::HeaderNew) => (
            Color::from_rgb8(0x06, 0x5f, 0x46),
            Color::from_rgba(0.02, 0.48, 0.32, 0.20),
            true,
        ),
        (AppTheme::Light, DiffLineKind::Hunk) => (
            Color::from_rgb8(0x03, 0x69, 0xa1),
            Color::from_rgba(0.02, 0.52, 0.78, 0.12),
            true,
        ),
        (AppTheme::Light, DiffLineKind::Delete) => (
            Color::from_rgb8(0xb9, 0x1c, 0x1c),
            Color::from_rgba(0.81, 0.13, 0.18, 0.10),
            false,
        ),
        (AppTheme::Light, DiffLineKind::Insert) => (
            Color::from_rgb8(0x04, 0x78, 0x57),
            Color::from_rgba(0.02, 0.48, 0.32, 0.10),
            false,
        ),
        (AppTheme::Light, DiffLineKind::Context) => (
            Color::from_rgb8(0x0f, 0x17, 0x2a),
            Color::TRANSPARENT,
            false,
        ),
    }
}

pub fn diff_line_fill(bg: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: if bg.a > 0.0 {
            Some(Background::Color(bg))
        } else {
            None
        },
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn danger(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0xf8, 0x71, 0x71),
        AppTheme::Light => Color::from_rgb8(0xcf, 0x22, 0x2e),
    }
}

pub fn warning(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0xfb, 0xbf, 0x24),
        AppTheme::Light => Color::from_rgb8(0xd9, 0x77, 0x06),
    }
}

pub fn success(theme: AppTheme) -> Color {
    match theme {
        AppTheme::Dark => Color::from_rgb8(0x34, 0xd3, 0x99),
        AppTheme::Light => Color::from_rgb8(0x05, 0x96, 0x69),
    }
}

pub fn wizard_input(
    app_theme: AppTheme,
    invalid: bool,
) -> impl Fn(&Theme, text_input::Status) -> text_input::Style {
    move |theme, status| {
        let mut style = text_input::default(theme, status);
        if invalid {
            style.border.color = danger(app_theme);
            style.border.width = 2.0;
        }
        style
    }
}

pub fn wizard_error_banner(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgba(0.97, 0.44, 0.44, 0.16),
            AppTheme::Light => Color::from_rgba(0.81, 0.13, 0.18, 0.10),
        })),
        text_color: Some(danger(app_theme)),
        border: Border {
            color: match app_theme {
                AppTheme::Dark => Color::from_rgba(0.97, 0.44, 0.44, 0.45),
                AppTheme::Light => Color::from_rgba(0.81, 0.13, 0.18, 0.35),
            },
            width: 1.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn warning_banner(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgba(0.98, 0.75, 0.14, 0.14),
            AppTheme::Light => Color::from_rgba(0.76, 0.46, 0.11, 0.10),
        })),
        text_color: Some(warning(app_theme)),
        border: Border {
            color: match app_theme {
                AppTheme::Dark => Color::from_rgba(0.98, 0.75, 0.14, 0.4),
                AppTheme::Light => Color::from_rgba(0.76, 0.46, 0.11, 0.35),
            },
            width: 1.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn wizard_success_banner(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgba(0.2, 0.83, 0.6, 0.16),
            AppTheme::Light => Color::from_rgba(0.1, 0.6, 0.3, 0.12),
        })),
        text_color: Some(match app_theme {
            AppTheme::Dark => Color::from_rgb8(0x34, 0xd3, 0x99),
            AppTheme::Light => Color::from_rgb8(0x1a, 0x7f, 0x37),
        }),
        border: Border {
            color: match app_theme {
                AppTheme::Dark => Color::from_rgba(0.2, 0.83, 0.6, 0.4),
                AppTheme::Light => Color::from_rgba(0.1, 0.6, 0.3, 0.3),
            },
            width: 1.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn top_nav(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
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

pub fn hardware_pill(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgb8(0x0a, 0x0e, 0x15),
            AppTheme::Light => Color::from_rgb8(0xff, 0xff, 0xff),
        })),
        border: Border {
            color: surface_border(app_theme),
            width: 1.0,
            radius: 20.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn status_bar(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgb8(0x0c, 0x11, 0x1a),
            AppTheme::Light => Color::from_rgb8(0xff, 0xff, 0xff),
        })),
        border: Border {
            color: surface_border(app_theme),
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn status_dot(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgb8(0x34, 0xd3, 0x99),
            AppTheme::Light => Color::from_rgb8(0x05, 0x96, 0x69),
        })),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn nav_badge(app_theme: AppTheme, active: bool) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(if active {
            primary_tint(app_theme, 0.22)
        } else {
            match app_theme {
                AppTheme::Dark => Color::from_rgb8(0x24, 0x32, 0x45),
                AppTheme::Light => Color::from_rgb8(0xe2, 0xe8, 0xf0),
            }
        })),
        text_color: Some(if active {
            primary_soft(app_theme)
        } else {
            match app_theme {
                AppTheme::Dark => Color::from_rgb8(0xcb, 0xd5, 0xe1),
                AppTheme::Light => Color::from_rgb8(0x33, 0x41, 0x55),
            }
        }),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 10.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn kbd_hint(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgb8(0x16, 0x1e, 0x2a),
            AppTheme::Light => Color::from_rgb8(0xe2, 0xe8, 0xf0),
        })),
        text_color: Some(muted(app_theme)),
        border: Border {
            color: surface_border(app_theme),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn table_sort_header(
    app_theme: AppTheme,
    active: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let text_color = if active {
            primary_soft(app_theme)
        } else if hovered {
            button_idle(app_theme)
        } else {
            muted(app_theme)
        };
        button::Style {
            background: Some(Background::Color(Color::TRANSPARENT)),
            text_color,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 0.0.into(),
            },
            ..button::Style::default()
        }
    }
}

pub fn dense_table(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(surface(app_theme))),
        border: Border {
            color: surface_border(app_theme),
            width: 1.0,
            radius: 10.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn dense_table_head(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgb8(0x0c, 0x11, 0x18),
            AppTheme::Light => Color::from_rgb8(0xf8, 0xfa, 0xfc),
        })),
        border: Border {
            color: surface_border(app_theme),
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn dense_row(
    app_theme: AppTheme,
    active: bool,
    hovered: bool,
) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(if hovered {
            match app_theme {
                AppTheme::Dark => Color::from_rgb8(0x1a, 0x25, 0x36),
                AppTheme::Light => Color::from_rgb8(0xf1, 0xf5, 0xf9),
            }
        } else if active {
            match app_theme {
                AppTheme::Dark => Color::from_rgba(0.06, 0.73, 0.51, 0.06),
                AppTheme::Light => Color::from_rgba(0.02, 0.59, 0.41, 0.08),
            }
        } else {
            Color::TRANSPARENT
        })),
        ..container::Style::default()
    }
}

pub fn boot_pill(app_theme: AppTheme) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(match app_theme {
            AppTheme::Dark => Color::from_rgb8(0x10, 0x16, 0x22),
            AppTheme::Light => Color::from_rgb8(0xff, 0xff, 0xff),
        })),
        border: Border {
            color: surface_border(app_theme),
            width: 1.0,
            radius: 7.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn tag_spec(app_theme: AppTheme, kind: &str) -> impl Fn(&Theme) -> container::Style {
    let kind = kind.to_string();
    move |_theme| {
        let (fg, border) = match kind.as_str() {
            "lto" => match app_theme {
                AppTheme::Dark => (
                    Color::from_rgb8(0xc0, 0x84, 0xfc),
                    Color::from_rgb8(0x93, 0x33, 0xea),
                ),
                AppTheme::Light => (
                    Color::from_rgb8(0x6d, 0x28, 0xd9),
                    Color::from_rgb8(0x8b, 0x5c, 0xf6),
                ),
            },
            "security" => match app_theme {
                AppTheme::Dark => (
                    Color::from_rgb8(0xf4, 0x3f, 0x5e),
                    Color::from_rgb8(0xe1, 0x1d, 0x48),
                ),
                AppTheme::Light => (
                    Color::from_rgb8(0xb9, 0x1c, 0x1c),
                    Color::from_rgb8(0xef, 0x44, 0x44),
                ),
            },
            "gaming" => match app_theme {
                AppTheme::Dark => (
                    Color::from_rgb8(0x38, 0xbd, 0xf8),
                    Color::from_rgb8(0x02, 0x84, 0xc7),
                ),
                AppTheme::Light => (
                    Color::from_rgb8(0x03, 0x69, 0xa1),
                    Color::from_rgb8(0x38, 0xbd, 0xf8),
                ),
            },
            _ => (primary_soft(app_theme), primary_tint(app_theme, 0.35)),
        };
        container::Style {
            background: Some(Background::Color(Color { a: 0.12, ..fg })),
            text_color: Some(fg),
            border: Border {
                color: border,
                width: 1.0,
                radius: 4.0.into(),
            },
            ..container::Style::default()
        }
    }
}

#[derive(Clone, Copy)]
pub enum PkgSourceKind {
    Official,
    Aur,
    Abs,
}

pub fn source_tag(app_theme: AppTheme, kind: PkgSourceKind) -> impl Fn(&Theme) -> container::Style {
    move |theme| match kind {
        PkgSourceKind::Aur => tag_spec(app_theme, "gaming")(theme),
        PkgSourceKind::Abs => tag_success(app_theme)(theme),
        PkgSourceKind::Official => tag_info(app_theme)(theme),
    }
}

pub fn isolation_tag(app_theme: AppTheme, alone: bool) -> impl Fn(&Theme) -> container::Style {
    move |_theme| {
        if alone {
            tag_warning(app_theme)(_theme)
        } else {
            tag_muted(app_theme)(_theme)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_pad_is_ten_percent_clamped() {
        assert!((shell_pad_x(800.0) - 80.0).abs() < f32::EPSILON);
        assert!((shell_pad_x(960.0) - SHELL_PAD_X_MAX).abs() < f32::EPSILON);
        assert!((shell_pad_x(2560.0) - SHELL_PAD_X_MAX).abs() < f32::EPSILON);
        assert!((shell_pad_x(200.0) - SHELL_PAD_X).abs() < f32::EPSILON);
    }

    #[test]
    fn dark_idle_button_text_is_brighter_than_muted() {
        let idle = button_idle(AppTheme::Dark);
        let muted = muted(AppTheme::Dark);
        let idle_luma = idle.r + idle.g + idle.b;
        let muted_luma = muted.r + muted.g + muted.b;
        assert!(idle_luma > muted_luma);
    }

    #[test]
    fn breadcrumb_sep_contrasts_more_than_hairline() {
        fn luma(c: Color) -> f32 {
            0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b
        }
        let cases = [
            (AppTheme::Dark, Color::from_rgb8(0x09, 0x0d, 0x13)),
            (AppTheme::Light, Color::from_rgb8(0xf8, 0xfa, 0xfc)),
        ];
        for (theme, bg) in cases {
            let sep = (luma(muted(theme)) - luma(bg)).abs();
            let hair = (luma(surface_border(theme)) - luma(bg)).abs();
            assert!(
                sep > hair,
                "{theme:?} breadcrumb > contrast {sep} vs hairline {hair}"
            );
        }
    }

    #[test]
    fn swatch_ring_contrasts_light_and_dark_fills() {
        let dark = Color::from_rgb8(0x0e, 0x11, 0x16);
        let light = Color::from_rgb8(0xfb, 0xf1, 0xc7);
        assert!(swatch_contrast_ring(dark).r > 0.9);
        assert!(swatch_contrast_ring(light).r < 0.3);
    }

    #[test]
    fn pkgbuild_diff_add_and_delete_use_distinct_colors() {
        for theme in [AppTheme::Dark, AppTheme::Light] {
            let (add_fg, add_bg, _) = diff_line_style(theme, DiffLineKind::Insert);
            let (del_fg, del_bg, _) = diff_line_style(theme, DiffLineKind::Delete);
            let (old_fg, _, old_bold) = diff_line_style(theme, DiffLineKind::HeaderOld);
            let (new_fg, _, new_bold) = diff_line_style(theme, DiffLineKind::HeaderNew);
            assert_ne!(add_fg, del_fg, "{theme:?} +/- text");
            assert_ne!(add_bg, del_bg, "{theme:?} +/- row");
            assert_ne!(old_fg, new_fg, "{theme:?} ---/+++");
            assert!(old_bold && new_bold);
        }
    }
}
