//! Centralized design tokens and themes for Koren UI.
//!
//! Visual tokens are inspired by compact desktop IDEs (island-on-canvas layering,
//! subtle luminance contrast, crisp 1px floating borders) while preserving Koren's
//! own identity and supporting both Dark and OLED-Black themes.

/// RGBA color in linear or sRGB float components (`0.0..=1.0`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    /// Red channel (`0.0..=1.0`).
    pub r: f32,
    /// Green channel (`0.0..=1.0`).
    pub g: f32,
    /// Blue channel (`0.0..=1.0`).
    pub b: f32,
    /// Alpha channel (`0.0..=1.0`).
    pub a: f32,
}

impl Color {
    /// Fully transparent color.
    pub const TRANSPARENT: Self = Self::rgba(0.0, 0.0, 0.0, 0.0);
    /// Pure white.
    pub const WHITE: Self = Self::rgb(1.0, 1.0, 1.0);
    /// Pure black.
    pub const BLACK: Self = Self::rgb(0.0, 0.0, 0.0);

    /// Creates an opaque color from normalized RGB components.
    #[must_use]
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// Creates a color from normalized RGBA components.
    #[must_use]
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Creates an opaque color from `0xRRGGBB` hex.
    #[must_use]
    pub const fn from_hex(hex: u32) -> Self {
        let r = ((hex >> 16) & 0xff) as f32 / 255.0;
        let g = ((hex >> 8) & 0xff) as f32 / 255.0;
        let b = (hex & 0xff) as f32 / 255.0;
        Self { r, g, b, a: 1.0 }
    }

    /// Creates a color from `0xRRGGBB` hex and alpha (`0.0..=1.0`).
    #[must_use]
    pub const fn from_hex_alpha(hex: u32, a: f32) -> Self {
        let mut c = Self::from_hex(hex);
        c.a = a;
        c
    }

    /// Returns a copy with modified alpha.
    #[must_use]
    pub const fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// Converts to `[f32; 4]` array for GPU uniforms/instances.
    #[must_use]
    pub const fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// Converts to `[u8; 4]` for `cosmic_text`.
    #[must_use]
    pub fn to_u8_rgba(self) -> [u8; 4] {
        [
            (self.r.clamp(0.0, 1.0) * 255.0).round() as u8,
            (self.g.clamp(0.0, 1.0) * 255.0).round() as u8,
            (self.b.clamp(0.0, 1.0) * 255.0).round() as u8,
            (self.a.clamp(0.0, 1.0) * 255.0).round() as u8,
        ]
    }
}

/// Supported built-in theme variants.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ThemeKind {
    /// Fleet-inspired dark slate theme (`#090909` canvas, `#18191b` islands).
    #[default]
    Dark,
    /// True OLED-black theme (`#000000` canvas, `#0b0c0e` islands, crisp borders).
    OledBlack,
}

/// Syntax highlighting palette tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SyntaxTheme {
    /// Default code foreground.
    pub plain: Color,
    /// Language keywords (`fn`, `let`, `use`, `pub`, `if`, `match`).
    pub keyword: Color,
    /// Types and structs (`String`, `HashMap`, `Result`).
    pub type_name: Color,
    /// String and char literals.
    pub string: Color,
    /// Numeric literals and booleans.
    pub number: Color,
    /// Comments and doc comments.
    pub comment: Color,
    /// Function and method identifiers.
    pub function: Color,
    /// Attributes, macros, and punctuation accents.
    pub macro_attr: Color,
}

/// Spacing and corner radius design tokens (in logical pixels at `1.0x` scale).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MetricsTokens {
    /// Outer margin around workbench islands (`8.0`).
    pub outer_margin: f32,
    /// Gap between adjacent islands (`8.0`).
    pub island_gap: f32,
    /// Top toolbar height (`36.0`).
    pub toolbar_height: f32,
    /// Bottom status bar height (`28.0`).
    pub statusbar_height: f32,
    /// Tab strip height (`34.0`).
    pub tabbar_height: f32,
    /// Tree/menu row height (`24.0`).
    pub row_height: f32,
    /// Island corner radius (`8.0`).
    pub radius_island: f32,
    /// Floating popover/toast/dialog corner radius (`8.0`).
    pub radius_floating: f32,
    /// Active tab / selected row pill corner radius (`6.0`).
    pub radius_pill: f32,
    /// Small control (button, checkbox, input) corner radius (`4.0`).
    pub radius_control: f32,
    /// Standard UI font size (`13.0`).
    pub font_ui: f32,
    /// Small caption / status font size (`12.0`).
    pub font_caption: f32,
    /// Code editor font size (`13.0`).
    pub font_code: f32,
    /// Code editor line height (`20.0`).
    pub line_height_code: f32,
}

impl Default for MetricsTokens {
    fn default() -> Self {
        Self {
            outer_margin: 8.0,
            island_gap: 8.0,
            toolbar_height: 36.0,
            statusbar_height: 28.0,
            tabbar_height: 34.0,
            row_height: 24.0,
            radius_island: 8.0,
            radius_floating: 8.0,
            radius_pill: 6.0,
            radius_control: 4.0,
            font_ui: 13.0,
            font_caption: 12.0,
            font_code: 13.0,
            line_height_code: 20.0,
        }
    }
}

/// Complete centralized theme token set for Koren UI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThemeTokens {
    /// Active theme variant.
    pub kind: ThemeKind,
    /// Outer window shell, toolbar, status bar, and inter-island gap background.
    pub canvas_bg: Color,
    /// Primary island surface (sidebar, editor, bottom panel).
    pub island_bg: Color,
    /// Subtle island border (used prominently in OLED-Black mode).
    pub island_border: Color,
    /// Floating surface (menus, popovers, toasts, dialogs, command palette).
    pub floating_bg: Color,
    /// `1px` border around floating surfaces.
    pub floating_border: Color,
    /// Active tab pill, active sidebar tab, selected tree row.
    pub pill_active_bg: Color,
    /// Hover highlight for interactive rows and buttons.
    pub hover_bg: Color,
    /// Subtle hairline separator inside islands/toolbars.
    pub separator: Color,
    /// Primary text color.
    pub text_primary: Color,
    /// Secondary / muted text color.
    pub text_secondary: Color,
    /// Dimmed text color (line numbers, placeholders, disabled items).
    pub text_muted: Color,
    /// Primary accent color (focus ring, primary button, active indicator).
    pub accent: Color,
    /// Accent hover color.
    pub accent_hover: Color,
    /// Current line highlight in code editor.
    pub editor_line_active: Color,
    /// Text selection highlight in code editor & inputs.
    pub selection_bg: Color,
    /// Indent guide vertical line color.
    pub indent_guide: Color,
    /// Semantic error color.
    pub error: Color,
    /// Semantic warning color.
    pub warning: Color,
    /// Semantic success / run indicator color.
    pub success: Color,
    /// Scrollbar thumb resting color.
    pub scrollbar_thumb: Color,
    /// Scrollbar thumb hover color.
    pub scrollbar_thumb_hover: Color,
    /// Soft shadow color for floating surfaces.
    pub shadow_color: Color,
    /// Syntax highlighting tokens.
    pub syntax: SyntaxTheme,
    /// Layout & typography metrics.
    pub metrics: MetricsTokens,
}

impl ThemeTokens {
    /// Returns the Fleet-inspired Dark theme.
    #[must_use]
    pub const fn dark() -> Self {
        Self {
            kind: ThemeKind::Dark,
            canvas_bg: Color::from_hex(0x090909),
            island_bg: Color::from_hex(0x18191b),
            island_border: Color::from_hex(0x232427),
            floating_bg: Color::from_hex(0x252629),
            floating_border: Color::from_hex(0x414245),
            pill_active_bg: Color::from_hex(0x28292b),
            hover_bg: Color::from_hex(0x222326),
            separator: Color::from_hex(0x26272a),
            text_primary: Color::from_hex(0xe3e4e7),
            text_secondary: Color::from_hex(0x86868a),
            text_muted: Color::from_hex(0x5a5b5f),
            accent: Color::from_hex(0x3574f0),
            accent_hover: Color::from_hex(0x4a85f7),
            editor_line_active: Color::from_hex(0x152639),
            selection_bg: Color::from_hex_alpha(0x264f78, 0.65),
            indent_guide: Color::from_hex(0x2b2d30),
            error: Color::from_hex(0xe05252),
            warning: Color::from_hex(0xe5a93c),
            success: Color::from_hex(0x499c54),
            scrollbar_thumb: Color::from_hex_alpha(0x86868a, 0.25),
            scrollbar_thumb_hover: Color::from_hex_alpha(0x86868a, 0.45),
            shadow_color: Color::from_hex_alpha(0x000000, 0.45),
            syntax: SyntaxTheme {
                plain: Color::from_hex(0xbcbec4),
                keyword: Color::from_hex(0xcf8e6d),
                type_name: Color::from_hex(0x56a8f5),
                string: Color::from_hex(0x6aab73),
                number: Color::from_hex(0x2aacb8),
                comment: Color::from_hex(0x7a7e85),
                function: Color::from_hex(0x56a8f5),
                macro_attr: Color::from_hex(0xb3ae60),
            },
            metrics: MetricsTokens {
                outer_margin: 8.0,
                island_gap: 8.0,
                toolbar_height: 36.0,
                statusbar_height: 28.0,
                tabbar_height: 34.0,
                row_height: 24.0,
                radius_island: 8.0,
                radius_floating: 8.0,
                radius_pill: 6.0,
                radius_control: 4.0,
                font_ui: 13.0,
                font_caption: 12.0,
                font_code: 13.0,
                line_height_code: 20.0,
            },
        }
    }

    /// Returns the OLED-Black high-contrast theme.
    #[must_use]
    pub const fn oled_black() -> Self {
        let mut t = Self::dark();
        t.kind = ThemeKind::OledBlack;
        t.canvas_bg = Color::from_hex(0x000000);
        t.island_bg = Color::from_hex(0x0b0c0e);
        t.island_border = Color::from_hex(0x1f2126);
        t.floating_bg = Color::from_hex(0x141519);
        t.floating_border = Color::from_hex(0x32353d);
        t.pill_active_bg = Color::from_hex(0x1c1e24);
        t.hover_bg = Color::from_hex(0x16181d);
        t.separator = Color::from_hex(0x1f2126);
        t.editor_line_active = Color::from_hex(0x0f1d2e);
        t
    }

    /// Returns a theme for the given [`ThemeKind`].
    #[must_use]
    pub const fn from_kind(kind: ThemeKind) -> Self {
        match kind {
            ThemeKind::Dark => Self::dark(),
            ThemeKind::OledBlack => Self::oled_black(),
        }
    }
}

impl Default for ThemeTokens {
    fn default() -> Self {
        Self::dark()
    }
}
