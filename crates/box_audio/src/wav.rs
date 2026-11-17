//! A minimal WAV writer (16-bit PCM), for sounds made in code: generated
//! test clips, procedural effects saved as assets.

/// Encodes interleaved samples in `-1.0..=1.0` (clamped) as a 16-bit PCM
/// WAV file with `channels` channels at `sample_rate` Hz.
pub fn encode_pcm16(samples: &[f32], sample_rate: u32, channels: u16) -> Vec<u8> {
    let channels = channels.max(1);
    let data_len = (samples.len() * 2) as u32;
    let block_align = channels * 2;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * u32::from(block_align)).to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AudioClip;

    #[test]
    fn written_files_decode_with_the_right_length() {
        let samples: Vec<f32> = (0..8000)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 8000.0).sin() * 0.5)
            .collect();
        let bytes = encode_pcm16(&samples, 8000, 1);
        assert_eq!(bytes.len(), 44 + 16000);
        let clip = AudioClip::decode(bytes).unwrap();
        assert!((clip.duration().as_secs_f64() - 1.0).abs() < 1e-3);
        assert!(!clip.is_streaming());
    }
}
