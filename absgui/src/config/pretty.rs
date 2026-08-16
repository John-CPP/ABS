//! Keep in sync with `src/toml_pretty.rs` in the abs crate.
//!
//! Render `abs.toml` as standard `[section]` tables with blank lines between sections.

use toml_edit::visit_mut::{visit_array_mut, visit_document_mut, visit_item_mut, VisitMut};
use toml_edit::{Array, DocumentMut, Item, Table};

pub fn humanize_in_place(doc: &mut DocumentMut) {
    let mut visitor = Humanize {
        skip_next_table: true,
    };
    visitor.visit_document_mut(doc);
}

pub fn render_human_toml(doc: &mut DocumentMut) -> String {
    humanize_in_place(doc);
    let mut text = doc.to_string();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

struct Humanize {
    skip_next_table: bool,
}

impl VisitMut for Humanize {
    fn visit_document_mut(&mut self, node: &mut DocumentMut) {
        self.skip_next_table = true;
        visit_document_mut(self, node);
    }

    fn visit_item_mut(&mut self, node: &mut Item) {
        expand_inline_item(node);
        visit_item_mut(self, node);
    }

    fn visit_table_mut(&mut self, node: &mut Table) {
        let skip = self.skip_next_table;
        self.skip_next_table = false;
        toml_edit::visit_mut::visit_table_mut(self, node);
        tidy_table_header_keys(node);
        if skip {
            return;
        }
        maybe_set_implicit(node);
        if !node.is_implicit() {
            ensure_blank_line_prefix(node);
        }
    }

    fn visit_array_mut(&mut self, node: &mut Array) {
        visit_array_mut(self, node);
        pretty_multiline_array(node);
    }
}

fn expand_inline_item(node: &mut Item) {
    let taken = std::mem::take(node);
    let taken = match taken.into_table() {
        Ok(table) => Item::Table(table),
        Err(rest) => rest,
    };
    let taken = match taken.into_array_of_tables() {
        Ok(aot) => Item::ArrayOfTables(aot),
        Err(rest) => rest,
    };
    *node = taken;
}

fn tidy_table_header_keys(table: &mut Table) {
    for (mut key, item) in table.iter_mut() {
        if item.is_table() || item.is_array_of_tables() {
            key.fmt();
        }
    }
}

fn maybe_set_implicit(table: &mut Table) {
    if table.is_empty() {
        return;
    }
    let only_nested = table
        .iter()
        .all(|(_, item)| item.is_table() || item.is_array_of_tables());
    if only_nested {
        table.set_implicit(true);
    }
}

fn ensure_blank_line_prefix(table: &mut Table) {
    let prefix = table
        .decor()
        .prefix()
        .and_then(|p| p.as_str())
        .unwrap_or("")
        .to_string();
    if prefix.is_empty() {
        table.decor_mut().set_prefix("\n");
        return;
    }
    if prefix.starts_with('\n') {
        return;
    }
    table.decor_mut().set_prefix(format!("\n{prefix}"));
}

fn pretty_multiline_array(arr: &mut Array) {
    if arr.len() < 2 {
        return;
    }
    for value in arr.iter_mut() {
        let prefix = value
            .decor()
            .prefix()
            .and_then(|p| p.as_str())
            .unwrap_or("");
        if !prefix.contains('\n') {
            value.decor_mut().set_prefix("\n    ");
        }
    }
    arr.set_trailing("\n");
    arr.set_trailing_comma(true);
}
