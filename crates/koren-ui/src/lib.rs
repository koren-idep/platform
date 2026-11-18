//! Koren UI — GPU-accelerated, declarative, themeable native Rust UI framework.
//!
//! Built on `wgpu`, `winit`, `taffy`, `cosmic-text`, `accesskit`, and `arboard`
//! with `#![forbid(unsafe_code)]` enforced by workspace lints.

pub mod app;
pub mod draw;
pub mod editor;
pub mod input;
pub mod layout;
pub mod renderer;
pub mod theme;
pub mod widgets;

pub use app::{NativeApp, WindowCommand, run_app};
pub use draw::{
    DrawList, Edges, FontWeight, IconKind, Point, QuadCommand, Rect, Size, TextCommand,
};
pub use editor::{CodeEditorState, LanguageMode};
pub use input::{ClipboardService, FocusId, HitRegion, InputState, KeyChord};
pub use layout::{Column, CrossAlign, Element, Length, Row, Spacer, UiContext, layout_and_paint};
pub use theme::{Color, MetricsTokens, SyntaxTheme, ThemeKind, ThemeTokens};
pub use winit;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::TextEngine;

    #[test]
    fn theme_tokens_match_measured_fleet_dark_and_oled() {
        let dark = ThemeTokens::dark();
        assert_eq!(dark.canvas_bg, Color::from_hex(0x090909));
        assert_eq!(dark.island_bg, Color::from_hex(0x18191b));
        assert_eq!(dark.floating_bg, Color::from_hex(0x252629));
        assert_eq!(dark.floating_border, Color::from_hex(0x414245));
        assert_eq!(dark.pill_active_bg, Color::from_hex(0x28292b));
        assert_eq!(dark.editor_line_active, Color::from_hex(0x152639));

        let oled = ThemeTokens::oled_black();
        assert_eq!(oled.canvas_bg, Color::BLACK);
        assert_eq!(oled.kind, ThemeKind::OledBlack);
    }

    #[test]
    fn focus_id_is_deterministic_and_distinct() {
        assert_eq!(FocusId::from_str("btn.save"), FocusId::from_str("btn.save"));
        assert_ne!(FocusId::from_str("btn.save"), FocusId::from_str("btn.run"));
    }

    #[test]
    fn code_editor_utf16_cursor_and_undo_redo() {
        let mut ed = CodeEditorState::new(
            FocusId::from_str("ed"),
            "let s = \"🦀\";\n",
            LanguageMode::Rust,
        );
        // Move past "let s = \"🦀" -> 10 chars, where '🦀' is 2 UTF-16 code units
        for _ in 0..10 {
            ed.move_cursor(0, 1, false);
        }
        let (line, col_utf16) = ed.cursor_display_pos();
        assert_eq!(line, 1);
        assert_eq!(col_utf16, 12); // 9 ASCII chars + 2 UTF-16 units + 1-based

        ed.insert_str("!");
        assert!(ed.lines[0].contains("🦀!"));
        assert!(ed.undo());
        assert!(!ed.lines[0].contains("🦀!"));
        assert!(ed.redo());
        assert!(ed.lines[0].contains("🦀!"));
    }

    #[test]
    fn layout_and_paint_computes_flex_tree() {
        let theme = ThemeTokens::dark();
        let mut draw = DrawList::new();
        let mut input = InputState::new();
        let mut text = TextEngine::new();
        let mut ctx = UiContext {
            theme: &theme,
            draw: &mut draw,
            input: &mut input,
            text: &mut text,
            scale: 1.0,
        };
        let tree = Column::new()
            .width(Length::Px(400.0))
            .height(Length::Px(200.0))
            .child(
                Row::new()
                    .height(Length::Px(36.0))
                    .background(theme.canvas_bg),
            )
            .child(Column::new().flex_grow(1.0).background(theme.island_bg));

        layout_and_paint(&tree, &mut ctx, Size::new(400.0, 200.0));
        assert_eq!(draw.iter_ops().count(), 2);
    }
}
