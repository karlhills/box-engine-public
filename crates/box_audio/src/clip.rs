//! [`AudioClip`] assets and their importer ([`AudioImporter`]).

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use box_assets::{ImportContext, ImportError, Importer};
use kira::sound::FromFileError;
use kira::sound::static_sound::StaticSoundData;
use kira::sound::streaming::StreamingSoundData;
use serde::{Deserialize, Serialize};

/// Clips longer than this stream with the default `"stream": "auto"`.
pub const STREAM_AUTO_SECONDS: f64 = 10.0;

/// A decoded (or streamable) sound file: an asset in `Assets<AudioClip>`,
/// imported from `.ogg`, `.wav`, `.mp3` and `.flac` files by
/// [`AudioImporter`]. Cheap to clone (the samples or encoded bytes are
/// shared).
///
/// Short clips are decoded to memory once, at import; long ones keep their
/// encoded bytes and are decoded while they play (see [`StreamMode`]).
#[derive(Clone)]
pub struct AudioClip {
    data: ClipData,
    duration: Duration,
}

#[derive(Clone)]
pub(crate) enum ClipData {
    /// Decoded frames, shared between plays (boxed: the settings make the
    /// value large, and clips are moved between threads and stores).
    Static(Box<StaticSoundData>),
    /// The encoded file, decoded on kira's streaming thread per play.
    Streaming(Arc<[u8]>),
}

impl std::fmt::Debug for AudioClip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioClip")
            .field("streaming", &self.is_streaming())
            .field("duration", &self.duration)
            .finish()
    }
}

impl AudioClip {
    /// Decodes a whole file (any supported format) into a static clip.
    pub fn decode(bytes: Vec<u8>) -> Result<Self, ImportError> {
        let data = StaticSoundData::from_cursor(Cursor::new(bytes)).map_err(decode_error)?;
        Ok(Self {
            duration: data.duration(),
            data: ClipData::Static(Box::new(data)),
        })
    }

    /// Keeps a file's encoded bytes for streaming, after checking that it
    /// opens and has a known length.
    pub fn streaming(bytes: Vec<u8>) -> Result<Self, ImportError> {
        let bytes: Arc<[u8]> = bytes.into();
        let probe =
            StreamingSoundData::from_cursor(Cursor::new(bytes.clone())).map_err(decode_error)?;
        Ok(Self {
            duration: probe.duration(),
            data: ClipData::Streaming(bytes),
        })
    }

    /// Length of the clip at normal speed.
    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// True when the clip is decoded while it plays (long ambience and
    /// music), false when it is decoded in memory.
    pub fn is_streaming(&self) -> bool {
        matches!(self.data, ClipData::Streaming(_))
    }

    pub(crate) fn data(&self) -> &ClipData {
        &self.data
    }
}

fn decode_error(e: FromFileError) -> ImportError {
    ImportError::Decode(e.to_string())
}

/// Whether a clip is decoded to memory at import or streamed while it plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamMode {
    /// Stream clips longer than [`STREAM_AUTO_SECONDS`] (ambience, music),
    /// decode shorter ones (effects).
    #[default]
    Auto,
    /// Always stream.
    Always,
    /// Always decode to memory.
    Never,
}

/// Import settings of an audio file (the `settings` object of its `.meta`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioImportSettings {
    /// `"auto"`, `"always"` or `"never"`.
    pub stream: StreamMode,
}

/// The `audio` importer: `.ogg` (Vorbis), `.wav`, `.mp3` and `.flac` files
/// as [`AudioClip`]s, decoded by symphonia through kira. A file that does
/// not decode is a failed import (logged, no clip); a file whose length
/// cannot be read is decoded to memory even when streaming was asked for.
pub struct AudioImporter;

impl Importer for AudioImporter {
    fn name(&self) -> &str {
        "audio"
    }

    fn extensions(&self) -> &[&str] {
        &["ogg", "wav", "mp3", "flac"]
    }

    fn default_settings(&self) -> serde_json::Value {
        serde_json::to_value(AudioImportSettings::default()).unwrap_or_default()
    }

    fn import(&self, ctx: &mut ImportContext) -> Result<(), ImportError> {
        let settings: AudioImportSettings = ctx.settings()?;
        let bytes = ctx.bytes().to_vec();
        let clip = match settings.stream {
            StreamMode::Never => AudioClip::decode(bytes)?,
            StreamMode::Always => match AudioClip::streaming(bytes.clone()) {
                Ok(clip) => clip,
                Err(e) => {
                    tracing::warn!(
                        target: "box_audio",
                        "{}: cannot stream ({e}); decoded to memory instead",
                        ctx.path()
                    );
                    AudioClip::decode(bytes)?
                }
            },
            StreamMode::Auto => match AudioClip::streaming(bytes.clone()) {
                Ok(clip) if clip.duration.as_secs_f64() > STREAM_AUTO_SECONDS => clip,
                _ => AudioClip::decode(bytes)?,
            },
        };
        ctx.add(None, clip);
        Ok(())
    }
}
