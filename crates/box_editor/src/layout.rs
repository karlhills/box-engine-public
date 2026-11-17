//! Dock layout: the panes, the default arrangement and the per-project layout
//! file `<project>/.box/layout.json` (`{version, tree}`).

use std::path::{Path, PathBuf};

use egui_tiles::{Linear, LinearDir, Tabs, TileId, Tiles, Tree};
use serde::{Deserialize, Serialize};

/// The dockable panels. The variants are the stable panel IDs stored in the
/// layout file (EDITOR.md section 3); tile ids inside the tree are arbitrary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Pane {
    Viewport,
    Hierarchy,
    Inspector,
    Console,
    /// The asset browser (M4).
    Assets,
}

impl Pane {
    pub const ALL: [Pane; 5] = [
        Pane::Viewport,
        Pane::Hierarchy,
        Pane::Inspector,
        Pane::Console,
        Pane::Assets,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Pane::Viewport => "Viewport",
            Pane::Hierarchy => "Hierarchy",
            Pane::Inspector => "Inspector",
            Pane::Console => "Console",
            Pane::Assets => "Assets",
        }
    }
}

/// Bump when the file format or the pane set changes incompatibly; older
/// files then fall back to the default layout.
pub const LAYOUT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct LayoutFile {
    version: u32,
    tree: Tree<Pane>,
}

#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("layout version {found}, expected {LAYOUT_VERSION}")]
    Version { found: u32 },
    #[error("layout has no root tile")]
    NoRoot,
    #[error("pane {pane:?} appears {count} times (expected once)")]
    PaneCount { pane: Pane, count: usize },
}

/// `<project>/.box/layout.json`.
pub fn layout_path(project: &Path) -> PathBuf {
    project.join(".box").join("layout.json")
}

fn tabs(tiles: &mut Tiles<Pane>, panes: &[Pane]) -> TileId {
    let ids: Vec<TileId> = panes.iter().map(|p| tiles.insert_pane(*p)).collect();
    tiles.insert_container(Tabs::new(ids))
}

fn linear(tiles: &mut Tiles<Pane>, dir: LinearDir, kids: &[(TileId, f32)]) -> TileId {
    let mut l = Linear::new(dir, kids.iter().map(|k| k.0).collect());
    for (id, share) in kids {
        l.shares.set_share(*id, *share);
    }
    tiles.insert_container(l)
}

fn tree_id() -> egui::Id {
    egui::Id::unique("box_editor_dock")
}

/// EDITOR.md section 3: hierarchy left of the viewport, assets and console
/// tabbed below both (assets in front), inspector full height on the right.
pub fn default_layout() -> Tree<Pane> {
    let mut tiles = Tiles::default();
    let hierarchy = tabs(&mut tiles, &[Pane::Hierarchy]);
    let viewport = tabs(&mut tiles, &[Pane::Viewport]);
    let console = tabs(&mut tiles, &[Pane::Assets, Pane::Console]);
    let inspector = tabs(&mut tiles, &[Pane::Inspector]);
    let top = linear(
        &mut tiles,
        LinearDir::Horizontal,
        &[(hierarchy, 0.24), (viewport, 0.76)],
    );
    let left = linear(
        &mut tiles,
        LinearDir::Vertical,
        &[(top, 0.70), (console, 0.30)],
    );
    let root = linear(
        &mut tiles,
        LinearDir::Horizontal,
        &[(left, 0.77), (inspector, 0.23)],
    );
    Tree::new(tree_id(), root, tiles)
}

/// The default layout for an editor with or without a project. Without
/// one the asset browser has nothing to show, so the Console is in front.
pub fn default_layout_for(project: bool) -> Tree<Pane> {
    let mut tree = default_layout();
    if !project {
        tree.make_active(|_, tile| matches!(tile, egui_tiles::Tile::Pane(Pane::Console)));
    }
    tree
}

/// Checks that the tree has a root and every [`Pane`] exactly once.
pub fn validate(tree: &Tree<Pane>) -> Result<(), LayoutError> {
    let root = tree.root().ok_or(LayoutError::NoRoot)?;
    if tree.tiles.get(root).is_none() {
        return Err(LayoutError::NoRoot);
    }
    for pane in Pane::ALL {
        let count = tree
            .tiles
            .iter()
            .filter(|(_, tile)| matches!(tile, egui_tiles::Tile::Pane(p) if *p == pane))
            .count();
        if count != 1 {
            return Err(LayoutError::PaneCount { pane, count });
        }
    }
    Ok(())
}

pub fn save_layout(path: &Path, tree: &Tree<Pane>) -> Result<(), LayoutError> {
    let io = |source| LayoutError::Io {
        path: path.to_owned(),
        source,
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let file = LayoutFile {
        version: LAYOUT_VERSION,
        tree: tree.clone(),
    };
    let json = serde_json::to_string_pretty(&file).map_err(|source| LayoutError::Json {
        path: path.to_owned(),
        source,
    })?;
    std::fs::write(path, json).map_err(io)
}

/// Reads and validates a layout file.
pub fn load_layout(path: &Path) -> Result<Tree<Pane>, LayoutError> {
    let text = std::fs::read_to_string(path).map_err(|source| LayoutError::Io {
        path: path.to_owned(),
        source,
    })?;
    let file: LayoutFile = serde_json::from_str(&text).map_err(|source| LayoutError::Json {
        path: path.to_owned(),
        source,
    })?;
    if file.version != LAYOUT_VERSION {
        return Err(LayoutError::Version {
            found: file.version,
        });
    }
    let mut tree = file.tree;
    add_missing_assets_tab(&mut tree);
    validate(&tree)?;
    Ok(tree)
}

/// Layouts saved before M4 have no Assets pane: add it as a tab next to the
/// console (or next to any tabbed pane), so such files still load.
fn add_missing_assets_tab(tree: &mut Tree<Pane>) {
    if tree.tiles.find_pane(&Pane::Assets).is_some() {
        return;
    }
    let anchor = [
        Pane::Console,
        Pane::Hierarchy,
        Pane::Inspector,
        Pane::Viewport,
    ]
    .iter()
    .find_map(|p| tree.tiles.find_pane(p));
    let Some(anchor) = anchor else {
        return;
    };
    let Some(parent) = tree.tiles.parent_of(anchor) else {
        return;
    };
    let assets = tree.tiles.insert_pane(Pane::Assets);
    if let Some(egui_tiles::Tile::Container(egui_tiles::Container::Tabs(tabs))) =
        tree.tiles.get_mut(parent)
    {
        tabs.add_child(assets);
    } else {
        tree.tiles.remove(assets);
    }
}

/// The saved layout, or the default with the reason it was not used (`None`
/// when there is simply no file yet).
pub fn load_or_default(path: &Path) -> (Tree<Pane>, Option<LayoutError>) {
    if !path.exists() {
        return (default_layout(), None);
    }
    match load_layout(path) {
        Ok(tree) => (tree, None),
        Err(e) => (default_layout(), Some(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("box_editor_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn default_layout_is_valid() {
        validate(&default_layout()).unwrap();
        validate(&default_layout_for(false)).unwrap();
    }

    /// The active tab of the tabs container holding `pane`.
    fn active_beside(tree: &Tree<Pane>, pane: Pane) -> Option<Pane> {
        let id = tree.tiles.find_pane(&pane)?;
        let parent = tree.tiles.parent_of(id)?;
        match tree.tiles.get(parent)? {
            egui_tiles::Tile::Container(egui_tiles::Container::Tabs(t)) => {
                tree.tiles.get_pane(&t.active?).copied()
            }
            _ => None,
        }
    }

    #[test]
    fn the_console_is_in_front_without_a_project() {
        assert_eq!(
            active_beside(&default_layout(), Pane::Console),
            Some(Pane::Assets)
        );
        assert_eq!(
            active_beside(&default_layout_for(true), Pane::Console),
            Some(Pane::Assets)
        );
        assert_eq!(
            active_beside(&default_layout_for(false), Pane::Console),
            Some(Pane::Console)
        );
    }

    #[test]
    fn layout_round_trips_through_the_file() {
        let dir = temp_dir("layout_rt");
        let path = layout_path(&dir);
        let tree = default_layout();
        save_layout(&path, &tree).unwrap();
        let loaded = load_layout(&path).unwrap();
        assert_eq!(
            serde_json::to_value(&loaded).unwrap(),
            serde_json::to_value(&tree).unwrap()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn layouts_from_before_m4_get_the_assets_tab() {
        let dir = temp_dir("layout_m3");
        let path = layout_path(&dir);
        let mut tree = default_layout();
        let assets = tree.tiles.find_pane(&Pane::Assets).unwrap();
        tree.remove_recursively(assets);
        assert!(tree.tiles.find_pane(&Pane::Assets).is_none());
        save_layout(&path, &tree).unwrap();
        let loaded = load_layout(&path).expect("an M3 layout still loads");
        let assets = loaded.tiles.find_pane(&Pane::Assets).unwrap();
        let console = loaded.tiles.find_pane(&Pane::Console).unwrap();
        assert_eq!(
            loaded.tiles.parent_of(assets),
            loaded.tiles.parent_of(console),
            "tabbed with the console"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_files_fall_back_to_the_default() {
        let dir = temp_dir("layout_bad");
        let path = layout_path(&dir);

        // Missing file: default, no error.
        let (_, err) = load_or_default(&path);
        assert!(err.is_none());

        // Garbage.
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        assert!(matches!(
            load_or_default(&path).1,
            Some(LayoutError::Json { .. })
        ));

        // Wrong version.
        let mut value = serde_json::to_value(LayoutFile {
            version: LAYOUT_VERSION,
            tree: default_layout(),
        })
        .unwrap();
        value["version"] = serde_json::json!(LAYOUT_VERSION + 1);
        std::fs::write(&path, value.to_string()).unwrap();
        assert!(matches!(
            load_or_default(&path).1,
            Some(LayoutError::Version { .. })
        ));

        // A pane missing: remove the console tile.
        let mut tree = default_layout();
        let console = tree.tiles.find_pane(&Pane::Console).unwrap();
        tree.remove_recursively(console);
        save_layout(&path, &tree).unwrap();
        let (fallback, err) = load_or_default(&path);
        assert!(
            matches!(
                err,
                Some(LayoutError::PaneCount {
                    pane: Pane::Console,
                    count: 0
                })
            ),
            "{err:?}"
        );
        validate(&fallback).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
