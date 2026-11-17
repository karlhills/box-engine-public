//! The `texture` importer: PNG and JPEG files → [`ImageData`].

use std::io::Cursor;

use box_core::image::{ColorSpace, ImageData};

use crate::error::ImportError;
use crate::importer::{ImportContext, Importer};
use crate::meta::TextureSettings;

/// PNG and JPEG → one RGBA8 [`ImageData`] (the file's main asset), in the
/// colour space from the `.meta` settings, with mips when enabled.
pub struct TextureImporter;

impl Importer for TextureImporter {
    fn name(&self) -> &str {
        "texture"
    }

    fn extensions(&self) -> &[&str] {
        &["png", "jpg", "jpeg"]
    }

    fn default_settings(&self) -> serde_json::Value {
        serde_json::to_value(TextureSettings::default()).unwrap_or_default()
    }

    fn import(&self, ctx: &mut ImportContext) -> Result<(), ImportError> {
        let settings: TextureSettings = ctx.settings()?;
        let image = decode_image(ctx.bytes(), settings.color_space.into(), settings.mipmaps)?;
        ctx.add(None, image);
        Ok(())
    }
}

/// Decodes PNG or JPEG bytes to RGBA8 pixels, refusing images larger than
/// [`ImageData::MAX_DIMENSION`].
pub(crate) fn decode_rgba(bytes: &[u8]) -> Result<image::RgbaImage, ImportError> {
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| ImportError::Io(e.to_string()))?;
    match reader.format() {
        Some(image::ImageFormat::Png | image::ImageFormat::Jpeg) => {}
        Some(other) => {
            return Err(ImportError::Unsupported(format!(
                "{other:?} images (use PNG or JPEG)"
            )));
        }
        None => {
            return Err(ImportError::Decode(
                "not a PNG or JPEG image (unrecognized header)".into(),
            ));
        }
    }
    let mut reader = reader;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(ImageData::MAX_DIMENSION);
    limits.max_image_height = Some(ImageData::MAX_DIMENSION);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|e| ImportError::Decode(format!("could not decode image: {e}")))?;
    Ok(decoded.into_rgba8())
}

/// [`decode_rgba`] into an [`ImageData`].
pub(crate) fn decode_image(
    bytes: &[u8],
    color_space: ColorSpace,
    mipmaps: bool,
) -> Result<ImageData, ImportError> {
    to_image_data(decode_rgba(bytes)?, color_space, mipmaps)
}

pub(crate) fn to_image_data(
    rgba: image::RgbaImage,
    color_space: ColorSpace,
    mipmaps: bool,
) -> Result<ImageData, ImportError> {
    let (w, h) = rgba.dimensions();
    let mut image = ImageData::new(w, h, color_space, rgba.into_raw())
        .map_err(|e| ImportError::Decode(e.to_string()))?;
    if mipmaps {
        image.generate_mips();
    }
    Ok(image)
}
