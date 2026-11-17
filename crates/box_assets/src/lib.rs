//! Asset pipeline for box-engine (docs/ASSETS.md).
//!
//! - [`AssetServer`]: `.meta` identity, background import, hot reload.
//! - [`Importer`]: the trait other crates implement to add file types.
//! - Built-in importers ([`importers`]): `texture` (PNG, JPEG → `ImageData`),
//!   `gltf` (glTF 2.0 / GLB → `MeshData`, `StandardMaterial`, `ImageData`,
//!   [`SceneAsset`]), `material` (`.material.json` → `StandardMaterial`, or
//!   `ShaderMaterial` when the file has a `"shader"` key), `shader` (`.wgsl`
//!   → `ShaderSource`, `@material` header validated), `scene` (`.scene.json`
//!   → [`SceneAsset`]), `json` (any other `.json` → [`JsonAsset`]: settings
//!   files), `font` (`.ttf`/`.otf`/`.ttc` → [`FontAsset`]), `strings`
//!   (`<language>.strings.json` → [`StringTable`]) and `data`
//!   (`.data.json` → [`DataTable`]).
//! - [`meta`]: the `.meta` format and the typed import settings.
//!
//! Setup in an app:
//!
//! ```no_run
//! use box_assets::{AssetPlugin, AssetServer};
//! use box_core::prelude::*;
//!
//! # fn main() -> Result<(), box_assets::AssetError> {
//! let server = AssetServer::new("my-game/assets")?;
//! server.watch(true)?; // hot reload (editor, development builds)
//! let mut app = App::new();
//! app.add_plugin(&AssetPlugin::new(server));
//! # Ok(()) }
//! ```

pub mod error;
pub mod importer;
pub mod importers;
mod index;
pub mod meta;
pub mod pak;
pub mod placeholder;
mod scene_asset;
mod server;
pub mod vfs;
pub mod watch;

pub use error::{AssetError, ImportError};
pub use importer::{ImportContext, Importer};
pub use importers::{DataTable, FontAsset, JsonAsset, StringTable};
pub use index::normalize_rel;
pub use pak::{Pak, PakError, PakWriter};
pub use scene_asset::SceneAsset;
pub use server::{AssetEvent, AssetEventKind, AssetServer, LoadState, SubAsset};

use box_core::app::{App, Plugin, Stage};
use box_core::assets::AssetLookup;
use box_core::world::World;

/// Resource in a Play world: the edit world's server, used only to request
/// loads. Its imports land in the edit world and reach the Play world
/// through `box_engine::asset_glue::AssetForwarder`; nothing runs this
/// server on the Play world.
#[derive(Clone)]
pub struct AssetMirror(pub AssetServer);

/// The server that loads for `world`: its own [`AssetServer`] resource, or
/// the edit world's through an [`AssetMirror`] (a Play world). Systems that
/// request assets (`load`) use this so they work in both.
pub fn loader(world: &World) -> Option<AssetServer> {
    world
        .get_resource::<AssetServer>()
        .cloned()
        .or_else(|| world.get_resource::<AssetMirror>().map(|m| m.0.clone()))
}

/// The system form of [`AssetServer::update`]: runs the world's
/// `AssetServer` resource, if there is one.
pub fn update_assets(world: &mut World) {
    if let Some(server) = world.get_resource::<AssetServer>().cloned() {
        server.update(world);
    }
}

/// Inserts an [`AssetServer`] as a world resource, the [`AssetLookup`]
/// resource and [`Events<AssetEvent>`](box_core::event::Events), and runs
/// [`update_assets`] in [`Stage::Update`]. Add it before plugins whose
/// Update systems read assets, so imports finished this frame are visible
/// to them.
pub struct AssetPlugin {
    server: AssetServer,
}

impl AssetPlugin {
    pub fn new(server: AssetServer) -> Self {
        Self { server }
    }
}

impl Plugin for AssetPlugin {
    fn build(&self, app: &mut App) {
        app.world.insert_resource(self.server.clone());
        app.world.resource_or_default::<AssetLookup>();
        app.add_event::<AssetEvent>();
        app.add_system(Stage::Update, update_assets);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn server_can_be_shared_across_threads() {
        fn check<T: Send + Sync + Clone + 'static>() {}
        check::<super::AssetServer>();
    }
}
