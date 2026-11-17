//! Writes the engine's sample sounds, synthesised here (no recorded or
//! third-party audio): `pickup.wav`, a short two-note chime, and
//! `ambience.wav`, a soft wind loop that repeats without a click.
//!
//! `cargo run -p box_audio --example gen_clips -- <folder>...`
//!
//! The committed copies are in `crates/box_engine/examples/audio_assets/audio`
//! and `examples/collector/assets/audio`. The output is deterministic.

use std::f32::consts::TAU;
use std::path::PathBuf;

use box_audio::wav::encode_pcm16;

/// A two-note rising chime with a fast exponential decay, 0.35 s.
fn pickup() -> (Vec<f32>, u32) {
    let rate = 22_050;
    let n = (0.35 * rate as f32) as usize;
    let note = |t: f32, f: f32| {
        let env = (-t * 14.0).exp() * (t * 400.0).min(1.0);
        env * ((TAU * f * t).sin()
            + 0.3 * (TAU * 2.0 * f * t).sin()
            + 0.1 * (TAU * 3.0 * f * t).sin())
    };
    let samples = (0..n)
        .map(|i| {
            let t = i as f32 / rate as f32;
            let second = (t - 0.07).max(0.0);
            0.35 * note(t, 987.77)
                + if t >= 0.07 {
                    0.4 * note(second, 1318.5)
                } else {
                    0.0
                }
        })
        .collect();
    (samples, rate)
}

/// Low-passed noise with a slow swell, 3 s, periodic: the filter runs over
/// the loop twice so its state at the start equals its state at the end,
/// and the swell has a whole number of cycles.
fn ambience() -> (Vec<f32>, u32) {
    let rate = 11_025;
    let n = 3 * rate as usize;
    let mut seed = 0x2545_f491_u32;
    let noise: Vec<f32> = (0..n)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32 * 2.0 - 1.0
        })
        .collect();
    let mut out = vec![0.0f32; n];
    let (mut a, mut b) = (0.0f32, 0.0f32);
    for pass in 0..2 {
        for (i, x) in noise.iter().enumerate() {
            // Two one-pole low-passes: a dull rush.
            a += 0.06 * (x - a);
            b += 0.06 * (a - b);
            if pass == 1 {
                out[i] = b;
            }
        }
    }
    for (i, s) in out.iter_mut().enumerate() {
        let t = i as f32 / n as f32;
        *s *= 0.75 + 0.25 * (TAU * t).sin() + 0.1 * (TAU * 3.0 * t).sin();
    }
    let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    for s in &mut out {
        *s *= 0.5 / peak;
    }
    (out, rate)
}

fn main() -> anyhow::Result<()> {
    let dirs: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    anyhow::ensure!(!dirs.is_empty(), "usage: gen_clips <folder>...");
    for dir in dirs {
        std::fs::create_dir_all(&dir)?;
        for (name, (samples, rate)) in [("pickup.wav", pickup()), ("ambience.wav", ambience())] {
            let path = dir.join(name);
            std::fs::write(&path, encode_pcm16(&samples, rate, 1))?;
            println!("{}", path.display());
        }
    }
    Ok(())
}
