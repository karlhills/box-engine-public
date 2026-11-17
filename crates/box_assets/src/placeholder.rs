//! Placeholder values stored in the slot of an asset that failed to load,
//! so references never dangle and the failure is visible.

use box_core::color::Color;
use box_core::image::{ColorSpace, ImageData};
use box_core::material::StandardMaterial;
use box_core::mesh::MeshData;
use box_core::shader_material::ShaderSource;
use glam::Vec3;

/// A 32x32 magenta/black checker (8 px cells), sRGB, with mips.
pub fn placeholder_image() -> ImageData {
    const SIZE: u32 = 32;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let magenta = ((x / 8) + (y / 8)) % 2 == 0;
            rgba.extend(if magenta {
                [255, 0, 255, 255]
            } else {
                [0, 0, 0, 255]
            });
        }
    }
    let mut image = ImageData {
        width: SIZE,
        height: SIZE,
        color_space: ColorSpace::Srgb,
        mips: vec![rgba],
    };
    image.generate_mips();
    image
}

/// A 1 m cube centred on the origin.
pub fn placeholder_mesh() -> MeshData {
    MeshData::cuboid(Vec3::ONE)
}

/// An empty shader source. The renderer draws materials using it with its
/// magenta error material, without compiling it (the import already
/// reported why the file failed).
pub fn placeholder_shader() -> ShaderSource {
    ShaderSource::default()
}

/// The default material with a magenta base colour.
pub fn placeholder_material() -> StandardMaterial {
    StandardMaterial {
        base_color: Color::linear(1.0, 0.0, 1.0),
        ..Default::default()
    }
}
