//! Native Rust virtualized code editor widget with syntax highlighting and UTF-16 mapping.

use crate::draw::{FontWeight, IconKind, Point, QuadCommand, Rect, TextCommand};
use crate::input::FocusId;
use crate::layout::{UiContext, register_hit};
use crate::theme::{Color, SyntaxTheme};
use crate::widgets::paint_scrollbar;
use accesskit::Role;
use unicode_segmentation::UnicodeSegmentation;

/// Language mode for syntax tokenization.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LanguageMode {
    /// Rust source (`.rs`).
    Rust,
    /// TOML configuration (`.toml`).
    Toml,
    /// JSON data (`.json`).
    Json,
    /// Lua / Luau script (`.lua`, `.luau`).
    Lua,
    /// Markdown document (`.md`).
    Markdown,
    /// Plain text fallback.
    #[default]
    PlainText,
}

impl LanguageMode {
    /// Infers language mode from a file path or extension.
    #[must_use]
    pub fn from_path(path: &str) -> Self {
        let lower = path.to_ascii_lowercase();
        if lower.ends_with(".rs") {
            Self::Rust
        } else if lower.ends_with(".toml") || lower.ends_with(".lock") {
            Self::Toml
        } else if lower.ends_with(".json") || lower.ends_with(".jsonc") {
            Self::Json
        } else if lower.ends_with(".lua") || lower.ends_with(".luau") {
            Self::Lua
        } else if lower.ends_with(".md") || lower.ends_with(".markdown") {
            Self::Markdown
        } else {
            Self::PlainText
        }
    }

    /// Human-readable status bar label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Toml => "TOML",
            Self::Json => "JSON",
            Self::Lua => "Lua",
            Self::Markdown => "Markdown",
            Self::PlainText => "Plain Text",
        }
    }

    /// Compact 2-char tab/tree badge and semantic color.
    #[must_use]
    pub fn badge(self) -> (&'static str, Color) {
        match self {
            Self::Rust => ("RS", Color::from_hex(0xde6b48)),
            Self::Toml => ("[T]", Color::from_hex(0x86868a)),
            Self::Json => ("{}", Color::from_hex(0xe5a93c)),
            Self::Lua => ("LU", Color::from_hex(0x56a8f5)),
            Self::Markdown => ("M↓", Color::from_hex(0x56a8f5)),
            Self::PlainText => ("TXT", Color::from_hex(0x86868a)),
        }
    }
}

#[derive(Clone, Debug)]
struct UndoEntry {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
}

/// Interactive code editor state for one open buffer.
pub struct CodeEditorState {
    /// Focus/hit ID.
    pub id: FocusId,
    /// Buffer lines (UTF-8).
    pub lines: Vec<String>,
    /// Active language mode.
    pub language: LanguageMode,
    /// Zero-based cursor line.
    pub cursor_line: usize,
    /// Zero-based cursor byte/char column (grapheme-aligned char index in `lines[cursor_line]`).
    pub cursor_col: usize,
    /// Optional selection anchor `(line, col)`.
    pub selection_anchor: Option<(usize, usize)>,
    /// Vertical scroll offset in logical pixels.
    pub scroll_y: f32,
    /// Horizontal scroll offset in logical pixels.
    pub scroll_x: f32,
    /// Measured monospace character advance width.
    pub char_advance: f32,
    undo_stack: Vec<UndoEntry>,
    redo_stack: Vec<UndoEntry>,
}

impl CodeEditorState {
    /// Creates a new editor state from UTF-8 text.
    #[must_use]
    pub fn new(id: FocusId, text: &str, language: LanguageMode) -> Self {
        let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
        if text.ends_with('\n') || lines.is_empty() {
            lines.push(String::new());
        }
        Self {
            id,
            lines,
            language,
            cursor_line: 0,
            cursor_col: 0,
            selection_anchor: None,
            scroll_y: 0.0,
            scroll_x: 0.0,
            char_advance: 7.8,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// Serializes the buffer back to a UTF-8 string.
    #[must_use]
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Returns 1-based `(line, utf16_col)` for status bar and LSP/protocol compatibility.
    #[must_use]
    pub fn cursor_display_pos(&self) -> (usize, usize) {
        let line = self.lines.get(self.cursor_line).map_or("", String::as_str);
        let utf16_col: usize = line
            .chars()
            .take(self.cursor_col)
            .map(char::len_utf16)
            .sum();
        (self.cursor_line + 1, utf16_col + 1)
    }

    fn push_undo(&mut self) {
        self.undo_stack.push(UndoEntry {
            lines: self.lines.clone(),
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
        });
        if self.undo_stack.len() > 200 {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    /// Undoes the last edit.
    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo_stack.pop() else {
            return false;
        };
        self.redo_stack.push(UndoEntry {
            lines: self.lines.clone(),
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
        });
        self.lines = prev.lines;
        self.cursor_line = prev.cursor_line;
        self.cursor_col = prev.cursor_col;
        self.selection_anchor = None;
        true
    }

    /// Redoes the last undone edit.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo_stack.pop() else {
            return false;
        };
        self.undo_stack.push(UndoEntry {
            lines: self.lines.clone(),
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
        });
        self.lines = next.lines;
        self.cursor_line = next.cursor_line;
        self.cursor_col = next.cursor_col;
        self.selection_anchor = None;
        true
    }

    /// Inserts UTF-8 text at the current cursor position.
    pub fn insert_str(&mut self, s: &str) {
        self.push_undo();
        self.selection_anchor = None;
        for ch in s.chars() {
            if ch == '\n' {
                self.split_line_raw();
            } else if ch != '\r' {
                let line = &mut self.lines[self.cursor_line];
                let byte_idx = char_to_byte(line, self.cursor_col);
                line.insert(byte_idx, ch);
                self.cursor_col += 1;
            }
        }
    }

    /// Inserts a newline with basic leading-whitespace auto-indent.
    pub fn insert_newline(&mut self) {
        self.push_undo();
        self.selection_anchor = None;
        let indent: String = self.lines[self.cursor_line]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        self.split_line_raw();
        self.lines[self.cursor_line].insert_str(0, &indent);
        self.cursor_col = indent.chars().count();
    }

    fn split_line_raw(&mut self) {
        let line = &mut self.lines[self.cursor_line];
        let byte_idx = char_to_byte(line, self.cursor_col);
        let tail = line.split_off(byte_idx);
        self.cursor_line += 1;
        self.cursor_col = 0;
        self.lines.insert(self.cursor_line, tail);
    }

    /// Deletes the grapheme/character before the cursor.
    pub fn backspace(&mut self) -> bool {
        if self.cursor_line == 0 && self.cursor_col == 0 {
            return false;
        }
        self.push_undo();
        self.selection_anchor = None;
        if self.cursor_col > 0 {
            let line = &mut self.lines[self.cursor_line];
            let prev_col = prev_grapheme_col(line, self.cursor_col);
            let start = char_to_byte(line, prev_col);
            let end = char_to_byte(line, self.cursor_col);
            line.replace_range(start..end, "");
            self.cursor_col = prev_col;
        } else {
            let tail = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            let prev_len = self.lines[self.cursor_line].chars().count();
            self.lines[self.cursor_line].push_str(&tail);
            self.cursor_col = prev_len;
        }
        true
    }

    /// Deletes the character after the cursor.
    pub fn delete_forward(&mut self) -> bool {
        let line_len = self.lines[self.cursor_line].chars().count();
        if self.cursor_col >= line_len && self.cursor_line + 1 >= self.lines.len() {
            return false;
        }
        self.push_undo();
        self.selection_anchor = None;
        if self.cursor_col < line_len {
            let line = &mut self.lines[self.cursor_line];
            let start = char_to_byte(line, self.cursor_col);
            let end = char_to_byte(line, self.cursor_col + 1);
            line.replace_range(start..end, "");
        } else {
            let next = self.lines.remove(self.cursor_line + 1);
            self.lines[self.cursor_line].push_str(&next);
        }
        true
    }

    /// Moves cursor up/down/left/right.
    pub fn move_cursor(&mut self, d_line: isize, d_col: isize, extend_selection: bool) {
        if extend_selection && self.selection_anchor.is_none() {
            self.selection_anchor = Some((self.cursor_line, self.cursor_col));
        } else if !extend_selection {
            self.selection_anchor = None;
        }
        if d_line != 0 {
            let target = (self.cursor_line as isize + d_line)
                .clamp(0, (self.lines.len().saturating_sub(1)) as isize)
                as usize;
            self.cursor_line = target;
            let max_col = self.lines[self.cursor_line].chars().count();
            self.cursor_col = self.cursor_col.min(max_col);
        }
        if d_col < 0 {
            if self.cursor_col > 0 {
                self.cursor_col = prev_grapheme_col(&self.lines[self.cursor_line], self.cursor_col);
            } else if self.cursor_line > 0 {
                self.cursor_line -= 1;
                self.cursor_col = self.lines[self.cursor_line].chars().count();
            }
        } else if d_col > 0 {
            let max_col = self.lines[self.cursor_line].chars().count();
            if self.cursor_col < max_col {
                self.cursor_col = next_grapheme_col(&self.lines[self.cursor_line], self.cursor_col);
            } else if self.cursor_line + 1 < self.lines.len() {
                self.cursor_line += 1;
                self.cursor_col = 0;
            }
        }
    }

    /// Selects the entire document.
    pub fn select_all(&mut self) {
        self.selection_anchor = Some((0, 0));
        self.cursor_line = self.lines.len().saturating_sub(1);
        self.cursor_col = self.lines[self.cursor_line].chars().count();
    }

    /// Places cursor from a mouse click in logical coordinates relative to `bounds`.
    pub fn click_at(&mut self, bounds: Rect, pos: Point, line_height: f32) {
        let gutter_w = 56.0;
        let rel_y = (pos.y - bounds.y - 6.0 + self.scroll_y).max(0.0);
        let line_idx =
            ((rel_y / line_height).floor() as usize).min(self.lines.len().saturating_sub(1));
        let rel_x = (pos.x - (bounds.x + gutter_w + 8.0) + self.scroll_x).max(0.0);
        let col = (rel_x / self.char_advance.max(4.0)).round() as usize;
        let max_col = self.lines[line_idx].chars().count();
        self.cursor_line = line_idx;
        self.cursor_col = col.min(max_col);
        self.selection_anchor = None;
    }

    /// Paints the virtualized code editor inside `bounds`.
    pub fn paint(&mut self, ctx: &mut UiContext<'_>, bounds: Rect) {
        register_hit(
            ctx,
            self.id,
            bounds,
            true,
            Role::MultilineTextInput,
            "Code Editor",
            0,
        );
        let line_h = ctx.theme.metrics.line_height_code;
        let font_sz = ctx.theme.metrics.font_code;
        self.char_advance =
            ctx.text
                .measure_width("MMMMMMMMMM", font_sz, FontWeight::Regular, true)
                / 10.0;

        let gutter_w = 56.0;
        let code_x = bounds.x + gutter_w + 8.0;
        let top_pad = 6.0;

        ctx.draw.push_clip(bounds);

        let first_line = ((self.scroll_y - top_pad) / line_h).floor().max(0.0) as usize;
        let visible_lines = (bounds.height / line_h).ceil() as usize + 2;
        let end_line = (first_line + visible_lines).min(self.lines.len());

        let focused = ctx.input.focused == Some(self.id);

        for line_idx in first_line..end_line {
            let y = bounds.y + top_pad + line_idx as f32 * line_h - self.scroll_y;
            let is_active = line_idx == self.cursor_line;

            // Current line highlight spanning gutter + editor
            if is_active {
                ctx.draw.push_quad(QuadCommand {
                    rect: Rect::new(bounds.x, y, bounds.width, line_h),
                    color: ctx.theme.editor_line_active,
                    ..Default::default()
                });
            }

            // Line number (right-aligned in gutter)
            let num_str = format!("{}", line_idx + 1);
            let num_w = ctx
                .text
                .measure_width(&num_str, font_sz, FontWeight::Regular, true);
            ctx.draw.push_text(TextCommand {
                pos: Point::new(bounds.x + 36.0 - num_w, y),
                max_width: Some(36.0),
                text: num_str,
                size: font_sz,
                line_height: line_h,
                color: if is_active {
                    ctx.theme.text_primary
                } else {
                    ctx.theme.text_muted
                },
                weight: FontWeight::Regular,
                monospace: true,
                italic: false,
                clip: Some(bounds),
            });

            let line_text = &self.lines[line_idx];
            // Green run gutter icon for Rust `fn main(`
            if self.language == LanguageMode::Rust && line_text.trim_start().starts_with("fn main(")
            {
                ctx.draw.push_icon_layer(
                    0,
                    IconKind::Play,
                    Rect::new(bounds.x + 40.0, y + (line_h - 14.0) * 0.5, 14.0, 14.0),
                    ctx.theme.success,
                );
            }

            // Indent guides (every 4 spaces)
            let leading_spaces = line_text.chars().take_while(|c| *c == ' ').count();
            for step in (4..=leading_spaces).step_by(4) {
                let gx = code_x + (step as f32) * self.char_advance - self.scroll_x;
                if gx > code_x && gx < bounds.x + bounds.width {
                    ctx.draw.push_quad(QuadCommand {
                        rect: Rect::new(gx, y, 1.0, line_h),
                        color: ctx.theme.indent_guide,
                        ..Default::default()
                    });
                }
            }

            // Selection highlight on this line
            if let Some(anchor) = self.selection_anchor {
                let (s_line, s_col, e_line, e_col) =
                    normalize_range(anchor, (self.cursor_line, self.cursor_col));
                if line_idx >= s_line && line_idx <= e_line {
                    let c0 = if line_idx == s_line { s_col } else { 0 };
                    let c1 = if line_idx == e_line {
                        e_col
                    } else {
                        line_text.chars().count()
                    };
                    if c1 > c0 {
                        ctx.draw.push_quad(QuadCommand {
                            rect: Rect::new(
                                code_x + c0 as f32 * self.char_advance - self.scroll_x,
                                y,
                                (c1 - c0) as f32 * self.char_advance,
                                line_h,
                            ),
                            color: ctx.theme.selection_bg,
                            ..Default::default()
                        });
                    }
                }
            }

            // Syntax-highlighted spans
            let spans = tokenize_line(line_text, self.language, &ctx.theme.syntax);
            let mut col_offset = 0usize;
            for (text_slice, color) in spans {
                let span_x = code_x + col_offset as f32 * self.char_advance - self.scroll_x;
                ctx.draw.push_text(TextCommand {
                    pos: Point::new(span_x, y),
                    max_width: None,
                    text: text_slice.to_owned(),
                    size: font_sz,
                    line_height: line_h,
                    color,
                    weight: FontWeight::Regular,
                    monospace: true,
                    italic: false,
                    clip: Some(Rect::new(
                        bounds.x + gutter_w,
                        bounds.y,
                        (bounds.width - gutter_w).max(0.0),
                        bounds.height,
                    )),
                });
                col_offset += text_slice.chars().count();
            }

            // Cursor bar
            if is_active && focused {
                let cx = code_x + self.cursor_col as f32 * self.char_advance - self.scroll_x;
                ctx.draw.push_quad(QuadCommand {
                    rect: Rect::new(cx, y + 2.0, 2.0, line_h - 4.0),
                    color: Color::WHITE,
                    ..Default::default()
                });
            }
        }

        let content_h = self.lines.len() as f32 * line_h + top_pad * 2.0;
        paint_scrollbar(ctx, bounds, self.scroll_y, content_h);
        ctx.draw.pop_clip();
    }
}

fn normalize_range(a: (usize, usize), b: (usize, usize)) -> (usize, usize, usize, usize) {
    if a <= b {
        (a.0, a.1, b.0, b.1)
    } else {
        (b.0, b.1, a.0, a.1)
    }
}

fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map_or(s.len(), |(idx, _)| idx)
}

fn prev_grapheme_col(s: &str, char_idx: usize) -> usize {
    let byte_idx = char_to_byte(s, char_idx);
    let prefix = &s[..byte_idx];
    if let Some((last_start, _)) = prefix.grapheme_indices(true).next_back() {
        prefix[..last_start].chars().count()
    } else {
        0
    }
}

fn next_grapheme_col(s: &str, char_idx: usize) -> usize {
    let byte_idx = char_to_byte(s, char_idx);
    let suffix = &s[byte_idx..];
    if let Some(g) = suffix.graphemes(true).next() {
        char_idx + g.chars().count()
    } else {
        char_idx
    }
}

fn tokenize_line<'a>(
    line: &'a str,
    lang: LanguageMode,
    syn: &SyntaxTheme,
) -> Vec<(&'a str, Color)> {
    if line.is_empty() {
        return Vec::new();
    }
    if lang == LanguageMode::PlainText {
        return vec![(line, syn.plain)];
    }
    if lang == LanguageMode::Markdown {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            return vec![(line, syn.keyword)];
        }
        return vec![(line, syn.plain)];
    }

    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        // Line comment
        if (lang == LanguageMode::Rust || lang == LanguageMode::Json)
            && i + 1 < bytes.len()
            && bytes[i] == b'/'
            && bytes[i + 1] == b'/'
        {
            out.push((&line[i..], syn.comment));
            break;
        }
        if lang == LanguageMode::Lua
            && i + 1 < bytes.len()
            && bytes[i] == b'-'
            && bytes[i + 1] == b'-'
        {
            out.push((&line[i..], syn.comment));
            break;
        }
        if lang == LanguageMode::Toml && bytes[i] == b'#' {
            out.push((&line[i..], syn.comment));
            break;
        }

        // String literal
        if bytes[i] == b'"' || (lang == LanguageMode::Lua && bytes[i] == b'\'') {
            let quote = bytes[i];
            let start = i;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else if bytes[i] == quote {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            out.push((&line[start..i], syn.string));
            continue;
        }

        // Identifier / keyword / type
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = &line[start..i];
            let color = if is_keyword(word, lang) {
                syn.keyword
            } else if word.chars().next().is_some_and(char::is_uppercase) {
                syn.type_name
            } else if i < bytes.len() && (bytes[i] == b'(' || bytes[i] == b'!') {
                syn.function
            } else {
                syn.plain
            };
            out.push((word, color));
            continue;
        }

        // Number
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.' || bytes[i] == b'_')
            {
                i += 1;
            }
            out.push((&line[start..i], syn.number));
            continue;
        }

        // Other UTF-8 char(s) until next interesting token
        let start = i;
        let ch_len = line[i..].chars().next().map_or(1, char::len_utf8);
        i += ch_len;
        out.push((&line[start..i], syn.plain));
    }
    out
}

fn is_keyword(word: &str, lang: LanguageMode) -> bool {
    match lang {
        LanguageMode::Rust => matches!(
            word,
            "use"
                | "mod"
                | "fn"
                | "let"
                | "mut"
                | "pub"
                | "struct"
                | "enum"
                | "impl"
                | "trait"
                | "if"
                | "else"
                | "match"
                | "for"
                | "while"
                | "loop"
                | "return"
                | "self"
                | "Self"
                | "const"
                | "static"
                | "type"
                | "where"
                | "async"
                | "await"
                | "in"
                | "as"
                | "true"
                | "false"
        ),
        LanguageMode::Lua => matches!(
            word,
            "local"
                | "function"
                | "end"
                | "if"
                | "then"
                | "else"
                | "elseif"
                | "for"
                | "while"
                | "do"
                | "return"
                | "true"
                | "false"
                | "nil"
                | "and"
                | "or"
                | "not"
        ),
        LanguageMode::Json | LanguageMode::Toml => matches!(word, "true" | "false" | "null"),
        _ => false,
    }
}
