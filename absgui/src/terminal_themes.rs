//! Curated terminal palettes for in-app logs and the App Settings preview.

use iced::Color;
use serde::{Deserialize, Serialize};

/// User-facing terminal color scheme. `match-app` follows the window Dark/Light theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TerminalTheme {
    #[default]
    MatchApp,
    CatppuccinMocha,
    CatppuccinMacchiato,
    CatppuccinFrappe,
    TokyoNight,
    TokyoNightStorm,
    Dracula,
    Nord,
    GruvboxDark,
    GruvboxMaterialDark,
    RosePine,
    RosePineMoon,
    EverforestDark,
    Kanagawa,
    KanagawaDragon,
    OneDark,
    Nightfox,
    AyuMirage,
    Oxocarbon,
    FlexokiDark,
    Poimandres,
    GithubDark,
    SolarizedDark,
    CatppuccinLatte,
    TokyoNightDay,
    RosePineDawn,
    EverforestLight,
    GruvboxLight,
    FlexokiLight,
    GithubLight,
    SolarizedLight,
}

/// Background, default text, and 16 ANSI colors used by logs and the fake terminal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogPalette {
    pub bg: Color,
    pub fg: Color,
    pub cursor: Color,
    pub hint: Color,
    pub selection: Color,
    pub border: Color,
    pub ansi: [Color; 8],
    pub bright: [Color; 8],
}

impl LogPalette {
    pub fn red(self) -> Color {
        self.ansi[1]
    }
    pub fn green(self) -> Color {
        self.ansi[2]
    }
    pub fn yellow(self) -> Color {
        self.ansi[3]
    }
    pub fn blue(self) -> Color {
        self.ansi[4]
    }
    pub fn magenta(self) -> Color {
        self.ansi[5]
    }
    pub fn cyan(self) -> Color {
        self.ansi[6]
    }
}

fn rgb(hex: u32) -> Color {
    Color::from_rgb8(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}

#[allow(clippy::too_many_arguments)]
fn pal(
    bg: u32,
    fg: u32,
    cursor: u32,
    hint: u32,
    border: u32,
    sel: u32,
    ansi: [u32; 8],
    bright: [u32; 8],
) -> LogPalette {
    LogPalette {
        bg: rgb(bg),
        fg: rgb(fg),
        cursor: rgb(cursor),
        hint: rgb(hint),
        border: rgb(border),
        selection: Color {
            a: 0.38,
            ..rgb(sel)
        },
        ansi: ansi.map(rgb),
        bright: bright.map(rgb),
    }
}

impl TerminalTheme {
    /// Display order: match-app, dark palettes, then light.
    pub const ALL: &'static [Self] = &[
        Self::MatchApp,
        Self::CatppuccinMocha,
        Self::CatppuccinMacchiato,
        Self::CatppuccinFrappe,
        Self::TokyoNight,
        Self::TokyoNightStorm,
        Self::Dracula,
        Self::Nord,
        Self::GruvboxDark,
        Self::GruvboxMaterialDark,
        Self::RosePine,
        Self::RosePineMoon,
        Self::EverforestDark,
        Self::Kanagawa,
        Self::KanagawaDragon,
        Self::OneDark,
        Self::Nightfox,
        Self::AyuMirage,
        Self::Oxocarbon,
        Self::FlexokiDark,
        Self::Poimandres,
        Self::GithubDark,
        Self::SolarizedDark,
        Self::CatppuccinLatte,
        Self::TokyoNightDay,
        Self::RosePineDawn,
        Self::EverforestLight,
        Self::GruvboxLight,
        Self::FlexokiLight,
        Self::GithubLight,
        Self::SolarizedLight,
    ];

    pub const DARK: &'static [Self] = &[
        Self::CatppuccinMocha,
        Self::CatppuccinMacchiato,
        Self::CatppuccinFrappe,
        Self::TokyoNight,
        Self::TokyoNightStorm,
        Self::Dracula,
        Self::Nord,
        Self::GruvboxDark,
        Self::GruvboxMaterialDark,
        Self::RosePine,
        Self::RosePineMoon,
        Self::EverforestDark,
        Self::Kanagawa,
        Self::KanagawaDragon,
        Self::OneDark,
        Self::Nightfox,
        Self::AyuMirage,
        Self::Oxocarbon,
        Self::FlexokiDark,
        Self::Poimandres,
        Self::GithubDark,
        Self::SolarizedDark,
    ];

    pub const LIGHT: &'static [Self] = &[
        Self::CatppuccinLatte,
        Self::TokyoNightDay,
        Self::RosePineDawn,
        Self::EverforestLight,
        Self::GruvboxLight,
        Self::FlexokiLight,
        Self::GithubLight,
        Self::SolarizedLight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::MatchApp => "Match app theme",
            Self::CatppuccinMocha => "Catppuccin Mocha",
            Self::CatppuccinMacchiato => "Catppuccin Macchiato",
            Self::CatppuccinFrappe => "Catppuccin Frappé",
            Self::TokyoNight => "Tokyo Night",
            Self::TokyoNightStorm => "Tokyo Night Storm",
            Self::Dracula => "Dracula",
            Self::Nord => "Nord",
            Self::GruvboxDark => "Gruvbox Dark",
            Self::GruvboxMaterialDark => "Gruvbox Material Dark",
            Self::RosePine => "Rosé Pine",
            Self::RosePineMoon => "Rosé Pine Moon",
            Self::EverforestDark => "Everforest Dark",
            Self::Kanagawa => "Kanagawa",
            Self::KanagawaDragon => "Kanagawa Dragon",
            Self::OneDark => "One Dark",
            Self::Nightfox => "Nightfox",
            Self::AyuMirage => "Ayu Mirage",
            Self::Oxocarbon => "Oxocarbon",
            Self::FlexokiDark => "Flexoki Dark",
            Self::Poimandres => "Poimandres",
            Self::GithubDark => "GitHub Dark",
            Self::SolarizedDark => "Solarized Dark",
            Self::CatppuccinLatte => "Catppuccin Latte",
            Self::TokyoNightDay => "Tokyo Night Day",
            Self::RosePineDawn => "Rosé Pine Dawn",
            Self::EverforestLight => "Everforest Light",
            Self::GruvboxLight => "Gruvbox Light",
            Self::FlexokiLight => "Flexoki Light",
            Self::GithubLight => "GitHub Light",
            Self::SolarizedLight => "Solarized Light",
        }
    }

    pub fn display_label(self) -> &'static str {
        match self {
            Self::MatchApp => abs_i18n::t("gui.settings.theme_match_app"),
            other => other.label(),
        }
    }

    /// Display order: match-app, then palettes that fit `app` (Dark or Light AbsGUI).
    /// `selected` is inserted if it is not already in that list (legacy mismatch).
    pub fn choices_for(app: crate::app_settings::AppTheme, selected: Self) -> Vec<Self> {
        use crate::app_settings::AppTheme;
        let mut out = vec![Self::MatchApp];
        let family = match app {
            AppTheme::Dark => Self::DARK,
            AppTheme::Light => Self::LIGHT,
        };
        out.extend(family.iter().copied());
        if !out.contains(&selected) {
            out.insert(1, selected);
        }
        out
    }

    #[allow(dead_code)]
    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|t| t.label() == label)
    }

    /// `dark` is the current app Dark/Light chrome; used only by [`Self::MatchApp`].
    pub fn palette(self, dark: bool) -> LogPalette {
        match self {
            Self::MatchApp => {
                if dark {
                    pal(
                        0x0e1116,
                        0xdde4ec,
                        0xdde4ec,
                        0x939eac,
                        0x2b333f,
                        0x60a5fa,
                        [
                            0x1a2028, 0xf87171, 0x34d399, 0xfbbf24, 0x60a5fa, 0xc084fc, 0x22d3ee,
                            0xe6eaf0,
                        ],
                        [
                            0x475569, 0xfca5a5, 0x6ee7b7, 0xfde68a, 0x93c5fd, 0xd8b4fe, 0x67e8f9,
                            0xf8fafc,
                        ],
                    )
                } else {
                    pal(
                        0xeef1f6,
                        0x1c2430,
                        0x1c2430,
                        0x4c596c,
                        0xe1e4ea,
                        0x4a72b4,
                        [
                            0xd5dae3, 0xc44a4a, 0x2e8b6b, 0xc2761c, 0x4a72b4, 0x7c3aed, 0x0e7490,
                            0x2a3340,
                        ],
                        [
                            0x94a3b8, 0xef4444, 0x059669, 0xd97706, 0x3b82f6, 0x8b5cf6, 0x06b6d4,
                            0x0f172a,
                        ],
                    )
                }
            }
            Self::CatppuccinMocha => pal(
                0x1e1e2e,
                0xcdd6f4,
                0xf5e0dc,
                0x6c7086,
                0x313244,
                0x89b4fa,
                [
                    0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xcba6f7, 0x94e2d5, 0xbac2de,
                ],
                [
                    0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
                ],
            ),
            Self::CatppuccinMacchiato => pal(
                0x24273a,
                0xcad3f5,
                0xf4dbd6,
                0x6e738d,
                0x363a4f,
                0x8aadf4,
                [
                    0x494d64, 0xed8796, 0xa6da95, 0xeed49f, 0x8aadf4, 0xc6a0f6, 0x8bd5ca, 0xb8c0e0,
                ],
                [
                    0x5b6078, 0xed8796, 0xa6da95, 0xeed49f, 0x8aadf4, 0xf5bde6, 0x8bd5ca, 0xa5adcb,
                ],
            ),
            Self::CatppuccinFrappe => pal(
                0x303446,
                0xc6d0f5,
                0xf2d5cf,
                0x737994,
                0x414559,
                0x8caaee,
                [
                    0x51576d, 0xe78284, 0xa6d189, 0xe5c890, 0x8caaee, 0xca9ee6, 0x81c8be, 0xb5bfe2,
                ],
                [
                    0x626880, 0xe78284, 0xa6d189, 0xe5c890, 0x8caaee, 0xf4b8e4, 0x81c8be, 0xa5adce,
                ],
            ),
            Self::TokyoNight => pal(
                0x1a1b26,
                0xc0caf5,
                0xc0caf5,
                0x565f89,
                0x292e42,
                0x7aa2f7,
                [
                    0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6,
                ],
                [
                    0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5,
                ],
            ),
            Self::TokyoNightStorm => pal(
                0x24283b,
                0xc0caf5,
                0xc0caf5,
                0x565f89,
                0x292e42,
                0x7aa2f7,
                [
                    0x1d202f, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6,
                ],
                [
                    0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5,
                ],
            ),
            Self::Dracula => pal(
                0x282a36,
                0xf8f8f2,
                0xf8f8f2,
                0x6272a4,
                0x44475a,
                0xbd93f9,
                [
                    0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2,
                ],
                [
                    0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff,
                ],
            ),
            Self::Nord => pal(
                0x2e3440,
                0xd8dee9,
                0xd8dee9,
                0x7b88a1,
                0x3b4252,
                0x88c0d0,
                [
                    0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0,
                ],
                [
                    0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4,
                ],
            ),
            Self::GruvboxDark => pal(
                0x282828,
                0xebdbb2,
                0xebdbb2,
                0x928374,
                0x3c3836,
                0x83a598,
                [
                    0x1d2021, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984,
                ],
                [
                    0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
                ],
            ),
            Self::GruvboxMaterialDark => pal(
                0x282828,
                0xd4be98,
                0xd4be98,
                0x7c6f64,
                0x32302f,
                0x7daea3,
                [
                    0x32302f, 0xea6962, 0xa9b665, 0xd8a657, 0x7daea3, 0xd3869b, 0x89b482, 0xd4be98,
                ],
                [
                    0x7c6f64, 0xea6962, 0xa9b665, 0xd8a657, 0x7daea3, 0xd3869b, 0x89b482, 0xddc7a1,
                ],
            ),
            Self::RosePine => pal(
                0x191724,
                0xe0def4,
                0xe0def4,
                0x6e6a86,
                0x26233a,
                0xc4a7e7,
                [
                    0x26233a, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4,
                ],
                [
                    0x6e6a86, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4,
                ],
            ),
            Self::RosePineMoon => pal(
                0x232136,
                0xe0def4,
                0xe0def4,
                0x6e6a86,
                0x393552,
                0xc4a7e7,
                [
                    0x393552, 0xeb6f92, 0x3e8fb0, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xea9a97, 0xe0def4,
                ],
                [
                    0x6e6a86, 0xeb6f92, 0x3e8fb0, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xea9a97, 0xe0def4,
                ],
            ),
            Self::EverforestDark => pal(
                0x2d353b,
                0xd3c6aa,
                0xd3c6aa,
                0x7a8478,
                0x343f44,
                0x7fbbb3,
                [
                    0x343f44, 0xe67e80, 0xa7c080, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x83c092, 0xd3c6aa,
                ],
                [
                    0x7a8478, 0xe67e80, 0xa7c080, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x83c092, 0xd3c6aa,
                ],
            ),
            Self::Kanagawa => pal(
                0x1f1f28,
                0xdcd7ba,
                0xdcd7ba,
                0x727169,
                0x2a2a37,
                0x7e9cd8,
                [
                    0x090618, 0xc34043, 0x76946a, 0xc0a36e, 0x7e9cd8, 0x957fb8, 0x6a9589, 0xc8c093,
                ],
                [
                    0x727169, 0xe82424, 0x98bb6c, 0xe6c384, 0x7fb4ca, 0x938aa9, 0x7aa89f, 0xdcd7ba,
                ],
            ),
            Self::KanagawaDragon => pal(
                0x181616,
                0xc5c9c5,
                0xc5c9c5,
                0x7a8382,
                0x282727,
                0x8ba4b0,
                [
                    0x0d0c0c, 0xc4746e, 0x87a987, 0xc4b28a, 0x8ba4b0, 0x8992a7, 0x8ea4a2, 0xa6a69c,
                ],
                [
                    0x7a8382, 0xe46876, 0x87a987, 0xe6c384, 0x7fb4ca, 0x938aa9, 0x7aa89f, 0xc5c9c5,
                ],
            ),
            Self::OneDark => pal(
                0x282c34,
                0xabb2bf,
                0xabb2bf,
                0x5c6370,
                0x3e4452,
                0x61afef,
                [
                    0x1e2127, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
                ],
                [
                    0x5c6370, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xffffff,
                ],
            ),
            Self::Nightfox => pal(
                0x192330,
                0xcdcecf,
                0xcdcecf,
                0x738091,
                0x212e3f,
                0x719cd6,
                [
                    0x393b44, 0xc94f6d, 0x81b29a, 0xdbc074, 0x719cd6, 0x9d79d6, 0x63cdcf, 0xdfdfe0,
                ],
                [
                    0x575860, 0xd16983, 0x8ebaa4, 0xe0c989, 0x86abdc, 0xbaa1e2, 0x7ad5d6, 0xe4e4e5,
                ],
            ),
            Self::AyuMirage => pal(
                0x1f2430,
                0xcccac2,
                0xffcc66,
                0x5c6773,
                0x242936,
                0x73d0ff,
                [
                    0x191e2a, 0xf28779, 0xbae67e, 0xffd580, 0x73d0ff, 0xd4bfff, 0x95e6cb, 0xcccac2,
                ],
                [
                    0x5c6773, 0xf28779, 0xbae67e, 0xffcc66, 0x73d0ff, 0xd4bfff, 0x95e6cb, 0xffffff,
                ],
            ),
            Self::Oxocarbon => pal(
                0x161616,
                0xf2f4f8,
                0xffffff,
                0x525252,
                0x262626,
                0x78a9ff,
                [
                    0x262626, 0xee5396, 0x42be65, 0xffe97b, 0x78a9ff, 0xbe95ff, 0x33b1ff, 0xdde1e6,
                ],
                [
                    0x525252, 0xff7eb6, 0x42be65, 0xffe97b, 0x82cfff, 0xbe95ff, 0x3ddbd9, 0xffffff,
                ],
            ),
            Self::FlexokiDark => pal(
                0x100f0f,
                0xcecdc3,
                0xcecdc3,
                0x878580,
                0x1c1b1a,
                0x4385be,
                [
                    0x1c1b1a, 0xd14d41, 0x879a39, 0xd0a215, 0x4385be, 0xce5d97, 0x3aa99f, 0xcecdc3,
                ],
                [
                    0x575653, 0xaf3029, 0x66800b, 0xad8301, 0x205ea6, 0xa02f6f, 0x24837b, 0xfffcf0,
                ],
            ),
            Self::Poimandres => pal(
                0x1b1e28,
                0xa6accd,
                0xa6accd,
                0x767c9d,
                0x303340,
                0x89ddff,
                [
                    0x1b1e28, 0xd0679d, 0x5de4c7, 0xfffac2, 0x89ddff, 0xfcc5e9, 0xadd7ff, 0xffffff,
                ],
                [
                    0xa6accd, 0xd0679d, 0x5de4c7, 0xfffac2, 0xadd7ff, 0xfcc5e9, 0x89ddff, 0xffffff,
                ],
            ),
            Self::GithubDark => pal(
                0x0d1117,
                0xc9d1d9,
                0xc9d1d9,
                0x8b949e,
                0x30363d,
                0x58a6ff,
                [
                    0x484f58, 0xff7b72, 0x3fb950, 0xd29922, 0x58a6ff, 0xbc8cff, 0x39c5cf, 0xb1bac4,
                ],
                [
                    0x6e7681, 0xffa198, 0x56d364, 0xe3b341, 0x79c0ff, 0xd2a8ff, 0x56d4dd, 0xffffff,
                ],
            ),
            Self::SolarizedDark => pal(
                0x002b36,
                0x839496,
                0x93a1a1,
                0x586e75,
                0x073642,
                0x268bd2,
                [
                    0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5,
                ],
                [
                    0x002b36, 0xcb4b16, 0x859900, 0xb58900, 0x268bd2, 0x6c71c4, 0x2aa198, 0xfdf6e3,
                ],
            ),
            Self::CatppuccinLatte => pal(
                0xeff1f5,
                0x4c4f69,
                0xdc8a78,
                0x9ca0b0,
                0xccd0da,
                0x1e66f5,
                [
                    0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0x8839ef, 0x179299, 0xacb0be,
                ],
                [
                    0x6c6f85, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0x4c4f69,
                ],
            ),
            Self::TokyoNightDay => pal(
                0xe1e2e7,
                0x3760bf,
                0x3760bf,
                0x848cb5,
                0xc4c8da,
                0x2e7de9,
                [
                    0xe9e9ed, 0xf52a65, 0x587539, 0x8c6c3e, 0x2e7de9, 0x9854f1, 0x007197, 0x6172b0,
                ],
                [
                    0xa1a6c5, 0xf52a65, 0x587539, 0x8c6c3e, 0x2e7de9, 0x9854f1, 0x007197, 0x3760bf,
                ],
            ),
            Self::RosePineDawn => pal(
                0xfaf4ed,
                0x575279,
                0x575279,
                0x9893a5,
                0xf2e9e1,
                0x907aa9,
                [
                    0xf2e9e1, 0xb4637a, 0x286983, 0xea9d34, 0x56949f, 0x907aa9, 0xd7827e, 0x575279,
                ],
                [
                    0x9893a5, 0xb4637a, 0x286983, 0xea9d34, 0x56949f, 0x907aa9, 0xd7827e, 0x575279,
                ],
            ),
            Self::EverforestLight => pal(
                0xf3ead3,
                0x5c6a72,
                0x5c6a72,
                0xa6b0a0,
                0xe5ddc8,
                0x3a94c5,
                [
                    0xe5ddc8, 0xf85552, 0x8da101, 0xdfa000, 0x3a94c5, 0xdf69ba, 0x35a77c, 0x5c6a72,
                ],
                [
                    0xa6b0a0, 0xf85552, 0x8da101, 0xdfa000, 0x3a94c5, 0xdf69ba, 0x35a77c, 0x5c6a72,
                ],
            ),
            Self::GruvboxLight => pal(
                0xfbf1c7,
                0x3c3836,
                0x3c3836,
                0x928374,
                0xebdbb2,
                0x076678,
                [
                    0xfbf1c7, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0x7c6f64,
                ],
                [
                    0x928374, 0x9d0006, 0x79740e, 0xb57614, 0x076678, 0x8f3f71, 0x427b58, 0x3c3836,
                ],
            ),
            Self::FlexokiLight => pal(
                0xfffcf0,
                0x100f0f,
                0x100f0f,
                0x6f6e69,
                0xe6e4d9,
                0x205ea6,
                [
                    0xe6e4d9, 0xaf3029, 0x66800b, 0xad8301, 0x205ea6, 0xa02f6f, 0x24837b, 0x100f0f,
                ],
                [
                    0x6f6e69, 0xd14d41, 0x879a39, 0xd0a215, 0x4385be, 0xce5d97, 0x3aa99f, 0x100f0f,
                ],
            ),
            Self::GithubLight => pal(
                0xffffff,
                0x1f2328,
                0x1f2328,
                0x636c76,
                0xd0d7de,
                0x0969da,
                [
                    0x24292f, 0xcf222e, 0x116329, 0x4d2d00, 0x0969da, 0x8250df, 0x1b7c83, 0x6e7781,
                ],
                [
                    0x57606a, 0xa40e26, 0x1a7f37, 0x9a6700, 0x218bff, 0xa475f9, 0x3192aa, 0x1f2328,
                ],
            ),
            Self::SolarizedLight => pal(
                0xfdf6e3,
                0x657b83,
                0x586e75,
                0x93a1a1,
                0xeee8d5,
                0x268bd2,
                [
                    0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5,
                ],
                [
                    0x002b36, 0xcb4b16, 0x859900, 0xb58900, 0x268bd2, 0x6c71c4, 0x2aa198, 0xfdf6e3,
                ],
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalTheme;

    #[test]
    fn choices_for_keeps_family_and_inserts_mismatch() {
        use crate::app_settings::AppTheme;
        let dark = TerminalTheme::choices_for(AppTheme::Dark, TerminalTheme::MatchApp);
        assert!(dark.contains(&TerminalTheme::MatchApp));
        assert!(dark.contains(&TerminalTheme::CatppuccinMocha));
        assert!(!dark.contains(&TerminalTheme::CatppuccinLatte));
        let light = TerminalTheme::choices_for(AppTheme::Light, TerminalTheme::Dracula);
        assert_eq!(light[1], TerminalTheme::Dracula);
        assert!(light.contains(&TerminalTheme::CatppuccinLatte));
        assert!(!light.contains(&TerminalTheme::Nord));
    }

    #[test]
    fn labels_round_trip() {
        for theme in TerminalTheme::ALL {
            assert_eq!(TerminalTheme::from_label(theme.label()), Some(*theme));
        }
        assert_eq!(TerminalTheme::from_label("not a theme"), None);
    }

    #[test]
    fn every_palette_has_opaque_slots() {
        for theme in TerminalTheme::ALL {
            for dark in [true, false] {
                let p = theme.palette(dark);
                assert!(p.bg.a > 0.9, "{:?} bg", theme);
                assert!(p.fg.a > 0.9, "{:?} fg", theme);
                assert_eq!(p.ansi.len(), 8);
                assert_eq!(p.bright.len(), 8);
                for (i, c) in p.ansi.iter().chain(p.bright.iter()).enumerate() {
                    assert!(c.a > 0.9, "{:?} ansi/bright {i}", theme);
                }
            }
        }
    }

    #[test]
    fn serde_kebab_names() {
        #[derive(serde::Deserialize)]
        struct Wrap {
            terminal_theme: TerminalTheme,
        }
        let parsed: Wrap = toml::from_str("terminal_theme = \"match-app\"").unwrap();
        assert_eq!(parsed.terminal_theme, TerminalTheme::MatchApp);
        let named: Wrap = toml::from_str("terminal_theme = \"catppuccin-mocha\"").unwrap();
        assert_eq!(named.terminal_theme, TerminalTheme::CatppuccinMocha);
        assert_eq!(TerminalTheme::ALL.len(), 31);
        assert_eq!(
            1 + TerminalTheme::DARK.len() + TerminalTheme::LIGHT.len(),
            TerminalTheme::ALL.len()
        );
    }

    #[test]
    fn missing_key_uses_default() {
        #[derive(serde::Deserialize, Default)]
        struct Wrap {
            #[serde(default)]
            terminal_theme: TerminalTheme,
        }
        let wrap: Wrap = toml::from_str("theme = \"dark\"").unwrap();
        assert_eq!(wrap.terminal_theme, TerminalTheme::MatchApp);
    }
}
