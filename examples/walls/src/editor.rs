//! The walls editor's tools (feature `editor`): **Draw Wall** and **Draw
//! Fence**, the engine's path tool ([`box_editor::path_tool`]) configured
//! for `WallPath` and `FencePath`, registered through an
//! [`EditorPlugin`] that `walls-editor` passes to
//! [`box_editor::run_game_with`].
//!
//! Walls snap to other walls' ends and bodies (within half their thickness
//! plus 10 cm of the centreline) and close by repeating the first point
//! (the rule mitres that junction). Fences snap to fences and to walls, so
//! a fence drawn up to a wall butts its face (the `fences` rule reads
//! walls).

use box_editor::path_tool::{Closing, PathTool, PathToolConfig, SnapSource};
use box_editor::{EditorPlugin, EditorState};

/// The Draw Wall tool's id (palette action `tool.draw_wall`).
pub const WALL_TOOL: &str = "draw_wall";
/// The Draw Fence tool's id (palette action `tool.draw_fence`).
pub const FENCE_TOOL: &str = "draw_fence";

/// Draw Wall: `WallPath`, 2.5 m high and 0.3 m thick, closable.
pub fn wall_tool() -> PathToolConfig {
    PathToolConfig::new(WALL_TOOL, "Wall", "WallPath", "points")
        .name("Draw Wall")
        .tooltip(
            "Draw walls: click to place corners (they snap to other walls' ends and faces), \
             Enter or click the last point to finish, click the first point to close a room, \
             Backspace removes the last point, Esc cancels. Drag a selected wall's points to \
             move them.",
        )
        .entity_name("Wall")
        .default_field("height", serde_json::json!(2.5))
        .default_field("thickness", serde_json::json!(0.3))
        .closing(Closing::RepeatFirstPoint)
        .snap_half_width("thickness", 0.5)
}

/// Draw Fence: `FencePath`, snapping to fences and walls.
pub fn fence_tool() -> PathToolConfig {
    PathToolConfig::new(FENCE_TOOL, "Fence", "FencePath", "points")
        .name("Draw Fence")
        .tooltip(
            "Draw fences: click to place posts (they snap to fences and walls; a fence that \
             meets a wall butts its face), Enter or click the last point to finish, \
             Backspace removes the last point, Esc cancels.",
        )
        .entity_name("Fence")
        .snap_half_width("post_size", 0.5)
        .snap_to(SnapSource {
            component: "WallPath",
            points: "points",
            half_width: Some(("thickness", 0.5)),
        })
}

/// Registers Draw Wall and Draw Fence (toolbar and palette).
pub struct WallsEditor;

impl EditorPlugin for WallsEditor {
    fn build(&self, editor: &mut EditorState) {
        editor.register_tool(PathTool::new(wall_tool()));
        editor.register_tool(PathTool::new(fence_tool()));
    }
}
