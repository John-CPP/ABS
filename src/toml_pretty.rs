//! Render `abs.toml` as standard `[section]` tables with blank lines between sections.
//!
//! `toml_edit` serde and compact user files use inline tables (`paths = { ... }`) that look
//! like JSON. Writers convert those to headers and keep package lists multiline.

use toml_edit::visit_mut::{VisitMut, visit_array_mut, visit_document_mut, visit_item_mut};
use toml_edit::{Array, DocumentMut, Item, Table};

/// Expand inline tables, space `[section]` headers, and format multi-entry arrays.
pub fn humanize_in_place(doc: &mut DocumentMut) {
    let mut visitor = Humanize {
        skip_next_table: true,
    };
    visitor.visit_document_mut(doc);
}

/// Humanize, then emit TOML ending in a newline.
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

#[cfg(test)]
mod tests {
    use super::*;

    const INLINE: &str = r#"
config_version = 1
manual_update_packages = ["curl", "wget"]
skip_install_packages = ["mesa-docs"]
# keep this comment
mystery_key = "stay"
paths = { packages_path = "/media/storage/packages/abs/packages", chroot_base_path = "/media/storage/packages/abs/chroot", ready_made_packages_path = "/media/storage/packages/abs/ready" }
ramdisk= { enabled = true, mount_point = "/run/abs-ram", size = "69G", mode = "0755", build_workdir = true, chroot = true, packages = true }
build= { default_environment = "local", ignore_compilation_failures = true, concurrent_compilations_limit = 2 }
system_update= { command_to_update_repositories = "yay -Sy --quiet", command_to_perform_system_update = "yay -Syu --quiet", ignore_flag = "--ignore", ignore_packages = [] }
repositories= { default = "arch", aur = "https://aur.archlinux.org" }
packages = { vim = { source = "arch", build_env = "local", tests = false } }
"#;

    #[test]
    fn expands_inline_tables_to_headers() {
        let mut doc: DocumentMut = INLINE.parse().unwrap();
        let text = render_human_toml(&mut doc);
        assert!(
            !text.contains("paths=") && !text.contains("paths = {"),
            "still inline:\n{text}"
        );
        assert!(text.contains("[paths]"), "{text}");
        assert!(text.contains("[ramdisk]"), "{text}");
        assert!(text.contains("[build]"), "{text}");
        assert!(text.contains("[system_update]"), "{text}");
        assert!(text.contains("[repositories]"), "{text}");
        assert!(text.contains("[packages.vim]"), "{text}");
        assert!(!text.contains("[packages]\n"), "{text}");
        assert!(text.contains("# keep this comment"), "{text}");
        assert!(text.contains("mystery_key"), "{text}");
        assert!(
            text.contains("\n    \"curl\",\n    \"wget\",\n"),
            "package list should be multiline:\n{text}"
        );
        let paths_at = text.find("[paths]").unwrap();
        let before = &text[..paths_at];
        assert!(
            before.ends_with("\n\n") || before.ends_with("\n\r\n"),
            "expected a blank line before [paths]:\n{text}"
        );
    }

    #[test]
    fn humanize_is_idempotent() {
        let mut doc: DocumentMut = INLINE.parse().unwrap();
        let once = render_human_toml(&mut doc);
        let mut again: DocumentMut = once.parse().unwrap();
        let twice = render_human_toml(&mut again);
        assert_eq!(once, twice);
    }

    #[test]
    fn example_style_keeps_comments() {
        let src = crate::config::example_config_text();
        let mut doc: DocumentMut = src.parse().unwrap();
        let text = render_human_toml(&mut doc);
        assert!(text.contains("abs --config-wizard"), "{text}");
        assert!(text.contains("[paths]"), "{text}");
        assert!(
            !text.contains("paths = {"),
            "example must stay as headers:\n{text}"
        );
    }
}
