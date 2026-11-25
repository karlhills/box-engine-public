//! box-engine audio on `kira` (decoding by symphonia through kira).
//! Games never use kira types.
//!
//! - [`AudioClip`]: an asset imported from `.ogg`, `.wav`, `.mp3` and
//!   `.flac` files by [`AudioImporter`]; short clips are decoded to memory,
//!   long ones streamed ([`StreamMode`], the `stream` import setting).
//! - [`AudioSource`]: a saved component that plays a clip while its entity
//!   exists (ambience loops, music, positional emitters), with fades.
//! - [`PlaySound`]: an event for one-shots, positional or not.
//! - [`AudioListener`]: where positional sounds are heard from (default: the
//!   active camera, through [`AudioPlugin::with_default_listener`]).
//! - [`AudioSettings`]: master and bus volumes, a project settings resource.
//! - [`Audio`]: the resource holding the output and the playing voices.
//! - Mixer: each voice's level at the listener ([`VoiceInfo::level`],
//!   measured on kira's audio thread, any output but silent), its distance,
//!   attenuation and pan ([`VoiceInfo::spatial`]), and runtime solo and
//!   mute of entities (with the sources below them), buses and clips
//!   ([`Audio::solo`], [`Audio::mute`], [`MixTarget`]).
//! - [`AudioOutput`]: device, silent or capture (tests). Nothing opens a
//!   device unless the process called [`enable_device_output`] (windowed
//!   runs do); a missing device logs one warning and plays silently.
//!
//! ```
//! use box_audio::{AudioOutput, AudioPlugin, PlaySound};
//! use box_core::prelude::*;
//!
//! let mut app = App::new();
//! app.world.insert_resource(AudioOutput::Silent);
//! app.add_plugin(&AudioPlugin::new());
//! app.world.send(PlaySound::new("audio/pickup.wav").at(Vec3::new(1.0, 0.0, -2.0)));
//! app.run_frames(1, 1.0 / 60.0);
//! ```

mod audio;
mod clip;
mod components;
mod meter;
mod output;
pub mod wav;

pub use audio::{Audio, OutputKind, PENDING_TIMEOUT, VoiceInfo, VoiceState, update_audio};
pub use clip::{AudioClip, AudioImportSettings, AudioImporter, STREAM_AUTO_SECONDS, StreamMode};
pub use components::{AudioBus, AudioListener, AudioSettings, AudioSource, PlaySound};
pub use meter::{Mix, MixTarget, Spatial, VoiceLevel, amplitude_db, spatial};
pub use output::{AudioOutput, OutputLevels, device_output_enabled, enable_device_output};

use box_assets::AssetServer;
use box_core::app::{App, Plugin, Stage};
use box_core::reflect::Registry;
use box_core::transform::GlobalTransform;
use box_core::world::World;

/// Registers the audio scene types: the [`AudioSource`] and
/// [`AudioListener`] components, the [`AudioSettings`] settings resource
/// and the [`PlaySound`] event. Registering twice is harmless.
pub fn register_audio_types(registry: &mut Registry) {
    for result in [
        registry.register::<AudioSource>(),
        registry.register::<AudioListener>(),
        registry.register_settings::<AudioSettings>(),
        registry.register_event::<PlaySound>(),
    ] {
        if let Err(e) = result {
            tracing::error!(target: "box_audio", "audio type registration failed: {e}");
        }
    }
}

/// Adds the [`AudioClip`] asset type and the [`AudioImporter`] to a server
/// (indexing the audio files in its folder). Call it once per server, right
/// after creating it; `box_engine::asset_server` does.
pub fn register_audio_assets(server: &AssetServer) {
    // No placeholder: a clip that failed to import plays nothing.
    server.register_asset_type::<AudioClip>(None);
    server.register_importer(AudioImporter);
}

/// Audio as a plugin: registers the audio types, adds the [`PlaySound`]
/// event and the [`Audio`] resource, and runs [`update_audio`] in
/// `PostUpdate` (after transform propagation, so positions are this
/// frame's).
///
/// The world needs an asset server with [`register_audio_assets`] (or an
/// `AssetLookup` that resolves clips) for sounds to find their clips.
#[derive(Clone, Copy, Default)]
pub struct AudioPlugin {
    default_listener: Option<fn(&World) -> Option<GlobalTransform>>,
}

impl AudioPlugin {
    pub fn new() -> Self {
        Self::default()
    }

    /// Where sounds are heard from when no entity has an
    /// [`AudioListener`]: box_engine passes the active camera.
    pub fn with_default_listener(mut self, f: fn(&World) -> Option<GlobalTransform>) -> Self {
        self.default_listener = Some(f);
        self
    }
}

impl Plugin for AudioPlugin {
    fn register(&self, registry: &mut Registry) {
        register_audio_types(registry);
    }

    fn build(&self, app: &mut App) {
        app.add_event::<PlaySound>();
        if !app.world.contains_resource::<Audio>() {
            app.world.insert_resource(Audio::new(self.default_listener));
        }
        app.add_system(Stage::PostUpdate, update_audio);
    }
}
