//! Reusable themed UI widgets for Koren IDE Platform.

use crate::draw::{FontWeight, IconKind, Point, QuadCommand, Rect, TextCommand};
use crate::input::FocusId;
use crate::layout::{Element, UiContext, register_hit};
use crate::theme::Color;
use accesskit::Role;
use taffy::prelude::*;

/// Button visual variant.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ButtonVariant {
    /// Accent-filled primary action button.
    Primary,
    /// Subtle bordered secondary button.
    #[default]
    Secondary,
    /// Transparent background until hovered.
    Ghost,
}

/// Interactive button widget (text, icon, or both).
pub struct Button {
    /// Stable focus/hit ID.
    pub id: FocusId,
    /// Optional button label.
    pub label: String,
    /// Optional leading icon.
    pub icon: Option<IconKind>,
    /// Visual variant.
    pub variant: ButtonVariant,
    /// Optional explicit width.
    pub width: Option<f32>,
    /// Explicit height (defaults to `26.0`).
    pub height: f32,
    /// Semantic icon color override.
    pub icon_color: Option<Color>,
    /// Whether this button is disabled.
    pub disabled: bool,
    /// Render on overlay/modal layer (`0`, `1`, or `2`).
    pub layer: usize,
    /// Left-align label text instead of centering (useful for list/palette rows).
    pub align_left: bool,
}

impl Button {
    /// Creates a secondary button with a text label.
    #[must_use]
    pub fn new(id: FocusId, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            icon: None,
            variant: ButtonVariant::Secondary,
            width: None,
            height: 26.0,
            icon_color: None,
            disabled: false,
            layer: 0,
            align_left: false,
        }
    }

    /// Creates a compact icon-only toolbar button (`26x26` by default).
    #[must_use]
    pub fn icon(id: FocusId, icon: IconKind, accessible_label: impl Into<String>) -> Self {
        Self {
            id,
            label: accessible_label.into(),
            icon: Some(icon),
            variant: ButtonVariant::Ghost,
            width: Some(26.0),
            height: 26.0,
            icon_color: None,
            disabled: false,
            layer: 0,
            align_left: false,
        }
    }

    /// Sets button variant.
    #[must_use]
    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Sets explicit icon color.
    #[must_use]
    pub fn icon_color(mut self, color: Color) -> Self {
        self.icon_color = Some(color);
        self
    }

    /// Sets render layer.
    #[must_use]
    pub fn layer(mut self, layer: usize) -> Self {
        self.layer = layer;
        self
    }

    /// Left-aligns button label text.
    #[must_use]
    pub fn align_left(mut self) -> Self {
        self.align_left = true;
        self
    }

    /// Paints the button directly at `bounds` (useful inside custom composite widgets).
    pub fn paint_at(&self, ctx: &mut UiContext<'_>, bounds: Rect) {
        let icon_only = self.width == Some(26.0) && self.icon.is_some();
        if !self.disabled {
            register_hit(
                ctx,
                self.id,
                bounds,
                true,
                Role::Button,
                self.label.clone(),
                self.layer,
            );
        }

        let hovered = !self.disabled && ctx.input.hovered == Some(self.id);
        let focused = !self.disabled && ctx.input.focused == Some(self.id);

        let (bg, border, fg) = match self.variant {
            ButtonVariant::Primary => (
                if hovered {
                    ctx.theme.accent_hover
                } else {
                    ctx.theme.accent
                },
                Color::TRANSPARENT,
                Color::WHITE,
            ),
            ButtonVariant::Secondary => (
                if hovered {
                    ctx.theme.pill_active_bg
                } else {
                    ctx.theme.hover_bg
                },
                ctx.theme.floating_border,
                ctx.theme.text_primary,
            ),
            ButtonVariant::Ghost => (
                if hovered {
                    ctx.theme.pill_active_bg
                } else {
                    Color::TRANSPARENT
                },
                Color::TRANSPARENT,
                if hovered {
                    ctx.theme.text_primary
                } else {
                    ctx.theme.text_secondary
                },
            ),
        };

        let border_col = if focused { ctx.theme.accent } else { border };
        let border_w = if focused {
            1.5
        } else if border.a > 0.0 {
            1.0
        } else {
            0.0
        };

        if bg.a > 0.0 || border_w > 0.0 {
            ctx.draw.push_quad_layer(
                self.layer,
                QuadCommand {
                    rect: bounds,
                    color: bg,
                    border_color: border_col,
                    border_width: border_w,
                    corner_radius: ctx.theme.metrics.radius_control,
                    ..Default::default()
                },
            );
        }

        if let Some(icon) = self.icon {
            let ic = self.icon_color.unwrap_or(fg);
            let icon_rect = if icon_only {
                bounds.inset(4.0)
            } else {
                Rect::new(
                    bounds.x + 8.0,
                    bounds.y + (bounds.height - 16.0) * 0.5,
                    16.0,
                    16.0,
                )
            };
            ctx.draw.push_icon_layer(self.layer, icon, icon_rect, ic);
        }

        if !icon_only && !self.label.is_empty() {
            let text_x = if self.icon.is_some() {
                bounds.x + 28.0
            } else if self.align_left {
                bounds.x + 10.0
            } else {
                let w = ctx.text.measure_width(
                    &self.label,
                    ctx.theme.metrics.font_ui,
                    FontWeight::Medium,
                    false,
                );
                bounds.x + ((bounds.width - w) * 0.5).max(8.0)
            };
            let text_y = bounds.y + (bounds.height - ctx.theme.metrics.font_ui * 1.25) * 0.5;
            ctx.draw.push_text_layer(
                self.layer,
                TextCommand {
                    pos: Point::new(text_x, text_y),
                    max_width: None,
                    text: self.label.clone(),
                    size: ctx.theme.metrics.font_ui,
                    line_height: ctx.theme.metrics.font_ui * 1.25,
                    color: if self.disabled {
                        ctx.theme.text_muted
                    } else {
                        fg
                    },
                    weight: FontWeight::Medium,
                    monospace: false,
                    italic: false,
                    clip: Some(bounds.inset(2.0)),
                },
            );
        }
    }
}

impl Element for Button {
    fn style(&self) -> Style {
        Style {
            size: Size {
                width: self.width.map_or(Dimension::auto(), Dimension::length),
                height: Dimension::length(self.height),
            },
            min_size: Size {
                width: Dimension::length(self.width.unwrap_or(68.0)),
                height: Dimension::length(self.height),
            },
            ..Default::default()
        }
    }

    fn paint(&self, ctx: &mut UiContext<'_>, bounds: Rect) {
        self.paint_at(ctx, bounds);
    }
}

/// Checkbox widget with aligned label.
pub struct Checkbox {
    /// Stable ID.
    pub id: FocusId,
    /// Label next to the checkbox.
    pub label: String,
    /// Checked state.
    pub checked: bool,
}

impl Checkbox {
    /// Paints the checkbox at `bounds`.
    pub fn paint_at(&self, ctx: &mut UiContext<'_>, bounds: Rect) {
        register_hit(
            ctx,
            self.id,
            bounds,
            true,
            Role::CheckBox,
            self.label.clone(),
            0,
        );
        let hovered = ctx.input.hovered == Some(self.id);
        let box_rect = Rect::new(
            bounds.x,
            bounds.y + (bounds.height - 16.0) * 0.5,
            16.0,
            16.0,
        );
        ctx.draw.push_quad(QuadCommand {
            rect: box_rect,
            color: if self.checked {
                ctx.theme.accent
            } else if hovered {
                ctx.theme.pill_active_bg
            } else {
                ctx.theme.hover_bg
            },
            border_color: if self.checked {
                ctx.theme.accent
            } else {
                ctx.theme.floating_border
            },
            border_width: 1.0,
            corner_radius: ctx.theme.metrics.radius_control,
            ..Default::default()
        });
        if self.checked {
            ctx.draw
                .push_icon_layer(0, IconKind::Check, box_rect.inset(2.0), Color::WHITE);
        }
        ctx.draw.push_text(TextCommand {
            pos: Point::new(
                bounds.x + 24.0,
                bounds.y + (bounds.height - ctx.theme.metrics.font_ui * 1.25) * 0.5,
            ),
            max_width: Some((bounds.width - 24.0).max(0.0)),
            text: self.label.clone(),
            size: ctx.theme.metrics.font_ui,
            line_height: ctx.theme.metrics.font_ui * 1.25,
            color: ctx.theme.text_primary,
            weight: FontWeight::Regular,
            monospace: false,
            italic: false,
            clip: Some(bounds),
        });
    }
}

/// Pill-shaped toggle switch with aligned label.
pub struct Toggle {
    /// Stable ID.
    pub id: FocusId,
    /// Label next to toggle.
    pub label: String,
    /// On/off state.
    pub on: bool,
}

impl Toggle {
    /// Paints the toggle switch at `bounds`.
    pub fn paint_at(&self, ctx: &mut UiContext<'_>, bounds: Rect) {
        register_hit(
            ctx,
            self.id,
            bounds,
            true,
            Role::Switch,
            self.label.clone(),
            0,
        );
        let track = Rect::new(
            bounds.x + bounds.width - 34.0,
            bounds.y + (bounds.height - 18.0) * 0.5,
            34.0,
            18.0,
        );
        ctx.draw.push_quad(QuadCommand {
            rect: track,
            color: if self.on {
                ctx.theme.accent
            } else {
                ctx.theme.pill_active_bg
            },
            border_color: if self.on {
                ctx.theme.accent
            } else {
                ctx.theme.floating_border
            },
            border_width: 1.0,
            corner_radius: 9.0,
            ..Default::default()
        });
        let thumb_x = if self.on {
            track.x + track.width - 15.0
        } else {
            track.x + 3.0
        };
        ctx.draw.push_quad(QuadCommand {
            rect: Rect::new(thumb_x, track.y + 3.0, 12.0, 12.0),
            color: Color::WHITE,
            corner_radius: 6.0,
            ..Default::default()
        });
        ctx.draw.push_text(TextCommand {
            pos: Point::new(
                bounds.x,
                bounds.y + (bounds.height - ctx.theme.metrics.font_ui * 1.25) * 0.5,
            ),
            max_width: Some((bounds.width - 42.0).max(0.0)),
            text: self.label.clone(),
            size: ctx.theme.metrics.font_ui,
            line_height: ctx.theme.metrics.font_ui * 1.25,
            color: ctx.theme.text_primary,
            weight: FontWeight::Regular,
            monospace: false,
            italic: false,
            clip: Some(bounds),
        });
    }
}

/// Single-line text input widget with cursor and placeholder.
pub struct TextInput<'a> {
    /// Stable focus ID.
    pub id: FocusId,
    /// Current text value.
    pub value: &'a str,
    /// Placeholder shown when empty.
    pub placeholder: &'a str,
    /// Render layer (`0`, `1`, or `2`).
    pub layer: usize,
}

impl<'a> TextInput<'a> {
    /// Paints the text input at `bounds`.
    pub fn paint_at(&self, ctx: &mut UiContext<'_>, bounds: Rect) {
        register_hit(
            ctx,
            self.id,
            bounds,
            true,
            Role::TextInput,
            self.placeholder,
            self.layer,
        );
        let focused = ctx.input.focused == Some(self.id);
        ctx.draw.push_quad_layer(
            self.layer,
            QuadCommand {
                rect: bounds,
                color: ctx.theme.island_bg,
                border_color: if focused {
                    ctx.theme.accent
                } else {
                    ctx.theme.floating_border
                },
                border_width: if focused { 1.5 } else { 1.0 },
                corner_radius: ctx.theme.metrics.radius_control,
                ..Default::default()
            },
        );
        let display = if self.value.is_empty() {
            self.placeholder
        } else {
            self.value
        };
        let color = if self.value.is_empty() {
            ctx.theme.text_muted
        } else {
            ctx.theme.text_primary
        };
        let text_y = bounds.y + (bounds.height - ctx.theme.metrics.font_ui * 1.25) * 0.5;
        ctx.draw.push_text_layer(
            self.layer,
            TextCommand {
                pos: Point::new(bounds.x + 10.0, text_y),
                max_width: Some((bounds.width - 20.0).max(0.0)),
                text: display.to_owned(),
                size: ctx.theme.metrics.font_ui,
                line_height: ctx.theme.metrics.font_ui * 1.25,
                color,
                weight: FontWeight::Regular,
                monospace: false,
                italic: false,
                clip: Some(bounds.inset(2.0)),
            },
        );
        if focused {
            let text_w = ctx.text.measure_width(
                self.value,
                ctx.theme.metrics.font_ui,
                FontWeight::Regular,
                false,
            );
            ctx.draw.push_quad_layer(
                self.layer,
                QuadCommand {
                    rect: Rect::new(
                        bounds.x + 10.0 + text_w + 1.0,
                        bounds.y + 6.0,
                        1.5,
                        bounds.height - 12.0,
                    ),
                    color: Color::WHITE,
                    ..Default::default()
                },
            );
        }
    }
}

/// Tab descriptor for [`TabBar`].
#[derive(Clone, Debug)]
pub struct TabItem {
    /// Tab identifier.
    pub id: FocusId,
    /// Close button identifier.
    pub close_id: FocusId,
    /// Display title (e.g. `main.rs`).
    pub title: String,
    /// Short language/file badge (e.g. `RS`, `MD`).
    pub badge: String,
    /// Badge tint color.
    pub badge_color: Color,
    /// Whether the document has unsaved changes.
    pub dirty: bool,
    /// Whether this tab is currently active.
    pub active: bool,
}

/// Paints a Fleet-style pill tab strip inside `bounds`.
pub fn paint_tab_bar(ctx: &mut UiContext<'_>, bounds: Rect, tabs: &[TabItem]) {
    let mut x = bounds.x + 8.0;
    let tab_h = 26.0;
    let y = bounds.y + (bounds.height - tab_h) * 0.5;

    for tab in tabs {
        let title_w = ctx.text.measure_width(
            &tab.title,
            ctx.theme.metrics.font_ui,
            FontWeight::Medium,
            false,
        );
        let tab_w = (26.0 + title_w + 26.0).clamp(84.0, 200.0);
        if x + tab_w > bounds.x + bounds.width - 8.0 {
            break;
        }
        let tab_rect = Rect::new(x, y, tab_w, tab_h);
        register_hit(ctx, tab.id, tab_rect, true, Role::Tab, tab.title.clone(), 0);
        let hovered = ctx.input.hovered == Some(tab.id) || ctx.input.hovered == Some(tab.close_id);

        if tab.active || hovered {
            ctx.draw.push_quad(QuadCommand {
                rect: tab_rect,
                color: if tab.active {
                    ctx.theme.pill_active_bg
                } else {
                    ctx.theme.hover_bg
                },
                corner_radius: ctx.theme.metrics.radius_pill,
                ..Default::default()
            });
        }

        // File badge
        ctx.draw.push_text(TextCommand {
            pos: Point::new(tab_rect.x + 8.0, tab_rect.y + 6.0),
            max_width: None,
            text: tab.badge.clone(),
            size: 10.0,
            line_height: 14.0,
            color: tab.badge_color,
            weight: FontWeight::Bold,
            monospace: true,
            italic: false,
            clip: Some(tab_rect),
        });

        // Title
        ctx.draw.push_text(TextCommand {
            pos: Point::new(tab_rect.x + 31.0, tab_rect.y + 5.0),
            max_width: None,
            text: tab.title.clone(),
            size: ctx.theme.metrics.font_ui,
            line_height: 16.0,
            color: if tab.active {
                ctx.theme.text_primary
            } else {
                ctx.theme.text_secondary
            },
            weight: if tab.active {
                FontWeight::Medium
            } else {
                FontWeight::Regular
            },
            monospace: false,
            italic: false,
            clip: Some(Rect::new(
                tab_rect.x + 30.0,
                tab_rect.y,
                (tab_w - 52.0).max(0.0),
                tab_rect.height,
            )),
        });

        // Close button or dirty dot
        let close_rect = Rect::new(tab_rect.x + tab_w - 20.0, tab_rect.y + 5.0, 16.0, 16.0);
        register_hit(
            ctx,
            tab.close_id,
            close_rect,
            false,
            Role::Button,
            format!("Close {}", tab.title),
            0,
        );
        if tab.dirty && ctx.input.hovered != Some(tab.close_id) {
            ctx.draw.push_quad(QuadCommand {
                rect: Rect::new(close_rect.x + 5.0, close_rect.y + 5.0, 6.0, 6.0),
                color: ctx.theme.accent,
                corner_radius: 3.0,
                ..Default::default()
            });
        } else if tab.active || hovered {
            let close_col = if ctx.input.hovered == Some(tab.close_id) {
                ctx.theme.text_primary
            } else {
                ctx.theme.text_secondary
            };
            ctx.draw
                .push_icon_layer(0, IconKind::Close, close_rect, close_col);
        }

        x += tab_w + 4.0;
    }
}

/// Tree row descriptor for [`paint_tree_view`].
#[derive(Clone, Debug)]
pub struct TreeRowItem {
    /// Row hit ID.
    pub id: FocusId,
    /// Display name.
    pub label: String,
    /// Nesting depth (`0` = root child).
    pub depth: usize,
    /// True if this item is a directory.
    pub is_dir: bool,
    /// True if directory is expanded.
    pub expanded: bool,
    /// True if this row is currently selected.
    pub selected: bool,
    /// Short file badge (`RS`, `MD`, etc.) for files.
    pub badge: String,
    /// Badge color.
    pub badge_color: Color,
}

/// Paints a virtualized/scrollable Fleet-style project tree inside `bounds`.
pub fn paint_tree_view(ctx: &mut UiContext<'_>, bounds: Rect, rows: &[TreeRowItem], scroll_y: f32) {
    let row_h = ctx.theme.metrics.row_height;
    ctx.draw.push_clip(bounds);

    let first_visible = (scroll_y / row_h).floor().max(0.0) as usize;
    let visible_count = (bounds.height / row_h).ceil() as usize + 2;
    let end = (first_visible + visible_count).min(rows.len());

    for (idx, row) in rows[first_visible..end].iter().enumerate() {
        let row_idx = first_visible + idx;
        let y = bounds.y + row_idx as f32 * row_h - scroll_y;
        let row_rect = Rect::new(bounds.x + 6.0, y, (bounds.width - 12.0).max(0.0), row_h);
        register_hit(
            ctx,
            row.id,
            row_rect,
            true,
            Role::TreeItem,
            row.label.clone(),
            0,
        );

        let hovered = ctx.input.hovered == Some(row.id);
        if row.selected || hovered {
            ctx.draw.push_quad(QuadCommand {
                rect: row_rect,
                color: if row.selected {
                    ctx.theme.pill_active_bg
                } else {
                    ctx.theme.hover_bg
                },
                corner_radius: ctx.theme.metrics.radius_pill,
                ..Default::default()
            });
        }

        let indent = row.depth as f32 * 14.0;
        let left_x = row_rect.x + 6.0 + indent;
        if row.is_dir {
            let chev = if row.expanded {
                IconKind::ChevronDown
            } else {
                IconKind::ChevronRight
            };
            ctx.draw.push_icon_layer(
                0,
                chev,
                Rect::new(left_x, y + 4.0, 14.0, 16.0),
                ctx.theme.text_secondary,
            );
            ctx.draw.push_text(TextCommand {
                pos: Point::new(left_x + 18.0, y + 4.0),
                max_width: Some((row_rect.x + row_rect.width - left_x - 22.0).max(0.0)),
                text: row.label.clone(),
                size: ctx.theme.metrics.font_ui,
                line_height: 16.0,
                color: if row.selected {
                    ctx.theme.text_primary
                } else {
                    ctx.theme.text_primary.with_alpha(0.9)
                },
                weight: FontWeight::Regular,
                monospace: false,
                italic: false,
                clip: Some(row_rect),
            });
        } else {
            ctx.draw.push_text(TextCommand {
                pos: Point::new(left_x + 2.0, y + 5.0),
                max_width: None,
                text: row.badge.clone(),
                size: 10.0,
                line_height: 14.0,
                color: row.badge_color,
                weight: FontWeight::Bold,
                monospace: true,
                italic: false,
                clip: Some(row_rect),
            });
            ctx.draw.push_text(TextCommand {
                pos: Point::new(left_x + 30.0, y + 4.0),
                max_width: None,
                text: row.label.clone(),
                size: ctx.theme.metrics.font_ui,
                line_height: 16.0,
                color: if row.selected {
                    Color::WHITE
                } else {
                    ctx.theme.text_primary
                },
                weight: if row.selected {
                    FontWeight::Medium
                } else {
                    FontWeight::Regular
                },
                monospace: false,
                italic: false,
                clip: Some(row_rect),
            });
        }
    }

    // Thin overlay scrollbar if needed
    let total_h = rows.len() as f32 * row_h;
    if total_h > bounds.height && bounds.height > 20.0 {
        paint_scrollbar(ctx, bounds, scroll_y, total_h);
    }

    ctx.draw.pop_clip();
}

/// Paints a thin overlay vertical scrollbar inside `bounds`.
pub fn paint_scrollbar(ctx: &mut UiContext<'_>, bounds: Rect, scroll_y: f32, content_h: f32) {
    if content_h <= bounds.height || bounds.height <= 0.0 {
        return;
    }
    let ratio = (bounds.height / content_h).clamp(0.08, 1.0);
    let thumb_h = (bounds.height * ratio).max(24.0);
    let max_scroll = (content_h - bounds.height).max(1.0);
    let thumb_y = bounds.y + (scroll_y / max_scroll).clamp(0.0, 1.0) * (bounds.height - thumb_h);
    ctx.draw.push_quad(QuadCommand {
        rect: Rect::new(
            bounds.x + bounds.width - 7.0,
            thumb_y + 2.0,
            5.0,
            thumb_h - 4.0,
        ),
        color: ctx.theme.scrollbar_thumb,
        corner_radius: 2.5,
        ..Default::default()
    });
}

/// Paints a floating Fleet-style notification toast card on layer `1`.
pub fn paint_toast(
    ctx: &mut UiContext<'_>,
    anchor_top_right: Point,
    title: &str,
    body: &str,
    is_error: bool,
    dismiss_id: FocusId,
) {
    let w = 320.0;
    let h = 76.0;
    let rect = Rect::new(anchor_top_right.x - w, anchor_top_right.y, w, h);
    ctx.draw.push_quad_layer(
        1,
        QuadCommand {
            rect,
            color: ctx.theme.floating_bg,
            border_color: ctx.theme.floating_border,
            border_width: 1.0,
            corner_radius: ctx.theme.metrics.radius_floating,
            shadow_blur: 14.0,
            shadow_color: ctx.theme.shadow_color,
            ..Default::default()
        },
    );
    ctx.draw.push_icon_layer(
        1,
        IconKind::Bolt,
        Rect::new(rect.x + 12.0, rect.y + 13.0, 16.0, 16.0),
        if is_error {
            ctx.theme.error
        } else {
            ctx.theme.accent
        },
    );
    ctx.draw.push_text_layer(
        1,
        TextCommand {
            pos: Point::new(rect.x + 36.0, rect.y + 11.0),
            max_width: Some(w - 68.0),
            text: title.to_owned(),
            size: ctx.theme.metrics.font_ui,
            line_height: 17.0,
            color: ctx.theme.text_primary,
            weight: FontWeight::SemiBold,
            monospace: false,
            italic: false,
            clip: Some(rect),
        },
    );
    ctx.draw.push_text_layer(
        1,
        TextCommand {
            pos: Point::new(rect.x + 36.0, rect.y + 32.0),
            max_width: Some(w - 52.0),
            text: body.to_owned(),
            size: ctx.theme.metrics.font_caption,
            line_height: 16.0,
            color: ctx.theme.text_secondary,
            weight: FontWeight::Regular,
            monospace: false,
            italic: false,
            clip: Some(rect),
        },
    );
    let close_rect = Rect::new(rect.x + w - 26.0, rect.y + 10.0, 18.0, 18.0);
    register_hit(
        ctx,
        dismiss_id,
        close_rect,
        false,
        Role::Button,
        "Dismiss notification",
        1,
    );
    ctx.draw
        .push_icon_layer(1, IconKind::Close, close_rect, ctx.theme.text_secondary);
}

/// Menu item for [`paint_menu`].
#[derive(Clone, Debug)]
pub struct MenuItem {
    /// Hit ID.
    pub id: FocusId,
    /// Action label.
    pub label: String,
    /// Optional shortcut hint.
    pub shortcut: String,
}

/// Paints a floating dropdown/context menu on layer `1`.
pub fn paint_menu(ctx: &mut UiContext<'_>, origin: Point, width: f32, items: &[MenuItem]) {
    let row_h = 26.0;
    let h = items.len() as f32 * row_h + 8.0;
    let rect = Rect::new(origin.x, origin.y, width, h);
    ctx.draw.push_quad_layer(
        1,
        QuadCommand {
            rect,
            color: ctx.theme.floating_bg,
            border_color: ctx.theme.floating_border,
            border_width: 1.0,
            corner_radius: ctx.theme.metrics.radius_floating,
            shadow_blur: 16.0,
            shadow_color: ctx.theme.shadow_color,
            ..Default::default()
        },
    );
    for (i, item) in items.iter().enumerate() {
        let row_rect = Rect::new(
            rect.x + 4.0,
            rect.y + 4.0 + i as f32 * row_h,
            width - 8.0,
            row_h,
        );
        register_hit(
            ctx,
            item.id,
            row_rect,
            true,
            Role::MenuItem,
            item.label.clone(),
            1,
        );
        if ctx.input.hovered == Some(item.id) {
            ctx.draw.push_quad_layer(
                1,
                QuadCommand {
                    rect: row_rect,
                    color: ctx.theme.pill_active_bg,
                    corner_radius: ctx.theme.metrics.radius_control,
                    ..Default::default()
                },
            );
        }
        ctx.draw.push_text_layer(
            1,
            TextCommand {
                pos: Point::new(row_rect.x + 10.0, row_rect.y + 5.0),
                max_width: None,
                text: item.label.clone(),
                size: ctx.theme.metrics.font_ui,
                line_height: 16.0,
                color: ctx.theme.text_primary,
                weight: FontWeight::Regular,
                monospace: false,
                italic: false,
                clip: Some(Rect::new(
                    row_rect.x,
                    row_rect.y,
                    (row_rect.width - 74.0).max(0.0),
                    row_rect.height,
                )),
            },
        );
        if !item.shortcut.is_empty() {
            let sw = ctx.text.measure_width(
                &item.shortcut,
                ctx.theme.metrics.font_caption,
                FontWeight::Regular,
                false,
            );
            ctx.draw.push_text_layer(
                1,
                TextCommand {
                    pos: Point::new(row_rect.x + row_rect.width - sw - 10.0, row_rect.y + 6.0),
                    max_width: Some(80.0),
                    text: item.shortcut.clone(),
                    size: ctx.theme.metrics.font_caption,
                    line_height: 15.0,
                    color: ctx.theme.text_secondary,
                    weight: FontWeight::Regular,
                    monospace: false,
                    italic: false,
                    clip: Some(row_rect),
                },
            );
        }
    }
}

/// Paints a modal dialog on layer `2` (used for approvals, settings, and command palette).
pub fn paint_dialog_frame(
    ctx: &mut UiContext<'_>,
    viewport: Rect,
    dialog_w: f32,
    dialog_h: f32,
    title: &str,
) -> Rect {
    // Scrim backdrop
    ctx.draw.push_quad_layer(
        2,
        QuadCommand {
            rect: viewport,
            color: Color::BLACK.with_alpha(0.55),
            ..Default::default()
        },
    );
    let rect = Rect::new(
        (viewport.width - dialog_w) * 0.5,
        (viewport.height - dialog_h) * 0.4,
        dialog_w,
        dialog_h,
    );
    ctx.draw.push_quad_layer(
        2,
        QuadCommand {
            rect,
            color: ctx.theme.floating_bg,
            border_color: ctx.theme.floating_border,
            border_width: 1.0,
            corner_radius: ctx.theme.metrics.radius_floating,
            shadow_blur: 24.0,
            shadow_color: ctx.theme.shadow_color,
            ..Default::default()
        },
    );
    ctx.draw.push_text_layer(
        2,
        TextCommand {
            pos: Point::new(rect.x + 16.0, rect.y + 14.0),
            max_width: Some(dialog_w - 32.0),
            text: title.to_owned(),
            size: 14.0,
            line_height: 18.0,
            color: ctx.theme.text_primary,
            weight: FontWeight::SemiBold,
            monospace: false,
            italic: false,
            clip: Some(rect),
        },
    );
    ctx.draw.push_quad_layer(
        2,
        QuadCommand {
            rect: Rect::new(rect.x, rect.y + 42.0, dialog_w, 1.0),
            color: ctx.theme.separator,
            ..Default::default()
        },
    );
    rect
}
