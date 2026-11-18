//! Input state, focus management, keyboard shortcuts, clipboard, IME, and accessibility.

use crate::draw::{Point, Rect};
use accesskit::{
    Node as AccessNode, NodeId as AccessNodeId, Rect as AccessRect, Role, Tree, TreeUpdate,
};
use arboard::Clipboard;
use std::collections::HashMap;
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// Unique identifier for focusable and interactive UI elements.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FocusId(pub u64);

impl FocusId {
    /// Derives a stable [`FocusId`] from a string label using FNV-1a.
    #[must_use]
    pub const fn from_str(label: &str) -> Self {
        let bytes = label.as_bytes();
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut i = 0;
        while i < bytes.len() {
            hash ^= bytes[i] as u64;
            hash = hash.wrapping_mul(0x0100_0000_01b3);
            i += 1;
        }
        Self(hash)
    }
}

/// Platform clipboard wrapper using `arboard`.
pub struct ClipboardService {
    inner: Option<Clipboard>,
}

impl ClipboardService {
    /// Initializes the system clipboard if available.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Clipboard::new().ok(),
        }
    }

    /// Reads UTF-8 text from the clipboard.
    pub fn get_text(&mut self) -> Option<String> {
        self.inner.as_mut()?.get_text().ok()
    }

    /// Writes UTF-8 text to the clipboard.
    pub fn set_text(&mut self, text: &str) -> bool {
        self.inner
            .as_mut()
            .is_some_and(|cb| cb.set_text(text.to_owned()).is_ok())
    }
}

impl Default for ClipboardService {
    fn default() -> Self {
        Self::new()
    }
}

/// A keyboard shortcut chord.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct KeyChord {
    /// Required modifier flags (`ctrl`, `shift`, `alt`, `super`).
    pub ctrl: bool,
    /// Shift modifier.
    pub shift: bool,
    /// Alt modifier.
    pub alt: bool,
    /// Normalized key string (lowercase char or named key).
    pub key: String,
}

impl KeyChord {
    /// Creates a `Ctrl+<key>` shortcut.
    #[must_use]
    pub fn ctrl(key: impl Into<String>) -> Self {
        Self {
            ctrl: true,
            shift: false,
            alt: false,
            key: key.into(),
        }
    }

    /// Creates a `Ctrl+Shift+<key>` shortcut.
    #[must_use]
    pub fn ctrl_shift(key: impl Into<String>) -> Self {
        Self {
            ctrl: true,
            shift: true,
            alt: false,
            key: key.into(),
        }
    }

    /// Matches a `winit` logical key and modifier state.
    #[must_use]
    pub fn matches(&self, logical_key: &Key, mods: ModifiersState) -> bool {
        if self.ctrl != mods.control_key() || self.alt != mods.alt_key() {
            return false;
        }
        // Allow Shift to match '+' on keyboards where '+' is Shift+'='
        if self.key != "+" && self.shift != mods.shift_key() {
            return false;
        }
        match logical_key {
            Key::Character(s) => s.eq_ignore_ascii_case(&self.key),
            Key::Named(named) => named_key_label(*named) == self.key,
            _ => false,
        }
    }
}

fn named_key_label(key: NamedKey) -> &'static str {
    match key {
        NamedKey::Escape => "Escape",
        NamedKey::Enter => "Enter",
        NamedKey::Tab => "Tab",
        NamedKey::Backspace => "Backspace",
        NamedKey::Delete => "Delete",
        NamedKey::ArrowUp => "ArrowUp",
        NamedKey::ArrowDown => "ArrowDown",
        NamedKey::ArrowLeft => "ArrowLeft",
        NamedKey::ArrowRight => "ArrowRight",
        NamedKey::Home => "Home",
        NamedKey::End => "End",
        NamedKey::PageUp => "PageUp",
        NamedKey::PageDown => "PageDown",
        NamedKey::Space => "Space",
        _ => "",
    }
}

/// Interactive hit-test region registered during layout/paint.
#[derive(Clone, Debug)]
pub struct HitRegion {
    /// Identifier of the widget.
    pub id: FocusId,
    /// Bounding rectangle in logical pixels.
    pub rect: Rect,
    /// Whether this region can receive keyboard focus.
    pub focusable: bool,
    /// Accessibility role.
    pub role: Role,
    /// Accessible label.
    pub label: String,
    /// Z-layer (`0` = base, `1` = overlay, `2` = modal).
    pub layer: usize,
}

/// Tracks pointer, focus, hover, IME, and accessibility state across frames.
pub struct InputState {
    /// Current pointer position in logical pixels.
    pub pointer: Point,
    /// Currently hovered widget ID.
    pub hovered: Option<FocusId>,
    /// Currently pressed widget ID.
    pub active: Option<FocusId>,
    /// Currently focused widget ID.
    pub focused: Option<FocusId>,
    /// Active keyboard modifiers.
    pub modifiers: ModifiersState,
    /// Active IME composition string, if any.
    pub ime_preedit: Option<String>,
    /// Registered hit-test regions from the latest frame.
    pub hit_regions: Vec<HitRegion>,
    /// Focusable IDs in tab order.
    pub focus_order: Vec<FocusId>,
    /// System clipboard.
    pub clipboard: ClipboardService,
    /// Named action bindings.
    pub shortcuts: HashMap<KeyChord, String>,
}

impl InputState {
    /// Creates a new input state with default IDE shortcuts.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pointer: Point::new(-1.0, -1.0),
            hovered: None,
            active: None,
            focused: None,
            modifiers: ModifiersState::empty(),
            ime_preedit: None,
            hit_regions: Vec::with_capacity(128),
            focus_order: Vec::with_capacity(64),
            clipboard: ClipboardService::new(),
            shortcuts: HashMap::new(),
        }
    }

    /// Clears per-frame hit regions before building the next frame.
    pub fn begin_frame(&mut self) {
        self.hit_regions.clear();
        self.focus_order.clear();
    }

    /// Registers an interactive region for hit-testing and accessibility.
    pub fn register_region(&mut self, region: HitRegion) {
        if region.focusable && !self.focus_order.contains(&region.id) {
            self.focus_order.push(region.id);
        }
        self.hit_regions.push(region);
    }

    /// Updates `hovered` from `pointer` and returns whether hover changed.
    pub fn update_hover(&mut self) -> bool {
        let hit = self.hit_test(self.pointer).map(|r| r.id);
        if hit != self.hovered {
            self.hovered = hit;
            true
        } else {
            false
        }
    }

    /// Returns the topmost [`HitRegion`] containing `point`.
    #[must_use]
    pub fn hit_test(&self, point: Point) -> Option<&HitRegion> {
        self.hit_regions
            .iter()
            .rev()
            .max_by_key(|r| {
                if r.rect.contains(point) {
                    (r.layer + 1) as isize
                } else {
                    -1
                }
            })
            .filter(|r| r.rect.contains(point))
    }

    /// Moves focus forward (`forward = true`) or backward (`forward = false`).
    pub fn cycle_focus(&mut self, forward: bool) {
        if self.focus_order.is_empty() {
            return;
        }
        let next_idx = match self
            .focused
            .and_then(|id| self.focus_order.iter().position(|x| *x == id))
        {
            Some(idx) => {
                if forward {
                    (idx + 1) % self.focus_order.len()
                } else {
                    (idx + self.focus_order.len() - 1) % self.focus_order.len()
                }
            }
            None => 0,
        };
        self.focused = Some(self.focus_order[next_idx]);
    }

    /// Builds an [`accesskit::TreeUpdate`] snapshot for screen readers.
    #[must_use]
    pub fn build_access_tree(&self, window_title: &str) -> TreeUpdate {
        let root_id = AccessNodeId(1);
        let mut nodes = Vec::with_capacity(self.hit_regions.len() + 1);
        let mut children = Vec::with_capacity(self.hit_regions.len());

        for region in &self.hit_regions {
            let nid = AccessNodeId(region.id.0.max(2));
            children.push(nid);
            let mut node = AccessNode::new(region.role);
            if !region.label.is_empty() {
                node.set_label(region.label.clone());
            }
            node.set_bounds(AccessRect::new(
                f64::from(region.rect.x),
                f64::from(region.rect.y),
                f64::from(region.rect.x + region.rect.width),
                f64::from(region.rect.y + region.rect.height),
            ));
            nodes.push((nid, node));
        }

        let mut root = AccessNode::new(Role::Window);
        root.set_label(window_title.to_owned());
        root.set_children(children);
        nodes.insert(0, (root_id, root));

        let focus = self
            .focused
            .map(|f| AccessNodeId(f.0.max(2)))
            .unwrap_or(root_id);

        TreeUpdate {
            nodes,
            tree: Some(Tree::new(root_id)),
            tree_id: accesskit::TreeId::ROOT,
            focus,
        }
    }
}

impl Default for InputState {
    fn default() -> Self {
        Self::new()
    }
}
