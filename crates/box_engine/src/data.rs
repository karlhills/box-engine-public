//! Data tables (`*.data.json` → [`DataTable`]) by path, for game code and
//! (later) scripts.
//!
//! ```no_run
//! use box_engine::prelude::*;
//!
//! #[derive(serde::Deserialize)]
//! struct Enemy { hp: u32 }
//!
//! fn spawn_wave(world: &mut World) {
//!     // `None` until the file is imported (the first frames); hot reload
//!     // replaces the value in place.
//!     let Some(table) = data_table(world, "data/enemies.data.json") else { return };
//!     let _slime: Option<Enemy> = table.row_as("slime").ok();
//! }
//! # fn main() {}
//! ```
//!
//! The first call requests the file through the world's asset server (the
//! edit world's in Play); [`data_version`] changes on every reload, so a
//! system can rebuild what it derived from a table only when needed.

use std::collections::HashMap;

use box_assets::DataTable;
use box_core::asset_ref::AssetRef;
use box_core::assets::{Assets, Handle};
use box_core::world::World;

/// Resource: the data tables requested so far, by path.
#[derive(Debug, Default)]
pub struct DataTables {
    handles: HashMap<String, Option<Handle<DataTable>>>,
}

fn handle(world: &mut World, path: &str) -> Option<Handle<DataTable>> {
    if let Some(h) = world
        .get_resource::<DataTables>()
        .and_then(|t| t.handles.get(path).copied())
    {
        return h;
    }
    let h = match box_assets::loader(world) {
        Some(server) => match server.load::<DataTable>(&AssetRef::path(path)) {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::warn!(target: "box_engine::data", "data table {path}: {e}");
                None
            }
        },
        None => {
            tracing::warn!(target: "box_engine::data", "data table {path}: this world has no asset server");
            None
        }
    };
    world
        .resource_or_default::<DataTables>()
        .handles
        .insert(path.to_string(), h);
    h
}

/// The data table at `path` (relative to `assets/`), once imported.
pub fn data_table<'w>(world: &'w mut World, path: &str) -> Option<&'w DataTable> {
    let h = handle(world, path)?;
    world.get_resource::<Assets<DataTable>>()?.get(h)
}

/// The version of the data table at `path` (changes on every hot reload);
/// `None` until imported.
pub fn data_version(world: &mut World, path: &str) -> Option<u64> {
    let h = handle(world, path)?;
    world.get_resource::<Assets<DataTable>>()?.version(h)
}
