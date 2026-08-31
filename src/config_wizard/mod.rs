//! Guided creation and reconfiguration of `abs.toml`.

mod catalog;
mod json_cli;

pub use json_cli::{run_apply, run_check, run_form};

use crate::config::{
    self, Config, etc_config_path, example_config_text, user_config_path, write_example_user_config,
};
use crate::die;
use crate::{blog, ewarn};
use abs_i18n::{self, Lang, t, t_or, tf};
use colored::Colorize;
use std::collections::HashSet;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use toml_edit::{Array, DocumentMut, Item, Table, TableLike, Value};

const EDIT_LATER: &str = "\
          abs --config-wizard
          abs --configure
          abs --configure=nano
        (or another editor you use; tested with vim, nano, and kate)";

pub(super) const KNOWN_REPOS: &[(&str, &str)] = &[
    (
        "arch",
        "https://gitlab.archlinux.org/archlinux/packaging/packages",
    ),
    ("aur", "https://aur.archlinux.org"),
    (
        "cachyos",
        "https://github.com/CachyOS/CachyOS-PKGBUILDS.git",
    ),
];

pub(super) const SUGGEST_PACKAGES_PATH: &str = "$XDG_CACHE_HOME/abs/packages";
pub(super) const SUGGEST_CHROOT_PATH: &str = "$XDG_CACHE_HOME/abs/chroot";
pub(super) const SUGGEST_READY_PATH: &str = "$XDG_CACHE_HOME/abs/ready";
pub(super) const SUGGEST_SYNC_CMD: &str = "sudo pacman -Sy";
pub(super) const SUGGEST_UPDATE_CMD: &str = "sudo pacman -Syu";
pub(super) const SUGGEST_NO_REFRESH_CMD: &str = "sudo pacman -Su";
pub(super) const SUGGEST_IGNORE_FLAG: &str = "--ignore";
pub(super) const SUGGEST_MOUNT: &str = "/run/abs-ram";
pub(super) const SUGGEST_SIZE: &str = "16G";
pub(super) const SUGGEST_MODE: &str = "0755";
pub(super) const SUGGEST_INSTALL_PATH: &str = "/usr/bin/abs";

static LANG_PROMPTED: AtomicBool = AtomicBool::new(false);

fn tr_step_title(step: &catalog::StepDef) -> &'static str {
    t_or(&format!("wizard.step.{}.title", step.id), step.title)
}

fn tr_step_blurb(step: &catalog::StepDef) -> &'static str {
    t_or(&format!("wizard.step.{}.blurb", step.id), step.blurb)
}

fn tr_field_title(field: &catalog::FieldDef) -> &'static str {
    t_or(&format!("wizard.field.{}.title", field.id), field.title)
}

fn tr_field_expl(field: &catalog::FieldDef) -> &'static str {
    t_or(
        &format!("wizard.field.{}.explanation", field.id),
        field.explanation,
    )
}

fn prompt_language_if_new_local_config(local_exists: bool) -> Lang {
    if local_exists {
        return abs_i18n::current_lang();
    }
    if LANG_PROMPTED.swap(true, Ordering::SeqCst) {
        return abs_i18n::current_lang();
    }
    let suggested = Lang::from_system().unwrap_or(Lang::En);
    abs_i18n::set_lang(suggested);
    let choices: Vec<Choice> = Lang::ALL
        .iter()
        .map(|l| {
            Choice::new(
                l.code(),
                l.picker_label(),
                t("wizard.language.help"),
                *l == suggested,
            )
        })
        .collect();
    let picked = prompt_choice(
        t("wizard.language.title"),
        t("wizard.language.explanation"),
        &choices,
        Some(suggested.code()),
        false,
    );
    let lang = Lang::parse(&picked).unwrap_or(suggested);
    abs_i18n::set_lang(lang);
    lang
}

fn apply_lang_from_doc(doc: &DocumentMut) {
    let code = get_root_str(doc, "lang", "");
    if let Some(lang) = Lang::parse(&code) {
        abs_i18n::set_lang(lang);
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct WizardPromptPrefs {
    never: bool,
    never_for_abs_version: Option<String>,
}

fn wizard_prompt_prefs_path() -> PathBuf {
    user_config_path()
        .parent()
        .map(|p| p.join("config-wizard-prompt.toml"))
        .unwrap_or_else(|| PathBuf::from("config-wizard-prompt.toml"))
}

fn prefs_suppress_prompt(prefs: &WizardPromptPrefs, abs_version: &str) -> bool {
    if prefs.never {
        return true;
    }
    prefs.never_for_abs_version.as_deref() == Some(abs_version)
}

fn parse_wizard_prompt_prefs(text: &str) -> WizardPromptPrefs {
    let Ok(doc) = text.parse::<DocumentMut>() else {
        return WizardPromptPrefs::default();
    };
    let table = doc.as_table();
    WizardPromptPrefs {
        never: table
            .get("never")
            .and_then(|i| i.as_bool())
            .unwrap_or(false),
        never_for_abs_version: table
            .get("never_for_abs_version")
            .and_then(|i| i.as_str())
            .map(str::to_string),
    }
}

fn load_wizard_prompt_prefs_from(path: &Path) -> WizardPromptPrefs {
    let Ok(text) = fs::read_to_string(path) else {
        return WizardPromptPrefs::default();
    };
    parse_wizard_prompt_prefs(&text)
}

fn render_wizard_prompt_prefs(prefs: &WizardPromptPrefs) -> String {
    let mut doc = DocumentMut::new();
    if prefs.never {
        doc["never"] = Item::Value(Value::from(true));
    }
    if let Some(v) = &prefs.never_for_abs_version {
        doc["never_for_abs_version"] = Item::Value(Value::from(v.as_str()));
    }
    doc.to_string()
}

fn save_wizard_prompt_prefs_to(path: &Path, prefs: &WizardPromptPrefs) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = crate::utils::write_file_mode(path, &render_wizard_prompt_prefs(prefs), 0o600);
}

pub(super) fn doc_has_path(doc: &DocumentMut, path: &str) -> bool {
    let mut parts = path.split('.');
    let Some(first) = parts.next() else {
        return false;
    };
    let mut item = match doc.as_table().get(first) {
        Some(i) => i,
        None => return false,
    };
    for part in parts {
        let Some(next_table) = item.as_table_like() else {
            return false;
        };
        item = match next_table.get(part) {
            Some(i) => i,
            None => return false,
        };
    }
    true
}

pub(super) fn ramdisk_enabled_in_doc(doc: &DocumentMut) -> bool {
    table_ref(doc, "ramdisk")
        .and_then(|t| t.get("enabled"))
        .and_then(|i| i.as_bool())
        .unwrap_or(false)
}

fn missing_required_keys(doc: &DocumentMut) -> Vec<String> {
    let mut out = Vec::new();
    for field in catalog::all_fields() {
        if !field.in_gap_fill {
            continue;
        }
        let key = catalog::gap_key(field);
        match field.visible_if {
            catalog::VisibleIf::Always => {
                if !doc_has_path(doc, key) {
                    out.push(key.to_string());
                }
            }
            catalog::VisibleIf::RamdiskEnabled => {
                if ramdisk_enabled_in_doc(doc) && !doc_has_path(doc, field.id) {
                    out.push(field.id.to_string());
                }
            }
            catalog::VisibleIf::PacmanFalse => {
                if doc_has_path(doc, "self_update_use_pacman")
                    && !get_root_bool(doc, "self_update_use_pacman", true)
                    && !doc_has_path(doc, field.id)
                {
                    out.push(field.id.to_string());
                }
            }
            catalog::VisibleIf::CpuFlexible => {}
        }
    }
    out
}

fn ask_key(gap: Option<&HashSet<String>>, key: &str) -> bool {
    match gap {
        None => true,
        Some(g) => g.contains(key),
    }
}

/// `abs --config-wizard`: create or reconfigure the user config, then exit.
pub fn run() {
    require_tty();
    let path = user_config_path();
    let lang = prompt_language_if_new_local_config(path.exists());
    abs_i18n::set_lang(lang);
    let (text, reconfigure) = load_wizard_source(&path);
    run_on_document(&path, &text, reconfigure, None);
    blog!("{}", t("wizard.intro.saved"));
}

/// First launch with no config: offer the wizard, example defaults, or quit.
pub fn offer_first_run(assume_yes: bool) {
    if config::config_exists() {
        return;
    }

    if assume_yes || crate::is_silent_mode() || !io::stdin().is_terminal() {
        let path = write_example_user_config();
        blog!(
            "{}",
            tf(
                "wizard.first_run.created_example",
                &[("path", &path.display().to_string())]
            )
        );
        println!("{EDIT_LATER}");
        return;
    }

    let lang = prompt_language_if_new_local_config(false);
    abs_i18n::set_lang(lang);

    println!();
    println!(
        "{}",
        format!("==> {}", t("wizard.first_run.title"))
            .yellow()
            .bold()
    );
    println!(
        "    {}",
        tf(
            "wizard.first_run.need_file",
            &[("path", &user_config_path().display().to_string())]
        )
    );
    println!("    {}", t("wizard.first_run.later_pkg"));
    println!();

    let choice = prompt_choice(
        t("wizard.first_run.how"),
        "",
        &[
            Choice::new(
                "wizard",
                t("wizard.first_run.wizard"),
                t("wizard.first_run.wizard_help"),
                true,
            ),
            Choice::new(
                "example",
                t("wizard.first_run.example"),
                tf(
                    "wizard.first_run.example_help",
                    &[("edit_later", EDIT_LATER)],
                ),
                false,
            ),
            Choice::new(
                "quit",
                t("wizard.first_run.quit"),
                t("wizard.first_run.quit_help"),
                false,
            ),
        ],
        Some("wizard"),
        false,
    );

    match choice.as_str() {
        "wizard" => {
            require_tty();
            let path = user_config_path();
            run_on_document(&path, example_config_text(), false, None);
            blog!(
                "{}",
                tf(
                    "wizard.first_run.saved_to",
                    &[("path", &path.display().to_string())]
                )
            );
        }
        "example" => {
            let path = write_example_user_config();
            blog!(
                "{}",
                tf(
                    "wizard.first_run.created_edit",
                    &[("path", &path.display().to_string())]
                )
            );
            println!("{EDIT_LATER}");
        }
        _ => {
            println!("{}", t("wizard.first_run.no_file"));
            crate::utils::wait_before_exit_if_needed();
            std::process::exit(0);
        }
    }
}

fn require_tty() {
    if !io::stdin().is_terminal() {
        die!("{}", t("wizard.intro.needs_tty"));
    }
}

/// If the loaded abs.toml is missing required wizard keys, offer a partial wizard.
pub fn offer_config_gap_fill(assume_yes: bool) {
    if assume_yes || crate::is_silent_mode() || !io::stdin().is_terminal() {
        return;
    }
    if std::env::var_os("ABS_GUI").is_some() {
        return;
    }
    if !config::config_exists() {
        return;
    }

    let prefs_path = wizard_prompt_prefs_path();
    let prefs = load_wizard_prompt_prefs_from(&prefs_path);
    if prefs_suppress_prompt(&prefs, env!("CARGO_PKG_VERSION")) {
        return;
    }

    let user_path = user_config_path();
    let inspect_path = if user_path.exists() {
        user_path.clone()
    } else {
        etc_config_path()
    };
    let text = match fs::read_to_string(&inspect_path) {
        Ok(t) => t,
        Err(_) => return,
    };
    let doc: DocumentMut = match text.parse() {
        Ok(d) => d,
        Err(_) => return,
    };
    let missing = missing_required_keys(&doc);
    if missing.is_empty() {
        return;
    }

    println!();
    println!(
        "{}",
        format!("==> {}", t("wizard.gap.title")).yellow().bold()
    );
    println!(
        "    {}",
        tf(
            "wizard.intro.file",
            &[("path", &inspect_path.display().to_string())]
        )
    );
    println!("    {}", t("wizard.gap.need_answers"));
    for key in &missing {
        println!("      {}", catalog::display_title_for_gap_key(key));
    }
    println!();

    let choice = prompt_choice(
        t("wizard.gap.ask"),
        "",
        &[
            Choice::new("yes", t("wizard.gap.yes"), t("wizard.gap.yes_help"), false),
            Choice::new(
                "not_now",
                t("wizard.gap.not_now"),
                t("wizard.gap.not_now_help"),
                true,
            ),
            Choice::new(
                "never_version",
                tf(
                    "wizard.gap.never_version",
                    &[("version", env!("CARGO_PKG_VERSION"))],
                ),
                t("wizard.gap.never_version_help"),
                false,
            ),
            Choice::new(
                "never",
                t("wizard.gap.never"),
                t("wizard.gap.never_help"),
                false,
            ),
        ],
        Some("not_now"),
        false,
    );

    match choice.as_str() {
        "yes" => {
            let missing_set: HashSet<String> = missing.into_iter().collect();
            run_on_document(&user_path, &text, true, Some(&missing_set));
            blog!(
                "{}",
                tf(
                    "wizard.intro.saved_missing",
                    &[("path", &user_path.display().to_string())]
                )
            );
        }
        "never_version" => {
            save_wizard_prompt_prefs_to(
                &prefs_path,
                &WizardPromptPrefs {
                    never: false,
                    never_for_abs_version: Some(env!("CARGO_PKG_VERSION").to_string()),
                },
            );
        }
        "never" => {
            save_wizard_prompt_prefs_to(
                &prefs_path,
                &WizardPromptPrefs {
                    never: true,
                    never_for_abs_version: None,
                },
            );
        }
        _ => {}
    }
}

fn load_wizard_source(user_path: &Path) -> (String, bool) {
    if user_path.exists() {
        return load_wizard_source_quiet(user_path).unwrap_or_else(|e| die!("{e}"));
    }

    let etc = etc_config_path();
    if etc.exists() {
        println!();
        println!(
            "{}",
            format!(
                "==> {}",
                tf(
                    "wizard.system_template.found",
                    &[("path", &etc.display().to_string())]
                )
            )
            .yellow()
        );
        println!(
            "    {}",
            tf(
                "wizard.system_template.writes",
                &[("path", &user_path.display().to_string())]
            )
        );
        let choice = prompt_choice(
            t("wizard.system_template.which"),
            t("wizard.system_template.which_help"),
            &[
                Choice::new(
                    "system",
                    t("wizard.system_template.system"),
                    t("wizard.system_template.system_help"),
                    true,
                ),
                Choice::new(
                    "example",
                    t("wizard.system_template.example"),
                    t("wizard.system_template.example_help"),
                    false,
                ),
            ],
            Some("system"),
            false,
        );
        if choice == "system" {
            let text = fs::read_to_string(&etc).unwrap_or_else(|e| {
                die!("Failed to read {}: {e}", etc.display());
            });
            return (text, true);
        }
    }

    (example_config_text().to_string(), false)
}

/// Non-interactive source: user file, else /etc, else the example. Used by JSON form/apply.
pub(super) fn load_wizard_source_quiet(user_path: &Path) -> Result<(String, bool), String> {
    if user_path.exists() {
        let text = fs::read_to_string(user_path)
            .map_err(|e| format!("Failed to read {}: {e}", user_path.display()))?;
        return Ok((text, true));
    }
    let etc = etc_config_path();
    if etc.exists() {
        let text = fs::read_to_string(&etc)
            .map_err(|e| format!("Failed to read {}: {e}", etc.display()))?;
        return Ok((text, true));
    }
    Ok((example_config_text().to_string(), false))
}

fn run_on_document(path: &Path, text: &str, reconfigure: bool, gap: Option<&HashSet<String>>) {
    let mut doc: DocumentMut = text.parse().unwrap_or_else(|e| {
        die!("The starting settings file could not be read. It may be damaged: {e}");
    });

    let local_exists = path.exists();
    if local_exists {
        apply_lang_from_doc(&doc);
    } else {
        let lang = prompt_language_if_new_local_config(false);
        set_root_str(&mut doc, "lang", lang.code());
    }

    println!();
    if gap.is_some() {
        println!(
            "{}",
            format!("==> {}", t("wizard.intro.setup_gap"))
                .green()
                .bold()
        );
        println!(
            "    {}",
            tf(
                "wizard.intro.file",
                &[("path", &path.display().to_string())]
            )
        );
        println!("    {}", t("wizard.intro.gap_only"));
        println!("    {}", t("wizard.intro.gap_keep"));
        println!(
            "    {}",
            tf(
                "wizard.intro.green_suggested",
                &[("tag", &format!("{}", t("wizard.ui.suggested_tag").green()))]
            )
        );
    } else {
        println!(
            "{}",
            format!("==> {}", t("wizard.intro.setup")).green().bold()
        );
        println!(
            "    {}",
            tf(
                "wizard.intro.file",
                &[("path", &path.display().to_string())]
            )
        );
        if reconfigure {
            println!("    {}", t("wizard.intro.reconfigure"));
            println!(
                "    {}",
                tf(
                    "wizard.intro.reconfigure_suggested",
                    &[("tag", &format!("{}", t("wizard.ui.suggested_tag").green()))]
                )
            );
            println!("    {}", t("wizard.intro.reconfigure_pkg"));
            println!("    {}", t("wizard.intro.reconfigure_pkg2"));
            if path.exists() {
                let bak = format!(
                    "{}.bak",
                    path.file_name()
                        .map(|n| n.to_string_lossy())
                        .unwrap_or_else(|| "abs.toml".into())
                );
                println!("    {}", tf("wizard.intro.backup", &[("name", &bak)]));
            }
        } else {
            println!("    {}", t("wizard.intro.new_file"));
            println!(
                "    {}",
                tf(
                    "wizard.intro.new_suggested",
                    &[("tag", &format!("{}", t("wizard.ui.suggested_tag").green()))]
                )
            );
        }
    }
    println!("    {}", t("wizard.ui.ctrl_c"));
    println!();

    let mut n = 0usize;
    for step in catalog::STEPS {
        if !step
            .fields
            .iter()
            .any(|f| should_prompt_field(f, gap, &doc))
        {
            continue;
        }
        n += 1;
        step_header(n, tr_step_title(step), tr_step_blurb(step));
        for field in step.fields {
            if should_prompt_field(field, gap, &doc) {
                prompt_catalog_field(&mut doc, field, gap);
            }
        }
    }

    let rendered = crate::toml_pretty::render_human_toml(&mut doc);
    if let Err(e) = Config::from_toml_text(&rendered) {
        ewarn!("{}", tf("wizard.intro.invalid_answers", &[("err", &e)]));
        die!("{}", t("wizard.intro.not_written"));
    }

    print_summary(&doc, path);
    let path_s = path.display().to_string();
    let write_help = if path.exists() {
        tf("wizard.ui.write_config_help_existing", &[("path", &path_s)])
    } else {
        tf("wizard.ui.write_config_help_new", &[("path", &path_s)])
    };
    if !prompt_bool(t("wizard.ui.write_config"), &write_help, true, true, false) {
        println!("{}", t("wizard.ui.no_changes"));
        crate::utils::wait_before_exit_if_needed();
        std::process::exit(0);
    }

    match write_wizard_file(path, &rendered) {
        Ok(Some(bak)) => blog!(
            "{}",
            tf(
                "wizard.intro.previous_bak",
                &[("path", &bak.display().to_string())]
            )
        ),
        Ok(None) => {}
        Err(e) => die!("{e}"),
    }
}

/// Next unused backup path beside `config_path` (`abs.toml.bak`, then `.bak.1`, …).
fn unique_backup_path(config_path: &Path) -> PathBuf {
    let file_name = config_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "abs.toml".into());
    let parent = config_path.parent().unwrap_or_else(|| Path::new("."));
    let first = parent.join(format!("{file_name}.bak"));
    if !first.exists() {
        return first;
    }
    for n in 1..10_000 {
        let candidate = parent.join(format!("{file_name}.bak.{n}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    parent.join(format!("{file_name}.bak.{nanos}"))
}

/// Copy an existing user config aside before the wizard overwrites it.
fn backup_existing_config(path: &Path) -> Result<PathBuf, String> {
    let dest = unique_backup_path(path);
    fs::copy(path, &dest).map_err(|e| {
        format!(
            "failed to copy {} to {}: {e}",
            path.display(),
            dest.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(0o600));
    }
    Ok(dest)
}

pub(super) fn write_wizard_file(path: &Path, rendered: &str) -> Result<Option<PathBuf>, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Failed to create config directory '{}': {e}",
                parent.display()
            )
        })?;
    }
    let bak = if path.exists() {
        Some(backup_existing_config(path)?)
    } else {
        None
    };
    crate::utils::write_file_mode(path, rendered, 0o600)
        .map_err(|e| format!("Failed to write '{}': {e}", path.display()))?;
    crate::config::sync_install_absgui_pref_from_toml(rendered);
    Ok(bak)
}

fn step_header(n: usize, title: &str, blurb: &str) {
    println!();
    println!(
        "{} {} — {}",
        "==>".green(),
        tf("wizard.ui.step", &[("n", &n.to_string())]).bold(),
        title.bold()
    );
    for line in blurb.lines() {
        println!("    {line}");
    }
    println!();
}

fn should_prompt_field(
    field: &catalog::FieldDef,
    gap: Option<&HashSet<String>>,
    doc: &DocumentMut,
) -> bool {
    if !catalog::is_visible_in_doc(field, doc) {
        return false;
    }
    let Some(g) = gap else {
        return true;
    };
    if !field.in_gap_fill {
        return false;
    }
    let key = catalog::gap_key(field);
    if g.contains(key) {
        return true;
    }
    if matches!(field.visible_if, catalog::VisibleIf::RamdiskEnabled)
        && g.contains("ramdisk.enabled")
        && ramdisk_enabled_in_doc(doc)
    {
        return true;
    }
    if matches!(field.visible_if, catalog::VisibleIf::PacmanFalse)
        && g.contains("self_update_use_pacman")
        && !get_root_bool(doc, "self_update_use_pacman", true)
    {
        return true;
    }
    false
}

fn suggested_str(field: &catalog::FieldDef) -> &str {
    match field.suggested {
        catalog::Suggested::Str(s) => s,
        _ => "",
    }
}

fn suggested_bool(field: &catalog::FieldDef) -> bool {
    match field.suggested {
        catalog::Suggested::Bool(v) => v,
        _ => false,
    }
}

fn suggested_usize(field: &catalog::FieldDef) -> usize {
    match field.suggested {
        catalog::Suggested::Usize(v) => v,
        _ => 0,
    }
}

fn choices_from_def(prefix: &str, defs: &[catalog::ChoiceDef]) -> Vec<Choice> {
    defs.iter()
        .map(|c| {
            Choice::new(
                c.value,
                t_or(&format!("{prefix}.{}.label", c.value), c.label),
                t_or(&format!("{prefix}.{}.help", c.value), c.help),
                c.suggested,
            )
        })
        .collect()
}

fn prompt_catalog_field(
    doc: &mut DocumentMut,
    field: &catalog::FieldDef,
    gap: Option<&HashSet<String>>,
) {
    let prefer = doc_has_path(doc, catalog::gap_key(field));
    let current_json = catalog::current_json(doc, field);
    match field.kind {
        catalog::FieldKind::Bool => {
            let current = catalog::as_bool(&current_json).unwrap_or(suggested_bool(field));
            let v = prompt_bool(
                tr_field_title(field),
                tr_field_expl(field),
                current,
                suggested_bool(field),
                prefer,
            );
            let _ = catalog::apply_answer(doc, field, &serde_json::Value::Bool(v));
        }
        catalog::FieldKind::Choice => {
            let prefix = match field.id {
                "build.default_environment" => "wizard.choice.env",
                "build.global_cpu_threads_mode" => "wizard.choice.cpu",
                "ramdisk.zram" => "wizard.choice.zram",
                _ => "wizard.choice",
            };
            let opts = choices_from_def(prefix, field.choices);
            let current = catalog::as_string(&current_json).unwrap_or_default();
            let v = prompt_choice(
                tr_field_title(field),
                tr_field_expl(field),
                &opts,
                Some(&current),
                prefer,
            );
            let _ = catalog::apply_answer(doc, field, &serde_json::Value::String(v));
        }
        catalog::FieldKind::Path => {
            let current = catalog::as_string(&current_json).unwrap_or_default();
            let v = prompt_path(
                tr_field_title(field),
                tr_field_expl(field),
                &current,
                suggested_str(field),
                field.id,
                prefer,
            );
            let _ = catalog::apply_answer(doc, field, &serde_json::Value::String(v));
        }
        catalog::FieldKind::Command => {
            let current = catalog::as_string(&current_json).unwrap_or_default();
            let v = prompt_command(
                tr_field_title(field),
                tr_field_expl(field),
                &current,
                suggested_str(field),
                prefer,
            );
            let _ = catalog::apply_answer(doc, field, &serde_json::Value::String(v));
        }
        catalog::FieldKind::String => {
            let current = catalog::as_string(&current_json).unwrap_or_default();
            let v = prompt_validated_string(
                tr_field_title(field),
                tr_field_expl(field),
                &current,
                suggested_str(field),
                prefer,
                |s| catalog::validate_field(field, &serde_json::Value::String(s.to_string()), None),
            );
            let _ = catalog::apply_answer(doc, field, &serde_json::Value::String(v));
        }
        catalog::FieldKind::Usize => {
            let current = catalog::as_usize(&current_json).unwrap_or(suggested_usize(field));
            let v = prompt_usize(
                tr_field_title(field),
                tr_field_expl(field),
                current,
                suggested_usize(field),
                field.usize_min,
                prefer,
            );
            let _ = catalog::apply_answer(doc, field, &serde_json::json!(v));
        }
        catalog::FieldKind::OptionalUsize => {
            let current = catalog::as_opt_usize(&current_json).ok().flatten();
            let mut v = prompt_optional_usize(
                tr_field_title(field),
                tr_field_expl(field),
                current,
                None,
                prefer,
            );
            if field.id == "build.maximum_cpu_threads_cap" {
                let soft = table_ref(doc, "build")
                    .and_then(|t| get_optional_usize(t, "global_cpu_threads_cap"));
                while let (Some(soft), Some(hard)) = (soft, v) {
                    if hard >= soft {
                        break;
                    }
                    print_invalid(&format!(
                        "The hard CPU maximum ({hard}) must be at least as high as the soft limit ({soft})"
                    ));
                    v = prompt_optional_usize(
                        tr_field_title(field),
                        tr_field_expl(field),
                        Some(hard),
                        None,
                        true,
                    );
                }
            }
            let json = match v {
                Some(n) => serde_json::json!(n),
                None => serde_json::Value::Null,
            };
            let _ = catalog::apply_answer(doc, field, &json);
        }
        catalog::FieldKind::OptionalPath => {
            let current = catalog::as_opt_string(&current_json).ok().flatten();
            let v = prompt_optional_string(
                tr_field_title(field),
                tr_field_expl(field),
                current.as_deref(),
                None,
                prefer,
                |p| validate_user_path(field.id, p),
            );
            let json = match v {
                Some(s) => serde_json::Value::String(s),
                None => serde_json::Value::Null,
            };
            let _ = catalog::apply_answer(doc, field, &json);
        }
        catalog::FieldKind::OptionalCommand => {
            let current = catalog::as_opt_string(&current_json).ok().flatten();
            let suggested = match field.suggested {
                catalog::Suggested::Str(s) => Some(s),
                _ => None,
            };
            let v = prompt_optional_string(
                tr_field_title(field),
                tr_field_expl(field),
                current.as_deref(),
                suggested,
                prefer,
                validate_command,
            );
            let json = match v {
                Some(s) => serde_json::Value::String(s),
                None => serde_json::Value::Null,
            };
            let _ = catalog::apply_answer(doc, field, &json);
        }
        catalog::FieldKind::StringList => {
            let current = catalog::as_string_list(&current_json).unwrap_or_default();
            let v = prompt_string_list(
                tr_field_title(field),
                tr_field_expl(field),
                &current,
                prefer,
            );
            let _ = catalog::apply_answer(
                doc,
                field,
                &serde_json::Value::Array(v.into_iter().map(serde_json::Value::String).collect()),
            );
        }
        catalog::FieldKind::SkipAfterList => {
            let present = doc_has_path(doc, field.id);
            let current = if present {
                catalog::as_string_list(&current_json).unwrap_or_default()
            } else {
                Vec::new()
            };
            match prompt_skip_after_list(
                tr_field_title(field),
                tr_field_expl(field),
                &current,
                present,
            ) {
                SkipAfterEdit::Keep => {}
                SkipAfterEdit::Unset => {
                    let _ = catalog::apply_answer(doc, field, &serde_json::Value::Null);
                }
                SkipAfterEdit::Set(items) => {
                    let _ = catalog::apply_answer(
                        doc,
                        field,
                        &serde_json::Value::Array(
                            items.into_iter().map(serde_json::Value::String).collect(),
                        ),
                    );
                }
            }
        }
        catalog::FieldKind::Repos => prompt_repos_field(doc, gap),
    }
}

fn prompt_repos_field(doc: &mut DocumentMut, gap: Option<&HashSet<String>>) {
    let (mut repos, mut default, default_present) = {
        let repos_table = table_mut(doc, "repositories");
        let repos = repo_entries(repos_table);
        let default_present = repos_table.contains_key("default");
        let default = get_str(repos_table, "default", "arch");
        (repos, default, default_present)
    };
    if repos.is_empty() {
        repos = KNOWN_REPOS
            .iter()
            .map(|(k, u)| ((*k).to_string(), (*u).to_string()))
            .collect();
    }

    if gap.is_none() {
        loop {
            println!("  {}", t("wizard.repos.list_header"));
            for (i, (name, url)) in repos.iter().enumerate() {
                let suggested = KNOWN_REPOS.iter().any(|(k, u)| *k == name && *u == url);
                println!(
                    "    [{}] {:<8} {}{}",
                    i + 1,
                    name,
                    url,
                    tags(false, suggested)
                );
            }
            println!();
            let action = prompt_choice(
                t("wizard.repos.what"),
                t("wizard.repos.what_help"),
                &[
                    Choice::new(
                        "done",
                        t("wizard.repos.done"),
                        t("wizard.repos.done_help"),
                        true,
                    ),
                    Choice::new(
                        "add",
                        t("wizard.repos.add"),
                        t("wizard.repos.add_help"),
                        false,
                    ),
                    Choice::new(
                        "remove",
                        t("wizard.repos.remove"),
                        t("wizard.repos.remove_help"),
                        false,
                    ),
                ],
                Some("done"),
                false,
            );
            match action.as_str() {
                "add" => {
                    if let Some((name, url)) = prompt_add_repo(&repos) {
                        if let Some(existing) = repos.iter_mut().find(|(n, _)| n == &name) {
                            existing.1 = url;
                        } else {
                            repos.push((name, url));
                        }
                    }
                }
                "remove" => {
                    if repos.len() == 1 {
                        println!(
                            "    {} {}",
                            t("wizard.ui.invalid_prefix").red(),
                            t("wizard.repos.keep_one")
                        );
                        continue;
                    }
                    println!("    {}", t("wizard.repos.which_remove"));
                    let line = read_line(&format!("    {}: ", t("wizard.repos.number")));
                    if line.is_empty() {
                        continue;
                    }
                    if let Ok(n) = line.parse::<usize>()
                        && n >= 1
                        && n <= repos.len()
                    {
                        let removed = repos.remove(n - 1);
                        if default == removed.0 {
                            default = suggested_default_name(&repos);
                        }
                    } else {
                        println!(
                            "    {} {}",
                            t("wizard.ui.invalid_prefix").red(),
                            t("wizard.repos.listed_number")
                        );
                    }
                }
                _ => break,
            }
            println!();
        }
    }

    if gap.is_some() && !ask_key(gap, "repositories.default") {
        return;
    }

    let suggest_default = suggested_default_name(&repos);
    let choices: Vec<Choice> = repos
        .iter()
        .map(|(name, _)| {
            Choice::new(
                name.clone(),
                name.clone(),
                t("wizard.repos.default_choice_help"),
                name == &suggest_default,
            )
        })
        .collect();
    let current_default = if repos.iter().any(|(n, _)| n == &default) {
        default
    } else {
        suggest_default.clone()
    };
    let picked = prompt_choice(
        t("wizard.repos.default_title"),
        t("wizard.repos.default_help"),
        &choices,
        Some(&current_default),
        default_present,
    );

    apply_repo_list(table_mut(doc, "repositories"), &repos, &picked);
}

fn prompt_add_repo(existing: &[(String, String)]) -> Option<(String, String)> {
    let mut name = read_line(t("wizard.repos.short_name"));
    if name.is_empty() {
        return None;
    }
    loop {
        match validate_repo_name(&name) {
            Ok(()) => break,
            Err(e) => {
                print_invalid(&e);
                name = read_retry(&name);
            }
        }
    }
    let known_url = KNOWN_REPOS
        .iter()
        .find(|(k, _)| *k == name)
        .map(|(_, u)| *u);
    let url_prompt = if let Some(url) = known_url {
        tf("wizard.repos.git_enter", &[("url", url)])
    } else {
        t("wizard.repos.git_https").to_string()
    };
    let url_line = read_line(&url_prompt);
    let mut url = if url_line.is_empty() {
        known_url.unwrap_or("").to_string()
    } else {
        url_line
    };
    loop {
        match validate_repo_url(&url) {
            Ok(()) => break,
            Err(e) => {
                print_invalid(&e);
                url = read_retry(&url);
            }
        }
    }
    if existing.iter().any(|(n, _)| n == &name) {
        let replace = prompt_bool(
            &tf("wizard.repos.exists", &[("name", &name)]),
            t("wizard.repos.replace"),
            false,
            false,
            false,
        );
        if !replace {
            return None;
        }
    }
    Some((name, url))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SkipAfterEdit {
    Keep,
    Unset,
    Set(Vec<String>),
}

fn prompt_skip_after_list(
    title: &str,
    explanation: &str,
    current: &[String],
    key_present: bool,
) -> SkipAfterEdit {
    let shown = if !key_present {
        t("wizard.repos.skip_after_unset").to_string()
    } else if current.is_empty() {
        t("wizard.ui.empty").to_string()
    } else {
        current.join(", ")
    };
    print_field(title, explanation);
    print_current_suggested(
        &shown,
        Some(t("wizard.repos.skip_after_empty_suggested")),
        key_present,
    );
    let line = read_line(&format!("    [{shown}]: "));
    if line.is_empty() {
        if !key_present {
            return SkipAfterEdit::Unset;
        }
        return SkipAfterEdit::Keep;
    }
    if line == "-" {
        return SkipAfterEdit::Set(Vec::new());
    }
    SkipAfterEdit::Set(parse_name_list(&line))
}

fn print_summary(doc: &DocumentMut, path: &Path) {
    let paths = table_ref(doc, "paths");
    let build = table_ref(doc, "build");
    let ram = table_ref(doc, "ramdisk");
    let repos = table_ref(doc, "repositories");
    println!();
    println!(
        "{}",
        format!("==> {}", t("wizard.ui.summary")).green().bold()
    );
    println!(
        "    {}",
        tf(
            "wizard.ui.saved_to",
            &[("path", &path.display().to_string())]
        )
    );
    if let Some(p) = paths {
        println!(
            "    {}: {}",
            t("wizard.ui.summary_sources"),
            get_str(p, "packages_path", "")
        );
        println!(
            "    {}: {}",
            t("wizard.ui.summary_packages"),
            get_str(p, "ready_made_packages_path", "")
        );
        println!(
            "    {}: {}",
            t("wizard.ui.summary_compile"),
            build
                .map(|b| {
                    match get_str(b, "default_environment", "local").as_str() {
                        "chroot" => t("wizard.ui.compile_chroot").to_string(),
                        _ => t("wizard.ui.compile_local").to_string(),
                    }
                })
                .unwrap_or_else(|| t("wizard.ui.compile_local").into())
        );
    }
    if let Some(r) = repos {
        let names: Vec<String> = repo_entries(r).into_iter().map(|(n, _)| n).collect();
        println!(
            "    {}: {}  {}",
            t("wizard.ui.summary_repos"),
            if names.is_empty() {
                t("wizard.ui.none").to_string()
            } else {
                names.join(", ")
            },
            tf(
                "wizard.ui.default_paren",
                &[("name", &get_str(r, "default", ""))]
            )
        );
    }
    if let Some(r) = ram {
        println!(
            "    {}: {}",
            t("wizard.ui.summary_ram"),
            if get_bool(r, "enabled", false) {
                t("wizard.ui.yes_word")
            } else {
                t("wizard.ui.no_word")
            }
        );
    }
    let manual = get_string_array(doc.as_table(), "manual_update_packages");
    println!(
        "    {}: {}",
        t("wizard.ui.summary_watched"),
        if manual.is_empty() {
            t("wizard.ui.none").to_string()
        } else {
            manual.join(", ")
        }
    );
}

fn print_invalid(err: &str) {
    println!("    {} {err}", t("wizard.ui.invalid_prefix").red());
    println!("    {}", t("wizard.ui.try_again"));
}

/// Empty Enter keeps `last`; any other line replaces it.
fn retry_input(last: &str, typed: &str) -> String {
    if typed.is_empty() {
        last.to_string()
    } else {
        typed.to_string()
    }
}

fn read_retry(last: &str) -> String {
    retry_input(last, &read_line(&format!("    [{last}]: ")))
}

fn prompt_path(
    title: &str,
    explanation: &str,
    current: &str,
    suggested: &str,
    key: &str,
    prefer_current: bool,
) -> String {
    prompt_validated_string(
        title,
        explanation,
        current,
        suggested,
        prefer_current,
        |v| validate_user_path(key, v),
    )
}

fn prompt_command(
    title: &str,
    explanation: &str,
    current: &str,
    suggested: &str,
    prefer_current: bool,
) -> String {
    prompt_validated_string(
        title,
        explanation,
        current,
        suggested,
        prefer_current,
        validate_command,
    )
}

pub(super) fn validate_command(cmd: &str) -> Result<(), String> {
    crate::utils::parse_command_argv(cmd).map(|_| ()).map_err(|e| {
        if e.contains("empty") {
            "Please type a command, for example: sudo pacman -Syu".into()
        } else if e.contains("metacharacter") || e.contains("shell with -c") {
            "Type a simple command only — no |, &&, or extra shell tricks. Example: sudo pacman -Syu"
                .into()
        } else if e.contains("unclosed quote") {
            "A quote mark is missing. Close the quotes, or remove them.".into()
        } else {
            e
        }
    })
}

pub(super) fn validate_ignore_flag(flag: &str) -> Result<(), String> {
    let flag = flag.trim();
    if flag.is_empty() {
        return Err("This cannot be empty. For pacman it is usually --ignore.".into());
    }
    if flag.split_whitespace().nth(1).is_some() {
        return Err("Type a single option, like --ignore — not a whole command.".into());
    }
    Ok(())
}

pub(super) fn validate_user_path(key: &str, raw: &str) -> Result<(), String> {
    let expanded = config::expand_user_path(raw);
    crate::utils::validate_config_path(key, &expanded.to_string_lossy()).map_err(friendly_path_err)
}

fn friendly_path_err(e: String) -> String {
    if e.contains("cannot be empty") {
        "Please type a folder or file path. It cannot be empty.".into()
    } else if e.contains("must not point at a system directory") {
        "Please pick a folder ABS can manage — not your home folder, /tmp, or a system folder."
            .into()
    } else {
        e
    }
}

pub(super) fn validate_mount_point(raw: &str) -> Result<(), String> {
    let expanded = config::expand_user_path(raw);
    let s = expanded.to_string_lossy();
    crate::utils::validate_config_path("ramdisk.mount_point", &s).map_err(friendly_path_err)?;
    crate::ramdisk::validate_ramdisk_mount_point(&s).map_err(|e| {
        if e.contains("must be a directory named abs") {
            "The last part of the folder name must start with “abs”, for example /run/abs-ram."
                .into()
        } else {
            friendly_path_err(e)
        }
    })
}

pub(super) fn validate_repo_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err("Please type a short name.".into());
    };
    if !first.is_ascii_alphabetic() {
        return Err("The name must start with a letter.".into());
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("Use only letters, digits, '-' or '_'.".into());
    }
    if name == "default" {
        return Err("The name “default” is reserved. Pick another short name.".into());
    }
    Ok(())
}

pub(super) fn validate_repo_url(url: &str) -> Result<(), String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("Please type a git address.".into());
    }
    if url.starts_with("https://") || url.starts_with("http://") || url.starts_with("git@") {
        Ok(())
    } else {
        Err("The address must start with https://, http://, or git@".into())
    }
}

fn read_line(prompt: &str) -> String {
    print!("{prompt}");
    let _ = io::stdout().flush();
    let mut buf = String::new();
    if io::stdin().read_line(&mut buf).is_err() {
        die!("Failed to read stdin");
    }
    buf.trim().to_string()
}

fn print_field(title: &str, explanation: &str) {
    println!("  {}", title.bold());
    for line in explanation.lines() {
        let line = line.trim();
        if !line.is_empty() {
            println!("    {line}");
        }
    }
}

fn tags(is_current: bool, is_suggested: bool) -> String {
    let mut parts = Vec::new();
    if is_current {
        parts.push(format!("{}", t("wizard.ui.current_tag").green()));
    }
    if is_suggested {
        parts.push(format!("{}", t("wizard.ui.suggested_tag").green()));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" {}", parts.join(" "))
    }
}

fn print_current_suggested(current: &str, suggested: Option<&str>, show_current_tag: bool) {
    let matches_suggested = suggested.is_some_and(|s| s == current);
    print!("    {} {current}", t("wizard.ui.current").dimmed());
    println!("{}", tags(show_current_tag, matches_suggested));
    if let Some(s) = suggested
        && !matches_suggested
    {
        println!(
            "    {} {} {}",
            t("wizard.ui.suggested").dimmed(),
            s,
            t("wizard.ui.suggested_tag").green()
        );
    }
}

/// 0-based index of the Enter default.
/// When `prefer_current` and `current` matches an option, that option wins;
/// otherwise the option marked `suggested` wins (then `current` as a fallback).
fn choice_default_index(
    options: &[Choice],
    current: Option<&str>,
    prefer_current: bool,
) -> Option<usize> {
    let match_current =
        || current.and_then(|c| options.iter().position(|o| o.value.eq_ignore_ascii_case(c)));
    if prefer_current {
        if let Some(i) = match_current() {
            return Some(i);
        }
    }
    options
        .iter()
        .position(|o| o.suggested)
        .or_else(match_current)
}

struct Choice {
    value: String,
    label: String,
    help: String,
    suggested: bool,
}

impl Choice {
    fn new(
        value: impl Into<String>,
        label: impl Into<String>,
        help: impl Into<String>,
        suggested: bool,
    ) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            help: help.into(),
            suggested: suggested,
        }
    }
}

fn prompt_choice(
    title: &str,
    explanation: &str,
    options: &[Choice],
    current: Option<&str>,
    prefer_current: bool,
) -> String {
    print_field(title, explanation);
    println!("    {}", t("wizard.ui.choose_number"));
    for (i, opt) in options.iter().enumerate() {
        let is_current = current.is_some_and(|c| c.eq_ignore_ascii_case(&opt.value));
        println!(
            "    [{}] {}{}",
            i + 1,
            opt.label,
            tags(prefer_current && is_current, opt.suggested)
        );
        for line in opt.help.lines() {
            let line = line.trim();
            if !line.is_empty() {
                println!("        {line}");
            }
        }
    }
    let default_idx = choice_default_index(options, current, prefer_current).map(|i| i + 1);
    let hint = match default_idx {
        Some(i) => format!("    {} [{i}]: ", t("wizard.ui.choice_prompt")),
        None => format!("    {}: ", t("wizard.ui.choice_prompt")),
    };
    loop {
        let line = read_line(&hint);
        if line.is_empty() {
            if let Some(i) = default_idx {
                return options[i - 1].value.clone();
            }
            println!("    {}", t("wizard.ui.please_number"));
            continue;
        }
        if let Ok(n) = line.parse::<usize>()
            && n >= 1
            && n <= options.len()
        {
            return options[n - 1].value.clone();
        }
        if let Some(opt) = options
            .iter()
            .find(|o| o.value.eq_ignore_ascii_case(&line) || o.label.eq_ignore_ascii_case(&line))
        {
            return opt.value.clone();
        }
        println!("    {}", t("wizard.ui.invalid_choice"));
    }
}

fn prompt_bool(
    title: &str,
    explanation: &str,
    current: bool,
    suggested: bool,
    prefer_current: bool,
) -> bool {
    let current_s = if current { "yes" } else { "no" };
    let choice = prompt_choice(
        title,
        explanation,
        &[
            Choice::new(
                "yes",
                t("wizard.bool.yes"),
                t("wizard.bool.yes_help"),
                suggested,
            ),
            Choice::new(
                "no",
                t("wizard.bool.no"),
                t("wizard.bool.no_help"),
                !suggested,
            ),
        ],
        Some(current_s),
        prefer_current,
    );
    choice == "yes"
}

fn prompt_string(
    title: &str,
    explanation: &str,
    current: &str,
    suggested: &str,
    prefer_current: bool,
) -> String {
    print_field(title, explanation);
    print_current_suggested(current, Some(suggested), prefer_current);
    let line = read_line(&format!("    [{current}]: "));
    if line.is_empty() {
        current.to_string()
    } else {
        line
    }
}

fn prompt_validated_string(
    title: &str,
    explanation: &str,
    current: &str,
    suggested: &str,
    prefer_current: bool,
    validate: impl Fn(&str) -> Result<(), String>,
) -> String {
    let mut value = prompt_string(title, explanation, current, suggested, prefer_current);
    loop {
        match validate(&value) {
            Ok(()) => return value,
            Err(e) => {
                print_invalid(&e);
                value = read_retry(&value);
            }
        }
    }
}

fn prompt_usize(
    title: &str,
    explanation: &str,
    current: usize,
    suggested: usize,
    min: usize,
    prefer_current: bool,
) -> usize {
    print_field(title, explanation);
    let current_s = current.to_string();
    let suggested_s = suggested.to_string();
    print_current_suggested(&current_s, Some(&suggested_s), prefer_current);
    let mut shown = current_s;
    loop {
        let line = read_line(&format!("    [{shown}]: "));
        let attempt = retry_input(&shown, &line);
        match attempt.parse::<usize>() {
            Ok(n) if n >= min => return n,
            Ok(_) => {
                print_invalid(&tf(
                    "wizard.ui.enter_at_least",
                    &[("min", &min.to_string())],
                ));
                shown = attempt;
            }
            Err(_) => {
                print_invalid(t("wizard.ui.enter_whole_number"));
                shown = attempt;
            }
        }
    }
}

fn prompt_optional_usize(
    title: &str,
    explanation: &str,
    current: Option<usize>,
    suggested: Option<usize>,
    prefer_current: bool,
) -> Option<usize> {
    print_field(title, explanation);
    let suggested_s = suggested
        .map(|n| n.to_string())
        .unwrap_or_else(|| t("wizard.ui.not_set").to_string());
    let mut typed: Option<String> = None;
    let mut first = true;
    loop {
        let shown = typed
            .clone()
            .or_else(|| current.map(|n| n.to_string()))
            .unwrap_or_else(|| t("wizard.ui.not_set").to_string());
        if first {
            print_current_suggested(&shown, Some(&suggested_s), prefer_current);
            first = false;
        }
        let line = read_line(&format!("    [{shown}]: "));
        let attempt = if line.is_empty() {
            if typed.is_none() {
                return current;
            }
            typed.clone().unwrap_or(shown)
        } else {
            line
        };
        if attempt == "-"
            || attempt.eq_ignore_ascii_case("none")
            || attempt.eq_ignore_ascii_case("unset")
        {
            return None;
        }
        match attempt.parse::<usize>() {
            Ok(n) if n >= 1 => return Some(n),
            Ok(_) => {
                print_invalid(t("wizard.ui.enter_at_least_1"));
                typed = Some(attempt);
            }
            Err(_) => {
                print_invalid(t("wizard.ui.enter_number_or_clear"));
                typed = Some(attempt);
            }
        }
    }
}

fn prompt_optional_string(
    title: &str,
    explanation: &str,
    current: Option<&str>,
    suggested: Option<&str>,
    prefer_current: bool,
    validate: impl Fn(&str) -> Result<(), String>,
) -> Option<String> {
    print_field(title, explanation);
    let suggested_s = suggested.unwrap_or(t("wizard.ui.not_set"));
    let mut current: Option<String> = current.map(str::to_string);
    let mut first = true;
    loop {
        let shown = current.as_deref().unwrap_or(t("wizard.ui.not_set"));
        if first {
            print_current_suggested(shown, Some(suggested_s), prefer_current);
            first = false;
        }
        let line = read_line(&format!("    [{shown}]: "));
        let attempt = if line.is_empty() {
            match current.take() {
                Some(c) => c,
                None => return None,
            }
        } else if line == "-" {
            return None;
        } else {
            line
        };
        match validate(&attempt) {
            Ok(()) => return Some(attempt),
            Err(e) => {
                print_invalid(&e);
                current = Some(attempt);
            }
        }
    }
}

fn prompt_string_list(
    title: &str,
    explanation: &str,
    current: &[String],
    prefer_current: bool,
) -> Vec<String> {
    let shown = if current.is_empty() {
        t("wizard.ui.empty").to_string()
    } else {
        current.join(", ")
    };
    print_field(title, explanation);
    print_current_suggested(&shown, Some(t("wizard.ui.empty")), prefer_current);
    let line = read_line(&format!("    [{shown}]: "));
    if line.is_empty() {
        return current.to_vec();
    }
    if line == "-" {
        return Vec::new();
    }
    parse_name_list(&line)
}

pub(super) fn parse_name_list(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = raw
        .split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    out.sort();
    out.dedup();
    out
}

fn inline_to_std_table(inline: &toml_edit::InlineTable) -> Table {
    let mut table = Table::new();
    for (key, value) in inline.iter() {
        table.insert(key, Item::Value(value.clone()));
    }
    table
}

/// `[section]` or inline `section = { ... }`. Writers convert inline tables to `[section]`.
pub(super) fn table_ref<'a>(doc: &'a DocumentMut, name: &str) -> Option<&'a dyn TableLike> {
    doc.get(name).and_then(Item::as_table_like)
}

pub(super) fn table_mut<'a>(doc: &'a mut DocumentMut, name: &str) -> &'a mut Table {
    let inline = doc.get(name).and_then(Item::as_inline_table).cloned();
    if let Some(inline) = inline {
        doc[name] = Item::Table(inline_to_std_table(&inline));
    } else if let Some(item) = doc.get(name)
        && !item.is_table()
    {
        die!("[{name}] is not a table");
    }
    doc.entry(name)
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .unwrap_or_else(|| die!("[{name}] is not a table"))
}

pub(super) fn get_str(table: &dyn TableLike, key: &str, default: &str) -> String {
    table
        .get(key)
        .and_then(|i| i.as_str())
        .unwrap_or(default)
        .to_string()
}

pub(super) fn get_bool(table: &dyn TableLike, key: &str, default: bool) -> bool {
    table.get(key).and_then(|i| i.as_bool()).unwrap_or(default)
}

pub(super) fn get_usize(table: &dyn TableLike, key: &str, default: usize) -> usize {
    let Some(item) = table.get(key) else {
        return default;
    };
    if let Some(n) = item.as_integer().and_then(|n| usize::try_from(n).ok()) {
        return n;
    }
    item.as_str()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(default)
}

pub(super) fn get_optional_usize(table: &dyn TableLike, key: &str) -> Option<usize> {
    table
        .get(key)
        .and_then(|i| i.as_integer())
        .and_then(|n| usize::try_from(n).ok())
}

pub(super) fn get_optional_str(table: &dyn TableLike, key: &str) -> Option<String> {
    table
        .get(key)
        .and_then(|i| i.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub(super) fn get_string_array(table: &dyn TableLike, key: &str) -> Vec<String> {
    let Some(arr) = table.get(key).and_then(|i| i.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

pub(super) fn get_root_str(doc: &DocumentMut, key: &str, default: &str) -> String {
    get_str(doc.as_table(), key, default)
}

pub(super) fn get_root_bool(doc: &DocumentMut, key: &str, default: bool) -> bool {
    get_bool(doc.as_table(), key, default)
}

pub(super) fn set_str(table: &mut Table, key: &str, value: &str) {
    if table.get(key).and_then(|i| i.as_str()) == Some(value) {
        return;
    }
    table[key] = Item::Value(Value::from(value));
}

pub(super) fn set_bool(table: &mut Table, key: &str, value: bool) {
    if table.get(key).and_then(|i| i.as_bool()) == Some(value) {
        return;
    }
    table[key] = Item::Value(Value::from(value));
}

pub(super) fn set_usize(table: &mut Table, key: &str, value: usize) {
    if table
        .get(key)
        .and_then(|i| i.as_integer())
        .and_then(|n| usize::try_from(n).ok())
        == Some(value)
    {
        return;
    }
    table[key] = Item::Value(Value::from(value as i64));
}

pub(super) fn set_optional_usize(table: &mut Table, key: &str, value: Option<usize>) {
    match value {
        Some(n) => set_usize(table, key, n),
        None => {
            table.remove(key);
        }
    }
}

pub(super) fn set_optional_str(table: &mut Table, key: &str, value: Option<&str>) {
    match value {
        Some(s) => set_str(table, key, s),
        None => {
            table.remove(key);
        }
    }
}

pub(super) fn set_string_array(table: &mut Table, key: &str, items: &[String]) {
    if get_string_array(table, key) == items {
        return;
    }
    let mut arr = Array::new();
    for s in items {
        arr.push(s.as_str());
    }
    table[key] = Item::Value(Value::Array(arr));
}

pub(super) fn set_root_str(doc: &mut DocumentMut, key: &str, value: &str) {
    set_str(doc.as_table_mut(), key, value);
}

pub(super) fn set_root_bool(doc: &mut DocumentMut, key: &str, value: bool) {
    set_bool(doc.as_table_mut(), key, value);
}

pub(super) fn repo_entries(table: &dyn TableLike) -> Vec<(String, String)> {
    table
        .iter()
        .filter(|(k, _)| *k != "default")
        .filter_map(|(k, v)| v.as_str().map(|s| (k.to_string(), s.to_string())))
        .collect()
}

pub(super) fn suggested_default_name(repos: &[(String, String)]) -> String {
    for known in ["arch", "aur", "cachyos"] {
        if repos.iter().any(|(n, _)| n == known) {
            return known.to_string();
        }
    }
    repos
        .first()
        .map(|(n, _)| n.clone())
        .unwrap_or_else(|| "arch".into())
}

pub(super) fn apply_repo_list(table: &mut Table, repos: &[(String, String)], default: &str) {
    let keep: HashSet<&str> = repos.iter().map(|(n, _)| n.as_str()).collect();
    let to_remove: Vec<String> = table
        .iter()
        .map(|(k, _)| k.to_string())
        .filter(|k| k != "default" && !keep.contains(k.as_str()))
        .collect();
    for k in to_remove {
        table.remove(&k);
    }
    for (name, url) in repos {
        set_str(table, name, url);
    }
    set_str(table, "default", default);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_template_validates() {
        Config::from_toml_text(example_config_text()).expect("abs.toml.example must validate");
    }

    #[test]
    fn wizard_defaults_roundtrip_document() {
        let mut doc: DocumentMut = example_config_text().parse().unwrap();
        let paths = table_mut(&mut doc, "paths");
        assert_eq!(get_str(paths, "packages_path", ""), SUGGEST_PACKAGES_PATH);
        set_str(paths, "packages_path", SUGGEST_PACKAGES_PATH);
        let build = table_mut(&mut doc, "build");
        set_str(build, "default_environment", "chroot");
        assert_eq!(get_str(build, "default_environment", ""), "chroot");
        Config::from_toml_text(&doc.to_string()).expect("mutated example still validates");
    }

    #[test]
    fn parse_name_list_splits_commas_and_spaces() {
        assert_eq!(
            parse_name_list("mesa, lib32-mesa  firefox"),
            vec!["firefox", "lib32-mesa", "mesa"]
        );
        assert!(parse_name_list("").is_empty());
    }

    #[test]
    fn retry_input_keeps_last_on_empty_enter() {
        assert_eq!(retry_input("/tmp/bad|path", ""), "/tmp/bad|path");
        assert_eq!(retry_input("/tmp/bad|path", "$HOME/abs"), "$HOME/abs");
    }

    #[test]
    fn validate_ignore_flag_rejects_empty_and_multiple_tokens() {
        assert!(validate_ignore_flag("--ignore").is_ok());
        assert!(validate_ignore_flag("").is_err());
        assert!(validate_ignore_flag("  ").is_err());
        assert!(validate_ignore_flag("--ignore extra").is_err());
    }

    #[test]
    fn validate_command_rejects_shell_metacharacters() {
        assert!(validate_command("sudo pacman -Syu").is_ok());
        assert!(validate_command("sudo pacman -Syu | less").is_err());
        assert!(validate_command("").is_err());
    }

    fn env_choices() -> [Choice; 2] {
        [
            Choice::new("local", "local", "", true),
            Choice::new("chroot", "chroot", "", false),
        ]
    }

    #[test]
    fn choice_default_index_prefers_current_when_key_present() {
        let opts = env_choices();
        assert_eq!(
            choice_default_index(&opts, Some("chroot"), true),
            Some(1),
            "in-file value is the Enter default"
        );
        assert_eq!(choice_default_index(&opts, Some("local"), true), Some(0));
    }

    #[test]
    fn choice_default_index_uses_suggested_when_key_missing() {
        let opts = env_choices();
        assert_eq!(
            choice_default_index(&opts, None, false),
            Some(0),
            "missing key uses Suggested"
        );
        assert_eq!(
            choice_default_index(&opts, Some("chroot"), false),
            Some(0),
            "value without prefer_current still uses Suggested"
        );
        assert_eq!(
            choice_default_index(&opts, Some("nope"), true),
            Some(0),
            "unrecognized current falls back to Suggested"
        );
    }

    #[test]
    fn tags_current_and_suggested_are_independent() {
        let current = t("wizard.ui.current_tag");
        let suggested = t("wizard.ui.suggested_tag");
        let both = tags(true, true);
        assert!(both.contains(current), "{both:?}");
        assert!(both.contains(suggested), "{both:?}");
        let only_suggested = tags(false, true);
        assert!(!only_suggested.contains(current), "{only_suggested:?}");
        assert!(only_suggested.contains(suggested), "{only_suggested:?}");
        let only_current = tags(true, false);
        assert!(only_current.contains(current), "{only_current:?}");
        assert!(!only_current.contains(suggested), "{only_current:?}");
    }

    #[test]
    fn unchanged_string_preserves_example_text() {
        let mut doc: DocumentMut = example_config_text().parse().unwrap();
        let text_before = doc.to_string();
        let paths = table_mut(&mut doc, "paths");
        let current = get_str(paths, "packages_path", "");
        set_str(paths, "packages_path", &current);
        assert_eq!(doc.to_string(), text_before);
    }

    #[test]
    fn repo_name_and_url_validation() {
        assert!(validate_repo_name("myrepo").is_ok());
        assert!(validate_repo_name("my-repo_1").is_ok());
        assert!(validate_repo_name("default").is_err());
        assert!(validate_repo_name("1bad").is_err());
        assert!(validate_repo_url("https://example.com/foo.git").is_ok());
        assert!(validate_repo_url("git@github.com:org/repo.git").is_ok());
        assert!(validate_repo_url("ftp://nope").is_err());
    }

    #[test]
    fn apply_repo_list_adds_removes_and_sets_default() {
        let mut doc: DocumentMut = example_config_text().parse().unwrap();
        let repos = table_mut(&mut doc, "repositories");
        let mut list = repo_entries(repos);
        assert!(list.iter().any(|(n, _)| n == "arch"));
        list.retain(|(n, _)| n != "cachyos");
        list.push(("myrepo".into(), "https://example.com/pkgbuilds.git".into()));
        apply_repo_list(repos, &list, "aur");
        let repos = table_mut(&mut doc, "repositories");
        let names: Vec<_> = repo_entries(repos).into_iter().map(|(n, _)| n).collect();
        assert!(names.contains(&"arch".into()));
        assert!(names.contains(&"aur".into()));
        assert!(names.contains(&"myrepo".into()));
        assert!(!names.contains(&"cachyos".into()));
        assert_eq!(get_str(repos, "default", ""), "aur");
        Config::from_toml_text(&doc.to_string()).expect("repo edits still validate");
    }

    #[test]
    fn unique_backup_path_uses_bak_then_numbers() {
        let dir = std::env::temp_dir().join(format!(
            "abs_wizard_bak_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("abs.toml");
        fs::write(&cfg, "current\n").unwrap();
        let bak = unique_backup_path(&cfg);
        assert_eq!(bak.file_name().unwrap(), "abs.toml.bak");
        fs::write(&bak, "first\n").unwrap();
        let bak1 = unique_backup_path(&cfg);
        assert_eq!(bak1.file_name().unwrap(), "abs.toml.bak.1");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_existing_config_copies_contents_and_leaves_original() {
        let dir = std::env::temp_dir().join(format!(
            "abs_wizard_bak_copy_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("abs.toml");
        fs::write(&cfg, "old-config\n").unwrap();
        let bak = backup_existing_config(&cfg).unwrap();
        assert_eq!(bak, dir.join("abs.toml.bak"));
        assert_eq!(fs::read_to_string(&bak).unwrap(), "old-config\n");
        assert_eq!(fs::read_to_string(&cfg).unwrap(), "old-config\n");
        fs::write(&cfg, "new-config\n").unwrap();
        let bak1 = backup_existing_config(&cfg).unwrap();
        assert_eq!(bak1, dir.join("abs.toml.bak.1"));
        assert_eq!(fs::read_to_string(&bak1).unwrap(), "new-config\n");
        assert_eq!(fs::read_to_string(&bak).unwrap(), "old-config\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn suggested_default_prefers_arch_then_aur() {
        let only_custom = vec![("mine".into(), "https://example.com/x.git".into())];
        assert_eq!(suggested_default_name(&only_custom), "mine");
        let aur_only = vec![
            ("mine".into(), "https://example.com/x.git".into()),
            ("aur".into(), "https://aur.archlinux.org".into()),
        ];
        assert_eq!(suggested_default_name(&aur_only), "aur");
    }

    #[test]
    fn example_config_has_no_required_wizard_gaps() {
        let doc: DocumentMut = example_config_text().parse().unwrap();
        assert!(
            missing_required_keys(&doc).is_empty(),
            "abs.toml.example must include every required wizard key: {:?}",
            missing_required_keys(&doc)
        );
    }

    #[test]
    fn missing_required_keys_flags_absent_ramdisk_enabled() {
        let text = r#"
config_version = 1
check_for_update_on_startup = true
auto_update_on_startup = false
self_update_use_pacman = true
self_update_at_updates = false
install_absgui = true
[paths]
packages_path = "$XDG_CACHE_HOME/abs/packages"
chroot_base_path = "$XDG_CACHE_HOME/abs/chroot"
ready_made_packages_path = "$XDG_CACHE_HOME/abs/ready"
[build]
default_environment = "local"
system_update_first = true
ignore_compilation_failures = false
compile_first_install_after = false
clean_install_by_default = false
ignore_already_made_packages = false
concurrent_repos_downloads_limit = 10
concurrent_compilations_limit = 1
fast_aur_rpc_update_checks = true
clean_chroot_after_compilation = true
global_cpu_threads_mode = "strict"
[system_update]
command_to_update_repositories = "sudo pacman -Sy"
command_to_perform_system_update = "sudo pacman -Syu"
ignore_flag = "--ignore"
[repositories]
arch = "https://gitlab.archlinux.org/archlinux/packaging/packages"
default = "arch"
"#;
        let doc: DocumentMut = text.parse().unwrap();
        let missing = missing_required_keys(&doc);
        assert!(
            missing.contains(&"ramdisk.enabled".to_string()),
            "{missing:?}"
        );
        assert!(
            !missing.iter().any(|k| k.starts_with("ramdisk.")
                && k != "ramdisk.enabled"
                && k != "ramdisk.zram"),
            "disabled/absent ramdisk must not flag tmpfs child keys: {missing:?}"
        );
    }

    #[test]
    fn ramdisk_enabled_false_does_not_flag_child_keys() {
        let text = r#"
config_version = 1
check_for_update_on_startup = true
auto_update_on_startup = false
self_update_use_pacman = true
self_update_at_updates = false
install_absgui = true
[paths]
packages_path = "$XDG_CACHE_HOME/abs/packages"
chroot_base_path = "$XDG_CACHE_HOME/abs/chroot"
ready_made_packages_path = "$XDG_CACHE_HOME/abs/ready"
[build]
default_environment = "local"
system_update_first = true
ignore_compilation_failures = false
compile_first_install_after = false
clean_install_by_default = false
ignore_already_made_packages = false
concurrent_repos_downloads_limit = 10
concurrent_compilations_limit = 1
fast_aur_rpc_update_checks = true
clean_chroot_after_compilation = true
global_cpu_threads_mode = "strict"
[system_update]
command_to_update_repositories = "sudo pacman -Sy"
command_to_perform_system_update = "sudo pacman -Syu"
ignore_flag = "--ignore"
[repositories]
default = "arch"
[ramdisk]
enabled = false
"#;
        let doc: DocumentMut = text.parse().unwrap();
        let missing = missing_required_keys(&doc);
        assert!(
            !missing
                .iter()
                .any(|k| k.starts_with("ramdisk.") && k != "ramdisk.zram"),
            "{missing:?}"
        );
    }

    #[test]
    fn ramdisk_enabled_true_flags_missing_child_keys() {
        let text = r#"
config_version = 1
check_for_update_on_startup = true
auto_update_on_startup = false
self_update_use_pacman = true
self_update_at_updates = false
install_absgui = true
[paths]
packages_path = "$XDG_CACHE_HOME/abs/packages"
chroot_base_path = "$XDG_CACHE_HOME/abs/chroot"
ready_made_packages_path = "$XDG_CACHE_HOME/abs/ready"
[build]
default_environment = "local"
system_update_first = true
ignore_compilation_failures = false
compile_first_install_after = false
clean_install_by_default = false
ignore_already_made_packages = false
concurrent_repos_downloads_limit = 10
concurrent_compilations_limit = 1
fast_aur_rpc_update_checks = true
clean_chroot_after_compilation = true
global_cpu_threads_mode = "strict"
[system_update]
command_to_update_repositories = "sudo pacman -Sy"
command_to_perform_system_update = "sudo pacman -Syu"
ignore_flag = "--ignore"
[repositories]
default = "arch"
[ramdisk]
enabled = true
"#;
        let doc: DocumentMut = text.parse().unwrap();
        let missing = missing_required_keys(&doc);
        assert!(
            missing.contains(&"ramdisk.mount_point".to_string()),
            "{missing:?}"
        );
        assert!(missing.contains(&"ramdisk.size".to_string()), "{missing:?}");
        assert!(
            !missing.contains(&"ramdisk.enabled".to_string()),
            "{missing:?}"
        );
    }

    #[test]
    fn never_and_never_for_version_prefs() {
        assert!(prefs_suppress_prompt(
            &WizardPromptPrefs {
                never: true,
                never_for_abs_version: None,
            },
            "1.6.0"
        ));
        assert!(prefs_suppress_prompt(
            &WizardPromptPrefs {
                never: false,
                never_for_abs_version: Some("1.6.0".into()),
            },
            "1.6.0"
        ));
        assert!(!prefs_suppress_prompt(
            &WizardPromptPrefs {
                never: false,
                never_for_abs_version: Some("1.5.0".into()),
            },
            "1.6.0"
        ));
        assert!(prefs_suppress_prompt(
            &WizardPromptPrefs {
                never: true,
                never_for_abs_version: Some("1.5.0".into()),
            },
            "1.6.0"
        ));
        let parsed = parse_wizard_prompt_prefs("never_for_abs_version = \"1.6.0\"\n");
        assert!(!parsed.never);
        assert_eq!(parsed.never_for_abs_version.as_deref(), Some("1.6.0"));
        let rendered = render_wizard_prompt_prefs(&parsed);
        assert!(rendered.contains("1.6.0"));
        assert!(!rendered.contains("never ="));
    }

    #[test]
    fn install_path_gap_only_when_pacman_is_false() {
        let with_pacman = r#"
check_for_update_on_startup = true
auto_update_on_startup = false
self_update_use_pacman = true
self_update_at_updates = false
install_absgui = true
[paths]
packages_path = "p"
chroot_base_path = "c"
ready_made_packages_path = "r"
[build]
default_environment = "local"
system_update_first = true
ignore_compilation_failures = false
compile_first_install_after = false
clean_install_by_default = false
ignore_already_made_packages = false
concurrent_repos_downloads_limit = 10
concurrent_compilations_limit = 1
fast_aur_rpc_update_checks = true
clean_chroot_after_compilation = true
global_cpu_threads_mode = "strict"
[system_update]
command_to_update_repositories = "sudo pacman -Sy"
command_to_perform_system_update = "sudo pacman -Syu"
ignore_flag = "--ignore"
[repositories]
default = "arch"
[ramdisk]
enabled = false
"#;
        let doc: DocumentMut = with_pacman.parse().unwrap();
        assert!(!missing_required_keys(&doc).contains(&"self_update_install_path".to_string()));
        let without = with_pacman.replace(
            "self_update_use_pacman = true",
            "self_update_use_pacman = false",
        );
        let doc: DocumentMut = without.parse().unwrap();
        assert!(missing_required_keys(&doc).contains(&"self_update_install_path".to_string()));
    }

    #[test]
    fn missing_required_keys_flags_absent_install_absgui() {
        let text = r#"
config_version = 1
check_for_update_on_startup = true
auto_update_on_startup = false
self_update_use_pacman = true
self_update_at_updates = false
[paths]
packages_path = "$XDG_CACHE_HOME/abs/packages"
chroot_base_path = "$XDG_CACHE_HOME/abs/chroot"
ready_made_packages_path = "$XDG_CACHE_HOME/abs/ready"
[build]
default_environment = "local"
system_update_first = true
ignore_compilation_failures = false
compile_first_install_after = false
clean_install_by_default = false
ignore_already_made_packages = false
concurrent_repos_downloads_limit = 10
concurrent_compilations_limit = 1
fast_aur_rpc_update_checks = true
clean_chroot_after_compilation = true
global_cpu_threads_mode = "strict"
[system_update]
command_to_update_repositories = "sudo pacman -Sy"
command_to_perform_system_update = "sudo pacman -Syu"
ignore_flag = "--ignore"
[repositories]
arch = "https://gitlab.archlinux.org/archlinux/packaging/packages"
default = "arch"
[ramdisk]
enabled = false
"#;
        let doc: DocumentMut = text.parse().unwrap();
        let missing = missing_required_keys(&doc);
        assert!(
            missing.contains(&"install_absgui".to_string()),
            "{missing:?}"
        );
    }

    #[test]
    fn language_question_skipped_when_local_config_exists() {
        LANG_PROMPTED.store(false, Ordering::SeqCst);
        let before = abs_i18n::current_lang();
        let lang = prompt_language_if_new_local_config(true);
        assert_eq!(lang, before);
        assert!(
            !LANG_PROMPTED.load(Ordering::SeqCst),
            "must not mark language as prompted when a local config already exists"
        );
    }

    #[test]
    fn language_prompt_flag_prevents_asking_twice() {
        LANG_PROMPTED.store(true, Ordering::SeqCst);
        let before = abs_i18n::current_lang();
        let lang = prompt_language_if_new_local_config(false);
        assert_eq!(lang, before);
        LANG_PROMPTED.store(false, Ordering::SeqCst);
    }

    const INLINE_USER_STYLE: &str = r#"
config_version = 1
manual_update_packages = ["curl"]
skip_install_packages = ["mesa-docs"]
check_for_update_on_startup = true
auto_update_on_startup = false
self_update_use_pacman = true
self_update_at_updates = false
install_absgui = true
paths= { packages_path = "/media/storage/packages/abs/packages", chroot_base_path = "/media/storage/packages/abs/chroot", ready_made_packages_path = "/media/storage/packages/abs/ready" }
ramdisk= { enabled = true, mount_point = "/run/abs-ram", size = "69G", mode = "0755", build_workdir = true, chroot = true, packages = true, sync_chroot_on_exit = false, min_free_ram_mb = 4096, zram = "full", warn_packages_ram = false, reclaim_mount_on_startup = true }
build= { default_environment = "local", ignore_compilation_failures = true, compile_first_install_after = true, clean_install_by_default = true, ignore_already_made_packages = false, concurrent_repos_downloads_limit = 10, concurrent_compilations_limit = 2, fast_aur_rpc_update_checks = true, system_update_first = true, clean_chroot_after_compilation = true, global_cpu_threads_mode = "strict" }
system_update= { command_to_update_repositories = "yay -Sy --quiet", command_to_perform_system_update = "yay -Syu --quiet", ignore_flag = "--ignore", ignore_packages = [] }
repositories= { default = "arch", venomo = "https://github.com/Ven0m0/PKG.git", arch = "https://gitlab.archlinux.org/archlinux/packaging/packages", aur = "https://aur.archlinux.org" }
packages = { vim = { source = "arch", build_env = "local", tests = false } }
"#;

    #[test]
    fn inline_tables_show_current_values_not_defaults() {
        let doc: DocumentMut = INLINE_USER_STYLE.parse().unwrap();
        let path_field = catalog::field_by_id("paths.packages_path").unwrap();
        assert_eq!(
            catalog::current_json(&doc, path_field).as_str(),
            Some("/media/storage/packages/abs/packages")
        );
        assert!(
            catalog::prefer_current(&doc, path_field),
            "Enter must keep the in-file path, not the Suggested default"
        );
        assert!(doc_has_path(&doc, "paths.packages_path"));
        assert!(ramdisk_enabled_in_doc(&doc));
        assert!(
            missing_required_keys(&doc).is_empty(),
            "{:?}",
            missing_required_keys(&doc)
        );
        let repos = catalog::current_json(&doc, catalog::field_by_id("repositories").unwrap());
        assert_eq!(repos["default"].as_str(), Some("arch"));
        assert!(
            repos["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["name"].as_str() == Some("venomo"))
        );
        let threads = catalog::field_by_id("build.concurrent_compilations_limit").unwrap();
        assert_eq!(catalog::current_json(&doc, threads), serde_json::json!(2));
        let ignore = catalog::field_by_id("build.ignore_compilation_failures").unwrap();
        assert_eq!(catalog::current_json(&doc, ignore), serde_json::json!(true));
    }

    #[test]
    fn suggested_repos_json_is_object_that_validates() {
        let field = catalog::field_by_id("repositories").unwrap();
        let suggested = catalog::suggested_json(field);
        assert!(
            suggested.is_object(),
            "GUI Use suggested must send an object, not {suggested}"
        );
        catalog::validate_field(field, &suggested, None)
            .expect("suggested repositories must pass check");
        assert_eq!(suggested["default"].as_str(), Some("arch"));
        let names: Vec<&str> = suggested["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e.get("name").and_then(serde_json::Value::as_str))
            .collect();
        assert!(names.contains(&"arch"));
        assert!(names.contains(&"aur"));
        assert!(
            catalog::validate_field(field, &serde_json::json!("arch"), None).is_err(),
            "a default-name string must not be accepted as the repositories value"
        );
    }

    #[test]
    fn inline_tables_save_picked_values_and_keep_packages() {
        let mut doc: DocumentMut = INLINE_USER_STYLE.parse().unwrap();
        let path_field = catalog::field_by_id("paths.packages_path").unwrap();
        catalog::apply_answer(
            &mut doc,
            path_field,
            &serde_json::Value::String("$XDG_CACHE_HOME/abs/wizard-picked-packages".into()),
        )
        .unwrap();
        let env_field = catalog::field_by_id("build.default_environment").unwrap();
        catalog::apply_answer(
            &mut doc,
            env_field,
            &serde_json::Value::String("chroot".into()),
        )
        .unwrap();
        let threads = catalog::field_by_id("build.concurrent_compilations_limit").unwrap();
        catalog::apply_answer(&mut doc, threads, &serde_json::json!(3)).unwrap();
        let ram = catalog::field_by_id("ramdisk.size").unwrap();
        catalog::apply_answer(&mut doc, ram, &serde_json::Value::String("1G".into())).unwrap();

        let rendered = doc.to_string();
        assert!(
            rendered.contains("$XDG_CACHE_HOME/abs/wizard-picked-packages"),
            "picked packages_path missing:\n{rendered}"
        );
        assert!(
            rendered.contains("/media/storage/packages/abs/chroot"),
            "unedited path must stay:\n{rendered}"
        );
        assert!(
            rendered.contains("vim"),
            "packages section must be left in place:\n{rendered}"
        );
        assert!(
            rendered.contains("venomo"),
            "custom repo must stay:\n{rendered}"
        );

        let cfg = Config::from_toml_text(&rendered).expect("wizard output must validate");
        assert!(cfg.paths.packages_path.contains("wizard-picked-packages"));
        assert_eq!(cfg.build.default_environment, "chroot");
        assert_eq!(cfg.build.concurrent_compilations_limit, 3);
        assert_eq!(cfg.ramdisk.size, "1G");
        assert_eq!(
            cfg.paths.chroot_base_path,
            "/media/storage/packages/abs/chroot"
        );
        assert!(cfg.packages.contains_key("vim"));
        assert_eq!(
            cfg.repositories.get("venomo").map(String::as_str),
            Some("https://github.com/Ven0m0/PKG.git")
        );
        assert_eq!(
            cfg.repositories.get("default").map(String::as_str),
            Some("arch")
        );
    }

    #[test]
    fn table_mut_converts_inline_paths_without_dropping_keys() {
        let mut doc: DocumentMut = INLINE_USER_STYLE.parse().unwrap();
        assert!(doc.get("paths").and_then(|i| i.as_inline_table()).is_some());
        {
            let paths = table_mut(&mut doc, "paths");
            assert_eq!(
                get_str(paths, "packages_path", ""),
                "/media/storage/packages/abs/packages"
            );
            set_str(paths, "packages_path", "/media/storage/packages/abs/picked");
        }
        assert!(doc.get("paths").and_then(|i| i.as_table()).is_some());
        assert!(doc.get("paths").and_then(|i| i.as_inline_table()).is_none());
        let paths = table_ref(&doc, "paths").unwrap();
        assert_eq!(
            get_str(paths, "ready_made_packages_path", ""),
            "/media/storage/packages/abs/ready"
        );
        assert_eq!(
            get_str(paths, "packages_path", ""),
            "/media/storage/packages/abs/picked"
        );
    }
}
