//! Where audio goes: a sound device, a capture buffer (tests), or nowhere.

use std::sync::atomic::{AtomicBool, Ordering};

use kira::backend::cpal::CpalBackend;
use kira::backend::{Backend, Renderer};
use kira::listener::ListenerHandle;
use kira::track::{MainTrackHandle, TrackBuilder, TrackHandle};
use kira::{AudioManager, AudioManagerSettings, Capacities, ResourceLimitReached};

static DEVICE_OUTPUT: AtomicBool = AtomicBool::new(false);

/// Lets worlds of this process play through the sound device (when they do
/// not choose an [`AudioOutput`] themselves). The windowed runner and the
/// windowed editor call it; tests and headless runs never do, so they stay
/// [`AudioOutput::Silent`] without opening a device.
pub fn enable_device_output() {
    DEVICE_OUTPUT.store(true, Ordering::Relaxed);
}

/// Whether [`enable_device_output`] was called.
pub fn device_output_enabled() -> bool {
    DEVICE_OUTPUT.load(Ordering::Relaxed)
}

/// Resource: the output a world's audio uses, read when the first sound
/// starts. Without it: [`Device`](Self::Device) after
/// [`enable_device_output`], else [`Silent`](Self::Silent).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioOutput {
    /// Nothing is played and no device is opened. Sounds are still tracked
    /// ([`Audio::voices`](crate::Audio::voices)), one-shots for the length
    /// of their clip.
    Silent,
    /// The default sound device. When none can be opened, one warning is
    /// logged and the world falls back to `Silent`.
    Device,
    /// Mixed in memory at `sample_rate` Hz, advanced by each frame's
    /// unscaled delta, nothing heard: [`Audio::levels`](crate::Audio::levels)
    /// reports what a device would have played. For tests.
    Capture { sample_rate: u32 },
}

impl AudioOutput {
    /// The output to use when the world has no `AudioOutput` resource.
    pub fn process_default() -> Self {
        if device_output_enabled() {
            Self::Device
        } else {
            Self::Silent
        }
    }
}

/// Loudness of the mixed output over the last frame (capture output).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OutputLevels {
    /// Root mean square of the left channel.
    pub left: f32,
    /// Root mean square of the right channel.
    pub right: f32,
    /// Largest absolute sample of either channel.
    pub peak: f32,
}

impl OutputLevels {
    /// Mean of both channels' RMS.
    pub fn rms(&self) -> f32 {
        (self.left + self.right) * 0.5
    }
}

/// A kira backend that mixes in memory when asked ([`AudioOutput::Capture`]).
pub(crate) struct CaptureBackend {
    sample_rate: u32,
    renderer: Option<Renderer>,
    buffer: Vec<f32>,
    /// Fraction of a frame carried to the next advance.
    carry: f64,
}

impl Backend for CaptureBackend {
    type Settings = u32;
    type Error = std::convert::Infallible;

    fn setup(sample_rate: u32, _internal_buffer_size: usize) -> Result<(Self, u32), Self::Error> {
        let sample_rate = sample_rate.max(1);
        Ok((
            Self {
                sample_rate,
                renderer: None,
                buffer: Vec::new(),
                carry: 0.0,
            },
            sample_rate,
        ))
    }

    fn start(&mut self, renderer: Renderer) -> Result<(), Self::Error> {
        self.renderer = Some(renderer);
        Ok(())
    }
}

impl CaptureBackend {
    /// Mixes `seconds` of audio and measures it.
    fn advance(&mut self, seconds: f64) -> OutputLevels {
        let Some(renderer) = &mut self.renderer else {
            return OutputLevels::default();
        };
        let exact = seconds.max(0.0) * f64::from(self.sample_rate) + self.carry;
        let frames = exact.floor() as usize;
        self.carry = exact - frames as f64;
        if frames == 0 {
            return OutputLevels::default();
        }
        self.buffer.clear();
        self.buffer.resize(frames * 2, 0.0);
        renderer.on_start_processing();
        renderer.process(&mut self.buffer, 2);
        let (mut l, mut r, mut peak) = (0.0f64, 0.0f64, 0.0f32);
        for frame in self.buffer.as_chunks::<2>().0 {
            l += f64::from(frame[0] * frame[0]);
            r += f64::from(frame[1] * frame[1]);
            peak = peak.max(frame[0].abs()).max(frame[1].abs());
        }
        let n = frames as f64;
        OutputLevels {
            left: (l / n).sqrt() as f32,
            right: (r / n).sqrt() as f32,
            peak,
        }
    }
}

/// An open kira manager on one of the two real backends.
pub(crate) enum Output {
    Device(Box<AudioManager<CpalBackend>>),
    Capture(Box<AudioManager<CaptureBackend>>),
}

/// Sub-tracks per bus: one per positional sound playing at once.
const BUS_SUB_TRACKS: usize = 256;
/// Sounds per bus or positional track.
const SOUNDS_PER_TRACK: usize = 256;

impl Output {
    fn settings<B: Backend>(backend_settings: B::Settings) -> AudioManagerSettings<B> {
        AudioManagerSettings {
            capacities: Capacities {
                sub_track_capacity: 16,
                ..Default::default()
            },
            main_track_builder: Default::default(),
            internal_buffer_size: 128,
            backend_settings,
        }
    }

    /// Opens the default sound device.
    pub fn device() -> Result<Self, String> {
        AudioManager::<CpalBackend>::new(Self::settings(Default::default()))
            .map(|m| Self::Device(Box::new(m)))
            .map_err(|e| e.to_string())
    }

    /// An in-memory mixer at `sample_rate` Hz.
    pub fn capture(sample_rate: u32) -> Self {
        match AudioManager::<CaptureBackend>::new(Self::settings(sample_rate)) {
            Ok(m) => Self::Capture(Box::new(m)),
            Err(e) => match e {},
        }
    }

    pub fn main_track(&mut self) -> &mut MainTrackHandle {
        match self {
            Self::Device(m) => m.main_track(),
            Self::Capture(m) => m.main_track(),
        }
    }

    pub fn add_bus(&mut self) -> Result<TrackHandle, ResourceLimitReached> {
        let builder = TrackBuilder::new()
            .sub_track_capacity(BUS_SUB_TRACKS)
            .sound_capacity(SOUNDS_PER_TRACK);
        match self {
            Self::Device(m) => m.add_sub_track(builder),
            Self::Capture(m) => m.add_sub_track(builder),
        }
    }

    pub fn add_listener(
        &mut self,
        position: mint::Vector3<f32>,
        orientation: mint::Quaternion<f32>,
    ) -> Result<ListenerHandle, ResourceLimitReached> {
        match self {
            Self::Device(m) => m.add_listener(position, orientation),
            Self::Capture(m) => m.add_listener(position, orientation),
        }
    }

    /// Advances a capture output by `seconds` and measures it; logs device
    /// stream errors. Returns levels only for a capture output.
    pub fn frame(&mut self, seconds: f64, logged: &mut impl FnMut(String)) -> Option<OutputLevels> {
        match self {
            Self::Device(m) => {
                while let Some(e) = m.backend_mut().pop_error() {
                    logged(format!("audio device error: {e}"));
                }
                None
            }
            Self::Capture(m) => Some(m.backend_mut().advance(seconds)),
        }
    }
}
