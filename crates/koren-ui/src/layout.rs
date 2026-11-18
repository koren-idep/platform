//! Declarative layout primitives backed by `taffy` flexbox layout.

use crate::draw::{DrawList, Edges, Point, QuadCommand, Rect};
use crate::input::{FocusId, HitRegion, InputState};
use crate::renderer::TextEngine;
use crate::theme::{Color, ThemeTokens};
use accesskit::Role;
use taffy::prelude::*;

/// Frame-level context passed to declarative components during layout & paint.
pub struct UiContext<'a> {
    /// Active centralized theme tokens.
    pub theme: &'a ThemeTokens,
    /// Draw list collecting batched GPU primitives.
    pub draw: &'a mut DrawList,
    /// Input, focus, and hit-test state.
    pub input: &'a mut InputState,
    /// Text shaping/measurement engine.
    pub text: &'a mut TextEngine,
    /// Effective UI scale (`system_dpr * user_zoom`).
    pub scale: f32,
}

/// Flex alignment along cross axis.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CrossAlign {
    /// Stretch to fill cross axis.
    #[default]
    Stretch,
    /// Align to start.
    Start,
    /// Center along cross axis.
    Center,
    /// Align to end.
    End,
}

impl From<CrossAlign> for AlignItems {
    fn from(v: CrossAlign) -> Self {
        match v {
            CrossAlign::Stretch => Self::Stretch,
            CrossAlign::Start => Self::FlexStart,
            CrossAlign::Center => Self::Center,
            CrossAlign::End => Self::FlexEnd,
        }
    }
}

/// Sizing specification for layout nodes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Length {
    /// Size to content.
    #[default]
    Auto,
    /// Fixed size in logical pixels.
    Px(f32),
    /// Percentage of parent (`0.0..=100.0`).
    Percent(f32),
}

impl From<Length> for Dimension {
    fn from(v: Length) -> Self {
        match v {
            Length::Auto => Dimension::auto(),
            Length::Px(px) => Dimension::length(px),
            Length::Percent(pct) => Dimension::percent(pct / 100.0),
        }
    }
}

/// A declarative UI element that can be laid out via `taffy` and painted into a [`DrawList`].
pub trait Element {
    /// Returns the `taffy::Style` describing this element's layout constraints.
    fn style(&self) -> Style;

    /// Returns child elements, if this is a container.
    fn children(&self) -> &[Box<dyn Element>] {
        &[]
    }

    /// Paints this element into `ctx` within `bounds` (already computed by `taffy`).
    fn paint(&self, ctx: &mut UiContext<'_>, bounds: Rect);
}

/// Vertical flex container (`Column`).
pub struct Column {
    gap: f32,
    padding: Edges,
    width: Length,
    height: Length,
    flex_grow: f32,
    align: CrossAlign,
    bg: Option<Color>,
    border: Option<(Color, f32)>,
    radius: f32,
    children: Vec<Box<dyn Element>>,
}

impl Column {
    /// Creates an empty vertical column.
    #[must_use]
    pub fn new() -> Self {
        Self {
            gap: 0.0,
            padding: Edges::ZERO,
            width: Length::Auto,
            height: Length::Auto,
            flex_grow: 0.0,
            align: CrossAlign::Stretch,
            bg: None,
            border: None,
            radius: 0.0,
            children: Vec::new(),
        }
    }

    /// Sets vertical spacing between children.
    #[must_use]
    pub fn spacing(mut self, gap: f32) -> Self {
        self.gap = gap;
        self
    }

    /// Sets inner padding.
    #[must_use]
    pub fn padding(mut self, padding: Edges) -> Self {
        self.padding = padding;
        self
    }

    /// Sets explicit width.
    #[must_use]
    pub fn width(mut self, width: Length) -> Self {
        self.width = width;
        self
    }

    /// Sets explicit height.
    #[must_use]
    pub fn height(mut self, height: Length) -> Self {
        self.height = height;
        self
    }

    /// Sets flex grow factor.
    #[must_use]
    pub fn flex_grow(mut self, grow: f32) -> Self {
        self.flex_grow = grow;
        self
    }

    /// Sets cross-axis alignment.
    #[must_use]
    pub fn align(mut self, align: CrossAlign) -> Self {
        self.align = align;
        self
    }

    /// Sets background fill color.
    #[must_use]
    pub fn background(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    /// Sets border color and width.
    #[must_use]
    pub fn border(mut self, color: Color, width: f32) -> Self {
        self.border = Some((color, width));
        self
    }

    /// Sets corner radius.
    #[must_use]
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    /// Appends a child element.
    #[must_use]
    pub fn child(mut self, child: impl Element + 'static) -> Self {
        self.children.push(Box::new(child));
        self
    }
}

impl Default for Column {
    fn default() -> Self {
        Self::new()
    }
}

impl Element for Column {
    fn style(&self) -> Style {
        Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            align_items: Some(self.align.into()),
            gap: Size {
                width: LengthPercentage::length(0.0),
                height: LengthPercentage::length(self.gap),
            },
            padding: taffy::Rect {
                left: LengthPercentage::length(self.padding.left),
                right: LengthPercentage::length(self.padding.right),
                top: LengthPercentage::length(self.padding.top),
                bottom: LengthPercentage::length(self.padding.bottom),
            },
            size: Size {
                width: self.width.into(),
                height: self.height.into(),
            },
            flex_grow: self.flex_grow,
            ..Default::default()
        }
    }

    fn children(&self) -> &[Box<dyn Element>] {
        &self.children
    }

    fn paint(&self, ctx: &mut UiContext<'_>, bounds: Rect) {
        if self.bg.is_some() || self.border.is_some() {
            let (border_color, border_width) = self.border.unwrap_or((Color::TRANSPARENT, 0.0));
            ctx.draw.push_quad(QuadCommand {
                rect: bounds,
                color: self.bg.unwrap_or(Color::TRANSPARENT),
                border_color,
                border_width,
                corner_radius: self.radius,
                ..Default::default()
            });
        }
    }
}

/// Horizontal flex container (`Row`).
pub struct Row {
    gap: f32,
    padding: Edges,
    width: Length,
    height: Length,
    flex_grow: f32,
    align: CrossAlign,
    bg: Option<Color>,
    border: Option<(Color, f32)>,
    radius: f32,
    children: Vec<Box<dyn Element>>,
}

impl Row {
    /// Creates an empty horizontal row.
    #[must_use]
    pub fn new() -> Self {
        Self {
            gap: 0.0,
            padding: Edges::ZERO,
            width: Length::Auto,
            height: Length::Auto,
            flex_grow: 0.0,
            align: CrossAlign::Center,
            bg: None,
            border: None,
            radius: 0.0,
            children: Vec::new(),
        }
    }

    /// Sets horizontal spacing between children.
    #[must_use]
    pub fn spacing(mut self, gap: f32) -> Self {
        self.gap = gap;
        self
    }

    /// Sets inner padding.
    #[must_use]
    pub fn padding(mut self, padding: Edges) -> Self {
        self.padding = padding;
        self
    }

    /// Sets explicit width.
    #[must_use]
    pub fn width(mut self, width: Length) -> Self {
        self.width = width;
        self
    }

    /// Sets explicit height.
    #[must_use]
    pub fn height(mut self, height: Length) -> Self {
        self.height = height;
        self
    }

    /// Sets flex grow factor.
    #[must_use]
    pub fn flex_grow(mut self, grow: f32) -> Self {
        self.flex_grow = grow;
        self
    }

    /// Sets cross-axis alignment.
    #[must_use]
    pub fn align(mut self, align: CrossAlign) -> Self {
        self.align = align;
        self
    }

    /// Sets background fill color.
    #[must_use]
    pub fn background(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    /// Sets border color and width.
    #[must_use]
    pub fn border(mut self, color: Color, width: f32) -> Self {
        self.border = Some((color, width));
        self
    }

    /// Sets corner radius.
    #[must_use]
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    /// Appends a child element.
    #[must_use]
    pub fn child(mut self, child: impl Element + 'static) -> Self {
        self.children.push(Box::new(child));
        self
    }
}

impl Default for Row {
    fn default() -> Self {
        Self::new()
    }
}

impl Element for Row {
    fn style(&self) -> Style {
        Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            align_items: Some(self.align.into()),
            gap: Size {
                width: LengthPercentage::length(self.gap),
                height: LengthPercentage::length(0.0),
            },
            padding: taffy::Rect {
                left: LengthPercentage::length(self.padding.left),
                right: LengthPercentage::length(self.padding.right),
                top: LengthPercentage::length(self.padding.top),
                bottom: LengthPercentage::length(self.padding.bottom),
            },
            size: Size {
                width: self.width.into(),
                height: self.height.into(),
            },
            flex_grow: self.flex_grow,
            ..Default::default()
        }
    }

    fn children(&self) -> &[Box<dyn Element>] {
        &self.children
    }

    fn paint(&self, ctx: &mut UiContext<'_>, bounds: Rect) {
        if self.bg.is_some() || self.border.is_some() {
            let (border_color, border_width) = self.border.unwrap_or((Color::TRANSPARENT, 0.0));
            ctx.draw.push_quad(QuadCommand {
                rect: bounds,
                color: self.bg.unwrap_or(Color::TRANSPARENT),
                border_color,
                border_width,
                corner_radius: self.radius,
                ..Default::default()
            });
        }
    }
}

/// Flexible spacer element that expands to fill available space.
pub struct Spacer {
    grow: f32,
}

impl Spacer {
    /// Creates a spacer with `flex_grow = 1.0`.
    #[must_use]
    pub fn new() -> Self {
        Self { grow: 1.0 }
    }
}

impl Default for Spacer {
    fn default() -> Self {
        Self::new()
    }
}

impl Element for Spacer {
    fn style(&self) -> Style {
        Style {
            flex_grow: self.grow,
            ..Default::default()
        }
    }

    fn paint(&self, _ctx: &mut UiContext<'_>, _bounds: Rect) {}
}

/// Computes `taffy` layout for an [`Element`] tree and paints it into `ctx`.
pub fn layout_and_paint(root: &dyn Element, ctx: &mut UiContext<'_>, viewport: crate::draw::Size) {
    let mut tree: TaffyTree<()> = TaffyTree::new();
    let root_node = build_taffy_node(&mut tree, root);
    let avail = Size {
        width: AvailableSpace::Definite(viewport.width),
        height: AvailableSpace::Definite(viewport.height),
    };
    let _ = tree.compute_layout(root_node, avail);
    paint_taffy_node(&tree, root_node, root, ctx, Point::new(0.0, 0.0));
}

fn build_taffy_node(tree: &mut TaffyTree<()>, elem: &dyn Element) -> NodeId {
    let children: Vec<NodeId> = elem
        .children()
        .iter()
        .map(|c| build_taffy_node(tree, c.as_ref()))
        .collect();
    tree.new_with_children(elem.style(), &children)
        .expect("taffy node creation failed")
}

fn paint_taffy_node(
    tree: &TaffyTree<()>,
    node: NodeId,
    elem: &dyn Element,
    ctx: &mut UiContext<'_>,
    parent_origin: Point,
) {
    let Ok(layout) = tree.layout(node) else {
        return;
    };
    let bounds = Rect::new(
        parent_origin.x + layout.location.x,
        parent_origin.y + layout.location.y,
        layout.size.width,
        layout.size.height,
    );
    elem.paint(ctx, bounds);
    if let Ok(child_ids) = tree.children(node) {
        for (child_id, child_elem) in child_ids.into_iter().zip(elem.children().iter()) {
            paint_taffy_node(
                tree,
                child_id,
                child_elem.as_ref(),
                ctx,
                Point::new(bounds.x, bounds.y),
            );
        }
    }
}

/// Registers an interactive hit-test region with `ctx.input`.
pub fn register_hit(
    ctx: &mut UiContext<'_>,
    id: FocusId,
    rect: Rect,
    focusable: bool,
    role: Role,
    label: impl Into<String>,
    layer: usize,
) {
    ctx.input.register_region(HitRegion {
        id,
        rect,
        focusable,
        role,
        label: label.into(),
        layer,
    });
}
