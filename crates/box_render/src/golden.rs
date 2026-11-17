//! Golden-image comparison.
//!
//! A pixel is *bad* when its largest per-channel difference exceeds a
//! threshold. An image passes when few pixels are bad **and** the bad pixels
//! are scattered: the largest 8-connected region of bad pixels is capped, so a
//! missing or moved object fails even when it is small, while dither noise and
//! anti-aliasing differences (isolated pixels) pass. Diagonal neighbours count
//! as connected, so a missing one-pixel diagonal line is one region.

use std::path::{Path, PathBuf};

use image::{Rgba, RgbaImage};

/// How different an image may be from its reference.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tolerance {
    /// Largest per-channel difference that still counts as equal.
    pub channel_threshold: u8,
    /// Fraction of pixels allowed to be bad.
    pub max_bad_fraction: f32,
    /// Largest allowed 8-connected region of bad pixels, in pixels.
    pub max_bad_region: u32,
}

/// Default tolerance used by [`check`]: a channel threshold of 8, at most
/// 0.05 % bad pixels (259 at 960x540) and no bad region larger than 24 pixels.
pub const DEFAULT_TOLERANCE: Tolerance = Tolerance {
    channel_threshold: 8,
    max_bad_fraction: 0.0005,
    max_bad_region: 24,
};

/// No difference at all.
pub const EXACT: Tolerance = Tolerance {
    channel_threshold: 0,
    max_bad_fraction: 0.0,
    max_bad_region: 0,
};

/// What [`compare`] measured.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DiffStats {
    pub bad_pixels: u64,
    pub total_pixels: u64,
    /// Size of the largest 8-connected region of bad pixels.
    pub largest_bad_region: u32,
}

impl DiffStats {
    pub fn bad_fraction(&self) -> f32 {
        self.bad_pixels as f32 / self.total_pixels.max(1) as f32
    }
}

/// Measures `actual` against `expected`. Errors if the sizes differ or the
/// difference exceeds `tolerance`.
pub fn compare(
    actual: &RgbaImage,
    expected: &RgbaImage,
    tolerance: &Tolerance,
) -> Result<DiffStats, String> {
    let stats = diff_stats(actual, expected, tolerance.channel_threshold)?;
    let fraction = stats.bad_fraction();
    if fraction > tolerance.max_bad_fraction {
        return Err(format!(
            "{} of {} pixels differ by more than {} ({:.3}% > {:.3}% allowed)",
            stats.bad_pixels,
            stats.total_pixels,
            tolerance.channel_threshold,
            fraction * 100.0,
            tolerance.max_bad_fraction * 100.0
        ));
    }
    if stats.largest_bad_region > tolerance.max_bad_region {
        return Err(format!(
            "a connected region of {} differing pixels (> {} allowed; {} bad in total)",
            stats.largest_bad_region, tolerance.max_bad_region, stats.bad_pixels
        ));
    }
    Ok(stats)
}

/// Counts bad pixels and finds the largest 8-connected bad region.
pub fn diff_stats(
    actual: &RgbaImage,
    expected: &RgbaImage,
    channel_threshold: u8,
) -> Result<DiffStats, String> {
    if actual.dimensions() != expected.dimensions() {
        return Err(format!(
            "size mismatch: actual {:?}, expected {:?}",
            actual.dimensions(),
            expected.dimensions()
        ));
    }
    let (w, h) = actual.dimensions();
    let mut bad: Vec<bool> = actual
        .pixels()
        .zip(expected.pixels())
        .map(|(a, e)| max_channel_diff(a, e) > channel_threshold)
        .collect();
    let bad_pixels = bad.iter().filter(|b| **b).count() as u64;
    // Flood fill, clearing visited pixels.
    let mut largest = 0u32;
    let mut stack = Vec::new();
    for start in 0..bad.len() {
        if !bad[start] {
            continue;
        }
        bad[start] = false;
        stack.push(start);
        let mut size = 0u32;
        while let Some(i) = stack.pop() {
            size += 1;
            let (x, y) = ((i as u32) % w, (i as u32) / w);
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    let j = (ny * w + nx) as usize;
                    if bad[j] {
                        bad[j] = false;
                        stack.push(j);
                    }
                }
            }
        }
        largest = largest.max(size);
    }
    Ok(DiffStats {
        bad_pixels,
        total_pixels: (w as u64 * h as u64).max(1),
        largest_bad_region: largest,
    })
}

fn max_channel_diff(a: &Rgba<u8>, e: &Rgba<u8>) -> u8 {
    a.0.iter()
        .zip(e.0.iter())
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

/// Compares against `<dir>/<name>.png` with [`DEFAULT_TOLERANCE`]. With
/// `BOX_UPDATE_GOLDEN=1` it writes the file instead and returns `Ok`. A
/// missing file is an `Err`, so a renamed test or a new platform cannot pass
/// without a reviewed reference. On mismatch writes `<name>.actual.png` and
/// `<name>.diff.png` next to it and returns `Err`.
pub fn check(dir: &Path, name: &str, actual: &RgbaImage) -> Result<(), String> {
    let update = std::env::var("BOX_UPDATE_GOLDEN").is_ok_and(|v| v == "1");
    check_with(dir, name, actual, update)
}

fn check_with(dir: &Path, name: &str, actual: &RgbaImage, update: bool) -> Result<(), String> {
    let expected_path = dir.join(format!("{name}.png"));
    if update {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        actual
            .save(&expected_path)
            .map_err(|e| format!("{}: {e}", expected_path.display()))?;
        return Ok(());
    }
    if !expected_path.exists() {
        return Err(format!(
            "golden '{name}' has no reference image at {}\n  \
             (set BOX_UPDATE_GOLDEN=1 to record it, then review the PNG)",
            expected_path.display()
        ));
    }
    let expected = image::open(&expected_path)
        .map_err(|e| format!("{}: {e}", expected_path.display()))?
        .to_rgba8();
    match compare(actual, &expected, &DEFAULT_TOLERANCE) {
        Ok(_) => Ok(()),
        Err(e) => {
            let actual_path = sibling(&expected_path, name, "actual");
            let diff_path = sibling(&expected_path, name, "diff");
            let _ = actual.save(&actual_path);
            if actual.dimensions() == expected.dimensions() {
                let _ = diff_image(actual, &expected, DEFAULT_TOLERANCE.channel_threshold)
                    .save(&diff_path);
            }
            Err(format!(
                "golden '{name}' mismatch: {e}\n  expected: {}\n  actual:   {}\n  diff:     {}\n  \
                 (set BOX_UPDATE_GOLDEN=1 to accept)",
                expected_path.display(),
                actual_path.display(),
                diff_path.display()
            ))
        }
    }
}

fn sibling(expected: &Path, name: &str, suffix: &str) -> PathBuf {
    expected.with_file_name(format!("{name}.{suffix}.png"))
}

/// Differing pixels in red over a dimmed copy of the expected image.
fn diff_image(actual: &RgbaImage, expected: &RgbaImage, threshold: u8) -> RgbaImage {
    RgbaImage::from_fn(actual.width(), actual.height(), |x, y| {
        let (a, e) = (actual.get_pixel(x, y), expected.get_pixel(x, y));
        if max_channel_diff(a, e) > threshold {
            Rgba([255, 0, 0, 255])
        } else {
            let g = ((e.0[0] as u16 + e.0[1] as u16 + e.0[2] as u16) / 9) as u8;
            Rgba([g, g, g, 255])
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba(c))
    }

    fn tol(channel_threshold: u8, max_bad_fraction: f32, max_bad_region: u32) -> Tolerance {
        Tolerance {
            channel_threshold,
            max_bad_fraction,
            max_bad_region,
        }
    }

    #[test]
    fn compare_counts_pixels_over_threshold() {
        let a = solid(10, 10, [100, 100, 100, 255]);
        let mut b = a.clone();
        assert_eq!(compare(&a, &b, &EXACT).unwrap().bad_pixels, 0);
        // Small differences under the threshold are ignored.
        b.put_pixel(0, 0, Rgba([104, 100, 100, 255]));
        assert!(compare(&a, &b, &tol(4, 0.0, 0)).is_ok());
        assert!(compare(&a, &b, &tol(3, 0.0, 100)).is_err());
        // Five bad pixels in a row = 5%, one region of 5.
        for x in 0..5 {
            b.put_pixel(x, 1, Rgba([0, 100, 100, 255]));
        }
        let stats = compare(&a, &b, &tol(4, 0.05, 5)).unwrap();
        assert_eq!(stats.bad_fraction(), 0.05);
        assert_eq!(stats.largest_bad_region, 5);
        let err = compare(&a, &b, &tol(4, 0.04, 100)).unwrap_err();
        assert!(err.contains("5 of 100"), "{err}");
    }

    #[test]
    fn compact_regions_fail_scattered_pixels_pass() {
        let a = solid(40, 40, [50, 50, 50, 255]);
        // 16 isolated pixels (a checker of every 10th pixel).
        let mut scattered = a.clone();
        for i in 0..16u32 {
            scattered.put_pixel((i % 4) * 10, (i / 4) * 10, Rgba([200, 50, 50, 255]));
        }
        let t = tol(8, 0.02, 4);
        let s = compare(&scattered, &a, &t).unwrap();
        assert_eq!((s.bad_pixels, s.largest_bad_region), (16, 1));
        // The same number of pixels as one 4x4 blob fails on region size.
        let mut blob = a.clone();
        for i in 0..16u32 {
            blob.put_pixel(20 + i % 4, 20 + i / 4, Rgba([200, 50, 50, 255]));
        }
        let err = compare(&blob, &a, &t).unwrap_err();
        assert!(err.contains("connected region of 16"), "{err}");
        // Diagonal neighbours are connected: a missing 1 px diagonal line is
        // one region.
        let mut diagonal = a.clone();
        for i in 0..6 {
            diagonal.put_pixel(i, i, Rgba([200, 50, 50, 255]));
            diagonal.put_pixel(39 - i, i, Rgba([200, 50, 50, 255]));
        }
        let err = compare(&diagonal, &a, &t).unwrap_err();
        assert!(err.contains("connected region of 6"), "{err}");
    }

    #[test]
    fn compare_rejects_size_mismatch() {
        let err = compare(&solid(2, 2, [0; 4]), &solid(3, 2, [0; 4]), &EXACT).unwrap_err();
        assert!(err.contains("size mismatch"));
    }

    #[test]
    fn check_rejects_a_missing_golden_and_detects_mismatch() {
        let dir = std::env::temp_dir().join(format!("box_golden_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let img = solid(8, 8, [10, 20, 30, 255]);
        let other = solid(8, 8, [200, 20, 30, 255]);

        // A missing reference fails and is not written.
        let err = check_with(&dir, "g", &img, false).unwrap_err();
        assert!(err.contains("BOX_UPDATE_GOLDEN=1"), "{err}");
        assert!(!dir.join("g.png").exists());

        // Recording writes it; after that it is compared.
        check_with(&dir, "g", &img, true).unwrap();
        assert!(dir.join("g.png").exists());
        check_with(&dir, "g", &img, false).unwrap();
        let err = check_with(&dir, "g", &other, false).unwrap_err();
        assert!(err.contains("mismatch"), "{err}");
        assert!(dir.join("g.actual.png").exists());
        assert!(dir.join("g.diff.png").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
