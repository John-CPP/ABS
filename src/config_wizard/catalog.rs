//! Shared wizard field catalog. TTY prompts and JSON form/check/apply all walk this.

#![allow(dead_code)] // Some helpers are JSON-form only; TTY walks STEPS.

use super::{
    KNOWN_REPOS, SUGGEST_CHROOT_PATH, SUGGEST_IGNORE_FLAG, SUGGEST_INSTALL_PATH, SUGGEST_MODE,
    SUGGEST_MOUNT, SUGGEST_NO_REFRESH_CMD, SUGGEST_PACKAGES_PATH, SUGGEST_READY_PATH, SUGGEST_SIZE,
    SUGGEST_SYNC_CMD, SUGGEST_UPDATE_CMD, SkipAfterEdit, apply_repo_list, doc_has_path, get_bool,
    get_optional_str, get_optional_usize, get_root_bool, get_str, get_string_array, get_usize,
    ramdisk_enabled_in_doc, repo_entries, set_bool, set_optional_str, set_optional_usize,
    set_root_bool, set_root_str, set_str, set_string_array, set_usize, suggested_default_name,
    table_mut, table_ref, validate_command, validate_ignore_flag, validate_mount_point,
    validate_repo_name, validate_repo_url, validate_user_path,
};
use serde_json::{Map, Value, json};
use toml_edit::DocumentMut;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Bool,
    Choice,
    Path,
    Command,
    String,
    Usize,
    OptionalUsize,
    OptionalPath,
    OptionalCommand,
    StringList,
    SkipAfterList,
    Repos,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VisibleIf {
    Always,
    RamdiskEnabled,
    PacmanFalse,
    CpuFlexible,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathPick {
    None,
    Folder,
    File,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Suggested {
    None,
    Str(&'static str),
    Bool(bool),
    Usize(usize),
}

#[derive(Clone, Copy, Debug)]
pub struct ChoiceDef {
    pub value: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub suggested: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct FieldDef {
    pub id: &'static str,
    pub kind: FieldKind,
    pub title: &'static str,
    pub explanation: &'static str,
    pub suggested: Suggested,
    pub optional: bool,
    pub in_gap_fill: bool,
    pub choices: &'static [ChoiceDef],
    pub usize_min: usize,
    pub visible_if: VisibleIf,
    pub path_pick: PathPick,
}

#[derive(Clone, Copy, Debug)]
pub struct StepDef {
    pub id: &'static str,
    pub title: &'static str,
    pub blurb: &'static str,
    pub fields: &'static [FieldDef],
}

pub fn gap_key(field: &FieldDef) -> &'static str {
    if field.kind == FieldKind::Repos {
        "repositories.default"
    } else {
        field.id
    }
}

pub fn display_title_for_gap_key(key: &str) -> String {
    all_fields()
        .find(|f| gap_key(f) == key || f.id == key)
        .map(|f| abs_i18n::t_or(&format!("wizard.field.{}.title", f.id), f.title).to_string())
        .unwrap_or_else(|| key.to_string())
}

pub fn all_fields() -> impl Iterator<Item = &'static FieldDef> {
    STEPS.iter().flat_map(|s| s.fields.iter())
}

pub fn field_by_id(id: &str) -> Option<&'static FieldDef> {
    all_fields()
        .find(|f| f.id == id || (f.kind == FieldKind::Repos && id == "repositories.default"))
}

pub fn is_visible_in_doc(field: &FieldDef, doc: &DocumentMut) -> bool {
    match field.visible_if {
        VisibleIf::Always => true,
        VisibleIf::RamdiskEnabled => ramdisk_enabled_in_doc(doc),
        VisibleIf::PacmanFalse => !get_root_bool(doc, "self_update_use_pacman", true),
        VisibleIf::CpuFlexible => table_ref(doc, "build")
            .map(|t| get_str(t, "global_cpu_threads_mode", "strict") == "flexible")
            .unwrap_or(false),
    }
}

pub fn is_visible_in_answers(field: &FieldDef, answers: &Map<String, Value>) -> bool {
    match field.visible_if {
        VisibleIf::Always => true,
        VisibleIf::RamdiskEnabled => answers
            .get("ramdisk.enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        VisibleIf::PacmanFalse => !answers
            .get("self_update_use_pacman")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        VisibleIf::CpuFlexible => {
            answers
                .get("build.global_cpu_threads_mode")
                .and_then(Value::as_str)
                .unwrap_or("strict")
                == "flexible"
        }
    }
}

pub fn visible_if_json(v: VisibleIf) -> Option<Value> {
    match v {
        VisibleIf::Always => None,
        VisibleIf::RamdiskEnabled => Some(json!({"field": "ramdisk.enabled", "equals": true})),
        VisibleIf::PacmanFalse => Some(json!({"field": "self_update_use_pacman", "equals": false})),
        VisibleIf::CpuFlexible => {
            Some(json!({"field": "build.global_cpu_threads_mode", "equals": "flexible"}))
        }
    }
}

pub fn kind_name(kind: FieldKind) -> &'static str {
    match kind {
        FieldKind::Bool => "bool",
        FieldKind::Choice => "choice",
        FieldKind::Path => "path",
        FieldKind::Command => "command",
        FieldKind::String => "string",
        FieldKind::Usize => "usize",
        FieldKind::OptionalUsize => "optional_usize",
        FieldKind::OptionalPath => "optional_path",
        FieldKind::OptionalCommand => "optional_command",
        FieldKind::StringList => "string_list",
        FieldKind::SkipAfterList => "skip_after_list",
        FieldKind::Repos => "repos",
    }
}

pub fn suggested_json(field: &FieldDef) -> Value {
    if field.kind == FieldKind::Repos {
        return suggested_repos_json();
    }
    match field.suggested {
        Suggested::None => Value::Null,
        Suggested::Str(v) => Value::String(v.to_string()),
        Suggested::Bool(v) => Value::Bool(v),
        Suggested::Usize(v) => json!(v),
    }
}

fn suggested_repos_json() -> Value {
    let entries: Vec<(String, String)> = KNOWN_REPOS
        .iter()
        .map(|(k, u)| ((*k).to_string(), (*u).to_string()))
        .collect();
    repos_json(&entries, &suggested_default_name(&entries))
}

fn repos_json(entries: &[(String, String)], default: &str) -> Value {
    json!({
        "default": default,
        "entries": entries.iter().map(|(name, url)| json!({"name": name, "url": url})).collect::<Vec<_>>(),
    })
}

pub fn current_json(doc: &DocumentMut, field: &FieldDef) -> Value {
    if field.id == "install_absgui" && !super::doc_has_path(doc, field.id) {
        if let Some(v) = crate::config::load_install_absgui_pref() {
            return Value::Bool(v);
        }
    }
    match field.kind {
        FieldKind::Bool => Value::Bool(dotted_bool(doc, field.id, suggested_bool(field.suggested))),
        FieldKind::Choice | FieldKind::Path | FieldKind::Command | FieldKind::String => {
            Value::String(dotted_str(
                doc,
                field.id,
                suggested_str(field.suggested).unwrap_or(""),
            ))
        }
        FieldKind::Usize => json!(dotted_usize(
            doc,
            field.id,
            suggested_usize(field.suggested).unwrap_or(0)
        )),
        FieldKind::OptionalUsize => match dotted_opt_usize(doc, field.id) {
            Some(n) => json!(n),
            None => Value::Null,
        },
        FieldKind::OptionalPath | FieldKind::OptionalCommand => {
            match dotted_opt_str(doc, field.id) {
                Some(s) => Value::String(s),
                None => Value::Null,
            }
        }
        FieldKind::StringList => Value::Array(
            dotted_string_list(doc, field.id)
                .into_iter()
                .map(Value::String)
                .collect(),
        ),
        FieldKind::SkipAfterList => {
            if !doc_has_path(doc, field.id) {
                Value::Null
            } else {
                Value::Array(
                    dotted_string_list(doc, field.id)
                        .into_iter()
                        .map(Value::String)
                        .collect(),
                )
            }
        }
        FieldKind::Repos => {
            let repos_table = table_ref(doc, "repositories");
            let mut entries = repos_table.map(repo_entries).unwrap_or_default();
            if entries.is_empty() {
                entries = KNOWN_REPOS
                    .iter()
                    .map(|(k, u)| ((*k).to_string(), (*u).to_string()))
                    .collect();
            }
            let default = repos_table
                .map(|t| get_str(t, "default", "arch"))
                .unwrap_or_else(|| suggested_default_name(&entries));
            repos_json(&entries, &default)
        }
    }
}

pub fn prefer_current(doc: &DocumentMut, field: &FieldDef) -> bool {
    if super::doc_has_path(doc, gap_key(field)) {
        return true;
    }
    field.id == "install_absgui" && crate::config::load_install_absgui_pref().is_some()
}

pub fn validate_field(
    field: &FieldDef,
    value: &Value,
    answers: Option<&Map<String, Value>>,
) -> Result<(), String> {
    match field.kind {
        FieldKind::Bool => {
            as_bool(value)?;
            Ok(())
        }
        FieldKind::Choice => {
            let s = as_string(value)?;
            if field.choices.is_empty() {
                return Ok(());
            }
            if field
                .choices
                .iter()
                .any(|c| c.value.eq_ignore_ascii_case(&s))
            {
                Ok(())
            } else {
                Err(format!(
                    "Please pick one of: {}",
                    field
                        .choices
                        .iter()
                        .map(|c| c.label)
                        .collect::<Vec<_>>()
                        .join("; ")
                ))
            }
        }
        FieldKind::Path => validate_user_path(field.id, &as_string(value)?),
        FieldKind::Command => validate_command(&as_string(value)?),
        FieldKind::String => validate_string_field(field, &as_string(value)?),
        FieldKind::Usize => {
            let n = as_usize(value)?;
            if n < field.usize_min {
                return Err(format!(
                    "Please enter a number that is at least {}",
                    field.usize_min
                ));
            }
            Ok(())
        }
        FieldKind::OptionalUsize => {
            let n = as_opt_usize(value)?;
            if let Some(n) = n
                && n < 1
            {
                return Err("Please enter a number that is at least 1".into());
            }
            if field.id == "build.maximum_cpu_threads_cap" {
                let soft = answers.and_then(|a| {
                    a.get("build.global_cpu_threads_cap")
                        .and_then(|v| as_opt_usize(v).ok())
                        .flatten()
                });
                validate_cpu_caps(soft, n)?;
            }
            Ok(())
        }
        FieldKind::OptionalPath => {
            if let Some(s) = as_opt_string(value)? {
                validate_user_path(field.id, &s)?;
            }
            Ok(())
        }
        FieldKind::OptionalCommand => {
            if let Some(s) = as_opt_string(value)? {
                validate_command(&s)?;
            }
            Ok(())
        }
        FieldKind::StringList => {
            as_string_list(value)?;
            Ok(())
        }
        FieldKind::SkipAfterList => {
            if !value.is_null() {
                as_string_list(value)?;
            }
            Ok(())
        }
        FieldKind::Repos => validate_repos_value(value),
    }
}

pub fn apply_answer(doc: &mut DocumentMut, field: &FieldDef, value: &Value) -> Result<(), String> {
    validate_field(field, value, None)?;
    match field.kind {
        FieldKind::Bool => dotted_set_bool(doc, field.id, as_bool(value)?),
        FieldKind::Choice | FieldKind::Path | FieldKind::Command | FieldKind::String => {
            dotted_set_str(doc, field.id, &as_string(value)?);
        }
        FieldKind::Usize => dotted_set_usize(doc, field.id, as_usize(value)?),
        FieldKind::OptionalUsize => dotted_set_opt_usize(doc, field.id, as_opt_usize(value)?),
        FieldKind::OptionalPath | FieldKind::OptionalCommand => {
            dotted_set_opt_str(doc, field.id, as_opt_string(value)?.as_deref());
        }
        FieldKind::StringList => dotted_set_string_list(doc, field.id, &as_string_list(value)?),
        FieldKind::SkipAfterList => {
            if value.is_null() {
                remove_dotted(doc, field.id);
            } else {
                dotted_set_string_list(doc, field.id, &as_string_list(value)?);
            }
        }
        FieldKind::Repos => apply_repos_value(doc, value)?,
    }
    Ok(())
}

pub fn apply_answers(doc: &mut DocumentMut, answers: &Map<String, Value>) -> Result<(), String> {
    for field in all_fields() {
        let Some(value) = answers.get(field.id) else {
            continue;
        };
        if !is_visible_in_answers(field, answers) {
            continue;
        }
        apply_answer(doc, field, value).map_err(|e| format!("{}: {e}", field.id))?;
    }
    if let (Some(soft_v), Some(hard_v)) = (
        answers.get("build.global_cpu_threads_cap"),
        answers.get("build.maximum_cpu_threads_cap"),
    ) {
        let mode = answers
            .get("build.global_cpu_threads_mode")
            .and_then(Value::as_str)
            .unwrap_or("strict");
        if mode == "flexible" {
            validate_cpu_caps(as_opt_usize(soft_v)?, as_opt_usize(hard_v)?)?;
        }
    }
    Ok(())
}

fn validate_cpu_caps(soft: Option<usize>, hard: Option<usize>) -> Result<(), String> {
    if let (Some(soft), Some(hard)) = (soft, hard)
        && hard < soft
    {
        return Err(format!(
            "The hard CPU maximum ({hard}) must be at least as high as the soft limit ({soft})"
        ));
    }
    Ok(())
}

fn validate_string_field(field: &FieldDef, s: &str) -> Result<(), String> {
    match field.id {
        "ramdisk.mount_point" => validate_mount_point(s),
        "ramdisk.size" => {
            crate::ramdisk::validate_ramdisk_size(s).map_err(|_| {
                "Use a size like 16G or 50%. Start with a number. No spaces or commas.".to_string()
            })?;
            if let Ok(total) = crate::ramdisk::mem_total_bytes() {
                crate::ramdisk::ensure_ramdisk_size_fits_ram(s, total).map_err(|_| {
                    "That is more memory than this computer has. Use a smaller size (for example 16G or 50%).".to_string()
                })?;
            }
            Ok(())
        }
        "ramdisk.mode" => crate::ramdisk::validate_ramdisk_mode(s).map_err(|_| {
            "Use a 3- or 4-digit permission code like 0755. Press Enter to keep the suggestion if you are unsure.".into()
        }),
        "system_update.ignore_flag" => validate_ignore_flag(s),
        _ => Ok(()),
    }
}

fn validate_repos_value(value: &Value) -> Result<(), String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "repositories must be an object".to_string())?;
    let entries = obj
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| "repositories.entries must be an array".to_string())?;
    if entries.is_empty() {
        return Err("Please keep at least one repository.".into());
    }
    let mut names = Vec::new();
    for entry in entries {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "repository name is required".to_string())?;
        let url = entry
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| "repository URL is required".to_string())?;
        validate_repo_name(name)?;
        validate_repo_url(url)?;
        names.push(name.to_string());
    }
    let default = obj
        .get("default")
        .and_then(Value::as_str)
        .ok_or_else(|| "repositories.default is required".to_string())?;
    if !names.iter().any(|n| n == default) {
        return Err(format!(
            "The default “{default}” is not in the list. Pick one of the repositories above."
        ));
    }
    Ok(())
}

fn apply_repos_value(doc: &mut DocumentMut, value: &Value) -> Result<(), String> {
    validate_repos_value(value)?;
    let obj = value.as_object().unwrap();
    let entries: Vec<(String, String)> = obj["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["name"].as_str().unwrap().to_string(),
                e["url"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let default = obj["default"].as_str().unwrap();
    apply_repo_list(table_mut(doc, "repositories"), &entries, default);
    Ok(())
}

fn suggested_str(s: Suggested) -> Option<&'static str> {
    match s {
        Suggested::Str(v) => Some(v),
        _ => None,
    }
}

fn suggested_bool(s: Suggested) -> bool {
    match s {
        Suggested::Bool(v) => v,
        _ => false,
    }
}

fn suggested_usize(s: Suggested) -> Option<usize> {
    match s {
        Suggested::Usize(v) => Some(v),
        _ => None,
    }
}

fn split_id(id: &str) -> (Option<&str>, &str) {
    match id.split_once('.') {
        Some((t, k)) => (Some(t), k),
        None => (None, id),
    }
}

fn dotted_bool(doc: &DocumentMut, id: &str, default: bool) -> bool {
    let (table, key) = split_id(id);
    match table {
        None => get_root_bool(doc, key, default),
        Some(t) => table_ref(doc, t)
            .map(|tbl| get_bool(tbl, key, default))
            .unwrap_or(default),
    }
}

fn dotted_str(doc: &DocumentMut, id: &str, default: &str) -> String {
    let (table, key) = split_id(id);
    match table {
        None => super::get_root_str(doc, key, default),
        Some(t) => table_ref(doc, t)
            .map(|tbl| get_str(tbl, key, default))
            .unwrap_or_else(|| default.to_string()),
    }
}

fn dotted_usize(doc: &DocumentMut, id: &str, default: usize) -> usize {
    let (table, key) = split_id(id);
    match table {
        None => get_usize(doc.as_table(), key, default),
        Some(t) => table_ref(doc, t)
            .map(|tbl| get_usize(tbl, key, default))
            .unwrap_or(default),
    }
}

fn dotted_opt_usize(doc: &DocumentMut, id: &str) -> Option<usize> {
    let (table, key) = split_id(id);
    match table {
        None => get_optional_usize(doc.as_table(), key),
        Some(t) => table_ref(doc, t).and_then(|tbl| get_optional_usize(tbl, key)),
    }
}

fn dotted_opt_str(doc: &DocumentMut, id: &str) -> Option<String> {
    let (table, key) = split_id(id);
    match table {
        None => get_optional_str(doc.as_table(), key),
        Some(t) => table_ref(doc, t).and_then(|tbl| get_optional_str(tbl, key)),
    }
}

fn dotted_string_list(doc: &DocumentMut, id: &str) -> Vec<String> {
    let (table, key) = split_id(id);
    match table {
        None => get_string_array(doc.as_table(), key),
        Some(t) => table_ref(doc, t)
            .map(|tbl| get_string_array(tbl, key))
            .unwrap_or_default(),
    }
}

fn dotted_set_bool(doc: &mut DocumentMut, id: &str, value: bool) {
    let (table, key) = split_id(id);
    match table {
        None => set_root_bool(doc, key, value),
        Some(t) => set_bool(table_mut(doc, t), key, value),
    }
}

fn dotted_set_str(doc: &mut DocumentMut, id: &str, value: &str) {
    let (table, key) = split_id(id);
    match table {
        None => set_root_str(doc, key, value),
        Some(t) => set_str(table_mut(doc, t), key, value),
    }
}

fn dotted_set_usize(doc: &mut DocumentMut, id: &str, value: usize) {
    let (table, key) = split_id(id);
    match table {
        None => set_usize(doc.as_table_mut(), key, value),
        Some(t) => set_usize(table_mut(doc, t), key, value),
    }
}

fn dotted_set_opt_usize(doc: &mut DocumentMut, id: &str, value: Option<usize>) {
    let (table, key) = split_id(id);
    match table {
        None => set_optional_usize(doc.as_table_mut(), key, value),
        Some(t) => set_optional_usize(table_mut(doc, t), key, value),
    }
}

fn dotted_set_opt_str(doc: &mut DocumentMut, id: &str, value: Option<&str>) {
    let (table, key) = split_id(id);
    match table {
        None => set_optional_str(doc.as_table_mut(), key, value),
        Some(t) => set_optional_str(table_mut(doc, t), key, value),
    }
}

fn dotted_set_string_list(doc: &mut DocumentMut, id: &str, items: &[String]) {
    let (table, key) = split_id(id);
    match table {
        None => set_string_array(doc.as_table_mut(), key, items),
        Some(t) => set_string_array(table_mut(doc, t), key, items),
    }
}

fn remove_dotted(doc: &mut DocumentMut, id: &str) {
    let (table, key) = split_id(id);
    match table {
        None => {
            doc.as_table_mut().remove(key);
        }
        Some(t) => {
            if let Some(tbl) = doc.get_mut(t).and_then(|i| i.as_table_like_mut()) {
                tbl.remove(key);
            }
        }
    }
}

pub fn as_bool(value: &Value) -> Result<bool, String> {
    if let Some(b) = value.as_bool() {
        return Ok(b);
    }
    if let Some(s) = value.as_str() {
        return match s.trim().to_ascii_lowercase().as_str() {
            "yes" | "true" | "1" => Ok(true),
            "no" | "false" | "0" => Ok(false),
            _ => Err("enter yes or no".into()),
        };
    }
    Err("expected a boolean".into())
}

pub fn as_string(value: &Value) -> Result<String, String> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(if *b { "true".into() } else { "false".into() }),
        _ => Err("expected a string".into()),
    }
}

pub fn as_opt_string(value: &Value) -> Result<Option<String>, String> {
    if value.is_null() {
        return Ok(None);
    }
    let s = as_string(value)?;
    if s == "-" || s.eq_ignore_ascii_case("none") || s.eq_ignore_ascii_case("unset") || s.is_empty()
    {
        return Ok(None);
    }
    Ok(Some(s))
}

pub fn as_usize(value: &Value) -> Result<usize, String> {
    if let Some(n) = value.as_u64() {
        return usize::try_from(n).map_err(|_| "number is too large".into());
    }
    if let Some(s) = value.as_str() {
        return s
            .trim()
            .parse::<usize>()
            .map_err(|_| "enter a whole number".to_string());
    }
    Err("enter a whole number".into())
}

pub fn as_opt_usize(value: &Value) -> Result<Option<usize>, String> {
    if value.is_null() {
        return Ok(None);
    }
    if let Some(s) = value.as_str()
        && (s.is_empty()
            || s == "-"
            || s.eq_ignore_ascii_case("none")
            || s.eq_ignore_ascii_case("unset"))
    {
        return Ok(None);
    }
    Ok(Some(as_usize(value)?))
}

pub fn as_string_list(value: &Value) -> Result<Vec<String>, String> {
    if let Some(arr) = value.as_array() {
        let mut out = Vec::new();
        for item in arr {
            let s = item
                .as_str()
                .ok_or_else(|| "list items must be strings".to_string())?
                .trim();
            if !s.is_empty() {
                out.push(s.to_string());
            }
        }
        out.sort();
        out.dedup();
        return Ok(out);
    }
    if let Some(s) = value.as_str() {
        return Ok(super::parse_name_list(s));
    }
    Err("expected a list of names".into())
}

pub fn skip_after_from_value(value: &Value) -> Result<SkipAfterEdit, String> {
    if value.is_null() {
        return Ok(SkipAfterEdit::Unset);
    }
    Ok(SkipAfterEdit::Set(as_string_list(value)?))
}

const ENV_CHOICES: &[ChoiceDef] = &[
    ChoiceDef {
        value: "local",
        label: "On this computer (simpler and faster)",
        help: "Uses the compilers and libraries already installed here. This is what most people want.",
        suggested: true,
    },
    ChoiceDef {
        value: "chroot",
        label: "In a clean mini-system (more isolated)",
        help: "The build cannot pick up extra packages from your real install.\n\
               Needs more disk space and the Arch package named `devtools`.",
        suggested: false,
    },
];

const CPU_MODE_CHOICES: &[ChoiceDef] = &[
    ChoiceDef {
        value: "strict",
        label: "Never go over the limit",
        help: "If another compile would use too many CPU cores, ABS waits. The computer stays more responsive.",
        suggested: true,
    },
    ChoiceDef {
        value: "flexible",
        label: "Soft limit, with an optional hard maximum",
        help: "ABS tries to stay under a soft limit, but may go a little over up to a hard maximum you can set.",
        suggested: false,
    },
];

const ZRAM_CHOICES: &[ChoiceDef] = &[
    ChoiceDef {
        value: "full",
        label: "As much as remaining RAM allows (full)",
        help: "Always sets up ABS zram using almost all remaining MemAvailable as the compressed-RAM cap, with disksize at 4× that. Used for compiles and system updates, not only converting profiles. Unused zram is a cap, not reserved up front.",
        suggested: true,
    },
    ChoiceDef {
        value: "off",
        label: "Do not add ABS zram (off)",
        help: "Never bring up the temporary abs-pgo zram device. Use this if you already have enough RAM or your own swap.",
        suggested: false,
    },
];

pub static STEPS: &[StepDef] = &[
    StepDef {
        id: "absgui",
        title: "AbsGui (the windowed app)",
        blurb: "AbsGui is an optional clickable window for the same settings, plus system updates and extra kernel compile tools.\n\
         Your answer is remembered when you reinstall or update ABS.",
        fields: ABSGUI_FIELDS,
    },
    StepDef {
        id: "paths",
        title: "Folders ABS will use",
        blurb: "These are folders on your computer. ABS creates them if they do not exist yet.\n\
         You can use $HOME (your home folder). Do not point these at your whole home folder or at system folders like /tmp.\n\
         If you have several computers, put the finished-packages folder on a shared drive so only one PC has to compile.",
        fields: PATH_FIELDS,
    },
    StepDef {
        id: "build",
        title: "How ABS compiles",
        blurb: "These defaults apply to every package. You can still change one package later in AbsGui or with `abs --wizard`.",
        fields: BUILD_FIELDS,
    },
    StepDef {
        id: "cpu",
        title: "How hard may ABS push the CPU?",
        blurb: "If several packages compile at once, ABS can limit how many CPU cores they use together so the computer stays usable for other work.",
        fields: CPU_FIELDS,
    },
    StepDef {
        id: "system_update",
        title: "Updating the rest of your system",
        blurb: "These are the commands ABS runs when you ask it to refresh or upgrade the system (for example `abs -U`).\n\
         Type a normal command; no pipes (|), &&, or extra shell tricks.",
        fields: SYSTEM_UPDATE_FIELDS,
    },
    StepDef {
        id: "repositories",
        title: "Where ABS downloads build recipes",
        blurb: "A repository here is a git website with build recipes (the instructions used to compile a package).\n\
         You can keep several, add your own, or remove ones you do not use.\n\
         When you type `abs mesa` without naming a repository, ABS uses the default you pick.",
        fields: REPO_FIELDS,
    },
    StepDef {
        id: "ramdisk",
        title: "Compile in RAM? (optional speed-up)",
        blurb: "A ramdisk is a temporary disk that lives in memory instead of on your SSD. Compiling there is often faster and wears the disk less,\n\
         but it needs free RAM and your password (sudo) to set up. The folders from the earlier step stay the real, permanent locations.\n\
         The ramdisk is created only when a build needs it, not when ABS starts.",
        fields: RAMDISK_FIELDS,
    },
    StepDef {
        id: "self_update",
        title: "Keeping ABS itself up to date",
        blurb: "ABS can check GitHub for a newer version of itself. If you install with pacman, the built packages go in the finished-packages folder so other computers can reuse them.",
        fields: SELF_UPDATE_FIELDS,
    },
    StepDef {
        id: "package_lists",
        title: "Package lists (optional)",
        blurb: "You can leave these empty. Press Enter to keep the current list (empty on a new config).\n\
         Type names separated by commas or spaces. Type '-' to clear a list.\n\
         You can also edit these later with `abs --wizard` or AbsGui.",
        fields: PACKAGE_LIST_FIELDS,
    },
];

const ABSGUI_FIELDS: &[FieldDef] = &[FieldDef {
    id: "install_absgui",
    kind: FieldKind::Bool,
    title: "Do you want AbsGui (the windowed app)?",
    explanation: "AbsGui is a clickable window for the same settings, plus system updates and extra kernel compile tools.\n\
         Yes also keeps AbsGui when ABS updates itself. No keeps the command-line tools only.\n\
         You can change this later.",
    suggested: Suggested::Bool(true),
    optional: false,
    in_gap_fill: true,
    choices: &[],
    usize_min: 0,
    visible_if: VisibleIf::Always,
    path_pick: PathPick::None,
}];

const PATH_FIELDS: &[FieldDef] = &[
    FieldDef {
        id: "paths.packages_path",
        kind: FieldKind::Path,
        title: "Where should downloaded source code live?",
        explanation: "Before compiling, ABS downloads each package’s source files into this folder.\n\
             Example: if you build mesa, that download lives here.\n\
             Finished installable packages go in a different folder (asked next).",
        suggested: Suggested::Str(SUGGEST_PACKAGES_PATH),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::Folder,
    },
    FieldDef {
        id: "paths.chroot_base_path",
        kind: FieldKind::Path,
        title: "Where should the isolated build environment live?",
        explanation: "If you later choose “clean mini-system”, ABS compiles inside a temporary copy of Linux so your real install stays untouched.\n\
             You only need this folder for that option. ABS still asks now so the setting is ready.",
        suggested: Suggested::Str(SUGGEST_CHROOT_PATH),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::Folder,
    },
    FieldDef {
        id: "paths.ready_made_packages_path",
        kind: FieldKind::Path,
        title: "Where should finished packages be saved?",
        explanation: "After a successful compile, ABS stores the installable package files here.\n\
             If several computers share this folder, only one needs to compile; the others can install from here.\n\
             Do not use the folder where pacman stores its own downloaded packages.",
        suggested: Suggested::Str(SUGGEST_READY_PATH),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::Folder,
    },
    FieldDef {
        id: "paths.chroot_makepkg_conf",
        kind: FieldKind::OptionalPath,
        title: "Custom compiler settings file for isolated builds? (optional)",
        explanation: "Advanced. Only if you already have a special compiler settings file (often named makepkg.conf) for isolated builds.\n\
             Leave empty unless you already have such a file. Press Enter to keep the current value, or type '-' to clear.",
        suggested: Suggested::None,
        optional: true,
        in_gap_fill: false,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::File,
    },
];

const BUILD_FIELDS: &[FieldDef] = &[
    FieldDef {
        id: "build.default_environment",
        kind: FieldKind::Choice,
        title: "Where should packages be compiled?",
        explanation: "This is the default for every package. “On this computer” is simpler and faster. “Clean mini-system” is more isolated but needs extra disk space.",
        suggested: Suggested::Str("local"),
        optional: false,
        in_gap_fill: true,
        choices: ENV_CHOICES,
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "build.system_update_first",
        kind: FieldKind::Bool,
        title: "Update the system before compiling?",
        explanation: "Yes runs a system update first, then compiles. That avoids broken programs when a library on disk does not match what you just built. Recommended for most people.",
        suggested: Suggested::Bool(true),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "build.ignore_compilation_failures",
        kind: FieldKind::Bool,
        title: "If one package fails, keep compiling the others?",
        explanation: "Yes continues with the remaining packages. No stops the whole run so you can fix the failure first. No is safer if you are unsure.",
        suggested: Suggested::Bool(false),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "build.compile_first_install_after",
        kind: FieldKind::Bool,
        title: "Compile everything first, then install?",
        explanation: "Yes finishes all compiles, then asks you to install. Handy if you want to leave the computer compiling overnight without answering questions. No may ask you to install after each package.",
        suggested: Suggested::Bool(false),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "build.clean_install_by_default",
        kind: FieldKind::Bool,
        title: "Start each compile from a clean folder?",
        explanation: "Yes deletes leftover temporary build folders before compiling, so old files cannot confuse a new build. A bit slower. No reuses those folders when possible.",
        suggested: Suggested::Bool(false),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "build.ignore_already_made_packages",
        kind: FieldKind::Bool,
        title: "Always recompile, even if a package file already exists?",
        explanation: "No (recommended) reuses a finished package of the same version and skips compiling. Choose No if other computers should reuse packages built on this machine.\n\
             Yes always compiles again. You can still force one rebuild later with `abs -n`.",
        suggested: Suggested::Bool(false),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "build.concurrent_repos_downloads_limit",
        kind: FieldKind::Usize,
        title: "How many packages may download at once?",
        explanation: "Higher numbers finish downloads faster on a good internet connection. Start with the suggestion if you are unsure.",
        suggested: Suggested::Usize(10),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 1,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "build.concurrent_compilations_limit",
        kind: FieldKind::Usize,
        title: "How many packages may compile at once?",
        explanation: "1 is safest and uses less memory. Raise this only if you have many CPU cores and plenty of RAM. Compiling several large packages at once can freeze a small computer.",
        suggested: Suggested::Usize(1),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 1,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "build.clean_chroot_after_compilation",
        kind: FieldKind::Bool,
        title: "Delete the temporary mini-system after an isolated compile?",
        explanation: "Yes frees disk space after compiling in a clean mini-system. No keeps it, which can speed up the next isolated compile but uses more disk.",
        suggested: Suggested::Bool(true),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "build.fast_aur_rpc_update_checks",
        kind: FieldKind::Bool,
        title: "Check AUR updates in one batch (faster)?",
        explanation: "The AUR is Arch’s user-contributed package collection. Yes asks the AUR website once for many packages instead of opening each download separately. Faster and recommended.",
        suggested: Suggested::Bool(true),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
];

const CPU_FIELDS: &[FieldDef] = &[
    FieldDef {
        id: "build.global_cpu_threads_mode",
        kind: FieldKind::Choice,
        title: "How strictly should ABS limit CPU use?",
        explanation: "This only matters when more than one package compiles at the same time.\n\
             “Never go over the limit” keeps the computer more responsive. The other choice is faster but you can set a hard maximum.",
        suggested: Suggested::Str("strict"),
        optional: false,
        in_gap_fill: true,
        choices: CPU_MODE_CHOICES,
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "build.default_compilation_threads",
        kind: FieldKind::OptionalUsize,
        title: "How many CPU cores may one package use? (optional)",
        explanation: "How many CPU cores one package may use. Leave empty to let the build tool decide. Type '-' to clear a saved number.",
        suggested: Suggested::None,
        optional: true,
        in_gap_fill: false,
        choices: &[],
        usize_min: 1,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "build.global_cpu_threads_cap",
        kind: FieldKind::OptionalUsize,
        title: "Maximum CPU cores for all compiles together? (optional)",
        explanation: "Caps the total cores used by every compile running at once, so the rest of the computer stays usable. Leave empty for no extra cap. Type '-' to clear.",
        suggested: Suggested::None,
        optional: true,
        in_gap_fill: false,
        choices: &[],
        usize_min: 1,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "build.maximum_cpu_threads_cap",
        kind: FieldKind::OptionalUsize,
        title: "Absolute maximum CPU cores? (optional)",
        explanation: "Only used with a soft limit. This is the hard ceiling, even if ABS starts extra jobs. It must be at least as high as the soft cap. Type '-' to clear.",
        suggested: Suggested::None,
        optional: true,
        in_gap_fill: false,
        choices: &[],
        usize_min: 1,
        visible_if: VisibleIf::CpuFlexible,
        path_pick: PathPick::None,
    },
];

const SYSTEM_UPDATE_FIELDS: &[FieldDef] = &[
    FieldDef {
        id: "system_update.command_to_update_repositories",
        kind: FieldKind::Command,
        title: "What command refreshes the package list (no upgrades yet)?",
        explanation: "This only downloads the latest list of available packages. It does not install or upgrade anything. Typical: sudo pacman -Sy",
        suggested: Suggested::Str(SUGGEST_SYNC_CMD),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "system_update.command_to_perform_system_update",
        kind: FieldKind::Command,
        title: "What command upgrades the rest of the system?",
        explanation: "This installs available updates. Typical: sudo pacman -Syu. If you normally use yay or paru, put that command here instead.",
        suggested: Suggested::Str(SUGGEST_UPDATE_CMD),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "system_update.command_to_perform_system_update_no_refresh",
        kind: FieldKind::OptionalCommand,
        title: "What upgrade command to use when the list is already fresh? (optional)",
        explanation: "Same upgrade as above, but skip refreshing the list because it was just refreshed. Typical: sudo pacman -Su. Type '-' to clear.",
        suggested: Suggested::Str(SUGGEST_NO_REFRESH_CMD),
        optional: true,
        in_gap_fill: false,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "system_update.auto_refresh_delay",
        kind: FieldKind::Usize,
        title: "How often should AbsGui refresh the pending-update list by itself? (minutes)",
        explanation: "0 means only when you press Refresh. 15 means every 15 minutes while the System update page is open, and when you come back after 15 minutes. The first visit still loads the list.",
        suggested: Suggested::Usize(0),
        optional: false,
        in_gap_fill: false,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "system_update.remember_sudo",
        kind: FieldKind::Bool,
        title: "Remember your sudo password until AbsGui is closed?",
        explanation: "Yes: type the password once; AbsGui keeps it in a private file until you quit. No: ask every time a command needs sudo (same as today).",
        suggested: Suggested::Bool(false),
        optional: false,
        in_gap_fill: false,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "system_update.ignore_flag",
        kind: FieldKind::String,
        title: "How should the updater skip a package?",
        explanation: "When ABS upgrades the rest of the system, it adds this option plus package names so your own compiled packages are not replaced by a pre-built update. For pacman, yay, and paru this is --ignore.",
        suggested: Suggested::Str(SUGGEST_IGNORE_FLAG),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
];

const REPO_FIELDS: &[FieldDef] = &[FieldDef {
    id: "repositories",
    kind: FieldKind::Repos,
    title: "Where should ABS get build recipes?",
    explanation: "Pick which git websites ABS may download build recipes from, and which one to use when you do not name one.",
    suggested: Suggested::Str("arch"),
    optional: false,
    in_gap_fill: true,
    choices: &[],
    usize_min: 0,
    visible_if: VisibleIf::Always,
    path_pick: PathPick::None,
}];

const RAMDISK_FIELDS: &[FieldDef] = &[
    FieldDef {
        id: "ramdisk.enabled",
        kind: FieldKind::Bool,
        title: "Use a RAM disk for compiling?",
        explanation: "Yes can make big compiles (like the kernel) faster. No is simpler and uses no extra RAM. If you are unsure, choose No.",
        suggested: Suggested::Bool(false),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "ramdisk.mount_point",
        kind: FieldKind::String,
        title: "Where should the RAM disk appear?",
        explanation: "Example: /run/abs-ram. The last part of the path must start with “abs” so ABS can find it safely.",
        suggested: Suggested::Str(SUGGEST_MOUNT),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::RamdiskEnabled,
        path_pick: PathPick::Folder,
    },
    FieldDef {
        id: "ramdisk.size",
        kind: FieldKind::String,
        title: "How much RAM may it use?",
        explanation: "This is a maximum, not reserved up front. Examples: 16G or 50%. It cannot be larger than this computer's RAM. Do not use commas.",
        suggested: Suggested::Str(SUGGEST_SIZE),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::RamdiskEnabled,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "ramdisk.mode",
        kind: FieldKind::String,
        title: "Who may use the RAM disk folder?",
        explanation: "This is a permission code. 0755 is the usual value: you can read and write the folder; other users can only read it. Leave the suggestion if you are unsure.",
        suggested: Suggested::Str(SUGGEST_MODE),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::RamdiskEnabled,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "ramdisk.build_workdir",
        kind: FieldKind::Bool,
        title: "Put the heavy compile folders in RAM?",
        explanation: "Yes stores temporary compile files and compiler caches in RAM. Good for large packages like the kernel. Needs free memory.",
        suggested: Suggested::Bool(false),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::RamdiskEnabled,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "ramdisk.chroot",
        kind: FieldKind::Bool,
        title: "Put the whole isolated mini-system in RAM?",
        explanation: "Yes can make isolated (clean mini-system) compiles faster but uses a lot of RAM. Only if you compile that way and have plenty of memory.",
        suggested: Suggested::Bool(false),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::RamdiskEnabled,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "ramdisk.packages",
        kind: FieldKind::Bool,
        title: "Put all downloaded sources in RAM?",
        explanation: "Yes copies every downloaded source folder into RAM. Uses a lot of memory — only if you have plenty to spare.",
        suggested: Suggested::Bool(false),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::RamdiskEnabled,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "ramdisk.min_free_ram_mb",
        kind: FieldKind::Usize,
        title: "How much free RAM (in MB) is required before using the RAM disk?",
        explanation: "If the computer has less free memory than this (in megabytes), ABS will not use the RAM disk. That helps avoid freezing the machine. 4096 means 4 GB.",
        suggested: Suggested::Usize(4096),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::RamdiskEnabled,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "ramdisk.zram",
        kind: FieldKind::Choice,
        title: "How should ABS size temporary compressed swap (zram)?",
        explanation: "full (recommended) always gives as much compressed swap as remaining RAM allows. Unused zram is a cap, not reserved up front. off disables ABS zram. This is independent of the RAM disk — used for compiles and system updates, not only converting profiles.",
        suggested: Suggested::Str("full"),
        optional: false,
        in_gap_fill: true,
        choices: ZRAM_CHOICES,
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
];

const SELF_UPDATE_FIELDS: &[FieldDef] = &[
    FieldDef {
        id: "check_for_update_on_startup",
        kind: FieldKind::Bool,
        title: "Tell you when a newer ABS is available?",
        explanation: "Yes checks in the background when you start abs, and reminds you at the end of the run if an update exists. It does not install anything by itself.",
        suggested: Suggested::Bool(true),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "auto_update_on_startup",
        kind: FieldKind::Bool,
        title: "Update ABS automatically at startup?",
        explanation: "Yes installs a newer ABS before doing anything else. That can take a while. No only notifies you (if you enabled the previous question).",
        suggested: Suggested::Bool(false),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "self_update_use_pacman",
        kind: FieldKind::Bool,
        title: "Install ABS updates with pacman?",
        explanation: "Yes is the usual Arch way: build packages and install them with pacman. The files go in the finished-packages folder.\n\
             No copies the abs (and AbsGui) programs into a folder you choose next.",
        suggested: Suggested::Bool(true),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "self_update_install_path",
        kind: FieldKind::Path,
        title: "Where should the abs program be copied?",
        explanation: "AbsGui is placed in the same folder. Typical: /usr/bin/abs",
        suggested: Suggested::Str(SUGGEST_INSTALL_PATH),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::PacmanFalse,
        path_pick: PathPick::File,
    },
    FieldDef {
        id: "self_update_at_updates",
        kind: FieldKind::Bool,
        title: "Also check for a newer ABS during a system update?",
        explanation: "Yes looks for a newer ABS when you run a system update (`abs -U`). No only checks at startup (if you enabled that).",
        suggested: Suggested::Bool(false),
        optional: false,
        in_gap_fill: true,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
];

const PACKAGE_LIST_FIELDS: &[FieldDef] = &[
    FieldDef {
        id: "manual_update_packages",
        kind: FieldKind::StringList,
        title: "Which packages should ABS watch and rebuild when newer?",
        explanation: "ABS checks these packages in their repository and can compile them automatically when a new version appears. Example: mesa. Leave empty if you are not sure yet.",
        suggested: Suggested::None,
        optional: true,
        in_gap_fill: false,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "skip_install_packages",
        kind: FieldKind::StringList,
        title: "Which packages should not be installed from the official repos?",
        explanation: "If you plan to compile a package yourself, you usually do not want pacman to install the pre-built version over it. Example: qemu*. Patterns with * work.",
        suggested: Suggested::None,
        optional: true,
        in_gap_fill: false,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "skip_install_packages_after_compilation",
        kind: FieldKind::SkipAfterList,
        title: "Which packages should not be installed after you compiled them? (optional)",
        explanation: "After compiling, ABS may offer to install the result. Names listed here are skipped, which speeds up that step.\n\
         Example: qemu-docs if you compiled qemu but do not want the documentation package.\n\
         Press Enter to keep the current value. Type '-' for an empty list. On a new file, Enter leaves this not set (same as the list above).",
        suggested: Suggested::None,
        optional: true,
        in_gap_fill: false,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
    FieldDef {
        id: "system_update.ignore_packages",
        kind: FieldKind::StringList,
        title: "Any extra packages to skip during a system update?",
        explanation: "These names are added to the skip list when ABS runs your system upgrade command, on top of the lists above. Leave empty if the other lists already cover what you compile yourself.",
        suggested: Suggested::None,
        optional: true,
        in_gap_fill: false,
        choices: &[],
        usize_min: 0,
        visible_if: VisibleIf::Always,
        path_pick: PathPick::None,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wizard_copy_is_plain_language_not_toml_keys() {
        for field in all_fields() {
            assert!(!field.title.is_empty(), "{}", field.id);
            assert!(!field.explanation.is_empty(), "{}", field.id);
            assert_ne!(field.title, field.id, "{}", field.id);
            let last = field.id.rsplit('.').next().unwrap();
            assert_ne!(field.title, last, "{}", field.id);
            assert!(
                !field.title.contains('.'),
                "title looks like a dotted key: {} -> {}",
                field.id,
                field.title
            );
            assert!(
                !field.title.contains("PGO") && !field.explanation.contains("PGO"),
                "copy uses jargon PGO: {} / {}",
                field.title,
                field.explanation
            );
            assert!(
                !field.explanation.contains(field.id),
                "explanation mentions raw id {}: {}",
                field.id,
                field.explanation
            );
        }
        for step in STEPS {
            assert!(!step.title.is_empty(), "{}", step.id);
            assert!(!step.blurb.is_empty(), "{}", step.id);
            assert!(
                !step.blurb.contains("PGO"),
                "step blurb uses jargon: {}",
                step.blurb
            );
        }
        let title = display_title_for_gap_key("paths.packages_path");
        assert!(!title.is_empty());
        assert_ne!(title, "paths.packages_path");
        assert_eq!(display_title_for_gap_key("unknown.key"), "unknown.key");
    }

    #[test]
    fn auto_refresh_delay_reads_quoted_number() {
        let doc: DocumentMut = r#"
[system_update]
auto_refresh_delay = "15"
remember_sudo = true
"#
        .parse()
        .unwrap();
        let delay = field_by_id("system_update.auto_refresh_delay").unwrap();
        assert_eq!(current_json(&doc, delay), json!(15));
        let remember = field_by_id("system_update.remember_sudo").unwrap();
        assert_eq!(current_json(&doc, remember), json!(true));
    }

    #[test]
    fn current_json_reads_inline_table_paths_for_gui_form() {
        // AbsGui --config-wizard-form current values use table_ref (sibling inline-table fix).
        let doc: DocumentMut = r#"
paths = { packages_path = "/tmp/abs-packages", chroot_base_path = "/tmp/abs-chroot", ready_made_packages_path = "/tmp/abs-ready" }
"#
        .parse()
        .unwrap();
        let field = field_by_id("paths.packages_path").unwrap();
        assert_eq!(current_json(&doc, field), json!("/tmp/abs-packages"));
    }
}
