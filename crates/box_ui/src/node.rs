//! UI documents: [`UiNode`] describes a node and its children, in Rust
//! (builder methods) or JSON (the same fields; unknown fields are errors).
//!
//! ```json
//! { "kind": "panel", "id": "title", "modal": true, "anchor": [0.5, 0.5],
//!   "children": [
//!     { "kind": "text", "class": "title", "key": "menu.title" },
//!     { "kind": "button", "id": "start", "key": "menu.start", "action": "start" },
//!     { "kind": "slider", "id": "volume", "key": "options.volume", "value": 0.8 },
//!     { "kind": "toggle", "id": "subtitles", "key": "options.subtitles", "checked": true }
//!   ] }
//! ```

use std::collections::BTreeMap;

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::style::{Size, Style};

/// What a node is. Containers lay out their children; the rest are leaves
/// (children of a leaf are laid out over it, as in a `stack`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// A column with the theme's panel look (background, padding).
    #[default]
    Panel,
    /// Children top to bottom.
    Column,
    /// Children left to right.
    Row,
    /// Children on top of each other, aligned by `align` (x) and `justify` (y).
    Stack,
    /// Text (styled with inline tags, see [`crate::markup`]).
    Text,
    /// An image asset (`image`).
    Image,
    /// Focusable; confirm or click sends [`UiEventKind::Click`](crate::UiEventKind::Click).
    Button,
    /// Focusable label + bar; left/right or drag sends `Change`.
    Slider,
    /// Focusable label + switch; confirm or click sends `Toggle`.
    Toggle,
    /// A column that scrolls to keep the focused item visible (and on the
    /// mouse wheel); its content is clipped.
    List,
    /// A bar filled to `value` between `min` and `max`.
    Progress,
    /// Empty space that grows (`grow`).
    Spacer,
}

impl NodeKind {
    /// The theme class every node of this kind gets.
    pub fn class(self) -> &'static str {
        match self {
            NodeKind::Panel => "panel",
            NodeKind::Column => "column",
            NodeKind::Row => "row",
            NodeKind::Stack => "stack",
            NodeKind::Text => "text",
            NodeKind::Image => "image",
            NodeKind::Button => "button",
            NodeKind::Slider => "slider",
            NodeKind::Toggle => "toggle",
            NodeKind::List => "list",
            NodeKind::Progress => "progress",
            NodeKind::Spacer => "spacer",
        }
    }

    /// Takes keyboard and gamepad focus.
    pub fn is_focusable(self) -> bool {
        matches!(self, NodeKind::Button | NodeKind::Slider | NodeKind::Toggle)
    }

    /// Lays out children in a row (otherwise a column or stack).
    pub(crate) fn is_row(self) -> bool {
        matches!(self, NodeKind::Row)
    }

    pub(crate) fn is_stack(self) -> bool {
        !matches!(
            self,
            NodeKind::Panel | NodeKind::Column | NodeKind::Row | NodeKind::List
        )
    }
}

/// A position in the 3D world that a root node follows (a world-anchored
/// label): an entity (by uid or name) or a point, plus an offset in metres.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorldAnchorDesc {
    /// The entity's `Uid` or `Name`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
    /// A world point (used when `entity` is not set).
    pub point: [f32; 3],
    /// Added to the entity's (or point's) position, in metres.
    pub offset: [f32; 3],
    /// Hidden beyond this distance from the camera (metres).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_distance: Option<f32>,
}

/// A node description: what [`Ui::add`](crate::Ui::add) instantiates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiNode {
    pub kind: NodeKind,
    /// Name for [`Ui::find`](crate::Ui::find) and in events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Extra theme classes, space separated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    /// Literal text; `{name}` placeholders are filled from `args`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// String table key; wins over `text`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Placeholder values for `text` or `key`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub args: BTreeMap<String, String>,
    /// Image asset path (relative to `assets/`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Inline style over the theme classes.
    pub style: Style,
    pub hidden: bool,
    pub disabled: bool,
    /// Slider and progress value.
    pub value: f32,
    pub min: f32,
    pub max: f32,
    /// Slider step for left/right.
    pub step: f32,
    /// Toggle state.
    pub checked: bool,
    /// Sent with this node's events (`"start_game"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// Root only: while visible it owns keyboard and gamepad focus and
    /// captures game input (the topmost modal root wins).
    pub modal: bool,
    /// Takes focus when its modal root opens.
    pub autofocus: bool,
    /// Root only: draw order (higher on top; menus default 100, others 0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<i32>,
    /// Root only: where on the screen (0..=1 per axis) the node sits, as
    /// its own pivot too (`[0.5, 0.5]` centres it; `[1, 0]` is top right).
    pub anchor: [f32; 2],
    /// Root only: added to the anchored position, in reference points.
    pub offset: [f32; 2],
    /// Root only: follow a world position instead of a screen anchor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub world: Option<WorldAnchorDesc>,
    pub children: Vec<UiNode>,
}

impl Default for UiNode {
    fn default() -> Self {
        Self {
            kind: NodeKind::Panel,
            id: None,
            class: None,
            text: None,
            key: None,
            args: BTreeMap::new(),
            image: None,
            style: Style::default(),
            hidden: false,
            disabled: false,
            value: 0.0,
            min: 0.0,
            max: 1.0,
            step: 0.1,
            checked: false,
            action: None,
            modal: false,
            autofocus: false,
            layer: None,
            anchor: [0.0, 0.0],
            offset: [0.0, 0.0],
            world: None,
            children: Vec::new(),
        }
    }
}

impl UiNode {
    pub fn new(kind: NodeKind) -> Self {
        Self {
            kind,
            ..Default::default()
        }
    }

    pub fn panel() -> Self {
        Self::new(NodeKind::Panel)
    }

    pub fn column() -> Self {
        Self::new(NodeKind::Column)
    }

    pub fn row() -> Self {
        Self::new(NodeKind::Row)
    }

    pub fn stack() -> Self {
        Self::new(NodeKind::Stack)
    }

    pub fn list() -> Self {
        Self::new(NodeKind::List)
    }

    pub fn spacer() -> Self {
        Self::new(NodeKind::Spacer)
    }

    /// A text node with literal text.
    pub fn text(text: impl Into<String>) -> Self {
        Self::new(NodeKind::Text).with_text(text)
    }

    /// A text node from a string table key.
    pub fn label_key(key: impl Into<String>) -> Self {
        Self::new(NodeKind::Text).with_key(key)
    }

    /// A button with this id (also its action) and string table key (none
    /// when empty; give literal text with [`with_text`](Self::with_text)).
    pub fn button(id: &str, key: &str) -> Self {
        Self::new(NodeKind::Button)
            .with_id(id)
            .with_action(id)
            .with_key(key)
    }

    /// A slider with this id and key, from `min` to `max`.
    pub fn slider(id: &str, key: &str, value: f32, min: f32, max: f32, step: f32) -> Self {
        Self {
            value,
            min,
            max,
            step,
            ..Self::new(NodeKind::Slider).with_id(id).with_key(key)
        }
    }

    /// A toggle with this id and key.
    pub fn toggle(id: &str, key: &str, checked: bool) -> Self {
        Self {
            checked,
            ..Self::new(NodeKind::Toggle).with_id(id).with_key(key)
        }
    }

    /// A progress bar with this id, filled to `value` in 0..=1.
    pub fn progress(id: &str, value: f32) -> Self {
        Self {
            value,
            ..Self::new(NodeKind::Progress).with_id(id)
        }
    }

    /// An image node showing an image asset.
    pub fn image(path: impl Into<String>) -> Self {
        Self {
            image: Some(path.into()),
            ..Self::new(NodeKind::Image)
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_class(mut self, class: impl Into<String>) -> Self {
        self.class = Some(class.into());
        self
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Shows the text of a string table key (an empty key clears it).
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        let key = key.into();
        self.key = (!key.is_empty()).then_some(key);
        self
    }

    pub fn with_arg(mut self, name: impl Into<String>, value: impl ToString) -> Self {
        self.args.insert(name.into(), value.to_string());
        self
    }

    pub fn with_action(mut self, action: impl Into<String>) -> Self {
        self.action = Some(action.into());
        self
    }

    pub fn with_style(mut self, f: impl FnOnce(&mut Style)) -> Self {
        f(&mut self.style);
        self
    }

    pub fn with_width(self, width: Size) -> Self {
        self.with_style(|s| s.width = Some(width))
    }

    pub fn with_height(self, height: Size) -> Self {
        self.with_style(|s| s.height = Some(height))
    }

    /// Root: where on the screen it sits (also its pivot), plus an offset.
    pub fn anchored(mut self, anchor: [f32; 2], offset: [f32; 2]) -> Self {
        self.anchor = anchor;
        self.offset = offset;
        self
    }

    /// Root: owns focus and captures game input while visible.
    pub fn modal(mut self) -> Self {
        self.modal = true;
        self
    }

    pub fn autofocus(mut self) -> Self {
        self.autofocus = true;
        self
    }

    pub fn hidden(mut self) -> Self {
        self.hidden = true;
        self
    }

    pub fn with_layer(mut self, layer: i32) -> Self {
        self.layer = Some(layer);
        self
    }

    /// Root: follow a world point.
    pub fn at_point(mut self, point: Vec3) -> Self {
        self.world = Some(WorldAnchorDesc {
            point: point.to_array(),
            ..Default::default()
        });
        self
    }

    /// Root: follow an entity (by uid or name) with an offset in metres.
    pub fn at_entity(mut self, entity: impl Into<String>, offset: Vec3) -> Self {
        self.world = Some(WorldAnchorDesc {
            entity: Some(entity.into()),
            offset: offset.to_array(),
            ..Default::default()
        });
        self
    }

    pub fn child(mut self, child: UiNode) -> Self {
        self.children.push(child);
        self
    }

    pub fn children(mut self, children: impl IntoIterator<Item = UiNode>) -> Self {
        self.children.extend(children);
        self
    }

    /// Parses a document.
    pub fn from_json(json: &str) -> Result<UiNode, serde_json::Error> {
        serde_json::from_str(json)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_round_trip_and_reject_unknown_fields() {
        let n = UiNode::panel()
            .with_id("menu")
            .modal()
            .anchored([0.5, 0.5], [0.0, 0.0])
            .child(UiNode::button("start", "menu.start"))
            .child(UiNode::slider(
                "volume",
                "options.volume",
                0.5,
                0.0,
                1.0,
                0.1,
            ));
        let back = UiNode::from_json(&n.to_json()).unwrap();
        assert_eq!(back, n);
        assert!(UiNode::from_json(r#"{"kind": "button", "lable": "x"}"#).is_err());
        assert!(UiNode::from_json(r#"{"kind": "dropdown"}"#).is_err());
    }
}
