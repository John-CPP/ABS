//! Save-log path templates, formats, and file writers.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_FILENAME: &str = "%date%_%time%_%log_name%.%ext%";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogSaveTarget {
    Build,
    Update,
}

impl LogSaveTarget {
    pub fn log_name(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Update => "update",
        }
    }

    pub fn log_title(self) -> &'static str {
        match self {
            Self::Build => "Build",
            Self::Update => "System-Update",
        }
    }

    pub fn dialog_title(self) -> &'static str {
        match self {
            Self::Build => abs_i18n::t("gui.log.save_build"),
            Self::Update => abs_i18n::t("gui.log.save_update"),
        }
    }

    pub fn settings_label(self) -> &'static str {
        match self {
            Self::Build => abs_i18n::t("gui.log.settings_build"),
            Self::Update => abs_i18n::t("gui.log.settings_update"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogSaveFormat {
    #[default]
    Txt,
    Html,
    Pdf,
    Md,
}

impl LogSaveFormat {
    pub const ALL: [Self; 4] = [Self::Txt, Self::Html, Self::Pdf, Self::Md];

    pub fn ext(self) -> &'static str {
        match self {
            Self::Txt => "txt",
            Self::Html => "html",
            Self::Pdf => "pdf",
            Self::Md => "md",
        }
    }

    pub fn filter_exts(self) -> &'static [&'static str] {
        match self {
            Self::Txt => &["txt", "log", "text"],
            Self::Html => &["html", "htm"],
            Self::Pdf => &["pdf"],
            Self::Md => &["md", "markdown"],
        }
    }

    pub fn from_ext(ext: &str) -> Option<Self> {
        match ext
            .trim()
            .trim_start_matches('.')
            .to_ascii_lowercase()
            .as_str()
        {
            "txt" | "log" | "text" => Some(Self::Txt),
            "html" | "htm" => Some(Self::Html),
            "pdf" => Some(Self::Pdf),
            "md" | "markdown" => Some(Self::Md),
            _ => None,
        }
    }
}

impl fmt::Display for LogSaveFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.ext())
    }
}

pub struct ExpandCtx {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub wday: u32,
    pub unix: i64,
    pub target: LogSaveTarget,
    pub format: LogSaveFormat,
    pub hostname: String,
    pub user: String,
    pub home: String,
    pub kernel: String,
    pub pid: u32,
    pub version: String,
}

impl ExpandCtx {
    pub fn now(target: LogSaveTarget, format: LogSaveFormat) -> Self {
        let (tm, unix) = local_now();
        let (hostname, kernel) = uname_pair();
        Self {
            year: tm.tm_year + 1900,
            month: (tm.tm_mon + 1).max(1) as u32,
            day: tm.tm_mday.max(1) as u32,
            hour: tm.tm_hour.max(0) as u32,
            minute: tm.tm_min.max(0) as u32,
            second: tm.tm_sec.clamp(0, 60) as u32,
            wday: tm.tm_wday.clamp(0, 6) as u32,
            unix,
            target,
            format,
            hostname,
            user: std::env::var("USER")
                .or_else(|_| std::env::var("LOGNAME"))
                .unwrap_or_else(|_| "user".into()),
            home: dirs::home_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "/".into()),
            kernel,
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").into(),
        }
    }

    fn hour12(&self) -> u32 {
        let h = self.hour % 12;
        if h == 0 {
            12
        } else {
            h
        }
    }

    fn ampm(&self) -> &'static str {
        if self.hour < 12 {
            "AM"
        } else {
            "PM"
        }
    }

    fn month_name(&self, short: bool) -> &'static str {
        const LONG: [&str; 12] = [
            "January",
            "February",
            "March",
            "April",
            "May",
            "June",
            "July",
            "August",
            "September",
            "October",
            "November",
            "December",
        ];
        const SHORT: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let i = self.month.clamp(1, 12) as usize - 1;
        if short {
            SHORT[i]
        } else {
            LONG[i]
        }
    }

    fn weekday(&self, short: bool) -> &'static str {
        const LONG: [&str; 7] = [
            "Sunday",
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
        ];
        const SHORT: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        let i = (self.wday as usize).min(6);
        if short {
            SHORT[i]
        } else {
            LONG[i]
        }
    }
}

fn lookup(token: &str, ctx: &ExpandCtx) -> Option<String> {
    let key = token.trim().to_ascii_lowercase().replace('-', "_");
    let pad2 = |n: u32| format!("{n:02}");
    Some(match key.as_str() {
        "date" => format!("{:04}-{:02}-{:02}", ctx.year, ctx.month, ctx.day),
        "time" => format!("{:02}-{:02}-{:02}", ctx.hour, ctx.minute, ctx.second),
        "year" | "yyyy" | "y" => format!("{:04}", ctx.year),
        "yy" => format!("{:02}", ctx.year.rem_euclid(100)),
        "month" | "mm" => pad2(ctx.month),
        "mon" => ctx.month_name(true).into(),
        "month_name" => ctx.month_name(false).into(),
        "day" | "dd" => pad2(ctx.day),
        "hour" | "hh" => pad2(ctx.hour),
        "hour12" | "h12" => pad2(ctx.hour12()),
        "minute" | "min" | "mi" => pad2(ctx.minute),
        "second" | "sec" | "ss" => pad2(ctx.second),
        "ampm" | "am_pm" => ctx.ampm().into(),
        "weekday" | "day_name" => ctx.weekday(false).into(),
        "dow" => ctx.weekday(true).into(),
        "timestamp" => format!(
            "{:04}{:02}{:02}-{:02}{:02}{:02}",
            ctx.year, ctx.month, ctx.day, ctx.hour, ctx.minute, ctx.second
        ),
        "iso" => format!(
            "{:04}-{:02}-{:02}T{:02}-{:02}-{:02}",
            ctx.year, ctx.month, ctx.day, ctx.hour, ctx.minute, ctx.second
        ),
        "unix" | "epoch" => ctx.unix.to_string(),
        "log_name" => ctx.target.log_name().into(),
        "log_title" => ctx.target.log_title().into(),
        "ext" | "extension" => ctx.format.ext().into(),
        "hostname" | "pc" | "pc_name" | "computer" | "host" => ctx.hostname.clone(),
        "user" | "username" => ctx.user.clone(),
        "kernel" | "uname" => ctx.kernel.clone(),
        "pid" => ctx.pid.to_string(),
        "home" => ctx.home.clone(),
        "version" | "absgui_version" | "abs_version" => ctx.version.clone(),
        _ => return None,
    })
}

/// Expand `%token%` placeholders. `%%` becomes a literal `%`. Unknown tokens are kept as-is.
pub fn expand_template(template: &str, ctx: &ExpandCtx) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while !rest.is_empty() {
        let Some(start) = rest.find('%') else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        rest = &rest[start + 1..];
        if rest.starts_with('%') {
            out.push('%');
            rest = &rest[1..];
            continue;
        }
        let Some(end) = rest.find('%') else {
            out.push('%');
            out.push_str(rest);
            break;
        };
        let token = &rest[..end];
        if let Some(value) = lookup(token, ctx) {
            out.push_str(&value);
        } else {
            out.push('%');
            out.push_str(token);
            out.push('%');
        }
        rest = &rest[end + 1..];
    }
    out
}

fn filename_to_keep(existing: &str) -> Option<&str> {
    if existing.is_empty() {
        return None;
    }
    let path = Path::new(existing);
    let name = path.file_name()?.to_str()?;
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    if name.contains('%') {
        return Some(name);
    }
    if path.extension().is_some() {
        return Some(name);
    }
    None
}

/// Join `folder` with the filename already in `existing`, or the default template.
pub fn apply_folder(existing: &str, folder: &str, default_filename: &str) -> String {
    let filename = filename_to_keep(existing.trim()).unwrap_or(default_filename);
    Path::new(folder.trim())
        .join(filename)
        .display()
        .to_string()
}

/// After a Save-file dialog, keep the filename template and use the chosen directory.
pub fn remember_save_dir(template: &str, saved_file: &str) -> String {
    let dir = Path::new(saved_file)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| ".".into());
    apply_folder(template, &dir, DEFAULT_FILENAME)
}

fn looks_like_directory(path: &str) -> bool {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return true;
    }
    if trimmed.ends_with('/') {
        return true;
    }
    let p = Path::new(trimmed);
    if p.exists() && p.is_dir() {
        return true;
    }
    filename_to_keep(trimmed).is_none()
}

pub fn suggested_save_path(template: &str, ctx: &ExpandCtx) -> String {
    let t = template.trim();
    let filled = if t.is_empty() {
        let home = ctx.home.trim_end_matches('/');
        format!("{home}/{DEFAULT_FILENAME}")
    } else {
        t.to_string()
    };
    let expanded = expand_template(&filled, ctx);
    if looks_like_directory(&expanded) {
        let dir = expanded.trim_end_matches('/');
        let name = expand_template(DEFAULT_FILENAME, ctx);
        format!("{dir}/{name}")
    } else {
        expanded
    }
}

const KNOWN_EXTS: [&str; 7] = [".txt", ".html", ".htm", ".pdf", ".md", ".markdown", ".log"];

fn has_ext_token(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("%ext%") || lower.contains("%extension%")
}

/// If the path ends with a known log extension, swap it for the selected format.
pub fn replace_known_extension(path: &str, format: LogSaveFormat) -> String {
    let path = path.trim();
    if path.is_empty() {
        return String::new();
    }
    let Some(name) = Path::new(path).file_name().and_then(|s| s.to_str()) else {
        return path.to_string();
    };
    if has_ext_token(name) {
        return path.to_string();
    }
    let lower = name.to_ascii_lowercase();
    for ext in KNOWN_EXTS {
        if lower.ends_with(ext) {
            let stem = &name[..name.len() - ext.len()];
            let parent = Path::new(path).parent();
            let new_name = format!("{stem}.{}", format.ext());
            return match parent.filter(|p| !p.as_os_str().is_empty()) {
                Some(dir) => dir.join(new_name).display().to_string(),
                None => new_name,
            };
        }
    }
    path.to_string()
}

pub fn format_from_path(path: &str) -> Option<LogSaveFormat> {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .and_then(LogSaveFormat::from_ext)
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    out
}

fn render_html(text: &str) -> String {
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <title>ABS log</title>\n<style>\n\
         body {{ background:#111; color:#ddd; font: 13px/1.45 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; margin: 16px; }}\n\
         pre {{ white-space: pre-wrap; word-break: break-word; }}\n\
         </style>\n</head>\n<body>\n<pre>{}</pre>\n</body>\n</html>\n",
        html_escape(text)
    )
}

fn render_markdown(text: &str) -> String {
    let fence = if text.contains("````") {
        "~~~~~"
    } else if text.contains("```") {
        "````"
    } else {
        "```"
    };
    format!("# ABS log\n\n{fence}\n{text}\n{fence}\n")
}

fn pdf_escape(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for ch in line.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '(' => out.push_str("\\("),
            ')' => out.push_str("\\)"),
            '\t' => out.push_str("    "),
            c if (c as u32) >= 32 && (c as u32) <= 126 => out.push(c),
            _ => out.push('?'),
        }
    }
    out
}

fn wrap_pdf_lines(text: &str) -> Vec<String> {
    const WIDTH: usize = 96;
    let mut lines = Vec::new();
    for raw in text.split('\n') {
        let mut rest = raw;
        if rest.is_empty() {
            lines.push(String::new());
            continue;
        }
        while rest.chars().count() > WIDTH {
            let mut end = WIDTH;
            while !rest.is_char_boundary(end) && end > 0 {
                end -= 1;
            }
            if end == 0 {
                end = rest.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
            }
            lines.push(rest[..end].to_string());
            rest = &rest[end..];
        }
        lines.push(rest.to_string());
    }
    lines
}

struct Pdf {
    buf: Vec<u8>,
    offsets: Vec<usize>,
}

impl Pdf {
    fn new() -> Self {
        Self {
            buf: b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec(),
            offsets: vec![0],
        }
    }

    fn add(&mut self, body: &str) -> usize {
        self.offsets.push(self.buf.len());
        let id = self.offsets.len() - 1;
        self.buf
            .extend_from_slice(format!("{id} 0 obj\n{body}\nendobj\n").as_bytes());
        id
    }

    fn add_stream(&mut self, data: &[u8]) -> usize {
        self.offsets.push(self.buf.len());
        let id = self.offsets.len() - 1;
        let header = format!("{id} 0 obj\n<< /Length {} >>\nstream\n", data.len());
        self.buf.extend_from_slice(header.as_bytes());
        self.buf.extend_from_slice(data);
        self.buf.extend_from_slice(b"\nendstream\nendobj\n");
        id
    }

    fn finish(mut self, catalog_id: usize) -> Vec<u8> {
        let xref = self.buf.len();
        let n = self.offsets.len();
        self.buf
            .extend_from_slice(format!("xref\n0 {n}\n").as_bytes());
        self.buf.extend_from_slice(b"0000000000 65535 f \n");
        for off in self.offsets.iter().skip(1) {
            self.buf
                .extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        self.buf.extend_from_slice(
            format!("trailer\n<< /Size {n} /Root {catalog_id} 0 R >>\nstartxref\n{xref}\n%%EOF\n")
                .as_bytes(),
        );
        self.buf
    }
}

fn page_stream(lines: &[String]) -> Vec<u8> {
    let mut s = String::from("BT\n/F1 9 Tf\n36 756 Td\n");
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            s.push_str("0 -10 Td\n");
        }
        s.push('(');
        s.push_str(&pdf_escape(line));
        s.push_str(") Tj\n");
    }
    s.push_str("ET\n");
    s.into_bytes()
}

fn render_pdf(text: &str) -> Vec<u8> {
    const LINES_PER_PAGE: usize = 70;
    let wrapped = wrap_pdf_lines(text);
    let chunks: Vec<&[String]> = if wrapped.is_empty() {
        Vec::new()
    } else {
        wrapped.chunks(LINES_PER_PAGE).collect()
    };

    let mut pdf = Pdf::new();
    let font_id = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>");
    let mut content_ids = Vec::new();
    if chunks.is_empty() {
        content_ids.push(pdf.add_stream(&page_stream(&[])));
    } else {
        for chunk in chunks {
            content_ids.push(pdf.add_stream(&page_stream(chunk)));
        }
    }
    let n = content_ids.len();
    let first_page_id = font_id + n + 1;
    let pages_id = first_page_id + n;
    let catalog_id = pages_id + 1;

    let mut page_ids = Vec::new();
    for cid in &content_ids {
        let id = pdf.add(&format!(
            "<< /Type /Page /Parent {pages_id} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font_id} 0 R >> >> /Contents {cid} 0 R >>"
        ));
        page_ids.push(id);
    }
    debug_assert_eq!(page_ids.first().copied(), Some(first_page_id));
    let kids = page_ids
        .iter()
        .map(|id| format!("{id} 0 R"))
        .collect::<Vec<_>>()
        .join(" ");
    let actual_pages = pdf.add(&format!(
        "<< /Type /Pages /Kids [{kids}] /Count {} >>",
        page_ids.len()
    ));
    debug_assert_eq!(actual_pages, pages_id);
    let actual_catalog = pdf.add(&format!("<< /Type /Catalog /Pages {pages_id} 0 R >>"));
    debug_assert_eq!(actual_catalog, catalog_id);
    pdf.finish(catalog_id)
}

pub fn write_log(path: &str, format: LogSaveFormat, text: &str) -> Result<(), String> {
    let path = Path::new(path);
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| format!("create dir: {e}"))?;
    }
    match format {
        LogSaveFormat::Txt => fs::write(path, text).map_err(|e| format!("write: {e}")),
        LogSaveFormat::Html => {
            fs::write(path, render_html(text)).map_err(|e| format!("write: {e}"))
        }
        LogSaveFormat::Md => {
            fs::write(path, render_markdown(text)).map_err(|e| format!("write: {e}"))
        }
        LogSaveFormat::Pdf => fs::write(path, render_pdf(text)).map_err(|e| format!("write: {e}")),
    }
}

fn local_now() -> (libc::tm, i64) {
    unsafe {
        let mut t: libc::time_t = 0;
        libc::time(&mut t);
        let mut tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        (tm, t as i64)
    }
}

fn uname_pair() -> (String, String) {
    unsafe {
        let mut u: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut u) != 0 {
            return ("unknown".into(), "unknown".into());
        }
        (c_to_string(&u.nodename), c_to_string(&u.release))
    }
}

fn c_to_string(buf: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .copied()
        .map(|c| c as u8)
        .take_while(|&b| b != 0)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

pub fn dialog_directory_hint(template: &str, ctx: &ExpandCtx) -> PathBuf {
    let suggested = suggested_save_path(template, ctx);
    Path::new(&suggested)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ExpandCtx {
        ExpandCtx {
            year: 2026,
            month: 8,
            day: 13,
            hour: 16,
            minute: 1,
            second: 9,
            wday: 4,
            unix: 1_786_000_000,
            target: LogSaveTarget::Update,
            format: LogSaveFormat::Html,
            hostname: "workstation".into(),
            user: "john".into(),
            home: "/home/john".into(),
            kernel: "6.15.0-cachyos".into(),
            pid: 4242,
            version: "2.0.0".into(),
        }
    }

    #[test]
    fn expands_default_filename() {
        let got = expand_template(DEFAULT_FILENAME, &ctx());
        assert_eq!(got, "2026-08-13_16-01-09_update.html");
    }

    #[test]
    fn each_format_filter_includes_primary_ext() {
        for format in LogSaveFormat::ALL {
            assert!(
                format.filter_exts().contains(&format.ext()),
                "{format} filter must include .{ext}",
                ext = format.ext()
            );
        }
    }

    #[test]
    fn expands_rich_tokens() {
        let t = "%year%-%mm%-%dd% %hh%:%min%:%ss% %weekday% %hostname% %user% %kernel% %pid% %home% %log_title% %ampm% %hour12% %mon% %%";
        let got = expand_template(t, &ctx());
        assert_eq!(
            got,
            "2026-08-13 16:01:09 Thursday workstation john 6.15.0-cachyos 4242 /home/john System-Update PM 04 Aug %"
        );
    }

    #[test]
    fn leaves_unknown_tokens() {
        assert_eq!(expand_template("%nope%", &ctx()), "%nope%");
    }

    #[test]
    fn apply_folder_uses_default_when_empty() {
        assert_eq!(
            apply_folder("", "/tmp/logs", DEFAULT_FILENAME),
            format!("/tmp/logs/{DEFAULT_FILENAME}")
        );
    }

    #[test]
    fn apply_folder_preserves_template_name() {
        assert_eq!(
            apply_folder("/old/%year%_%log_name%.%ext%", "/new/out", DEFAULT_FILENAME),
            "/new/out/%year%_%log_name%.%ext%"
        );
    }

    #[test]
    fn apply_folder_preserves_concrete_filename() {
        assert_eq!(
            apply_folder("/old/absgui-build.log", "/new", DEFAULT_FILENAME),
            "/new/absgui-build.log"
        );
    }

    #[test]
    fn apply_folder_treats_bare_dir_as_catalog() {
        assert_eq!(
            apply_folder("/old/logs", "/new", DEFAULT_FILENAME),
            format!("/new/{DEFAULT_FILENAME}")
        );
    }

    #[test]
    fn remember_dir_keeps_template() {
        let t = "/home/john/logs/%date%_%time%_%log_name%.%ext%";
        assert_eq!(
            remember_save_dir(t, "/tmp/out/2026-08-13_16-01-09_update.html"),
            "/tmp/out/%date%_%time%_%log_name%.%ext%"
        );
    }

    #[test]
    fn replace_ext_skips_token() {
        let p = "/tmp/%date%_%log_name%.%ext%";
        assert_eq!(replace_known_extension(p, LogSaveFormat::Pdf), p);
    }

    #[test]
    fn replace_ext_swaps_known() {
        assert_eq!(
            replace_known_extension("/tmp/notes.txt", LogSaveFormat::Pdf),
            "/tmp/notes.pdf"
        );
    }

    #[test]
    fn suggested_path_appends_filename_for_dir() {
        let path = suggested_save_path("/tmp/logs", &ctx());
        assert_eq!(path, "/tmp/logs/2026-08-13_16-01-09_update.html");
    }

    #[test]
    fn pdf_starts_with_header() {
        let bytes = render_pdf("hello (world)");
        assert!(bytes.starts_with(b"%PDF-1.4"));
        assert!(bytes.windows(5).any(|w| w == b"%%EOF"));
    }

    #[test]
    fn markdown_escapes_fences() {
        let md = render_markdown("```\ncode\n```");
        assert!(md.contains("````"));
    }
}
