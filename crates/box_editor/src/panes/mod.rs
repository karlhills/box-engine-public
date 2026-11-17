//! Pane contents. Each pane is a function of `(&mut Ui, &mut EditorState)`;
//! the dock calls them (see `dock.rs`).

pub mod asset_inspector;
pub mod assets;
pub mod console;
pub mod hierarchy;
pub mod inspector;
pub mod viewport;

pub use assets::assets_ui;
pub use console::console_ui;
pub use hierarchy::hierarchy_ui;
pub use inspector::inspector_ui;
pub use viewport::viewport_ui;
