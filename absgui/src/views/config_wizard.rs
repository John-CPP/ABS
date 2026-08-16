//! AbsGui config wizard: thin client of `abs --config-wizard-form|check|apply`.

use crate::abs_runner::{self, WizardChoice, WizardField, WizardForm, WizardStep};
use crate::app_settings::AppTheme;
use crate::messages::{Message, PathKind};
use crate::style;
use crate::widgets::{card_section, help_line, path_browse_button, themed_pick_list};
use iced::widget::{button, checkbox, column, container, row, text, text_input, Space};
use iced::{Alignment, Element, Length, Padding};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const DEBOUNCE: Duration = Duration::from_millis(280);
const PULSE: Duration = Duration::from_millis(550);

#[derive(Debug, Clone)]
pub struct FieldError {
    pub message: String,
    pub pulse_at: Instant,
}

pub struct WizardSession {
    pub loading: bool,
    pub applying: bool,
    pub load_error: Option<String>,
    pub apply_result: Option<Result<String, String>>,
    pub form: Option<WizardForm>,
    pub answers: Map<String, Value>,
    pub step: usize,
    pub errors: HashMap<String, FieldError>,
    pub list_draft: HashMap<String, String>,
    pub repo_draft_name: String,
    pub repo_draft_url: String,
    check_seq: u64,
    debounce: Option<(Instant, String, u64)>,
}

impl Default for WizardSession {
    fn default() -> Self {
        Self {
            loading: false,
            applying: false,
            load_error: None,
            apply_result: None,
            form: None,
            answers: Map::new(),
            step: 0,
            errors: HashMap::new(),
            list_draft: HashMap::new(),
            repo_draft_name: String::new(),
            repo_draft_url: String::new(),
            check_seq: 0,
            debounce: None,
        }
    }
}

impl WizardSession {
    pub fn load_task() -> iced::Task<Message> {
        iced::Task::perform(
            async { abs_runner::fetch_wizard_form() },
            Message::WizardFormLoaded,
        )
    }

    pub fn needs_timer(&self) -> bool {
        self.debounce.is_some() || self.errors.values().any(|e| e.pulse_at.elapsed() < PULSE)
    }

    pub fn on_form_loaded(&mut self, result: Result<WizardForm, String>) {
        self.loading = false;
        match result {
            Ok(form) => {
                self.answers = initial_answers(&form);
                self.step = self.step.min(form.steps.len().saturating_sub(1));
                self.errors.clear();
                self.apply_result = None;
                self.load_error = None;
                self.form = Some(form);
            }
            Err(e) => {
                self.load_error = Some(e);
                self.form = None;
            }
        }
    }

    pub fn set_value(&mut self, id: String, value: Value, immediate: bool) -> iced::Task<Message> {
        self.answers.insert(id.clone(), value);
        self.apply_result = None;
        self.schedule_check(id, immediate)
    }

    pub fn schedule_check(&mut self, id: String, immediate: bool) -> iced::Task<Message> {
        self.check_seq = self.check_seq.wrapping_add(1);
        let gen = self.check_seq;
        if immediate {
            self.debounce = None;
            self.spawn_check(id, gen)
        } else {
            self.debounce = Some((Instant::now() + DEBOUNCE, id, gen));
            iced::Task::none()
        }
    }

    pub fn on_timer(&mut self) -> iced::Task<Message> {
        let now = Instant::now();
        if let Some((at, id, gen)) = self.debounce.take() {
            if now >= at {
                return self.spawn_check(id, gen);
            }
            self.debounce = Some((at, id, gen));
        }
        iced::Task::none()
    }

    fn spawn_check(&self, id: String, gen: u64) -> iced::Task<Message> {
        let value = self.answers.get(&id).cloned().unwrap_or(Value::Null);
        let answers = self.answers.clone();
        let check_id = id.clone();
        iced::Task::perform(
            async move { abs_runner::wizard_check(&check_id, &value, &answers) },
            move |r| Message::WizardCheckResult(gen, id, r),
        )
    }

    pub fn on_check_result(&mut self, gen: u64, id: String, result: Result<(), String>) {
        if gen != self.check_seq
            && self
                .debounce
                .as_ref()
                .is_some_and(|(_, pending, g)| pending == &id && *g > gen)
        {
            return;
        }
        match result {
            Ok(()) => {
                self.errors.remove(&id);
            }
            Err(message) => {
                self.errors.insert(
                    id,
                    FieldError {
                        message,
                        pulse_at: Instant::now(),
                    },
                );
            }
        }
    }

    pub fn visible_fields<'a>(&'a self, step: &'a WizardStep) -> Vec<&'a WizardField> {
        step.fields
            .iter()
            .filter(|f| field_visible(f, &self.answers))
            .collect()
    }

    pub fn current_step(&self) -> Option<&WizardStep> {
        self.form.as_ref()?.steps.get(self.step)
    }

    pub fn validate_current_step(&self) -> iced::Task<Message> {
        let Some(step) = self.current_step() else {
            return iced::Task::none();
        };
        let fields: Vec<(String, Value)> = self
            .visible_fields(step)
            .into_iter()
            .map(|f| {
                (
                    f.id.clone(),
                    self.answers.get(&f.id).cloned().unwrap_or(Value::Null),
                )
            })
            .collect();
        let answers = self.answers.clone();
        iced::Task::perform(
            async move {
                let mut errors = Vec::new();
                for (id, value) in fields {
                    if let Err(e) = abs_runner::wizard_check(&id, &value, &answers) {
                        errors.push((id, e));
                    }
                }
                errors
            },
            Message::WizardStepChecked,
        )
    }

    pub fn on_step_checked(&mut self, errors: Vec<(String, String)>) -> bool {
        if errors.is_empty() {
            return true;
        }
        let now = Instant::now();
        for (id, message) in errors {
            self.errors.insert(
                id,
                FieldError {
                    message,
                    pulse_at: now,
                },
            );
        }
        false
    }

    pub fn apply_task(&self) -> iced::Task<Message> {
        let answers = self.answers.clone();
        iced::Task::perform(
            async move { abs_runner::wizard_apply(&answers) },
            Message::WizardApplyDone,
        )
    }

    pub fn use_suggested(&mut self, id: String) -> iced::Task<Message> {
        let Some(field) = self.find_field(&id) else {
            return iced::Task::none();
        };
        let value = field.suggested.clone();
        let kind = field.kind.clone();
        self.set_value(id, value, !is_text_kind(&kind))
    }

    fn find_field(&self, id: &str) -> Option<&WizardField> {
        self.form
            .as_ref()?
            .steps
            .iter()
            .flat_map(|s| s.fields.iter())
            .find(|f| f.id == id)
    }
}

fn is_text_kind(kind: &str) -> bool {
    matches!(
        kind,
        "path"
            | "command"
            | "string"
            | "usize"
            | "optional_usize"
            | "optional_path"
            | "optional_command"
            | "string_list"
            | "skip_after_list"
    )
}

fn initial_answers(form: &WizardForm) -> Map<String, Value> {
    let mut answers = Map::new();
    for field in form.steps.iter().flat_map(|s| s.fields.iter()) {
        let mut value = if field.prefer_current && !field.current.is_null() {
            field.current.clone()
        } else if !field.suggested.is_null() {
            field.suggested.clone()
        } else {
            field.current.clone()
        };
        if matches!(field.kind.as_str(), "usize" | "optional_usize") && value.is_number() {
            value = json!(value_text(&value));
        }
        answers.insert(field.id.clone(), value);
    }
    answers
}

fn field_visible(field: &WizardField, answers: &Map<String, Value>) -> bool {
    let Some(cond) = field.visible_if.as_ref().and_then(Value::as_object) else {
        return true;
    };
    let Some(other) = cond.get("field").and_then(Value::as_str) else {
        return true;
    };
    let Some(equals) = cond.get("equals") else {
        return true;
    };
    answers.get(other) == Some(equals)
}

fn shake_px(err: Option<&FieldError>) -> f32 {
    let Some(err) = err else {
        return 0.0;
    };
    let t = err.pulse_at.elapsed().as_secs_f32();
    if t >= 0.55 {
        return 0.0;
    }
    (t * 48.0).sin() * 7.0 * (1.0 - t / 0.55)
}

fn value_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Object(obj) if obj.get("entries").and_then(Value::as_array).is_some() => {
            let names: Vec<&str> = obj["entries"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|e| e.get("name").and_then(Value::as_str))
                .collect();
            let default = obj.get("default").and_then(Value::as_str).unwrap_or("");
            if default.is_empty() {
                names.join(", ")
            } else {
                format!("{} (default {default})", names.join(", "))
            }
        }
        other => other.to_string(),
    }
}

pub fn view<'a>(session: &'a WizardSession, theme: AppTheme) -> Element<'a, Message> {
    let crumb = session
        .form
        .as_ref()
        .and_then(|f| {
            f.steps
                .get(session.step.min(f.steps.len().saturating_sub(1)))
        })
        .map(|s| s.title.clone())
        .unwrap_or_else(|| abs_i18n::t("gui.wizard.title").to_string());
    let mut col = column![
        crate::widgets::breadcrumb_row(abs_i18n::t("gui.nav.config_wizard"), crumb, None, theme,),
        text(abs_i18n::t("gui.wizard.subtitle"))
            .size(style::TEXT_HELP)
            .color(style::muted(theme)),
    ]
    .spacing(12);

    if session.loading {
        col = col.push(
            text(abs_i18n::t("gui.wizard.loading"))
                .size(15)
                .color(style::muted(theme)),
        );
        return col.into();
    }
    if let Some(err) = &session.load_error {
        col = col.push(error_banner(
            abs_i18n::tf("gui.wizard.load_failed", &[("e", err)]),
            theme,
        ));
        col = col.push(
            button(text(abs_i18n::t("gui.common.refresh")).size(14))
                .style(style::btn_primary(theme))
                .on_press(Message::OpenConfigWizard),
        );
        return col.into();
    }
    let Some(form) = session.form.as_ref() else {
        return col.into();
    };

    col = col.push(
        text(abs_i18n::tf(
            "gui.wizard.current_file",
            &[("path", form.path.as_str())],
        ))
        .size(style::TEXT_HELP)
        .color(style::muted(theme)),
    );
    col = col.push(
        text(if form.reconfigure {
            abs_i18n::t("gui.wizard.reconfigure")
        } else {
            abs_i18n::t("gui.wizard.first_run")
        })
        .size(13)
        .color(style::muted(theme)),
    );

    if let Some(Ok(path)) = &session.apply_result {
        col = col.push(success_banner(
            abs_i18n::tf("gui.wizard.saved", &[("path", path.as_str())]),
            theme,
        ));
        col = col.push(
            button(text(abs_i18n::t("gui.nav.abs_settings")).size(14))
                .style(style::btn_primary(theme))
                .on_press(Message::OpenAbsSettings),
        );
        return col.into();
    }
    if let Some(Err(e)) = &session.apply_result {
        col = col.push(error_banner(
            abs_i18n::tf("gui.wizard.apply_failed", &[("e", e)]),
            theme,
        ));
    }

    let total = form.steps.len().max(1);
    let step_idx = session.step.min(total - 1);
    let step = &form.steps[step_idx];
    col = col.push(crate::widgets::wizard_stepper(step_idx, total, theme));
    col = col.push(card_section(
        "",
        theme,
        column![
            text(&step.blurb).size(13).color(style::muted(theme)),
            fields_column(session, step, theme),
        ]
        .spacing(12),
    ));

    let mut nav = row![].spacing(8).align_y(Alignment::Center);
    if step_idx > 0 {
        nav = nav.push(
            button(text(abs_i18n::t("gui.wizard.back")).size(14))
                .style(style::btn_secondary(theme))
                .on_press(Message::WizardBack),
        );
    }
    nav = nav.push(
        button(text(abs_i18n::t("gui.common.cancel")).size(14))
            .style(style::btn_secondary(theme))
            .on_press(Message::WizardCancel),
    );
    nav = nav.push(Space::new().width(Length::Fill));
    if session.applying {
        nav = nav.push(
            text(abs_i18n::t("gui.wizard.applying"))
                .size(13)
                .color(style::muted(theme)),
        );
    } else if step_idx + 1 < total {
        nav = nav.push(
            button(text(abs_i18n::t("gui.wizard.next")).size(14))
                .style(style::btn_primary(theme))
                .on_press(Message::WizardNext),
        );
    } else {
        nav = nav.push(
            button(text(abs_i18n::t("gui.wizard.apply")).size(14))
                .style(style::btn_primary(theme))
                .on_press(Message::WizardApply),
        );
    }
    col = col.push(nav);
    col.into()
}

fn fields_column<'a>(
    session: &'a WizardSession,
    step: &'a WizardStep,
    theme: AppTheme,
) -> Element<'a, Message> {
    let mut col = column![].spacing(16);
    for field in session.visible_fields(step) {
        col = col.push(field_widget(session, field, theme));
    }
    col.into()
}

fn field_widget<'a>(
    session: &'a WizardSession,
    field: &'a WizardField,
    theme: AppTheme,
) -> Element<'a, Message> {
    let err = session.errors.get(&field.id);
    let shake = shake_px(err);
    let mut body = column![
        text(&field.title).size(15),
        help_line(&field.explanation, theme),
    ]
    .spacing(6);
    if !field.suggested.is_null() && session.answers.get(&field.id) != Some(&field.suggested) {
        let hint = abs_i18n::tf(
            "gui.wizard.suggested_value",
            &[("value", value_text(&field.suggested).as_str())],
        );
        let id = field.id.clone();
        body = body.push(
            row![
                text(hint).size(12).color(style::primary(theme)),
                button(text(abs_i18n::t("gui.wizard.use_suggested")).size(12))
                    .style(style::btn_secondary(theme))
                    .on_press(Message::WizardUseSuggested(id)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }
    body = body.push(input_for_kind(session, field, err.is_some(), theme));
    if let Some(err) = err {
        body = body.push(text(&err.message).size(13).color(style::danger(theme)));
    }
    container(body)
        .padding(Padding {
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: (shake + 8.0).max(0.0),
        })
        .width(Length::Fill)
        .into()
}

fn input_for_kind<'a>(
    session: &'a WizardSession,
    field: &'a WizardField,
    invalid: bool,
    theme: AppTheme,
) -> Element<'a, Message> {
    let id = field.id.clone();
    let current = session.answers.get(&field.id).unwrap_or(&Value::Null);
    match field.kind.as_str() {
        "bool" => bool_input(id, current.as_bool().unwrap_or(false), theme),
        "choice" => choice_input(id, field.options.as_deref().unwrap_or(&[]), current, theme),
        "path" | "optional_path" => path_input(
            id,
            current.as_str().unwrap_or(""),
            field.path_pick.as_deref(),
            field.kind == "optional_path",
            invalid,
            theme,
        ),
        "command" | "string" | "optional_command" => textish_input(
            id,
            current.as_str().unwrap_or(""),
            field.kind.starts_with("optional"),
            invalid,
            theme,
        ),
        "usize" | "optional_usize" => textish_input(
            id,
            current.as_str().unwrap_or(""),
            field.kind == "optional_usize",
            invalid,
            theme,
        ),
        "string_list" => list_input(session, field, current, false, theme),
        "skip_after_list" => list_input(session, field, current, true, theme),
        "repos" => repos_input(session, field, current, invalid, theme),
        _ => textish_input(id, current.as_str().unwrap_or(""), false, invalid, theme),
    }
}

fn bool_input(id: String, on: bool, theme: AppTheme) -> Element<'static, Message> {
    let yes_id = id.clone();
    let no_id = id;
    row![
        chip(
            abs_i18n::t("gui.common.yes"),
            on,
            theme,
            Message::WizardFieldChanged(yes_id, json!(true), true),
        ),
        chip(
            abs_i18n::t("gui.common.no"),
            !on,
            theme,
            Message::WizardFieldChanged(no_id, json!(false), true),
        ),
    ]
    .spacing(8)
    .into()
}

fn choice_input<'a>(
    id: String,
    options: &'a [WizardChoice],
    current: &Value,
    theme: AppTheme,
) -> Element<'a, Message> {
    let selected = current.as_str().unwrap_or("");
    let mut col = column![].spacing(8);
    let mut chips = row![].spacing(8);
    for opt in options {
        let msg = Message::WizardFieldChanged(id.clone(), json!(opt.value), true);
        chips = chips.push(chip(&opt.label, opt.value == selected, theme, msg));
    }
    col = col.push(chips);
    if let Some(opt) = options.iter().find(|o| o.value == selected) {
        if !opt.help.is_empty() {
            col = col.push(help_line(&opt.help, theme));
        }
    }
    col.into()
}

fn chip<'a>(label: &'a str, active: bool, theme: AppTheme, msg: Message) -> Element<'a, Message> {
    let btn = button(text(label).size(13)).on_press(msg);
    if active {
        btn.style(style::btn_primary(theme)).into()
    } else {
        btn.style(style::btn_secondary(theme)).into()
    }
}

fn path_input<'a>(
    id: String,
    value: &'a str,
    pick: Option<&str>,
    optional: bool,
    invalid: bool,
    theme: AppTheme,
) -> Element<'a, Message> {
    let kind = match pick {
        Some("file") => PathKind::File,
        _ => PathKind::Folder,
    };
    let id_input = id.clone();
    let mut row = row![
        text_input("", value)
            .on_input(move |s| {
                let v = if optional && s.trim().is_empty() {
                    Value::Null
                } else {
                    json!(s)
                };
                Message::WizardFieldChanged(id_input.clone(), v, false)
            })
            .padding(8)
            .style(style::wizard_input(theme, invalid))
            .width(Length::Fill),
        path_browse_button(kind, Message::WizardBrowse(id, kind), theme),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if optional && !value.is_empty() {
        // Clear is handled by deleting text; keep the row simple.
        let _ = &mut row;
    }
    row.into()
}

fn textish_input<'a>(
    id: String,
    value: &'a str,
    optional: bool,
    invalid: bool,
    theme: AppTheme,
) -> Element<'a, Message> {
    text_input("", value)
        .on_input(move |s| {
            let v = if optional && s.trim().is_empty() {
                Value::Null
            } else {
                json!(s)
            };
            Message::WizardFieldChanged(id.clone(), v, false)
        })
        .padding(8)
        .style(style::wizard_input(theme, invalid))
        .width(Length::Fill)
        .into()
}

fn list_input<'a>(
    session: &'a WizardSession,
    field: &'a WizardField,
    current: &'a Value,
    skip_after: bool,
    theme: AppTheme,
) -> Element<'a, Message> {
    let id = field.id.clone();
    let unset = skip_after && current.is_null();
    let mut col = column![].spacing(8);
    if skip_after {
        let toggle_id = id.clone();
        col = col.push(
            checkbox(!unset)
                .label(abs_i18n::t("gui.wizard.set_separate"))
                .on_toggle(move |on| {
                    let v = if on { json!([]) } else { Value::Null };
                    Message::WizardFieldChanged(toggle_id.clone(), v, true)
                }),
        );
        if unset {
            col = col.push(
                text(abs_i18n::t("gui.wizard.optional_unset"))
                    .size(style::TEXT_HELP)
                    .color(style::muted(theme)),
            );
            return col.into();
        }
    }
    let items: Vec<&str> = current
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    for (i, item) in items.iter().enumerate() {
        let remove_id = id.clone();
        let mut next: Vec<String> = items.iter().map(|s| (*s).to_string()).collect();
        next.remove(i);
        col = col.push(
            row![
                container(text(*item).size(13))
                    .padding(6)
                    .style(style::tag(theme)),
                button(text(abs_i18n::t("gui.common.remove")).size(12))
                    .style(style::btn_secondary(theme))
                    .on_press(Message::WizardFieldChanged(remove_id, json!(next), true)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }
    let draft = session
        .list_draft
        .get(&field.id)
        .map(String::as_str)
        .unwrap_or("");
    let draft_id = id.clone();
    col = col.push(
        row![
            text_input(abs_i18n::t("gui.wizard.package_name"), draft)
                .on_input(move |s| Message::WizardListDraft(draft_id.clone(), s))
                .padding(8)
                .width(Length::Fill),
            button(text(abs_i18n::t("gui.common.add")).size(13))
                .style(style::btn_secondary(theme))
                .on_press(Message::WizardListAdd(id)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    );
    col.into()
}

fn repos_input<'a>(
    session: &'a WizardSession,
    field: &'a WizardField,
    current: &Value,
    invalid: bool,
    theme: AppTheme,
) -> Element<'a, Message> {
    let id = field.id.clone();
    let obj = current.as_object();
    let entries = obj
        .and_then(|o| o.get("entries"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let default = obj
        .and_then(|o| o.get("default"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut col = column![].spacing(10);
    for (i, entry) in entries.iter().enumerate() {
        let name = entry.get("name").and_then(Value::as_str).unwrap_or("");
        let url = entry.get("url").and_then(Value::as_str).unwrap_or("");
        let name_id = id.clone();
        let url_id = id.clone();
        let remove_id = id.clone();
        let entries_name = entries.clone();
        let entries_url = entries.clone();
        let mut entries_remove = entries.clone();
        let default_name = default.clone();
        let default_url = default.clone();
        let default_remove = default.clone();
        col = col.push(
            column![
                row![
                    text_input(abs_i18n::t("gui.wizard.repo_name"), name)
                        .on_input(move |s| {
                            Message::WizardFieldChanged(
                                name_id.clone(),
                                repos_patch_entry(&entries_name, i, "name", s, &default_name),
                                false,
                            )
                        })
                        .padding(8)
                        .style(style::wizard_input(theme, invalid))
                        .width(Length::FillPortion(1)),
                    button(text(abs_i18n::t("gui.common.remove")).size(12))
                        .style(style::btn_secondary(theme))
                        .on_press({
                            let removed_name = name.to_string();
                            entries_remove.remove(i);
                            let def = if default_remove == removed_name {
                                entries_remove
                                    .first()
                                    .and_then(|e| e.get("name"))
                                    .and_then(Value::as_str)
                                    .unwrap_or("arch")
                                    .to_string()
                            } else {
                                default_remove
                            };
                            Message::WizardFieldChanged(
                                remove_id,
                                json!({"default": def, "entries": entries_remove}),
                                true,
                            )
                        }),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                text_input(abs_i18n::t("gui.wizard.repo_url"), url)
                    .on_input(move |s| {
                        Message::WizardFieldChanged(
                            url_id.clone(),
                            repos_patch_entry(&entries_url, i, "url", s, &default_url),
                            false,
                        )
                    })
                    .padding(8)
                    .style(style::wizard_input(theme, invalid))
                    .width(Length::Fill),
            ]
            .spacing(6),
        );
    }
    let names: Vec<String> = entries
        .iter()
        .filter_map(|e| e.get("name").and_then(Value::as_str).map(str::to_string))
        .collect();
    let selected = if default.is_empty() {
        None
    } else {
        Some(default.clone())
    };
    let def_id = id.clone();
    let entries_for_default = entries.clone();
    col = col.push(help_line(abs_i18n::t("gui.wizard.repo_default"), theme));
    col = col.push(themed_pick_list(
        names,
        selected,
        move |name| {
            Message::WizardFieldChanged(
                def_id.clone(),
                json!({"default": name, "entries": entries_for_default}),
                true,
            )
        },
        theme,
        Length::Fill,
    ));
    let add_id = id;
    col = col.push(
        row![
            text_input(
                abs_i18n::t("gui.wizard.repo_name"),
                &session.repo_draft_name
            )
            .on_input(Message::WizardRepoDraftName)
            .padding(8)
            .width(Length::FillPortion(1)),
            text_input(abs_i18n::t("gui.wizard.repo_url"), &session.repo_draft_url)
                .on_input(Message::WizardRepoDraftUrl)
                .padding(8)
                .width(Length::FillPortion(2)),
            button(text(abs_i18n::t("gui.abs.add_repo")).size(13))
                .style(style::btn_secondary(theme))
                .on_press(Message::WizardRepoAdd(add_id)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    );
    col.into()
}

fn repos_patch_entry(
    entries: &[Value],
    index: usize,
    key: &str,
    value: String,
    default: &str,
) -> Value {
    let mut entries = entries.to_vec();
    if let Some(obj) = entries.get_mut(index).and_then(Value::as_object_mut) {
        obj.insert(key.to_string(), json!(value));
    }
    json!({"default": default, "entries": entries})
}

fn error_banner(msg: String, theme: AppTheme) -> Element<'static, Message> {
    container(text(msg).size(14))
        .padding(12)
        .width(Length::Fill)
        .style(style::wizard_error_banner(theme))
        .into()
}

fn success_banner(msg: String, theme: AppTheme) -> Element<'static, Message> {
    container(text(msg).size(14))
        .padding(12)
        .width(Length::Fill)
        .style(style::wizard_success_banner(theme))
        .into()
}
