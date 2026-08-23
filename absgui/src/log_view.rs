use crate::abs_runner::strip_ansi;
use crate::terminal_themes::LogPalette;
use iced::advanced::clipboard::{self, Clipboard};
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::text::{self, Text};
use iced::advanced::widget::{tree, Tree};
use iced::advanced::{Shell, Widget};
use iced::alignment;
use iced::keyboard;
use iced::mouse;
use iced::{Background, Color, Element, Event, Font, Length, Pixels, Point, Rectangle, Size};
use std::cmp::Ordering;
use std::collections::VecDeque;

const FONT_SIZE: f32 = 14.0;
const LINE_HEIGHT: f32 = 18.0;
const PAD_X: f32 = 10.0;
const PAD_Y: f32 = 10.0;
const CHAR_W: f32 = FONT_SIZE * 0.6;

/// Fast log body: layout is O(lines), draw only paints the visible rows.
pub struct LogLines<'a> {
    lines: &'a VecDeque<String>,
    palette: LogPalette,
    placeholder: Option<&'a str>,
}

impl<'a> LogLines<'a> {
    pub fn new(lines: &'a VecDeque<String>, palette: LogPalette) -> Self {
        Self {
            lines,
            palette,
            placeholder: None,
        }
    }

    pub fn placeholder(mut self, text: &'a str) -> Self {
        self.placeholder = Some(text);
        self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Caret {
    line: usize,
    col: usize,
}

impl PartialOrd for Caret {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Caret {
    fn cmp(&self, other: &Self) -> Ordering {
        self.line.cmp(&other.line).then(self.col.cmp(&other.col))
    }
}

#[derive(Debug, Default)]
struct State {
    anchor: Caret,
    head: Caret,
    dragging: bool,
}

impl State {
    fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    fn range(&self) -> (Caret, Caret) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}

fn hit_test(pos: Point, bounds: Rectangle, n_lines: usize) -> Caret {
    if n_lines == 0 {
        return Caret::default();
    }
    let y = (pos.y - bounds.y - PAD_Y).max(0.0);
    let line = ((y / LINE_HEIGHT) as usize).min(n_lines - 1);
    let x = (pos.x - bounds.x - PAD_X).max(0.0);
    let col = (x / CHAR_W).round().max(0.0) as usize;
    Caret { line, col }
}

/// Scrollable translates `cursor` into content space; the raw `Event` position stays window-local.
fn caret_from_cursor(cursor: mouse::Cursor, bounds: Rectangle, n_lines: usize) -> Option<Caret> {
    cursor
        .land()
        .position()
        .map(|pos| hit_test(pos, bounds, n_lines))
}

fn chars_range(s: &str, start: usize, end: usize) -> String {
    let n = s.chars().count();
    let start = start.min(n);
    let end = end.min(n);
    if start >= end {
        String::new()
    } else {
        s.chars().skip(start).take(end - start).collect()
    }
}

fn visible_text(line: &str) -> String {
    strip_ansi(line)
}

fn selected_text(lines: &VecDeque<String>, a: Caret, b: Caret) -> String {
    if lines.is_empty() || a == b {
        return String::new();
    }
    let (start, end) = if a <= b { (a, b) } else { (b, a) };
    let last = lines.len() - 1;
    if start.line > last {
        return String::new();
    }
    let start_line = start.line;
    let end_line = end.line.min(last);
    let first_plain = lines
        .get(start_line)
        .map(|s| visible_text(s))
        .unwrap_or_default();
    if start_line == end_line {
        return chars_range(&first_plain, start.col, end.col);
    }
    let mut out = String::new();
    out.push_str(&chars_range(
        &first_plain,
        start.col,
        first_plain.chars().count(),
    ));
    for i in (start_line + 1)..end_line {
        out.push('\n');
        if let Some(line) = lines.get(i) {
            out.push_str(&visible_text(line));
        }
    }
    out.push('\n');
    if let Some(last_line) = lines.get(end_line) {
        let plain = visible_text(last_line);
        out.push_str(&chars_range(&plain, 0, end.col));
    }
    out
}

fn copy_selection(state: &State, lines: &VecDeque<String>, clipboard: &mut dyn Clipboard) {
    let text = selected_text(lines, state.anchor, state.head);
    if text.is_empty() {
        return;
    }
    clipboard.write(clipboard::Kind::Standard, text.clone());
    clipboard.write(clipboard::Kind::Primary, text);
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer> for LogLines<'_>
where
    Renderer: text::Renderer<Font = Font>,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Fill,
            height: Length::Shrink,
        }
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let rows = if self.lines.is_empty() {
            1
        } else {
            self.lines.len()
        };
        let height = PAD_Y * 2.0 + rows as f32 * LINE_HEIGHT;
        let size = limits.resolve(Length::Fill, Length::Fixed(height), Size::new(0.0, height));
        layout::Node::new(size)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let state = tree.state.downcast_mut::<State>();
        let n_lines = self.lines.len();
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(pos) = cursor.position_over(bounds) else {
                    return;
                };
                let caret = hit_test(pos, bounds, n_lines);
                state.anchor = caret;
                state.head = caret;
                state.dragging = true;
                shell.request_redraw();
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) if state.dragging => {
                if let Some(caret) = caret_from_cursor(cursor, bounds, n_lines) {
                    state.head = caret;
                    shell.request_redraw();
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if state.dragging => {
                if let Some(caret) = caret_from_cursor(cursor, bounds, n_lines) {
                    state.head = caret;
                }
                state.dragging = false;
                if !state.is_empty() {
                    copy_selection(state, self.lines, clipboard);
                }
                shell.request_redraw();
                shell.capture_event();
            }
            Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                if !modifiers.command() {
                    return;
                }
                let keyboard::Key::Character(c) = key.as_ref() else {
                    return;
                };
                if !c.eq_ignore_ascii_case("c") || state.is_empty() {
                    return;
                }
                if cursor.is_over(bounds) {
                    copy_selection(state, self.lines, clipboard);
                    shell.capture_event();
                }
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        if cursor.is_over(layout.bounds()) {
            mouse::Interaction::Text
        } else {
            mouse::Interaction::None
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let Some(clip) = bounds.intersection(viewport) else {
            return;
        };
        let text_left = bounds.x + PAD_X;
        let text_width = (bounds.width - PAD_X * 2.0).max(1.0);
        let text_top = bounds.y + PAD_Y;

        if self.lines.is_empty() {
            if let Some(placeholder) = self.placeholder {
                fill_line(
                    renderer,
                    placeholder,
                    text_left,
                    text_top,
                    text_width,
                    self.palette.hint,
                    clip,
                );
            }
            return;
        }

        let state = tree.state.downcast_ref::<State>();
        let start = ((clip.y - text_top) / LINE_HEIGHT).floor().max(0.0) as usize;
        let end = (((clip.y + clip.height - text_top) / LINE_HEIGHT).ceil() as usize + 1)
            .min(self.lines.len());

        if !state.is_empty() {
            let (sel_a, sel_b) = state.range();
            for i in start..end {
                let n_chars = self
                    .lines
                    .get(i)
                    .map(|s| visible_text(s).chars().count())
                    .unwrap_or(0);
                if let Some(rect) =
                    selection_rect(i, n_chars, sel_a, sel_b, text_left, text_top, text_width)
                {
                    if let Some(quad_bounds) = rect.intersection(&clip) {
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds: quad_bounds,
                                ..renderer::Quad::default()
                            },
                            Background::Color(self.palette.selection),
                        );
                    }
                }
            }
        }

        for i in start..end {
            let Some(line) = self.lines.get(i) else {
                break;
            };
            let y = text_top + i as f32 * LINE_HEIGHT;
            fill_colored_line(renderer, line, text_left, y, text_width, self.palette, clip);
        }
    }
}

fn selection_rect(
    line: usize,
    line_chars: usize,
    start: Caret,
    end: Caret,
    text_left: f32,
    text_top: f32,
    text_width: f32,
) -> Option<Rectangle> {
    if line < start.line || line > end.line {
        return None;
    }
    let (col0, col1) = if start.line == end.line {
        (start.col, end.col)
    } else if line == start.line {
        (start.col, line_chars.max(start.col).saturating_add(1))
    } else if line == end.line {
        (0, end.col)
    } else {
        (0, line_chars.saturating_add(1))
    };
    if col1 <= col0 {
        return None;
    }
    let x0 = (col0 as f32 * CHAR_W).min(text_width);
    let x1 = (col1 as f32 * CHAR_W).min(text_width).max(x0 + 1.0);
    Some(Rectangle {
        x: text_left + x0,
        y: text_top + line as f32 * LINE_HEIGHT,
        width: x1 - x0,
        height: LINE_HEIGHT,
    })
}

fn fill_colored_line<Renderer: text::Renderer<Font = Font>>(
    renderer: &mut Renderer,
    line: &str,
    x: f32,
    y: f32,
    width: f32,
    palette: LogPalette,
    clip: Rectangle,
) {
    let spans = colorize_line(line, palette);
    let right = x + width;
    let mut cx = x;
    for (piece, color) in spans {
        if cx >= right {
            break;
        }
        fill_line(renderer, &piece, cx, y, (right - cx).max(1.0), color, clip);
        cx += piece.chars().count() as f32 * CHAR_W;
    }
}

fn colorize_line(line: &str, palette: LogPalette) -> Vec<(String, Color)> {
    if line.contains('\u{1b}') {
        let spans = ansi_spans(line, palette);
        if !spans.is_empty() {
            return spans;
        }
    }
    heuristic_spans(line, palette)
}

fn ansi_spans(line: &str, palette: LogPalette) -> Vec<(String, Color)> {
    let mut spans = Vec::new();
    let mut current = String::new();
    let mut ink = palette.fg;
    let mut bold = false;
    let mut chars = line.chars().peekable();
    let flush = |spans: &mut Vec<(String, Color)>, current: &mut String, ink: Color| {
        if !current.is_empty() {
            spans.push((std::mem::take(current), ink));
        }
    };
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            let mut params = String::new();
            let mut cmd = 'm';
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() || next == '~' {
                    cmd = next;
                    break;
                }
                params.push(next);
            }
            if cmd == 'm' {
                flush(&mut spans, &mut current, ink);
                apply_sgr(&params, &mut ink, &mut bold, palette);
            }
            continue;
        }
        current.push(c);
    }
    flush(&mut spans, &mut current, ink);
    spans
}

fn apply_sgr(params: &str, ink: &mut Color, bold: &mut bool, palette: LogPalette) {
    if params.is_empty() {
        *bold = false;
        *ink = palette.fg;
        return;
    }
    for code in params.split(';') {
        let Ok(n) = code.parse::<u16>() else {
            continue;
        };
        match n {
            0 => {
                *bold = false;
                *ink = palette.fg;
            }
            1 => *bold = true,
            2 => *ink = palette.hint,
            22 => *bold = false,
            30..=37 => {
                let i = (n - 30) as usize;
                *ink = if *bold {
                    palette.bright[i]
                } else {
                    palette.ansi[i]
                };
            }
            39 => *ink = palette.fg,
            90..=97 => *ink = palette.bright[(n - 90) as usize],
            _ => {}
        }
    }
}

fn heuristic_spans(line: &str, palette: LogPalette) -> Vec<(String, Color)> {
    let trimmed = line.trim_start();
    let pad = &line[..line.len() - trimmed.len()];
    let mut out = Vec::new();
    if !pad.is_empty() {
        out.push((pad.to_string(), palette.fg));
    }
    let push_prefix = |prefix: &str, color: Color| -> Option<Vec<(String, Color)>> {
        let rest = trimmed.strip_prefix(prefix)?;
        let mut spans = out.clone();
        spans.push((prefix.to_string(), color));
        if !rest.is_empty() {
            spans.push((rest.to_string(), palette.fg));
        }
        Some(spans)
    };
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("==> error") {
        out.push((trimmed.to_string(), palette.red()));
        return out;
    }
    if let Some(spans) = push_prefix("==>", palette.green()) {
        return spans;
    }
    if let Some(spans) = push_prefix("->", palette.cyan()) {
        return spans;
    }
    if let Some(spans) = push_prefix("::", palette.blue()) {
        return spans;
    }
    if let Some(spans) = push_prefix("[stderr]", palette.yellow()) {
        return spans;
    }
    if lower.starts_with("warning:") {
        out.push((trimmed.to_string(), palette.yellow()));
        return out;
    }
    if lower.starts_with("error:") {
        out.push((trimmed.to_string(), palette.red()));
        return out;
    }
    if let Some(spans) = push_prefix("$ ", palette.magenta()) {
        return spans;
    }
    if trimmed.starts_with("--- ") {
        return vec![(line.to_string(), palette.hint)];
    }
    vec![(line.to_string(), palette.fg)]
}

/// iced's `fill_text` cache path drops `Wrapping::None` and wraps against `bounds.width`.
/// Keep that width at least as wide as the unwrapped line so compile commands cannot
/// wrap into the next log row and paint over it.
fn log_line_draw_width(content: &str, pane_width: f32) -> f32 {
    let unwrapped = content.chars().count() as f32 * FONT_SIZE;
    unwrapped.max(pane_width).max(1.0)
}

fn fill_line<Renderer: text::Renderer<Font = Font>>(
    renderer: &mut Renderer,
    content: &str,
    x: f32,
    y: f32,
    width: f32,
    color: Color,
    clip: Rectangle,
) {
    renderer.fill_text(
        Text {
            content: content.to_string(),
            bounds: Size::new(log_line_draw_width(content, width), LINE_HEIGHT),
            size: Pixels(FONT_SIZE),
            line_height: text::LineHeight::Absolute(Pixels(LINE_HEIGHT)),
            font: Font::MONOSPACE,
            align_x: text::Alignment::Left,
            align_y: alignment::Vertical::Top,
            shaping: text::Shaping::Basic,
            wrapping: text::Wrapping::None,
        },
        Point::new(x, y),
        color,
        clip,
    );
}

impl<'a, Message: 'a, Theme, Renderer> From<LogLines<'a>> for Element<'a, Message, Theme, Renderer>
where
    Renderer: text::Renderer<Font = Font> + 'a,
    Theme: 'a,
{
    fn from(widget: LogLines<'a>) -> Self {
        Element::new(widget)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(items: &[&str]) -> VecDeque<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn selected_text_same_line() {
        let log = lines(&["abcdef"]);
        let got = selected_text(&log, Caret { line: 0, col: 1 }, Caret { line: 0, col: 4 });
        assert_eq!(got, "bcd");
    }

    #[test]
    fn selected_text_multi_line() {
        let log = lines(&["ab", "cd", "ef"]);
        let got = selected_text(&log, Caret { line: 0, col: 1 }, Caret { line: 2, col: 1 });
        assert_eq!(got, "b\ncd\ne");
    }

    #[test]
    fn selected_text_empty_when_caret_equal() {
        let log = lines(&["ab"]);
        assert_eq!(
            selected_text(&log, Caret { line: 0, col: 1 }, Caret { line: 0, col: 1 }),
            ""
        );
    }

    #[test]
    fn hit_test_is_relative_to_layout_bounds() {
        let bounds = Rectangle {
            x: 20.0,
            y: 80.0,
            width: 400.0,
            height: 900.0,
        };
        let pos = Point::new(
            20.0 + PAD_X + CHAR_W * 3.0,
            80.0 + PAD_Y + LINE_HEIGHT * 5.0,
        );
        assert_eq!(hit_test(pos, bounds, 40), Caret { line: 5, col: 3 });
    }

    fn pal() -> LogPalette {
        crate::terminal_themes::TerminalTheme::MatchApp.palette(true)
    }

    #[test]
    fn heuristic_colors_abs_prefixes() {
        let palette = pal();
        let spans = colorize_line("==> Building linux-cachyos", palette);
        assert_eq!(spans[0].0, "==>");
        assert_eq!(spans[0].1, palette.green());
        assert_eq!(spans[1].1, palette.fg);
        let warn = colorize_line("warning: skipping", palette);
        assert_eq!(warn[0].1, palette.yellow());
        let err = colorize_line("==> ERROR: failed", palette);
        assert_eq!(err[0].1, palette.red());
    }

    #[test]
    fn ansi_maps_to_theme_palette() {
        let palette = pal();
        let spans = colorize_line("\u{1b}[32m==>\u{1b}[0m rest", palette);
        assert_eq!(spans[0].0, "==>");
        assert_eq!(spans[0].1, palette.ansi[2]);
        assert_eq!(spans[1].0, " rest");
        assert_eq!(spans[1].1, palette.fg);
    }

    #[test]
    fn log_line_draw_width_does_not_wrap_long_compile_lines() {
        let content = format!(
            "[filezilla] /bin/sh ../../libtool --tag=CXX --mode=compile g++ {}",
            "-O3 ".repeat(80)
        );
        let pane_width = 640.0;
        let w = log_line_draw_width(&content, pane_width);
        let unwrapped = content.chars().count() as f32 * CHAR_W;
        assert!(
            w + f32::EPSILON >= unwrapped,
            "draw width {w} wraps a {unwrapped}px compile line into the pane ({pane_width})"
        );
    }
}
