use crate::config::ConfigDocument;
use iced::widget::text_editor::Content;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageListField {
    ManualUpdate,
    SkipInstall,
    SkipInstallAfter,
    SysUpdateIgnore,
}

pub struct ListEditors {
    pub manual_update: Content,
    pub skip_install: Content,
    pub skip_install_after: Content,
    pub sys_update_ignore: Content,
}

impl ListEditors {
    pub fn from_config(doc: &ConfigDocument) -> Self {
        Self {
            manual_update: Content::with_text(&lines_to_text(&doc.manual_update_packages)),
            skip_install: Content::with_text(&lines_to_text(&doc.skip_install_packages)),
            skip_install_after: Content::with_text(&lines_to_text(
                doc.skip_install_packages_after_compilation
                    .as_ref()
                    .unwrap_or(&doc.skip_install_packages),
            )),
            sys_update_ignore: Content::with_text(&lines_to_text(
                &doc.system_update.ignore_packages,
            )),
        }
    }

    pub fn content(&self, field: PackageListField) -> &Content {
        match field {
            PackageListField::ManualUpdate => &self.manual_update,
            PackageListField::SkipInstall => &self.skip_install,
            PackageListField::SkipInstallAfter => &self.skip_install_after,
            PackageListField::SysUpdateIgnore => &self.sys_update_ignore,
        }
    }

    pub fn content_mut(&mut self, field: PackageListField) -> &mut Content {
        match field {
            PackageListField::ManualUpdate => &mut self.manual_update,
            PackageListField::SkipInstall => &mut self.skip_install,
            PackageListField::SkipInstallAfter => &mut self.skip_install_after,
            PackageListField::SysUpdateIgnore => &mut self.sys_update_ignore,
        }
    }

    pub fn apply_field(&self, field: PackageListField, doc: &mut ConfigDocument) {
        let lines = parse_lines(&self.content(field).text());
        match field {
            PackageListField::ManualUpdate => doc.manual_update_packages = lines,
            PackageListField::SkipInstall => doc.skip_install_packages = lines,
            PackageListField::SkipInstallAfter => {
                if doc.skip_install_packages_after_compilation.is_some() {
                    doc.skip_install_packages_after_compilation = Some(lines);
                }
            }
            PackageListField::SysUpdateIgnore => doc.system_update.ignore_packages = lines,
        }
    }

    pub fn apply_all(&self, doc: &mut ConfigDocument) {
        for field in [
            PackageListField::ManualUpdate,
            PackageListField::SkipInstall,
            PackageListField::SkipInstallAfter,
            PackageListField::SysUpdateIgnore,
        ] {
            self.apply_field(field, doc);
        }
    }
}

pub fn lines_to_text(lines: &[String]) -> String {
    lines.join("\n")
}

pub fn parse_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_package_lines() {
        let input = "linux-cachyos\n\nnvidia\n";
        let parsed = parse_lines(input);
        assert_eq!(parsed, vec!["linux-cachyos", "nvidia"]);
        assert_eq!(lines_to_text(&parsed), "linux-cachyos\nnvidia");
    }
}
