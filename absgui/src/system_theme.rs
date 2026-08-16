//! Detect the desktop color scheme for Follow system in App Settings.
//!
//! Follow-system uses the XDG Settings portal when a desktop provides it (GNOME,
//! KDE, COSMIC, XFCE with a portal, darkman, and others on both X11 and Wayland).
//! File and toolkit fallbacks cover sessions where the portal is missing.
//!
//! The window and taskbar icon is the light logo (`icon_light.png` / packaged
//! hicolor `absgui.png`). Taskbars look up `Icon=absgui` from the packaged
//! hicolor theme, like other apps. Do not overlay Breeze or rewrite hicolor
//! caches at runtime — that hides themed icons for every other application.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const HICOLOR_SIZES: [u32; 6] = [32, 48, 64, 128, 256, 512];
const BREEZE_SIZES: [u32; 4] = [16, 22, 32, 48];
const STUB_HICOLOR_DIRECTORIES: &str =
    "32x32/apps,48x48/apps,64x64/apps,128x128/apps,256x256/apps,512x512/apps";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorScheme {
    Dark,
    Light,
}

pub fn detect() -> ColorScheme {
    if let Some(scheme) = portal_color_scheme() {
        return scheme;
    }
    if let Some(scheme) = desktop_specific_scheme() {
        return scheme;
    }
    if let Some(scheme) = generic_toolkit_scheme() {
        return scheme;
    }
    ColorScheme::Dark
}

/// Remove runtime Breeze/hicolor overlays and the stub icon cache from older AbsGui builds.
pub fn cleanup_legacy_icon_overlays() {
    if let Some(data) = dirs::data_dir() {
        cleanup_legacy_icon_overlays_in(&data.join("icons"));
    }
    if let Some(path) = stamp_path() {
        let _ = fs::remove_file(path);
    }
}

fn cleanup_legacy_icon_overlays_in(icons: &Path) {
    for size in HICOLOR_SIZES {
        remove_file_and_empty_parents(
            &icons
                .join("hicolor")
                .join(format!("{size}x{size}"))
                .join("apps")
                .join("absgui.png"),
            icons,
        );
    }
    let hicolor = icons.join("hicolor");
    let index = hicolor.join("index.theme");
    if fs::read_to_string(&index).is_ok_and(|t| is_stub_hicolor_index(&t)) {
        let _ = fs::remove_file(&index);
    }
    let _ = fs::remove_file(hicolor.join("icon-theme.cache"));
    for theme in ["breeze", "breeze-dark"] {
        for size in BREEZE_SIZES {
            remove_file_and_empty_parents(
                &icons
                    .join(theme)
                    .join("apps")
                    .join(size.to_string())
                    .join("absgui.png"),
                icons,
            );
        }
    }
}

fn is_stub_hicolor_index(text: &str) -> bool {
    text.lines()
        .find_map(|line| line.trim().strip_prefix("Directories="))
        .is_some_and(|dirs| dirs.trim() == STUB_HICOLOR_DIRECTORIES)
}

fn remove_file_and_empty_parents(path: &Path, stop_at: &Path) {
    let _ = fs::remove_file(path);
    let mut dir = path.parent();
    while let Some(current) = dir {
        if current == stop_at {
            break;
        }
        match fs::remove_dir(current) {
            Ok(()) => dir = current.parent(),
            Err(_) => break,
        }
    }
}

fn stamp_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("abs").join("taskbar-icon-scheme"))
}

fn desktop_specific_scheme() -> Option<ColorScheme> {
    let desktop = desktop_tokens();
    if desktop_has(&desktop, &["cosmic"]) {
        return cosmic_is_dark();
    }
    if desktop_has(&desktop, &["kde", "plasma"]) {
        return kde_window_scheme();
    }
    if desktop_has(&desktop, &["gnome", "unity", "pantheon", "budgie", "pop"]) {
        return gnome_interface_scheme();
    }
    if desktop_has(&desktop, &["cinnamon"]) {
        return cinnamon_scheme().or_else(gnome_interface_scheme);
    }
    if desktop_has(&desktop, &["mate"]) {
        return mate_scheme();
    }
    if desktop_has(&desktop, &["xfce"]) {
        return xfce_scheme();
    }
    if desktop_has(&desktop, &["lxqt"]) {
        return lxqt_scheme().or_else(qt_ct_scheme);
    }
    if desktop_has(&desktop, &["deepin", "dde"]) {
        return deepin_scheme();
    }
    if desktop_has(&desktop, &["ukui"]) {
        return ukui_scheme();
    }
    None
}

fn generic_toolkit_scheme() -> Option<ColorScheme> {
    gtk_settings_scheme()
        .or_else(cosmic_is_dark)
        .or_else(gnome_color_scheme_explicit)
        .or_else(kde_window_scheme)
        .or_else(qt_ct_scheme)
        .or_else(lxqt_scheme)
        .or_else(gtk_theme_env_scheme)
}

pub(crate) fn desktop_tokens() -> Vec<String> {
    let mut joined = String::new();
    for var in [
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_DESKTOP",
        "DESKTOP_SESSION",
    ] {
        if let Ok(value) = std::env::var(var) {
            if !joined.is_empty() {
                joined.push(':');
            }
            joined.push_str(&value);
        }
    }
    joined
        .split(|c: char| c == ':' || c == ',' || c == ';')
        .flat_map(|tok| tok.split('-'))
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| {
            !t.is_empty() && !matches!(t.as_str(), "wayland" | "x11" | "xorg" | "waylandsession")
        })
        .collect()
}

pub(crate) fn desktop_has(tokens: &[String], needles: &[&str]) -> bool {
    needles
        .iter()
        .any(|needle| tokens.iter().any(|token| token == needle))
}

enum Run {
    NotFound,
    Failed,
    Ok(String),
}

fn run_cmd(program: &str, args: &[&str]) -> Run {
    match Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    {
        Err(err) if err.kind() == ErrorKind::NotFound => Run::NotFound,
        Err(_) => Run::Failed,
        Ok(output) if output.status.success() => {
            Run::Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        Ok(_) => Run::Failed,
    }
}

fn run_stdout(program: &str, args: &[&str]) -> Option<String> {
    match run_cmd(program, args) {
        Run::Ok(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Run::NotFound | Run::Failed => None,
    }
}

fn gsettings_get(schema: &str, key: &str) -> Option<String> {
    run_stdout("gsettings", &["get", schema, key])
}

fn portal_color_scheme() -> Option<ColorScheme> {
    let attempts: [(&str, &[&str]); 3] = [
        (
            "gdbus",
            &[
                "call",
                "--session",
                "--timeout",
                "1",
                "--dest",
                "org.freedesktop.portal.Desktop",
                "--object-path",
                "/org/freedesktop/portal/desktop",
                "--method",
                "org.freedesktop.portal.Settings.Read",
                "org.freedesktop.appearance",
                "color-scheme",
            ],
        ),
        (
            "busctl",
            &[
                "--user",
                "--timeout=1",
                "call",
                "org.freedesktop.portal.Desktop",
                "/org/freedesktop/portal/desktop",
                "org.freedesktop.portal.Settings",
                "Read",
                "ss",
                "org.freedesktop.appearance",
                "color-scheme",
            ],
        ),
        (
            "dbus-send",
            &[
                "--session",
                "--print-reply",
                "--reply-timeout=1000",
                "--dest=org.freedesktop.portal.Desktop",
                "/org/freedesktop/portal/desktop",
                "org.freedesktop.portal.Settings.Read",
                "string:org.freedesktop.appearance",
                "string:color-scheme",
            ],
        ),
    ];
    for (program, args) in attempts {
        match run_cmd(program, args) {
            Run::NotFound => continue,
            Run::Failed => return None,
            Run::Ok(out) => return parse_portal_color_scheme(&out),
        }
    }
    None
}

pub(crate) fn parse_portal_color_scheme(out: &str) -> Option<ColorScheme> {
    match parse_portal_uint(out)? {
        1 => Some(ColorScheme::Dark),
        2 => Some(ColorScheme::Light),
        _ => None,
    }
}

fn parse_portal_uint(out: &str) -> Option<u32> {
    let bytes = out.as_bytes();
    let mut i = 0;
    let mut found = None;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"uint32") {
            i += 6;
            i = skip_ascii_ws(bytes, i);
            if let Some((value, n)) = parse_u32_at(&bytes[i..]) {
                found = Some(value);
                i += n;
                continue;
            }
        }
        let at_token = i == 0 || bytes[i - 1].is_ascii_whitespace();
        if at_token && bytes[i] == b'u' {
            let next = i + 1;
            if next < bytes.len() && bytes[next].is_ascii_whitespace() {
                let after = skip_ascii_ws(bytes, next);
                if let Some((value, n)) = parse_u32_at(&bytes[after..]) {
                    found = Some(value);
                    i = after + n;
                    continue;
                }
            }
        }
        i += 1;
    }
    found
}

fn skip_ascii_ws(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

fn parse_u32_at(bytes: &[u8]) -> Option<(u32, usize)> {
    if bytes.is_empty() || !bytes[0].is_ascii_digit() {
        return None;
    }
    let mut n = 0;
    let mut value: u32 = 0;
    while n < bytes.len() && bytes[n].is_ascii_digit() {
        value = value
            .saturating_mul(10)
            .saturating_add(u32::from(bytes[n] - b'0'));
        n += 1;
    }
    Some((value, n))
}

fn gnome_interface_scheme() -> Option<ColorScheme> {
    match gsettings_get("org.gnome.desktop.interface", "color-scheme") {
        Some(raw) => {
            if let Some(scheme) = parse_gsettings_color_scheme(&raw) {
                return Some(scheme);
            }
            if let Some(theme) = gsettings_get("org.gnome.desktop.interface", "gtk-theme") {
                if let Some(scheme) = scheme_from_theme_name(&theme) {
                    return Some(scheme);
                }
            }
            if is_gsettings_default(&raw) {
                return Some(ColorScheme::Light);
            }
            None
        }
        None => gsettings_get("org.gnome.desktop.interface", "gtk-theme")
            .and_then(|theme| scheme_from_theme_name(&theme)),
    }
}

fn gnome_color_scheme_explicit() -> Option<ColorScheme> {
    gsettings_get("org.gnome.desktop.interface", "color-scheme")
        .and_then(|raw| parse_gsettings_color_scheme(&raw))
}

pub(crate) fn parse_gsettings_color_scheme(out: &str) -> Option<ColorScheme> {
    let value = strip_quotes(out).to_ascii_lowercase();
    match value.as_str() {
        "prefer-dark" | "dark" => Some(ColorScheme::Dark),
        "prefer-light" | "light" => Some(ColorScheme::Light),
        _ => None,
    }
}

fn is_gsettings_default(out: &str) -> bool {
    matches!(strip_quotes(out).to_ascii_lowercase().as_str(), "default")
}

fn cinnamon_scheme() -> Option<ColorScheme> {
    if let Some(raw) = gsettings_get("org.cinnamon.desktop.interface", "color-scheme") {
        if let Some(scheme) = parse_gsettings_color_scheme(&raw) {
            return Some(scheme);
        }
    }
    if let Some(theme) = gsettings_get("org.cinnamon.desktop.interface", "gtk-theme") {
        if let Some(scheme) = scheme_from_theme_name(&theme) {
            return Some(scheme);
        }
    }
    gsettings_get("org.cinnamon.theme", "name").and_then(|theme| scheme_from_theme_name(&theme))
}

fn mate_scheme() -> Option<ColorScheme> {
    gsettings_get("org.mate.interface", "gtk-theme")
        .and_then(|theme| scheme_from_theme_name(&theme))
}

fn deepin_scheme() -> Option<ColorScheme> {
    gsettings_get("com.deepin.dde.appearance", "gtk-theme")
        .and_then(|theme| scheme_from_theme_name(&theme))
        .or_else(|| {
            gsettings_get("com.deepin.xsettings", "theme-name")
                .and_then(|theme| scheme_from_theme_name(&theme))
        })
}

fn ukui_scheme() -> Option<ColorScheme> {
    gsettings_get("org.ukui.style", "style-name").and_then(|name| scheme_from_theme_name(&name))
}

fn xfce_scheme() -> Option<ColorScheme> {
    if let Some(out) = run_stdout(
        "xfconf-query",
        &["-c", "xsettings", "-p", "/Gtk/ApplicationPreferDarkTheme"],
    ) {
        match out.trim() {
            "true" | "1" => return Some(ColorScheme::Dark),
            "false" | "0" => return Some(ColorScheme::Light),
            _ => {}
        }
    }
    match run_stdout("xfconf-query", &["-c", "xsettings", "-p", "/Net/ThemeName"]) {
        Some(name) => match scheme_from_theme_name(&name) {
            Some(scheme) => Some(scheme),
            None => Some(ColorScheme::Light),
        },
        None => None,
    }
}

fn cosmic_is_dark() -> Option<ColorScheme> {
    let path = config_home().join("cosmic/com.system76.CosmicTheme.Mode/v1/is_dark");
    let text = fs::read_to_string(path).ok()?;
    parse_cosmic_is_dark(&text)
}

pub(crate) fn parse_cosmic_is_dark(text: &str) -> Option<ColorScheme> {
    match text
        .trim()
        .trim_end_matches(',')
        .to_ascii_lowercase()
        .as_str()
    {
        "true" | "1" => Some(ColorScheme::Dark),
        "false" | "0" => Some(ColorScheme::Light),
        _ => None,
    }
}

fn kde_window_scheme() -> Option<ColorScheme> {
    let path = config_home().join("kdeglobals");
    let text = fs::read_to_string(path).ok()?;
    parse_kde_window_scheme(&text)
}

pub(crate) fn parse_kde_window_scheme(text: &str) -> Option<ColorScheme> {
    if let Some((r, g, b)) = parse_kde_window_rgb(text) {
        return Some(scheme_from_rgb(r, g, b));
    }
    parse_kde_color_scheme_name(text)
}

pub(crate) fn parse_kde_window_rgb(text: &str) -> Option<(u8, u8, u8)> {
    let mut in_window = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_window = line == "[Colors:Window]";
            continue;
        }
        if in_window {
            if let Some(rest) = line.strip_prefix("BackgroundNormal=") {
                return parse_rgb(rest);
            }
        }
    }
    None
}

fn parse_kde_color_scheme_name(text: &str) -> Option<ColorScheme> {
    let mut in_general = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_general = line.eq_ignore_ascii_case("[General]");
            continue;
        }
        if in_general {
            if let Some(rest) = line.strip_prefix("ColorScheme=") {
                return scheme_from_theme_name(rest);
            }
        }
    }
    None
}

fn parse_rgb(s: &str) -> Option<(u8, u8, u8)> {
    let mut parts = s.split(',');
    let r = parts.next()?.trim().parse().ok()?;
    let g = parts.next()?.trim().parse().ok()?;
    let b = parts.next()?.trim().parse().ok()?;
    Some((r, g, b))
}

pub(crate) fn scheme_from_rgb(r: u8, g: u8, b: u8) -> ColorScheme {
    let y = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
    if y >= 140.0 {
        ColorScheme::Light
    } else {
        ColorScheme::Dark
    }
}

fn gtk_settings_scheme() -> Option<ColorScheme> {
    for rel in ["gtk-4.0/settings.ini", "gtk-3.0/settings.ini"] {
        let path = config_home().join(rel);
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        if let Some(scheme) = parse_gtk_settings(&text) {
            return Some(scheme);
        }
    }
    None
}

pub(crate) fn parse_gtk_settings(text: &str) -> Option<ColorScheme> {
    if let Some(scheme) = parse_gtk_prefer_dark(text) {
        return Some(scheme);
    }
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("gtk-theme-name=") {
            return scheme_from_theme_name(rest);
        }
    }
    None
}

pub(crate) fn parse_gtk_prefer_dark(text: &str) -> Option<ColorScheme> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("gtk-application-prefer-dark-theme=") {
            let v = rest.trim();
            return Some(if v == "true" || v == "1" {
                ColorScheme::Dark
            } else if v == "false" || v == "0" {
                ColorScheme::Light
            } else {
                return None;
            });
        }
    }
    None
}

fn gtk_theme_env_scheme() -> Option<ColorScheme> {
    std::env::var("GTK_THEME")
        .ok()
        .and_then(|value| scheme_from_theme_name(&value))
}

fn qt_ct_scheme() -> Option<ColorScheme> {
    for rel in ["qt6ct/qt6ct.conf", "qt5ct/qt5ct.conf"] {
        let path = config_home().join(rel);
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        if let Some(scheme) = parse_qt_ct_scheme(&text) {
            return Some(scheme);
        }
    }
    None
}

pub(crate) fn parse_qt_ct_scheme(text: &str) -> Option<ColorScheme> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("color_scheme_path=") {
            return scheme_from_theme_name(rest);
        }
        if let Some(rest) = line.strip_prefix("color_scheme=") {
            return scheme_from_theme_name(rest);
        }
    }
    None
}

fn lxqt_scheme() -> Option<ColorScheme> {
    let path = config_home().join("lxqt/lxqt.conf");
    let text = fs::read_to_string(path).ok()?;
    parse_lxqt_scheme(&text)
}

pub(crate) fn parse_lxqt_scheme(text: &str) -> Option<ColorScheme> {
    let mut in_general = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_general = line.eq_ignore_ascii_case("[General]");
            continue;
        }
        if in_general {
            if let Some(rest) = line.strip_prefix("theme=") {
                return scheme_from_theme_name(rest);
            }
        }
    }
    None
}

pub(crate) fn scheme_from_theme_name(name: &str) -> Option<ColorScheme> {
    let n = strip_quotes(name).to_ascii_lowercase();
    if n.is_empty() {
        return None;
    }
    if let Some((_, variant)) = n.split_once(':') {
        if variant.contains("dark") {
            return Some(ColorScheme::Dark);
        }
        if variant.contains("light") {
            return Some(ColorScheme::Light);
        }
    }
    let dark =
        n.contains("dark") || n.contains("night") || n.contains("black") || n.contains("noir");
    let light = n.contains("light")
        || n.contains("-day")
        || n.contains("_day")
        || n.contains("/day")
        || n.ends_with("day");
    match (dark, light) {
        (true, false) => Some(ColorScheme::Dark),
        (false, true) => Some(ColorScheme::Light),
        _ => None,
    }
}

fn strip_quotes(value: &str) -> String {
    value
        .trim()
        .trim_matches('\'')
        .trim_matches('"')
        .trim()
        .to_string()
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_uint32_1_is_dark_2_is_light() {
        assert_eq!(
            parse_portal_color_scheme("(<<uint32 1>>,)"),
            Some(ColorScheme::Dark)
        );
        assert_eq!(
            parse_portal_color_scheme("(<<uint32 2>>,)"),
            Some(ColorScheme::Light)
        );
        assert_eq!(parse_portal_color_scheme("(<<uint32 0>>,)"), None);
    }

    #[test]
    fn portal_busctl_and_dbus_send_formats() {
        assert_eq!(
            parse_portal_color_scheme("v v u 1\n"),
            Some(ColorScheme::Dark)
        );
        assert_eq!(
            parse_portal_color_scheme("v v u 2\n"),
            Some(ColorScheme::Light)
        );
        assert_eq!(
            parse_portal_color_scheme(
                "method return sender=:1.42\n   variant       variant          uint32 2\n"
            ),
            Some(ColorScheme::Light)
        );
        assert_eq!(parse_portal_color_scheme("v v u 0\n"), None);
    }

    #[test]
    fn kde_window_luminance_classifies_breeze_light_and_dark() {
        let light = "[Colors:Window]\nBackgroundNormal=239,240,241\n";
        assert_eq!(parse_kde_window_scheme(light), Some(ColorScheme::Light));
        let dark = "[Colors:Window]\nBackgroundNormal=35,38,41\n";
        assert_eq!(parse_kde_window_scheme(dark), Some(ColorScheme::Dark));
        let nested = "\
[Colors:Button]
BackgroundNormal=252,252,252
[Colors:Window]
BackgroundNormal=42,46,50
[Colors:View]
BackgroundNormal=255,255,255
";
        assert_eq!(parse_kde_window_scheme(nested), Some(ColorScheme::Dark));
    }

    #[test]
    fn kde_color_scheme_name_fallback() {
        let text = "[General]\nColorScheme=BreezeLight\n";
        assert_eq!(parse_kde_window_scheme(text), Some(ColorScheme::Light));
        let dark = "[General]\nColorScheme=BreezeDark\n";
        assert_eq!(parse_kde_window_scheme(dark), Some(ColorScheme::Dark));
    }

    #[test]
    fn gtk_prefer_dark_ini() {
        assert_eq!(
            parse_gtk_prefer_dark("gtk-application-prefer-dark-theme=true\n"),
            Some(ColorScheme::Dark)
        );
        assert_eq!(
            parse_gtk_prefer_dark("gtk-application-prefer-dark-theme=false\n"),
            Some(ColorScheme::Light)
        );
    }

    #[test]
    fn gtk_theme_name_from_settings_ini() {
        let text = "[Settings]\ngtk-theme-name=Adwaita-dark\n";
        assert_eq!(parse_gtk_settings(text), Some(ColorScheme::Dark));
        let prefer_wins = "\
gtk-application-prefer-dark-theme=false
gtk-theme-name=Adwaita-dark
";
        assert_eq!(parse_gtk_settings(prefer_wins), Some(ColorScheme::Light));
    }

    #[test]
    fn gsettings_color_scheme_values() {
        assert_eq!(
            parse_gsettings_color_scheme("'prefer-dark'\n"),
            Some(ColorScheme::Dark)
        );
        assert_eq!(
            parse_gsettings_color_scheme("prefer-light"),
            Some(ColorScheme::Light)
        );
        assert_eq!(parse_gsettings_color_scheme("'default'"), None);
    }

    #[test]
    fn cosmic_is_dark_file() {
        assert_eq!(parse_cosmic_is_dark("true\n"), Some(ColorScheme::Dark));
        assert_eq!(parse_cosmic_is_dark("false"), Some(ColorScheme::Light));
    }

    #[test]
    fn theme_name_dark_and_light_tokens() {
        assert_eq!(
            scheme_from_theme_name("Adwaita:dark"),
            Some(ColorScheme::Dark)
        );
        assert_eq!(
            scheme_from_theme_name("'Arc-Dark'"),
            Some(ColorScheme::Dark)
        );
        assert_eq!(
            scheme_from_theme_name("/usr/share/qt6ct/colors/darker.conf"),
            Some(ColorScheme::Dark)
        );
        assert_eq!(
            scheme_from_theme_name("BreezeLight"),
            Some(ColorScheme::Light)
        );
        assert_eq!(scheme_from_theme_name("Adwaita"), None);
    }

    #[test]
    fn qt_ct_and_lxqt_conf() {
        let qt = "[Appearance]\ncolor_scheme_path=/usr/share/qt6ct/colors/simple.conf\n";
        assert_eq!(parse_qt_ct_scheme(qt), None);
        let qt_dark = "color_scheme_path=/usr/share/qt6ct/colors/darker.conf\n";
        assert_eq!(parse_qt_ct_scheme(qt_dark), Some(ColorScheme::Dark));
        let lxqt = "[General]\ntheme=frost-dark\n";
        assert_eq!(parse_lxqt_scheme(lxqt), Some(ColorScheme::Dark));
    }

    #[test]
    fn stub_hicolor_index_is_the_short_apps_only_list() {
        let stub = "\
[Icon Theme]
Name=Hicolor
Hidden=true
Directories=32x32/apps,48x48/apps,64x64/apps,128x128/apps,256x256/apps,512x512/apps
";
        assert!(super::is_stub_hicolor_index(stub));
        let real = "\
[Icon Theme]
Name=Hicolor
Hidden=true
Directories=16x16/actions,32x32/apps,48x48/apps,64x64/apps,128x128/apps,256x256/apps,512x512/apps,scalable/apps
";
        assert!(!super::is_stub_hicolor_index(real));
    }

    #[test]
    fn cleanup_removes_absgui_overlays_and_keeps_other_icons() {
        let dir = std::env::temp_dir().join(format!(
            "absgui_icons_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let hicolor_app = dir.join("hicolor/48x48/apps");
        let breeze_app = dir.join("breeze/apps/32");
        std::fs::create_dir_all(&hicolor_app).unwrap();
        std::fs::create_dir_all(&breeze_app).unwrap();
        std::fs::write(hicolor_app.join("absgui.png"), b"abs").unwrap();
        std::fs::write(hicolor_app.join("my-manual-app.png"), b"keep").unwrap();
        std::fs::write(breeze_app.join("absgui.png"), b"abs").unwrap();
        std::fs::write(
            dir.join("hicolor/index.theme"),
            "[Icon Theme]\nName=Hicolor\nHidden=true\nDirectories=32x32/apps,48x48/apps,64x64/apps,128x128/apps,256x256/apps,512x512/apps\n",
        )
        .unwrap();
        std::fs::write(dir.join("hicolor/icon-theme.cache"), b"cache").unwrap();

        super::cleanup_legacy_icon_overlays_in(&dir);

        assert!(!hicolor_app.join("absgui.png").exists());
        assert!(hicolor_app.join("my-manual-app.png").exists());
        assert!(!breeze_app.join("absgui.png").exists());
        assert!(!dir.join("hicolor/index.theme").exists());
        assert!(!dir.join("hicolor/icon-theme.cache").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
