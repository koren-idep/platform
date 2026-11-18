//! Batched GPU draw primitives and vector icon helpers.

use crate::theme::Color;

/// A 2D point in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    /// Horizontal coordinate.
    pub x: f32,
    /// Vertical coordinate.
    pub y: f32,
}

impl Point {
    /// Creates a new point.
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// A 2D size in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    /// Width in logical pixels.
    pub width: f32,
    /// Height in logical pixels.
    pub height: f32,
}

impl Size {
    /// Creates a new size.
    #[must_use]
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

/// An axis-aligned rectangle in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}

impl Rect {
    /// Creates a new rectangle.
    #[must_use]
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Returns `true` if `point` lies inside this rectangle.
    #[must_use]
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.x
            && point.x <= self.x + self.width
            && point.y >= self.y
            && point.y <= self.y + self.height
    }

    /// Returns the intersection of two rectangles, or `None` if disjoint.
    #[must_use]
    pub fn intersect(&self, other: &Self) -> Option<Self> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = (self.x + self.width).min(other.x + other.width);
        let y1 = (self.y + self.height).min(other.y + other.height);
        if x1 > x0 && y1 > y0 {
            Some(Self::new(x0, y0, x1 - x0, y1 - y0))
        } else {
            None
        }
    }

    /// Insets the rectangle uniformly by `amount` on all sides.
    #[must_use]
    pub fn inset(&self, amount: f32) -> Self {
        Self::new(
            self.x + amount,
            self.y + amount,
            (self.width - 2.0 * amount).max(0.0),
            (self.height - 2.0 * amount).max(0.0),
        )
    }
}

/// Padding or margin insets in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Edges {
    /// Top inset.
    pub top: f32,
    /// Right inset.
    pub right: f32,
    /// Bottom inset.
    pub bottom: f32,
    /// Left inset.
    pub left: f32,
}

impl Edges {
    /// Zero insets on all sides.
    pub const ZERO: Self = Self::all(0.0);

    /// Uniform insets on all sides.
    #[must_use]
    pub const fn all(v: f32) -> Self {
        Self {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }

    /// Symmetric horizontal and vertical insets.
    #[must_use]
    pub const fn xy(horizontal: f32, vertical: f32) -> Self {
        Self {
            top: vertical,
            right: horizontal,
            bottom: vertical,
            left: horizontal,
        }
    }
}

/// GPU-accelerated rounded rectangle primitive with optional border and drop shadow.
#[derive(Clone, Copy, Debug)]
pub struct QuadCommand {
    /// Bounding rectangle in logical pixels.
    pub rect: Rect,
    /// Fill color.
    pub color: Color,
    /// Border color.
    pub border_color: Color,
    /// Border thickness in logical pixels.
    pub border_width: f32,
    /// Uniform corner radius in logical pixels.
    pub corner_radius: f32,
    /// Soft shadow blur radius (`0.0` disables shadow).
    pub shadow_blur: f32,
    /// Shadow color.
    pub shadow_color: Color,
    /// Optional scissor clip rectangle.
    pub clip: Option<Rect>,
}

impl Default for QuadCommand {
    fn default() -> Self {
        Self {
            rect: Rect::default(),
            color: Color::TRANSPARENT,
            border_color: Color::TRANSPARENT,
            border_width: 0.0,
            corner_radius: 0.0,
            shadow_blur: 0.0,
            shadow_color: Color::TRANSPARENT,
            clip: None,
        }
    }
}

/// Font weight for text commands.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FontWeight {
    /// Regular (400).
    #[default]
    Regular,
    /// Medium (500).
    Medium,
    /// SemiBold (600).
    SemiBold,
    /// Bold (700).
    Bold,
}

/// Text draw primitive shaped and rasterized via `cosmic-text`.
#[derive(Clone, Debug)]
pub struct TextCommand {
    /// Top-left anchor in logical pixels.
    pub pos: Point,
    /// Maximum layout width (`None` for unconstrained single-line).
    pub max_width: Option<f32>,
    /// Text string.
    pub text: String,
    /// Font size in logical pixels.
    pub size: f32,
    /// Line height in logical pixels.
    pub line_height: f32,
    /// Text color.
    pub color: Color,
    /// Font weight.
    pub weight: FontWeight,
    /// Use monospace font family.
    pub monospace: bool,
    /// Use italic style.
    pub italic: bool,
    /// Optional scissor clip rectangle.
    pub clip: Option<Rect>,
}

/// Built-in crisp geometric UI icons (no proprietary assets).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IconKind {
    /// Koren brand mark.
    KorenMark,
    /// Hamburger menu.
    Menu,
    /// Left sidebar toggle.
    PanelLeft,
    /// Bottom panel toggle.
    PanelBottom,
    /// Right panel toggle.
    PanelRight,
    /// Play / Run triangle.
    Play,
    /// Search magnifying glass.
    Search,
    /// Settings gear.
    Settings,
    /// Notification bell.
    Bell,
    /// Lightning / status.
    Bolt,
    /// Folder closed/open.
    Folder,
    /// Regular file.
    File,
    /// Chevron right.
    ChevronRight,
    /// Chevron down.
    ChevronDown,
    /// Close `×`.
    Close,
    /// Plus `+`.
    Plus,
    /// Minimize `—`.
    Minimize,
    /// Maximize `□`.
    Maximize,
    /// Checkmark.
    Check,
}

/// Collects batched draw commands for a single frame, layered by z-order.
#[derive(Default)]
pub struct DrawList {
    layers: [Vec<DrawOp>; 3],
    clip_stack: Vec<Rect>,
}

#[derive(Clone, Debug)]
pub(crate) enum DrawOp {
    Quad(QuadCommand),
    Text(TextCommand),
}

impl DrawList {
    /// Creates an empty draw list.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears all queued commands for reuse across frames.
    pub fn clear(&mut self) {
        for layer in &mut self.layers {
            layer.clear();
        }
        self.clip_stack.clear();
    }

    /// Pushes a scissor clip rectangle onto the stack (intersected with parent).
    pub fn push_clip(&mut self, rect: Rect) {
        let intersected = if let Some(top) = self.clip_stack.last() {
            top.intersect(&rect)
                .unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0))
        } else {
            rect
        };
        self.clip_stack.push(intersected);
    }

    /// Pops the current scissor clip rectangle.
    pub fn pop_clip(&mut self) {
        self.clip_stack.pop();
    }

    /// Returns the active clip rectangle, if any.
    #[must_use]
    pub fn current_clip(&self) -> Option<Rect> {
        self.clip_stack.last().copied()
    }

    /// Adds a rounded-rectangle quad to layer `0` (base), `1` (overlay), or `2` (modal).
    pub fn push_quad_layer(&mut self, layer: usize, mut cmd: QuadCommand) {
        if cmd.clip.is_none() {
            cmd.clip = self.current_clip();
        }
        let idx = layer.min(2);
        self.layers[idx].push(DrawOp::Quad(cmd));
    }

    /// Adds a rounded-rectangle quad to the base layer.
    pub fn push_quad(&mut self, cmd: QuadCommand) {
        self.push_quad_layer(0, cmd);
    }

    /// Adds a text draw command to layer `0`, `1`, or `2`.
    pub fn push_text_layer(&mut self, layer: usize, mut cmd: TextCommand) {
        if cmd.clip.is_none() {
            cmd.clip = self.current_clip();
        }
        let idx = layer.min(2);
        self.layers[idx].push(DrawOp::Text(cmd));
    }

    /// Adds a text draw command to the base layer.
    pub fn push_text(&mut self, cmd: TextCommand) {
        self.push_text_layer(0, cmd);
    }

    /// Draws a crisp geometric icon centered inside `rect` on the given layer.
    pub fn push_icon_layer(&mut self, layer: usize, icon: IconKind, rect: Rect, color: Color) {
        let cx = rect.x + rect.width * 0.5;
        let cy = rect.y + rect.height * 0.5;
        let s = rect.width.min(rect.height);
        let q = |x: f32, y: f32, w: f32, h: f32, r: f32| QuadCommand {
            rect: Rect::new(x, y, w, h),
            color,
            corner_radius: r,
            ..Default::default()
        };
        match icon {
            IconKind::KorenMark => {
                // Stylized Koren K-diamond mark
                self.push_quad_layer(
                    layer,
                    QuadCommand {
                        rect: Rect::new(cx - s * 0.38, cy - s * 0.38, s * 0.76, s * 0.76),
                        color: color.with_alpha(0.18),
                        border_color: color,
                        border_width: 1.5,
                        corner_radius: s * 0.22,
                        ..Default::default()
                    },
                );
                self.push_quad_layer(layer, q(cx - s * 0.16, cy - s * 0.22, 2.0, s * 0.44, 1.0));
                self.push_quad_layer(layer, q(cx - s * 0.04, cy - s * 0.04, s * 0.22, 2.0, 1.0));
            }
            IconKind::Menu => {
                for dy in [-3.5, 0.0, 3.5] {
                    self.push_quad_layer(layer, q(cx - 5.5, cy + dy - 0.75, 11.0, 1.5, 0.75));
                }
            }
            IconKind::PanelLeft => {
                self.push_quad_layer(
                    layer,
                    QuadCommand {
                        rect: Rect::new(cx - 6.0, cy - 5.0, 12.0, 10.0),
                        border_color: color,
                        border_width: 1.25,
                        corner_radius: 2.0,
                        ..Default::default()
                    },
                );
                self.push_quad_layer(layer, q(cx - 2.5, cy - 5.0, 1.25, 10.0, 0.0));
            }
            IconKind::PanelBottom => {
                self.push_quad_layer(
                    layer,
                    QuadCommand {
                        rect: Rect::new(cx - 6.0, cy - 5.0, 12.0, 10.0),
                        border_color: color,
                        border_width: 1.25,
                        corner_radius: 2.0,
                        ..Default::default()
                    },
                );
                self.push_quad_layer(layer, q(cx - 6.0, cy + 1.5, 12.0, 1.25, 0.0));
            }
            IconKind::PanelRight => {
                self.push_quad_layer(
                    layer,
                    QuadCommand {
                        rect: Rect::new(cx - 6.0, cy - 5.0, 12.0, 10.0),
                        border_color: color,
                        border_width: 1.25,
                        corner_radius: 2.0,
                        ..Default::default()
                    },
                );
                self.push_quad_layer(layer, q(cx + 1.5, cy - 5.0, 1.25, 10.0, 0.0));
            }
            IconKind::Play => {
                // Crisp right-pointing triangle with 0.75px steps
                for i in 0..13 {
                    let dy = (i as f32 - 6.0) * 0.75;
                    let w = (4.8 - dy.abs()) * 1.9;
                    if w > 0.6 {
                        self.push_quad_layer(layer, q(cx - 3.5, cy + dy - 0.5, w, 1.0, 0.2));
                    }
                }
            }
            IconKind::Search => {
                self.push_quad_layer(
                    layer,
                    QuadCommand {
                        rect: Rect::new(cx - 5.5, cy - 5.5, 8.5, 8.5),
                        border_color: color,
                        border_width: 1.4,
                        corner_radius: 4.25,
                        ..Default::default()
                    },
                );
                // Smooth 45-degree diagonal handle
                for i in 0..8 {
                    let d = i as f32 * 0.55;
                    self.push_quad_layer(layer, q(cx + 1.5 + d, cy + 1.5 + d, 1.4, 1.4, 0.3));
                }
            }
            IconKind::Settings => {
                // Clean hexagonal/gear outline + center ring matching old Koren UI
                self.push_quad_layer(
                    layer,
                    QuadCommand {
                        rect: Rect::new(cx - 5.2, cy - 5.2, 10.4, 10.4),
                        border_color: color,
                        border_width: 1.4,
                        corner_radius: 3.2,
                        ..Default::default()
                    },
                );
                self.push_quad_layer(
                    layer,
                    QuadCommand {
                        rect: Rect::new(cx - 2.1, cy - 2.1, 4.2, 4.2),
                        border_color: color,
                        border_width: 1.3,
                        corner_radius: 2.1,
                        ..Default::default()
                    },
                );
            }
            IconKind::Bell => {
                self.push_quad_layer(
                    layer,
                    QuadCommand {
                        rect: Rect::new(cx - 4.0, cy - 4.5, 8.0, 7.5),
                        border_color: color,
                        border_width: 1.3,
                        corner_radius: 3.5,
                        ..Default::default()
                    },
                );
                self.push_quad_layer(layer, q(cx - 5.0, cy + 2.5, 10.0, 1.3, 0.6));
                self.push_quad_layer(layer, q(cx - 1.2, cy + 4.2, 2.4, 1.5, 0.7));
            }
            IconKind::Bolt => {
                // Connected 2-segment diagonal lightning bolt
                for i in 0..7 {
                    let t = i as f32 * 0.7;
                    self.push_quad_layer(
                        layer,
                        q(cx + 0.5 - t * 0.55, cy - 5.0 + t, 2.4, 1.3, 0.3),
                    );
                }
                self.push_quad_layer(layer, q(cx - 2.8, cy - 0.6, 5.6, 1.4, 0.3));
                for i in 0..7 {
                    let t = i as f32 * 0.7;
                    self.push_quad_layer(
                        layer,
                        q(cx + 0.5 - t * 0.55, cy + 0.2 + t, 2.2, 1.3, 0.3),
                    );
                }
            }
            IconKind::Folder => {
                self.push_quad_layer(layer, q(cx - 5.5, cy - 4.0, 5.0, 2.0, 1.0));
                self.push_quad_layer(
                    layer,
                    QuadCommand {
                        rect: Rect::new(cx - 5.5, cy - 2.5, 11.0, 7.5),
                        color: color.with_alpha(0.22),
                        border_color: color,
                        border_width: 1.2,
                        corner_radius: 2.0,
                        ..Default::default()
                    },
                );
            }
            IconKind::File => {
                self.push_quad_layer(
                    layer,
                    QuadCommand {
                        rect: Rect::new(cx - 4.5, cy - 5.5, 9.0, 11.0),
                        border_color: color,
                        border_width: 1.2,
                        corner_radius: 1.5,
                        ..Default::default()
                    },
                );
                self.push_quad_layer(layer, q(cx - 2.5, cy - 1.5, 5.0, 1.2, 0.5));
                self.push_quad_layer(layer, q(cx - 2.5, cy + 1.2, 4.0, 1.2, 0.5));
            }
            IconKind::ChevronRight => {
                for i in -5..=5 {
                    let y_off = i as f32 * 0.65;
                    let x_off = 2.0 - y_off.abs() * 0.7;
                    self.push_quad_layer(
                        layer,
                        q(cx - 1.2 + x_off, cy + y_off - 0.6, 1.4, 1.2, 0.3),
                    );
                }
            }
            IconKind::ChevronDown => {
                for i in -5..=5 {
                    let x_off = i as f32 * 0.65;
                    let y_off = 2.0 - x_off.abs() * 0.7;
                    self.push_quad_layer(
                        layer,
                        q(cx + x_off - 0.6, cy - 1.2 + y_off, 1.2, 1.4, 0.3),
                    );
                }
            }
            IconKind::Close => {
                // Smooth diagonal '×'
                for i in -6..=6 {
                    let d = i as f32 * 0.65;
                    self.push_quad_layer(layer, q(cx + d - 0.65, cy + d - 0.65, 1.3, 1.3, 0.3));
                    self.push_quad_layer(layer, q(cx + d - 0.65, cy - d - 0.65, 1.3, 1.3, 0.3));
                }
            }
            IconKind::Plus => {
                self.push_quad_layer(layer, q(cx - 4.5, cy - 0.75, 9.0, 1.5, 0.75));
                self.push_quad_layer(layer, q(cx - 0.75, cy - 4.5, 1.5, 9.0, 0.75));
            }
            IconKind::Minimize => {
                self.push_quad_layer(layer, q(cx - 4.5, cy - 0.6, 9.0, 1.2, 0.6));
            }
            IconKind::Maximize => {
                self.push_quad_layer(
                    layer,
                    QuadCommand {
                        rect: Rect::new(cx - 4.5, cy - 4.5, 9.0, 9.0),
                        border_color: color,
                        border_width: 1.2,
                        corner_radius: 1.5,
                        ..Default::default()
                    },
                );
            }
            IconKind::Check => {
                self.push_quad_layer(layer, q(cx - 3.5, cy + 0.5, 2.5, 1.6, 0.6));
                self.push_quad_layer(layer, q(cx - 1.5, cy - 1.5, 5.0, 1.6, 0.6));
            }
        }
    }

    /// Iterates over all draw operations in z-order (layer 0 -> 1 -> 2).
    pub(crate) fn iter_ops(&self) -> impl Iterator<Item = &DrawOp> {
        self.layers.iter().flat_map(|l| l.iter())
    }
}
