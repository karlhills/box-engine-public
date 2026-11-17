//! CPU image data: [`ImageData`], the format textures are imported into and
//! the renderer uploads from.

use crate::pmath;
use thiserror::Error;

/// How the stored RGBA8 values are encoded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColorSpace {
    /// sRGB-encoded colour (base colour, emissive). Sampled as linear by an
    /// sRGB texture format.
    #[default]
    Srgb,
    /// Linear data (normal maps, occlusion/roughness/metallic).
    Linear,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ImageError {
    #[error("image has a zero dimension ({width}x{height})")]
    Empty { width: u32, height: u32 },
    #[error("image is {width}x{height}, above the {max} px limit")]
    TooLarge { width: u32, height: u32, max: u32 },
    #[error("mip level {level} has {got} bytes, expected {expected}")]
    BadLevel {
        level: usize,
        got: usize,
        expected: usize,
    },
}

/// An RGBA8 image with its mip chain (`mips[0]` is full size; each next level
/// halves both dimensions, rounding down, to a minimum of 1).
#[derive(Clone, Debug, PartialEq)]
pub struct ImageData {
    pub width: u32,
    pub height: u32,
    pub color_space: ColorSpace,
    /// Tightly packed RGBA8 rows per level.
    pub mips: Vec<Vec<u8>>,
}

impl ImageData {
    /// Largest accepted dimension (wgpu's default `max_texture_dimension_2d`).
    pub const MAX_DIMENSION: u32 = 8192;

    /// A single-level image from tightly packed RGBA8 pixels.
    pub fn new(
        width: u32,
        height: u32,
        color_space: ColorSpace,
        rgba: Vec<u8>,
    ) -> Result<Self, ImageError> {
        let image = Self {
            width,
            height,
            color_space,
            mips: vec![rgba],
        };
        image.validate()?;
        Ok(image)
    }

    /// A 1x1 image of one RGBA8 colour (placeholders, default textures).
    pub fn solid(rgba: [u8; 4], color_space: ColorSpace) -> Self {
        Self {
            width: 1,
            height: 1,
            color_space,
            mips: vec![rgba.to_vec()],
        }
    }

    /// Size of mip `level`.
    pub fn level_size(&self, level: usize) -> (u32, u32) {
        ((self.width >> level).max(1), (self.height >> level).max(1))
    }

    /// Number of levels in a full chain for this size.
    pub fn full_mip_count(&self) -> usize {
        32 - self.width.max(self.height).max(1).leading_zeros() as usize
    }

    /// Checks dimensions and the byte length of every level.
    pub fn validate(&self) -> Result<(), ImageError> {
        let (width, height) = (self.width, self.height);
        if width == 0 || height == 0 {
            return Err(ImageError::Empty { width, height });
        }
        if width > Self::MAX_DIMENSION || height > Self::MAX_DIMENSION {
            return Err(ImageError::TooLarge {
                width,
                height,
                max: Self::MAX_DIMENSION,
            });
        }
        for (level, data) in self.mips.iter().enumerate() {
            let (w, h) = self.level_size(level);
            let expected = w as usize * h as usize * 4;
            if data.len() != expected {
                return Err(ImageError::BadLevel {
                    level,
                    got: data.len(),
                    expected,
                });
            }
        }
        if self.mips.is_empty() {
            return Err(ImageError::BadLevel {
                level: 0,
                got: 0,
                expected: width as usize * height as usize * 4,
            });
        }
        Ok(())
    }

    /// Replaces levels 1.. with a full chain made by 2x2 box filtering level 0
    /// (in linear light for sRGB images; alpha is averaged linearly).
    pub fn generate_mips(&mut self) {
        self.mips.truncate(1);
        let levels = self.full_mip_count();
        let srgb = self.color_space == ColorSpace::Srgb;
        let to_lin = srgb_to_linear_table();
        for level in 1..levels {
            let (pw, ph) = self.level_size(level - 1);
            let (w, h) = self.level_size(level);
            let prev = &self.mips[level - 1];
            let mut out = Vec::with_capacity(w as usize * h as usize * 4);
            for y in 0..h {
                for x in 0..w {
                    // Odd sizes: the last row/column folds into its neighbour.
                    let xs = [(2 * x).min(pw - 1), (2 * x + 1).min(pw - 1)];
                    let ys = [(2 * y).min(ph - 1), (2 * y + 1).min(ph - 1)];
                    let mut acc = [0.0f32; 4];
                    for sy in ys {
                        for sx in xs {
                            let i = ((sy * pw + sx) * 4) as usize;
                            for c in 0..4 {
                                let v = prev[i + c];
                                acc[c] += if srgb && c < 3 {
                                    to_lin[v as usize]
                                } else {
                                    v as f32 / 255.0
                                };
                            }
                        }
                    }
                    for (c, a) in acc.iter().enumerate() {
                        let v = a / 4.0;
                        let v = if srgb && c < 3 { linear_to_srgb(v) } else { v };
                        out.push((v * 255.0 + 0.5).clamp(0.0, 255.0) as u8);
                    }
                }
            }
            self.mips.push(out);
        }
    }

    /// Bytes held by all levels.
    pub fn byte_len(&self) -> usize {
        self.mips.iter().map(Vec::len).sum()
    }
}

fn srgb_to_linear_table() -> [f32; 256] {
    let mut t = [0.0; 256];
    for (i, v) in t.iter_mut().enumerate() {
        let c = i as f32 / 255.0;
        *v = if c <= 0.04045 {
            c / 12.92
        } else {
            pmath::powf((c + 0.055) / 1.055, 2.4)
        };
    }
    t
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * pmath::powf(c, 1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_sizes() {
        assert!(ImageData::new(2, 2, ColorSpace::Srgb, vec![0; 16]).is_ok());
        assert_eq!(
            ImageData::new(0, 2, ColorSpace::Srgb, vec![]),
            Err(ImageError::Empty {
                width: 0,
                height: 2
            })
        );
        assert!(matches!(
            ImageData::new(2, 2, ColorSpace::Srgb, vec![0; 15]),
            Err(ImageError::BadLevel { level: 0, .. })
        ));
        assert!(matches!(
            ImageData::new(9000, 1, ColorSpace::Srgb, vec![0; 36000]),
            Err(ImageError::TooLarge { .. })
        ));
    }

    #[test]
    fn mip_chain_has_the_right_sizes_and_averages() {
        // 5x3 linear checker of 0 and 255: every level averages near 127.
        let (w, h) = (5u32, 3u32);
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let v = if (x + y) % 2 == 0 { 255 } else { 0 };
                px.extend([v, v, v, 255]);
            }
        }
        let mut img = ImageData::new(w, h, ColorSpace::Linear, px).unwrap();
        img.generate_mips();
        assert_eq!(img.mips.len(), 3); // 5x3, 2x1, 1x1
        assert_eq!(img.level_size(1), (2, 1));
        assert_eq!(img.level_size(2), (1, 1));
        img.validate().unwrap();
        let last = &img.mips[2];
        assert!((100..=160).contains(&last[0]), "{last:?}");
        assert_eq!(last[3], 255);
    }

    #[test]
    fn srgb_mips_average_in_linear_light() {
        // Black and white average to linear 0.5, which is sRGB ~188, not 128.
        let px = [0, 0, 0, 255, 255, 255, 255, 255].to_vec();
        let mut img = ImageData::new(2, 1, ColorSpace::Srgb, px).unwrap();
        img.generate_mips();
        let v = img.mips[1][0];
        assert!((185..=190).contains(&v), "{v}");
        let mut lin = img.clone();
        lin.color_space = ColorSpace::Linear;
        lin.generate_mips();
        assert!((127..=128).contains(&lin.mips[1][0]));
    }
}
