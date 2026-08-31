//! Non-interactive JSON form / check / apply for the config wizard (absgui client).

use super::catalog::{self, FieldDef, FieldKind, PathPick};
use super::{load_wizard_source_quiet, write_wizard_file};
use crate::config::{Config, user_config_path};
use abs_i18n::t_or;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::io::{self, Read, Write};
use toml_edit::DocumentMut;

#[derive(Serialize)]
struct WizardForm {
    path: String,
    reconfigure: bool,
    steps: Vec<WizardStepJson>,
}

#[derive(Serialize)]
struct WizardStepJson {
    id: String,
    title: String,
    blurb: String,
    fields: Vec<WizardFieldJson>,
}

#[derive(Serialize)]
struct WizardFieldJson {
    id: String,
    kind: String,
    title: String,
    explanation: String,
    current: Value,
    suggested: Value,
    prefer_current: bool,
    optional: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<Vec<ChoiceJson>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    visible_if: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usize_min: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path_pick: Option<&'static str>,
}

#[derive(Serialize)]
struct ChoiceJson {
    value: String,
    label: String,
    help: String,
    suggested: bool,
}

#[derive(Deserialize)]
struct CheckRequest {
    id: String,
    value: Value,
    #[serde(default)]
    answers: Option<Map<String, Value>>,
}

#[derive(Deserialize)]
struct ApplyRequest {
    answers: Map<String, Value>,
}

pub fn run_form() {
    match build_form() {
        Ok(form) => print_json(&form),
        Err(e) => fail_json(&e),
    }
}

pub fn run_check() {
    match read_json::<CheckRequest>().and_then(check_request) {
        Ok(()) => print_json(&json!({"ok": true})),
        Err(e) => {
            print_json(&json!({"ok": false, "error": e}));
            std::process::exit(1);
        }
    }
}

pub fn run_apply() {
    match read_json::<ApplyRequest>().and_then(apply_request) {
        Ok(path) => print_json(&json!({"ok": true, "path": path})),
        Err(e) => fail_json(&e),
    }
}

fn build_form() -> Result<WizardForm, String> {
    let path = user_config_path();
    let (text, reconfigure) = load_wizard_source_quiet(&path)?;
    let doc: DocumentMut = text
        .parse()
        .map_err(|e| format!("Failed to parse starting config as TOML: {e}"))?;
    Ok(form_from_document(
        &path.display().to_string(),
        reconfigure,
        &doc,
    ))
}

fn form_from_document(path: &str, reconfigure: bool, doc: &DocumentMut) -> WizardForm {
    let steps = catalog::STEPS
        .iter()
        .map(|step| WizardStepJson {
            id: step.id.to_string(),
            title: t_or(&format!("wizard.step.{}.title", step.id), step.title).to_string(),
            blurb: t_or(&format!("wizard.step.{}.blurb", step.id), step.blurb).to_string(),
            fields: step.fields.iter().map(|f| field_json(doc, f)).collect(),
        })
        .collect();
    WizardForm {
        path: path.to_string(),
        reconfigure,
        steps,
    }
}

fn field_json(doc: &DocumentMut, field: &FieldDef) -> WizardFieldJson {
    let options = if field.choices.is_empty() {
        None
    } else {
        Some(
            field
                .choices
                .iter()
                .map(|c| {
                    let prefix = match field.id {
                        "build.default_environment" => "wizard.choice.env",
                        "build.global_cpu_threads_mode" => "wizard.choice.cpu",
                        "ramdisk.zram" => "wizard.choice.zram",
                        _ => "wizard.choice",
                    };
                    ChoiceJson {
                        value: c.value.to_string(),
                        label: t_or(&format!("{prefix}.{}.label", c.value), c.label).to_string(),
                        help: t_or(&format!("{prefix}.{}.help", c.value), c.help).to_string(),
                        suggested: c.suggested,
                    }
                })
                .collect(),
        )
    };
    let usize_min = match field.kind {
        FieldKind::Usize | FieldKind::OptionalUsize => Some(field.usize_min),
        _ => None,
    };
    let path_pick = match field.path_pick {
        PathPick::None => None,
        PathPick::Folder => Some("folder"),
        PathPick::File => Some("file"),
    };
    WizardFieldJson {
        id: field.id.to_string(),
        kind: catalog::kind_name(field.kind).to_string(),
        title: t_or(&format!("wizard.field.{}.title", field.id), field.title).to_string(),
        explanation: t_or(
            &format!("wizard.field.{}.explanation", field.id),
            field.explanation,
        )
        .to_string(),
        current: catalog::current_json(doc, field),
        suggested: catalog::suggested_json(field),
        prefer_current: catalog::prefer_current(doc, field),
        optional: field.optional,
        options,
        visible_if: catalog::visible_if_json(field.visible_if),
        usize_min,
        path_pick,
    }
}

fn check_request(req: CheckRequest) -> Result<(), String> {
    let field =
        catalog::field_by_id(&req.id).ok_or_else(|| format!("unknown wizard field {}", req.id))?;
    catalog::validate_field(field, &req.value, req.answers.as_ref())
}

pub(super) fn apply_answers_and_write(
    path: &std::path::Path,
    source_text: &str,
    answers: &Map<String, Value>,
) -> Result<(), String> {
    let mut doc: DocumentMut = source_text
        .parse()
        .map_err(|e| format!("Failed to parse starting config as TOML: {e}"))?;
    catalog::apply_answers(&mut doc, answers)?;
    let rendered = crate::toml_pretty::render_human_toml(&mut doc);
    Config::from_toml_text(&rendered)
        .map_err(|e| format!("The answers do not produce a valid config: {e}"))?;
    write_wizard_file(path, &rendered)?;
    Ok(())
}

fn apply_request(req: ApplyRequest) -> Result<String, String> {
    let path = user_config_path();
    let (text, _) = load_wizard_source_quiet(&path)?;
    apply_answers_and_write(&path, &text, &req.answers)?;
    Ok(path.display().to_string())
}

fn read_json<T: serde::de::DeserializeOwned>() -> Result<T, String> {
    let mut buf = String::new();
    io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| format!("read stdin: {e}"))?;
    serde_json::from_str(&buf).map_err(|e| format!("parse JSON: {e}"))
}

fn print_json(value: &impl Serialize) {
    let mut stdout = io::stdout();
    serde_json::to_writer(&mut stdout, value).expect("write JSON");
    let _ = writeln!(stdout);
}

fn fail_json(err: &str) -> ! {
    print_json(&json!({"ok": false, "error": err}));
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn form_includes_every_catalog_id_and_reads_inline_tables() {
        // GUI current values depend on table_ref() so inline `paths = { ... }` is visible.
        let doc: DocumentMut = r#"
paths = { packages_path = "/tmp/abs-packages", chroot_base_path = "/tmp/abs-chroot", ready_made_packages_path = "/tmp/abs-ready" }
"#
        .parse()
        .unwrap();
        let form = form_from_document("/tmp/abs.toml", true, &doc);
        let ids: Vec<&str> = form
            .steps
            .iter()
            .flat_map(|s| s.fields.iter().map(|f| f.id.as_str()))
            .collect();
        for field in catalog::all_fields() {
            assert!(ids.contains(&field.id), "form missing {}", field.id);
        }
        let pkg = form
            .steps
            .iter()
            .flat_map(|s| &s.fields)
            .find(|f| f.id == "paths.packages_path")
            .expect("packages_path");
        assert_eq!(pkg.current, json!("/tmp/abs-packages"));
        assert_eq!(pkg.path_pick, Some("folder"));
        let repos = form
            .steps
            .iter()
            .flat_map(|s| &s.fields)
            .find(|f| f.id == "repositories")
            .expect("repositories");
        assert!(
            repos.suggested.is_object(),
            "suggested repositories must be {{default, entries}}, not {}",
            repos.suggested
        );
        catalog::validate_field(
            catalog::field_by_id("repositories").unwrap(),
            &repos.suggested,
            None,
        )
        .expect("form suggested repositories must pass check");
    }

    #[test]
    fn check_rejects_root_path_and_piped_command() {
        let path = catalog::field_by_id("paths.packages_path").unwrap();
        assert!(catalog::validate_field(path, &json!("/"), None).is_err());
        let cmd = catalog::field_by_id("system_update.command_to_perform_system_update").unwrap();
        assert!(catalog::validate_field(cmd, &json!("pacman | x"), None).is_err());
        assert!(catalog::validate_field(cmd, &json!("sudo pacman -Syu"), None).is_ok());
    }

    #[test]
    fn apply_writes_temp_file_not_user_config() {
        let dir = std::env::temp_dir().join(format!(
            "abs-wizard-apply-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("abs.toml");
        let source = crate::config::example_config_text();
        let mut answers = Map::new();
        answers.insert(
            "paths.packages_path".into(),
            json!("$XDG_CACHE_HOME/abs/gui-wizard-test"),
        );
        apply_answers_and_write(&path, source, &answers).expect("apply temp");
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("$XDG_CACHE_HOME/abs/gui-wizard-test"));
        assert!(
            written.contains("[paths]"),
            "wizard apply must write [paths] headers, not inline tables:\n{written}"
        );
        assert!(
            !written.contains("paths = {"),
            "wizard apply must not write JSON-like inline tables:\n{written}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
