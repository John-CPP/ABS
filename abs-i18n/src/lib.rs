//! Shared translations for `abs` and `absgui`.
//!
//! Locale files in `locales/*.toml` are compiled into perfect-hash maps.
//! Lookups are O(1). Missing keys fall back to English, then to the key.

use std::sync::atomic::{AtomicU8, Ordering};

include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));

static LANG: AtomicU8 = AtomicU8::new(Lang::En as u8);

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    En = 0,
    De = 1,
    Es = 2,
    Ar = 3,
    Ru = 4,
    Zh = 5,
    Ja = 6,
}

impl Lang {
    pub const ALL: [Lang; 7] = [
        Lang::En,
        Lang::De,
        Lang::Es,
        Lang::Ar,
        Lang::Ru,
        Lang::Zh,
        Lang::Ja,
    ];

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::De => "de",
            Lang::Es => "es",
            Lang::Ar => "ar",
            Lang::Ru => "ru",
            Lang::Zh => "zh",
            Lang::Ja => "ja",
        }
    }

    /// Language name written in that language.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::De => "Deutsch",
            Lang::Es => "Español",
            Lang::Ar => "العربية",
            Lang::Ru => "Русский",
            Lang::Zh => "中文",
            Lang::Ja => "日本語",
        }
    }

    pub fn flag(self) -> &'static str {
        match self {
            Lang::En => "🇬🇧",
            Lang::De => "🇩🇪",
            Lang::Es => "🇪🇸",
            Lang::Ar => "🇸🇦",
            Lang::Ru => "🇷🇺",
            Lang::Zh => "🇨🇳",
            Lang::Ja => "🇯🇵",
        }
    }

    /// `🇩🇪 Deutsch` — for pickers.
    pub fn picker_label(self) -> String {
        format!("{} {}", self.flag(), self.native_name())
    }

    pub fn parse(code: &str) -> Option<Self> {
        let s = code.trim();
        if s.is_empty() {
            return None;
        }
        let primary = s
            .split(['.', '@'])
            .next()
            .unwrap_or(s)
            .split(['_', '-'])
            .next()
            .unwrap_or(s)
            .to_ascii_lowercase();
        match primary.as_str() {
            "en" => Some(Lang::En),
            "de" => Some(Lang::De),
            "es" => Some(Lang::Es),
            "ar" => Some(Lang::Ar),
            "ru" => Some(Lang::Ru),
            "zh" => Some(Lang::Zh),
            "ja" => Some(Lang::Ja),
            _ => None,
        }
    }

    pub fn from_system() -> Option<Self> {
        for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(val) = std::env::var(var) {
                if val == "C" || val == "POSIX" {
                    continue;
                }
                if let Some(lang) = Self::parse(&val) {
                    return Some(lang);
                }
            }
        }
        None
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => Lang::De,
            2 => Lang::Es,
            3 => Lang::Ar,
            4 => Lang::Ru,
            5 => Lang::Zh,
            6 => Lang::Ja,
            _ => Lang::En,
        }
    }

    fn catalog(self) -> &'static phf::Map<&'static str, &'static str> {
        match self {
            Lang::En => &EN,
            Lang::De => &DE,
            Lang::Es => &ES,
            Lang::Ar => &AR,
            Lang::Ru => &RU,
            Lang::Zh => &ZH,
            Lang::Ja => &JA,
        }
    }
}

pub fn set_lang(lang: Lang) {
    LANG.store(lang as u8, Ordering::Relaxed);
}

pub fn current_lang() -> Lang {
    Lang::from_u8(LANG.load(Ordering::Relaxed))
}

/// Current language, then English, then the key itself.
pub fn t(key: &str) -> &'static str {
    lookup(current_lang(), key)
        .or_else(|| lookup(Lang::En, key))
        .unwrap_or_else(|| intern_miss(key))
}

/// Same as [`t`], but if both catalogs miss the key, return `fallback`.
pub fn t_or(key: &str, fallback: &'static str) -> &'static str {
    lookup(current_lang(), key)
        .or_else(|| lookup(Lang::En, key))
        .unwrap_or(fallback)
}

/// Replace `{name}` placeholders in the translated string.
///
/// Values are copied as-is and are not scanned for further placeholders, so a
/// path containing `{lang}` cannot rewrite another argument.
pub fn tf(key: &str, args: &[(&str, &str)]) -> String {
    let template = t(key);
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        rest = &rest[start + 1..];
        if let Some(end) = rest.find('}') {
            let name = &rest[..end];
            if let Some((_, value)) = args.iter().find(|(n, _)| *n == name) {
                out.push_str(value);
            } else {
                out.push('{');
                out.push_str(name);
                out.push('}');
            }
            rest = &rest[end + 1..];
        } else {
            out.push('{');
            out.push_str(rest);
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

fn lookup(lang: Lang, key: &str) -> Option<&'static str> {
    lang.catalog().get(key).copied()
}

fn intern_miss(key: &str) -> &'static str {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static MAP: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let mut map = MAP
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("i18n intern");
    if let Some(s) = map.get(key) {
        return s;
    }
    let leaked: &'static str = Box::leak(key.to_string().into_boxed_str());
    map.insert(key.to_string(), leaked);
    leaked
}

/// Read the root `lang = "..."` key from abs.toml / absgui-settings.toml text.
///
/// Stops at the first `[table]` header so a nested `lang` is not treated as the
/// document language.
pub fn peek_lang_toml(text: &str) -> Option<Lang> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            break;
        }
        let Some(rest) = line.strip_prefix("lang") else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim().trim_matches('"').trim_matches('\'');
        if let Some(lang) = Lang::parse(rest) {
            return Some(lang);
        }
    }
    None
}

/// Apply system language when it is one of the shipped translations.
pub fn init_from_system() {
    if let Some(lang) = Lang::from_system() {
        set_lang(lang);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::sync::Mutex;

    fn with_lang<T>(lang: Lang, f: impl FnOnce() -> T) -> T {
        static LOCK: Mutex<()> = Mutex::new(());
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = current_lang();
        set_lang(lang);
        let out = f();
        set_lang(prev);
        out
    }

    #[test]
    fn parse_locale_tags() {
        assert_eq!(Lang::parse("de_DE.UTF-8"), Some(Lang::De));
        assert_eq!(Lang::parse("zh-CN"), Some(Lang::Zh));
        assert_eq!(Lang::parse("ja_JP"), Some(Lang::Ja));
        assert_eq!(Lang::parse("pt_BR"), None);
        assert_eq!(Lang::parse("en"), Some(Lang::En));
    }

    #[test]
    fn english_fallback_when_key_missing() {
        with_lang(Lang::De, || {
            assert_eq!(
                t("wizard.bool.yes"),
                DE.get("wizard.bool.yes").copied().unwrap_or("Yes")
            );
            let missing = "definitely.not.a.real.key";
            assert_eq!(t(missing), missing);
            assert_eq!(t_or(missing, "fallback"), "fallback");
        });
    }

    #[test]
    fn interpolation() {
        with_lang(Lang::En, || {
            let s = tf(
                "cli.set_lang.saved",
                &[("lang", "Deutsch"), ("path", "/tmp/abs.toml")],
            );
            assert!(s.contains("Deutsch"), "{s}");
            assert!(s.contains("/tmp/abs.toml"), "{s}");
            let nested = tf(
                "cli.set_lang.saved",
                &[("lang", "{path}"), ("path", "/tmp/abs.toml")],
            );
            assert!(
                nested.contains("{path}"),
                "value must not be re-parsed as a placeholder: {nested}"
            );
            assert!(nested.contains("/tmp/abs.toml"), "{nested}");
        });
    }

    #[test]
    fn peek_lang_toml_reads_root_key() {
        assert_eq!(peek_lang_toml("lang = \"ja\"\n"), Some(Lang::Ja));
        assert_eq!(
            peek_lang_toml("# lang = \"de\"\nlang = \"es\"\n"),
            Some(Lang::Es)
        );
        assert!(peek_lang_toml("install_absgui = true\n").is_none());
        assert!(
            peek_lang_toml("[packages.foo]\nlang = \"ja\"\n").is_none(),
            "nested lang must be ignored"
        );
        assert_eq!(
            peek_lang_toml("lang = \"de\"\n[packages.foo]\nlang = \"ja\"\n"),
            Some(Lang::De)
        );
    }

    #[test]
    fn picker_labels_use_native_names() {
        assert!(Lang::De.picker_label().contains("Deutsch"));
        assert!(Lang::Ja.picker_label().contains("日本語"));
        assert!(Lang::Ar.picker_label().contains("العربية"));
    }

    #[test]
    fn every_english_key_exists_in_other_catalogs() {
        for lang in Lang::ALL {
            if lang == Lang::En {
                continue;
            }
            for key in EN.keys() {
                assert!(
                    lang.catalog().contains_key(key),
                    "missing key {key:?} in {}",
                    lang.code()
                );
            }
            for key in lang.catalog().keys() {
                assert!(EN.contains_key(key), "extra key {key:?} in {}", lang.code());
            }
        }
    }

    #[test]
    fn placeholders_match_english_in_every_catalog() {
        for lang in Lang::ALL {
            if lang == Lang::En {
                continue;
            }
            for key in EN.keys() {
                let en = EN.get(key).copied().unwrap();
                let Some(translated) = lang.catalog().get(key).copied() else {
                    continue;
                };
                assert_eq!(
                    placeholder_names(en),
                    placeholder_names(translated),
                    "placeholders for {key:?} in {} differ from English",
                    lang.code()
                );
            }
        }
    }

    #[test]
    fn translations_are_not_untranslated_english() {
        for lang in Lang::ALL {
            if lang == Lang::En {
                continue;
            }
            for key in EN.keys() {
                if key.starts_with("meta.") {
                    continue;
                }
                let en = EN.get(key).copied().unwrap();
                let Some(translated) = lang.catalog().get(key).copied() else {
                    continue;
                };
                if translated != en {
                    continue;
                }
                assert!(
                    identical_to_english_ok(lang, key),
                    "{key:?} in {} is still English: {en:?}",
                    lang.code()
                );
            }
        }
    }

    fn placeholder_names(s: &str) -> BTreeSet<&str> {
        let mut names = BTreeSet::new();
        let mut rest = s;
        while let Some(start) = rest.find('{') {
            rest = &rest[start + 1..];
            let Some(end) = rest.find('}') else {
                break;
            };
            let name = &rest[..end];
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                names.insert(name);
            }
            rest = &rest[end + 1..];
        }
        names
    }

    fn identical_to_english_ok(lang: Lang, key: &str) -> bool {
        match key {
            "gui.system_update.aur"
            | "gui.system_update.source_aur"
            | "gui.system_update.source_abs"
            | "gui.system_update.old_to_new"
            | "gui.abs.ramdisk_c"
            | "gui.kernels.spec_lto"
            | "gui.kernels.filter_lto"
            | "gui.kernels.lto_tag"
            | "gui.kernels.lto_pill"
            | "gui.packages.filter_pgo"
            | "gui.packages.filter_aur"
            | "gui.packages.unset"
            | "gui.pkgbuild.title"
            | "gui.pkgbuild.title_version"
            | "gui.chrome.search_shortcut"
            | "gui.wizard.repo_url"
            | "gui.field.ansi_magenta" => true,
            "gui.kernels.status" | "gui.pgo.phase" | "gui.packages.upstream"
                if lang == Lang::De =>
            {
                true
            }
            "gui.wizard.repo_name"
            | "gui.system_update.col_repo"
            | "gui.system_update.col_version"
            | "gui.kernels.col_scheduler"
            | "gui.packages.col_threads"
            | "gui.packages.col_isolation"
            | "gui.packages.isolation_parallel"
            | "gui.field.preview_normal"
                if lang == Lang::De =>
            {
                true
            }
            "gui.common.no"
            | "wizard.bool.no"
            | "wizard.ui.no_word"
            | "gui.field.preview_normal"
                if lang == Lang::Es =>
            {
                true
            }
            "gui.packages.upstream" if lang == Lang::Ru => true,
            _ => false,
        }
    }
}
