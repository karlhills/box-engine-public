//! In-game UI for box-engine (D11): a retained tree of nodes that game code
//! (and later scripts) describes and updates, with keyboard and gamepad
//! focus navigation, theme assets, styled text from string tables and
//! world-anchored labels.
//!
//! Games never draw UI every frame. They add nodes once (Rust builders or a
//! JSON document), keep [`NodeId`]s or look nodes up by `id`, change them
//! with setters, and react to [`UiEvent`]s:
//!
//! ```
//! use box_ui::{NodeKind, Ui, UiEvent, UiEventKind, UiNode};
//!
//! let mut ui = Ui::new();
//! let menu = ui.add(
//!     None,
//!     UiNode::panel()
//!         .modal()
//!         .anchored([0.5, 0.5], [0.0, 0.0])
//!         .child(UiNode::button("start", "menu.start"))
//!         .child(UiNode::button("quit", "menu.quit")),
//! );
//! let hud = ui
//!     .add_json(None, r#"{"kind": "text", "id": "coins", "key": "hud.coins", "args": {"count": "0"}}"#)
//!     .unwrap();
//! ui.set_arg(hud, "count", 3);
//! ui.set_visible(menu, false);
//! assert_eq!(ui.kind(ui.find(None, "quit").unwrap()), Some(NodeKind::Button));
//! # let _ = |e: &UiEvent| e.kind == UiEventKind::Click;
//! ```
//!
//! - [`node`]: node kinds and the document format; [`style`]: styles and
//!   themes (`*.theme.json`); [`layout`]: the layout model; [`input`]:
//!   actions, focus and input capture; [`strings`]: string tables;
//!   [`markup`]: inline text styling.
//! - Rendering: layout and paint run only when something changed; the
//!   result is the `box_render::Overlay` resource, which the renderer draws
//!   over the tone-mapped image in every target (window, capture, the
//!   editor's game view). Internally the painter is epaint (egui's
//!   tessellator and text engine); no egui type is part of this API.
//! - Scripts (`script` feature): the `ui` and `strings` Luau globals
//!   ([`script_api`]), added by [`UiPlugin`].
//! - [`UiPlugin`] (added by every `box_engine` app) installs the resources,
//!   events and systems; `box_engine` keeps [`UiScreen`] equal to the
//!   window or view size.

pub mod input;
pub mod layout;
pub mod markup;
pub mod node;
mod paint;
#[cfg(feature = "script")]
pub mod script_api;
pub mod strings;
pub mod style;
mod systems;
mod ui;

pub use input::{UI_ACTIONS, UI_BACK, UI_CONFIRM, UI_NAVIGATE, UI_NEXT, UI_PREV, default_actions};
pub use node::{NodeKind, UiNode, WorldAnchorDesc};
pub use paint::DEFAULT_FONT;
pub use strings::{Strings, fill, update_strings};
pub use style::{Align, Edges, FocusRing, Justify, Shadow, Size, Style, TextAlign, Theme};
pub use systems::{UiScreen, request_ui_assets, ui_input_system, ui_layout_system};
pub use ui::{NodeId, ScaleMode, Ui, UiError, UiEvent, UiEventKind, UiSettings};

use box_core::action::ActionMap;
use box_core::app::{App, Plugin, Stage};
use box_core::world::World;

/// Installs the [`Ui`], [`Strings`] and [`UiScreen`] resources,
/// `Events<UiEvent>`, the default UI actions (merged into the `ActionMap`
/// for names it lacks, also if the game replaces the map later), and the
/// systems: `ui_input` at the start of `Update`, string table loading and
/// `ui_layout` in `PostUpdate`.
pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.world.resource_or_default::<Ui>();
        app.world.resource_or_default::<Strings>();
        app.world.resource_or_default::<UiScreen>();
        app.add_event::<UiEvent>();
        merge_ui_actions(&mut app.world);
        app.system(Stage::Update, |w: &mut World| {
            merge_ui_actions(w);
            ui_input_system(w);
        })
        .label("ui_input");
        #[cfg(feature = "script")]
        {
            use box_script::AppScriptExt;
            app.add_script_api(script_api::UiApi)
                .add_script_api(script_api::StringsApi);
            let mut cursor = box_core::event::EventCursor::<UiEvent>::default();
            app.system(Stage::Update, move |w: &mut World| {
                script_api::forward_ui_events(&mut cursor, w)
            })
            .label("ui_script_events")
            .after("ui_input");
        }
        app.system(Stage::PostUpdate, update_strings)
            .label("ui_strings")
            .before("ui_layout");
        app.system(Stage::PostUpdate, ui_layout_system)
            .label("ui_layout");
    }
}

/// Adds the default UI actions the world's `ActionMap` lacks (cheap when
/// present: one lookup).
fn merge_ui_actions(world: &mut World) {
    let map = world.resource_or_default::<ActionMap>();
    if map.get(UI_NAVIGATE).is_none() || map.get(UI_CONFIRM).is_none() || map.get(UI_BACK).is_none()
    {
        map.merge_missing(&default_actions());
    }
}
