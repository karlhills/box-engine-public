//! The retained UI tree: the [`Ui`] resource, node handles and events.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use box_assets::FontAsset;
use box_core::assets::Handle;
use box_core::image::ImageData;
use box_core::world::Entity;
use epaint::{Rect, Vec2};
use glam::Vec3;

use crate::node::{NodeKind, UiNode, WorldAnchorDesc};
use crate::paint::Painter;
use crate::style::{Resolved, Style, Theme};

/// A node handle. Stale after [`Ui::remove`]: methods then do nothing and
/// return `false` / `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId {
    pub(crate) index: u32,
    pub(crate) generation: u32,
}

/// What happened to a node, sent as [`UiEvent`]s (`Events<UiEvent>`) during
/// the `ui_input` system (start of `Update`), so game systems read them the
/// same frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UiEventKind {
    /// A button was activated (confirm, or a click released over it).
    Click,
    /// A slider's value changed (by the player).
    Change(f32),
    /// A toggle was switched (by the player).
    Toggle(bool),
    /// The node took focus.
    Focus,
    /// The node lost focus.
    Blur,
    /// Back (`ui_back`) was pressed; sent for the active modal root.
    Back,
}

impl UiEventKind {
    /// The kind as a lowercase name (`"click"`), for scripts and logs.
    pub fn name(self) -> &'static str {
        match self {
            UiEventKind::Click => "click",
            UiEventKind::Change(_) => "change",
            UiEventKind::Toggle(_) => "toggle",
            UiEventKind::Focus => "focus",
            UiEventKind::Blur => "blur",
            UiEventKind::Back => "back",
        }
    }
}

/// One UI event.
#[derive(Clone, Debug, PartialEq)]
pub struct UiEvent {
    pub node: NodeId,
    /// The node's `id`, if it has one.
    pub id: Option<String>,
    /// The node's `action`, if it has one.
    pub action: Option<String>,
    pub kind: UiEventKind,
}

impl UiEvent {
    /// True for a click on a node whose action or id is `name`.
    pub fn is_click(&self, name: &str) -> bool {
        self.kind == UiEventKind::Click
            && (self.action.as_deref() == Some(name) || self.id.as_deref() == Some(name))
    }
}

/// How reference points map to pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScaleMode {
    /// Lay out on a virtual screen of at least this many points, scaled to
    /// fit the window (`min(w / width, h / height)`): the same layout at any
    /// resolution, like a console game. The default is 1920 x 1080.
    Reference { width: f32, height: f32 },
    /// One point is this many logical pixels (times the window's scale
    /// factor): text keeps its physical size, like a desktop app.
    Dpi(f32),
}

impl Default for ScaleMode {
    fn default() -> Self {
        ScaleMode::Reference {
            width: 1920.0,
            height: 1080.0,
        }
    }
}

/// Behaviour settings of the [`Ui`].
#[derive(Clone, Debug, PartialEq)]
pub struct UiSettings {
    /// Actions that still reach the game while a modal root captures input
    /// (besides the `ui_*` actions), e.g. `"pause"` to close a pause menu.
    pub passthrough_actions: Vec<String>,
    /// Directional navigation wraps around at the edges.
    pub wrap_navigation: bool,
    /// Seconds a direction is held before it repeats.
    pub repeat_delay: f32,
    /// Seconds between repeats.
    pub repeat_interval: f32,
    /// Stick deflection that counts as a direction.
    pub stick_threshold: f32,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            passthrough_actions: Vec::new(),
            wrap_navigation: true,
            repeat_delay: 0.4,
            repeat_interval: 0.12,
            stick_threshold: 0.5,
        }
    }
}

/// What a world-anchored root follows.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AnchorTarget {
    Point(Vec3),
    Entity(Entity),
    /// Uid or name, resolved to an entity when found and again when that
    /// entity is gone (only when the entity count changed since the last
    /// try).
    Named {
        name: String,
        cached: Option<Entity>,
        tried_at: Option<u32>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WorldAnchor {
    pub target: AnchorTarget,
    pub offset: Vec3,
    pub max_distance: Option<f32>,
    /// Projected position in points; `None` while hidden (behind the
    /// camera, too far, entity gone).
    pub screen: Option<Vec2>,
}

impl WorldAnchor {
    fn from_desc(d: &WorldAnchorDesc) -> Self {
        Self {
            target: match &d.entity {
                Some(e) => AnchorTarget::Named {
                    name: e.clone(),
                    cached: None,
                    tried_at: None,
                },
                None => AnchorTarget::Point(Vec3::from_array(d.point)),
            },
            offset: Vec3::from_array(d.offset),
            max_distance: d.max_distance,
            screen: None,
        }
    }
}

/// One node: its description and computed layout.
pub(crate) struct Node {
    pub kind: NodeKind,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub text: Option<String>,
    pub key: Option<String>,
    pub args: BTreeMap<String, String>,
    pub image: Option<String>,
    pub image_handle: Option<Handle<ImageData>>,
    pub style: Style,
    pub hidden: bool,
    pub disabled: bool,
    pub value: f32,
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub checked: bool,
    pub action: Option<String>,
    pub modal: bool,
    pub autofocus: bool,
    pub layer: i32,
    pub anchor: [f32; 2],
    pub offset: [f32; 2],
    pub world: Option<WorldAnchor>,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    /// Creation order (draw order among roots of one layer).
    pub order: u64,
    // Computed by layout (points).
    pub rect: Rect,
    pub clip: Rect,
    pub shown: bool,
    pub resolved: Resolved,
    pub display_text: String,
    pub galley: Option<Arc<epaint::Galley>>,
    pub scroll: f32,
}

struct Slot {
    generation: u32,
    node: Option<Node>,
}

/// Resource: the game's UI. See the [crate docs](crate).
pub struct Ui {
    slots: Vec<Slot>,
    free: Vec<u32>,
    pub(crate) roots: Vec<NodeId>,
    next_order: u64,
    pub(crate) theme: Theme,
    pub(crate) theme_path: Option<String>,
    pub(crate) theme_loaded: Option<(Handle<box_assets::JsonAsset>, u64)>,
    pub(crate) theme_overlay: Option<Theme>,
    pub(crate) scale_mode: ScaleMode,
    /// Pixels per point, from the last layout.
    pub(crate) scale: f32,
    /// Screen size in points, from the last layout.
    pub(crate) screen: Vec2,
    pub settings: UiSettings,
    pub(crate) focused: Option<NodeId>,
    pub(crate) hovered: Option<NodeId>,
    pub(crate) pressed: Option<NodeId>,
    /// Last focused node per modal root, restored when it is active again.
    pub(crate) remembered: HashMap<NodeId, NodeId>,
    pub(crate) active_modal: Option<NodeId>,
    pub(crate) nav: crate::input::NavState,
    pub(crate) events: Vec<UiEvent>,
    /// Layout and paint are needed.
    pub(crate) dirty: bool,
    pub(crate) painter: Painter,
    pub(crate) warned: HashSet<String>,
    /// Font assets requested for theme roles: role → (path, handle).
    pub(crate) font_handles: BTreeMap<String, (String, Handle<FontAsset>)>,
    /// What the last layout saw (change detection).
    pub(crate) seen: Seen,
    /// Changes whenever the effective theme is rebuilt.
    pub(crate) theme_revision: u64,
    /// (font store generation, theme revision) fonts were last synced for.
    pub(crate) seen_fonts_key: Option<(u64, u64)>,
    /// Image nodes may need their asset requested.
    pub(crate) images_pending: bool,
}

/// Inputs of the last layout, compared each frame to decide whether layout
/// must run again.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Seen {
    pub screen: (u32, u32, u32),
    pub strings: u64,
    pub images: u64,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            roots: Vec::new(),
            next_order: 0,
            theme: Theme::builtin(),
            theme_path: None,
            theme_loaded: None,
            theme_overlay: None,
            scale_mode: ScaleMode::default(),
            scale: 1.0,
            screen: Vec2::new(1920.0, 1080.0),
            settings: UiSettings::default(),
            focused: None,
            hovered: None,
            pressed: None,
            remembered: HashMap::new(),
            active_modal: None,
            nav: Default::default(),
            events: Vec::new(),
            dirty: true,
            painter: Painter::default(),
            warned: HashSet::new(),
            font_handles: BTreeMap::new(),
            seen: Seen::default(),
            theme_revision: 0,
            seen_fonts_key: None,
            images_pending: false,
        }
    }
}

/// Errors from building UI out of JSON.
#[derive(Debug, thiserror::Error)]
pub enum UiError {
    #[error("UI document: {0}")]
    Json(#[from] serde_json::Error),
    #[error("theme: {0}")]
    Theme(String),
}

impl Ui {
    pub fn new() -> Self {
        Self::default()
    }

    // ----- building -----

    /// Adds `desc` (with its children) under `parent`, or as a root.
    /// Returns the new node.
    pub fn add(&mut self, parent: Option<NodeId>, desc: UiNode) -> NodeId {
        let parent = parent.filter(|p| self.contains(*p));
        let UiNode {
            kind,
            id,
            class,
            text,
            key,
            args,
            image,
            style,
            hidden,
            disabled,
            value,
            min,
            max,
            step,
            checked,
            action,
            modal,
            autofocus,
            layer,
            anchor,
            offset,
            world,
            children,
        } = desc;
        let world = world.as_ref().map(WorldAnchor::from_desc);
        self.images_pending |= image.is_some();
        let layer = layer.unwrap_or(if modal {
            100
        } else if world.is_some() {
            -100
        } else {
            0
        });
        let order = self.next_order;
        self.next_order += 1;
        let node = Node {
            kind,
            id,
            classes: class
                .map(|c| c.split_whitespace().map(str::to_string).collect())
                .unwrap_or_default(),
            text,
            key,
            args,
            image,
            image_handle: None,
            style,
            hidden,
            disabled,
            value: if max > min {
                value.clamp(min, max)
            } else {
                value
            },
            min,
            max,
            step,
            checked,
            action,
            modal,
            autofocus,
            layer,
            anchor,
            offset,
            world,
            parent,
            children: Vec::new(),
            order,
            rect: Rect::NOTHING,
            clip: Rect::EVERYTHING,
            shown: false,
            resolved: Resolved::default(),
            display_text: String::new(),
            galley: None,
            scroll: 0.0,
        };
        let id = match self.free.pop() {
            Some(i) => {
                let slot = &mut self.slots[i as usize];
                slot.generation += 1;
                slot.node = Some(node);
                NodeId {
                    index: i,
                    generation: slot.generation,
                }
            }
            None => {
                self.slots.push(Slot {
                    generation: 0,
                    node: Some(node),
                });
                NodeId {
                    index: self.slots.len() as u32 - 1,
                    generation: 0,
                }
            }
        };
        match parent {
            Some(p) => {
                if let Some(pn) = self.node_mut(p) {
                    pn.children.push(id);
                }
            }
            None => self.roots.push(id),
        }
        for child in children {
            self.add(Some(id), child);
        }
        self.dirty = true;
        id
    }

    /// Parses a JSON document ([`UiNode`]) and adds it.
    pub fn add_json(&mut self, parent: Option<NodeId>, json: &str) -> Result<NodeId, UiError> {
        let desc = UiNode::from_json(json)?;
        Ok(self.add(parent, desc))
    }

    /// Removes a node and its descendants.
    pub fn remove(&mut self, node: NodeId) -> bool {
        let Some(n) = self.node(node) else {
            return false;
        };
        match n.parent {
            Some(p) => {
                if let Some(pn) = self.node_mut(p) {
                    pn.children.retain(|c| *c != node);
                }
            }
            None => self.roots.retain(|r| *r != node),
        }
        let mut stack = vec![node];
        while let Some(id) = stack.pop() {
            let slot = &mut self.slots[id.index as usize];
            if let Some(n) = slot.node.take() {
                stack.extend(n.children);
                self.free.push(id.index);
            }
            for s in [&mut self.focused, &mut self.hovered, &mut self.pressed] {
                if *s == Some(id) {
                    *s = None;
                }
            }
            self.remembered.remove(&id);
        }
        self.remembered.retain(|_, v| {
            self.slots[v.index as usize].generation == v.generation
                && self.slots[v.index as usize].node.is_some()
        });
        self.dirty = true;
        true
    }

    /// Removes every node.
    pub fn clear(&mut self) {
        for r in self.roots.clone() {
            self.remove(r);
        }
    }

    // ----- lookup -----

    pub fn contains(&self, node: NodeId) -> bool {
        self.node(node).is_some()
    }

    pub(crate) fn node(&self, id: NodeId) -> Option<&Node> {
        let slot = self.slots.get(id.index as usize)?;
        (slot.generation == id.generation)
            .then_some(slot.node.as_ref())
            .flatten()
    }

    pub(crate) fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        let slot = self.slots.get_mut(id.index as usize)?;
        (slot.generation == id.generation)
            .then_some(slot.node.as_mut())
            .flatten()
    }

    /// Every node, in slot order.
    pub(crate) fn all_nodes(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.slots.iter().enumerate().filter_map(|(i, s)| {
            s.node.as_ref().map(|n| {
                (
                    NodeId {
                        index: i as u32,
                        generation: s.generation,
                    },
                    n,
                )
            })
        })
    }

    /// The first node (pre-order) with this `id` under `scope` (inclusive),
    /// or in the whole UI.
    pub fn find(&self, scope: Option<NodeId>, id: &str) -> Option<NodeId> {
        let mut stack: Vec<NodeId> = match scope {
            Some(s) => vec![s],
            None => self.roots.iter().rev().copied().collect(),
        };
        while let Some(n) = stack.pop() {
            let node = self.node(n)?;
            if node.id.as_deref() == Some(id) {
                return Some(n);
            }
            stack.extend(node.children.iter().rev());
        }
        None
    }

    /// The root nodes, in creation order.
    pub fn roots(&self) -> &[NodeId] {
        &self.roots
    }

    pub fn parent(&self, node: NodeId) -> Option<NodeId> {
        self.node(node)?.parent
    }

    pub fn children(&self, node: NodeId) -> &[NodeId] {
        self.node(node).map_or(&[], |n| n.children.as_slice())
    }

    /// The root of `node`'s tree.
    pub fn root_of(&self, mut node: NodeId) -> Option<NodeId> {
        self.node(node)?;
        while let Some(p) = self.parent(node) {
            node = p;
        }
        Some(node)
    }

    pub fn kind(&self, node: NodeId) -> Option<NodeKind> {
        self.node(node).map(|n| n.kind)
    }

    /// The node's `id`.
    pub fn id_of(&self, node: NodeId) -> Option<&str> {
        self.node(node)?.id.as_deref()
    }

    /// The text as last laid out (string table and placeholders applied,
    /// inline tags removed).
    pub fn text(&self, node: NodeId) -> Option<String> {
        self.node(node)
            .map(|n| crate::markup::plain(&n.display_text))
    }

    pub fn value(&self, node: NodeId) -> Option<f32> {
        self.node(node).map(|n| n.value)
    }

    pub fn checked(&self, node: NodeId) -> Option<bool> {
        self.node(node).map(|n| n.checked)
    }

    /// The node's own `hidden` flag is off.
    pub fn is_visible(&self, node: NodeId) -> bool {
        self.node(node).is_some_and(|n| !n.hidden)
    }

    /// Drawn in the last layout (it and its ancestors visible, a world
    /// anchor on screen).
    pub fn is_shown(&self, node: NodeId) -> bool {
        self.node(node).is_some_and(|n| n.shown)
    }

    pub fn is_enabled(&self, node: NodeId) -> bool {
        self.node(node).is_some_and(|n| !n.disabled)
    }

    /// The node's rectangle from the last layout, in physical pixels:
    /// `[x, y, width, height]`.
    pub fn rect(&self, node: NodeId) -> Option<[f32; 4]> {
        let n = self.node(node)?;
        if !n.rect.is_positive() {
            return None;
        }
        let s = self.scale;
        Some([
            n.rect.min.x * s,
            n.rect.min.y * s,
            n.rect.width() * s,
            n.rect.height() * s,
        ])
    }

    /// The focused node.
    pub fn focused(&self) -> Option<NodeId> {
        self.focused
    }

    /// The visible modal root that owns focus and captures input.
    pub fn active_modal(&self) -> Option<NodeId> {
        self.active_modal
    }

    /// True while a modal root captures game input.
    pub fn captures_input(&self) -> bool {
        self.active_modal.is_some()
    }

    /// True while the pointer is over an interactive node (the game should
    /// ignore clicks).
    pub fn pointer_over_ui(&self) -> bool {
        self.hovered.is_some() || self.pressed.is_some()
    }

    /// Pixels per reference point (from the last layout).
    pub fn scale(&self) -> f32 {
        self.scale
    }

    // ----- changing -----

    fn edit(&mut self, node: NodeId, f: impl FnOnce(&mut Node) -> bool) -> bool {
        let Some(n) = self.node_mut(node) else {
            return false;
        };
        if f(n) {
            self.dirty = true;
        }
        true
    }

    /// Sets literal text (clears the string table key).
    pub fn set_text(&mut self, node: NodeId, text: impl Into<String>) -> bool {
        let text = text.into();
        self.edit(node, |n| {
            let changed = n.text.as_deref() != Some(text.as_str()) || n.key.is_some();
            n.text = Some(text);
            n.key = None;
            changed
        })
    }

    /// Shows the text of a string table key.
    pub fn set_key(&mut self, node: NodeId, key: impl Into<String>) -> bool {
        let key = key.into();
        self.edit(node, |n| {
            let changed = n.key.as_deref() != Some(key.as_str());
            n.key = Some(key);
            changed
        })
    }

    /// Sets a `{name}` placeholder value.
    pub fn set_arg(&mut self, node: NodeId, name: &str, value: impl ToString) -> bool {
        let value = value.to_string();
        self.edit(node, |n| {
            n.args.insert(name.to_string(), value.clone()) != Some(value)
        })
    }

    pub fn set_visible(&mut self, node: NodeId, visible: bool) -> bool {
        self.edit(node, |n| {
            std::mem::replace(&mut n.hidden, !visible) == visible
        })
    }

    pub fn set_enabled(&mut self, node: NodeId, enabled: bool) -> bool {
        self.edit(node, |n| {
            std::mem::replace(&mut n.disabled, !enabled) == enabled
        })
    }

    /// Sets a slider or progress value (clamped to its range). Sends no event.
    pub fn set_value(&mut self, node: NodeId, value: f32) -> bool {
        self.edit(node, |n| {
            let v = if n.max > n.min {
                value.clamp(n.min, n.max)
            } else {
                value
            };
            std::mem::replace(&mut n.value, v) != v
        })
    }

    /// Sets a slider or progress range.
    pub fn set_range(&mut self, node: NodeId, min: f32, max: f32) -> bool {
        self.edit(node, |n| {
            n.min = min;
            n.max = max;
            if max > min {
                n.value = n.value.clamp(min, max);
            }
            true
        })
    }

    /// Sets a toggle. Sends no event.
    pub fn set_checked(&mut self, node: NodeId, checked: bool) -> bool {
        self.edit(node, |n| {
            std::mem::replace(&mut n.checked, checked) != checked
        })
    }

    /// Replaces the extra classes (space separated).
    pub fn set_class(&mut self, node: NodeId, class: &str) -> bool {
        let classes: Vec<String> = class.split_whitespace().map(str::to_string).collect();
        self.edit(node, |n| {
            std::mem::replace(&mut n.classes, classes.clone()) != classes
        })
    }

    /// Replaces the inline style.
    pub fn set_style(&mut self, node: NodeId, style: Style) -> bool {
        self.edit(node, |n| {
            std::mem::replace(&mut n.style, style.clone()) != style
        })
    }

    /// Changes the inline style in place.
    pub fn style_mut(&mut self, node: NodeId, f: impl FnOnce(&mut Style)) -> bool {
        self.edit(node, |n| {
            let before = n.style.clone();
            f(&mut n.style);
            n.style != before
        })
    }

    /// Shows another image asset.
    pub fn set_image(&mut self, node: NodeId, path: impl Into<String>) -> bool {
        let path = path.into();
        self.edit(node, |n| {
            let changed = n.image.as_deref() != Some(path.as_str());
            if changed {
                n.image = Some(path);
                n.image_handle = None;
            }
            changed
        }) && {
            self.images_pending = true;
            true
        }
    }

    pub fn set_action(&mut self, node: NodeId, action: Option<String>) -> bool {
        self.edit(node, |n| {
            n.action = action;
            false
        })
    }

    /// Root: places it on the screen (anchor is also the pivot).
    pub fn set_anchor(&mut self, node: NodeId, anchor: [f32; 2], offset: [f32; 2]) -> bool {
        self.edit(node, |n| {
            n.anchor = anchor;
            n.offset = offset;
            true
        })
    }

    /// Root: follows an entity's position plus `offset` (metres).
    pub fn anchor_to_entity(&mut self, node: NodeId, entity: Entity, offset: Vec3) -> bool {
        self.set_world_anchor(node, AnchorTarget::Entity(entity), offset)
    }

    /// Root: follows a world point.
    pub fn anchor_to_point(&mut self, node: NodeId, point: Vec3) -> bool {
        self.set_world_anchor(node, AnchorTarget::Point(point), Vec3::ZERO)
    }

    /// Root: back to its screen anchor.
    pub fn clear_world_anchor(&mut self, node: NodeId) -> bool {
        self.edit(node, |n| n.world.take().is_some())
    }

    fn set_world_anchor(&mut self, node: NodeId, target: AnchorTarget, offset: Vec3) -> bool {
        self.edit(node, |n| {
            let max_distance = n.world.as_ref().and_then(|w| w.max_distance);
            n.world = Some(WorldAnchor {
                target,
                offset,
                max_distance,
                screen: None,
            });
            true
        })
    }

    /// Moves focus to a focusable node (sends `Blur` and `Focus`).
    pub fn focus(&mut self, node: NodeId) -> bool {
        if !self.node(node).is_some_and(|n| n.kind.is_focusable()) {
            return false;
        }
        self.set_focus(Some(node));
        true
    }

    pub fn clear_focus(&mut self) {
        self.set_focus(None);
    }

    pub(crate) fn set_focus(&mut self, node: Option<NodeId>) {
        if self.focused == node {
            return;
        }
        if let Some(old) = self.focused.take() {
            self.push_event(old, UiEventKind::Blur);
        }
        self.focused = node;
        if let Some(n) = node {
            if let Some(root) = self.root_of(n) {
                self.remembered.insert(root, n);
            }
            self.push_event(n, UiEventKind::Focus);
        }
        self.dirty = true;
    }

    pub(crate) fn push_event(&mut self, node: NodeId, kind: UiEventKind) {
        let (id, action) = self
            .node(node)
            .map(|n| (n.id.clone(), n.action.clone()))
            .unwrap_or_default();
        self.events.push(UiEvent {
            node,
            id,
            action,
            kind,
        });
    }

    /// Uses this theme file (`*.theme.json`, merged over the built-in
    /// theme), loaded through the world's asset server and hot-reloaded. A
    /// file that does not parse keeps the last good theme (with a warning).
    pub fn set_theme(&mut self, path: impl Into<String>) {
        self.theme_path = Some(path.into());
        self.theme_loaded = None;
    }

    /// Uses this theme (merged over the built-in one) instead of an asset.
    pub fn set_theme_value(&mut self, theme: Theme) {
        self.theme_path = None;
        self.theme_loaded = None;
        self.theme_overlay = Some(theme);
        self.rebuild_theme(None);
    }

    pub(crate) fn rebuild_theme(&mut self, asset: Option<&Theme>) {
        let mut t = Theme::builtin();
        if let Some(o) = &self.theme_overlay {
            t.overlay(o);
        }
        if let Some(a) = asset {
            t.overlay(a);
        }
        self.theme = t;
        self.theme_revision += 1;
        self.dirty = true;
    }

    /// The effective theme.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    pub fn set_scale_mode(&mut self, mode: ScaleMode) {
        if self.scale_mode != mode {
            self.scale_mode = mode;
            self.dirty = true;
        }
    }

    pub fn scale_mode(&self) -> ScaleMode {
        self.scale_mode
    }

    /// Forces layout and paint next frame (they otherwise run only when
    /// something changed).
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Roots in draw order (layer, then creation).
    pub(crate) fn draw_order(&self) -> Vec<NodeId> {
        let mut roots: Vec<(i32, u64, NodeId)> = self
            .roots
            .iter()
            .filter_map(|r| self.node(*r).map(|n| (n.layer, n.order, *r)))
            .collect();
        roots.sort();
        roots.into_iter().map(|(_, _, r)| r).collect()
    }

    /// `node`'s subtree in pre-order.
    pub(crate) fn preorder(&self, root: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            if let Some(node) = self.node(n) {
                out.push(n);
                stack.extend(node.children.iter().rev());
            }
        }
        out
    }

    /// Logs a warning once per distinct message.
    pub(crate) fn warn_once(&mut self, message: String) {
        if self.warned.insert(message.clone()) {
            tracing::warn!(target: "box_ui", "{message}");
        }
    }
}
