//! GPU copies of `ImageData` assets, default textures and the material
//! sampler. Uploads are change-driven by asset version.

use std::collections::HashMap;

use box_core::assets::Assets;
use box_core::image::{ColorSpace, ImageData};

use crate::gpu_mesh::UploadStats;

/// What a material texture slot falls back to while its image is missing,
/// still loading or invalid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DefaultTexture {
    /// sRGB white: base colour and emissive factors pass through.
    White,
    /// Flat tangent-space normal (0.5, 0.5, 1).
    Normal,
    /// Linear white: occlusion 1, and roughness and metallic factors pass
    /// through.
    Orm,
}

struct GpuTexture {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    version: u64,
    /// Size, format and mip count, to rewrite in place when unchanged.
    shape: (u32, u32, wgpu::TextureFormat, u32),
}

pub(crate) struct TextureCache {
    textures: HashMap<u64, GpuTexture>,
    /// Version of each asset that failed validation, so it is reported and
    /// skipped once per version.
    rejected: HashMap<u64, u64>,
    /// Bumped whenever a texture view is created or freed, so materials
    /// re-resolve their textures only then.
    generation: u64,
    white: (wgpu::Texture, wgpu::TextureView),
    normal: (wgpu::Texture, wgpu::TextureView),
    orm: (wgpu::Texture, wgpu::TextureView),
    /// Linear filtering with mipmaps, repeat addressing, 8x anisotropy.
    pub sampler: wgpu::Sampler,
    /// Calls to `upload` (tests).
    #[cfg(test)]
    pub(crate) uploads: u32,
}

fn format_for(space: ColorSpace) -> wgpu::TextureFormat {
    match space {
        ColorSpace::Srgb => wgpu::TextureFormat::Rgba8UnormSrgb,
        ColorSpace::Linear => wgpu::TextureFormat::Rgba8Unorm,
    }
}

impl TextureCache {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let solid = |label: &str, rgba: [u8; 4], space: ColorSpace| {
            let image = ImageData::solid(rgba, space);
            let texture = create_texture(device, label, &image);
            write_levels(queue, &texture, &image);
            let view = texture.create_view(&Default::default());
            (texture, view)
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("material sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        Self {
            textures: HashMap::new(),
            rejected: HashMap::new(),
            generation: 0,
            white: solid("default white texture", [255; 4], ColorSpace::Srgb),
            normal: solid(
                "default normal texture",
                [128, 128, 255, 255],
                ColorSpace::Linear,
            ),
            orm: solid("default orm texture", [255; 4], ColorSpace::Linear),
            sampler,
            #[cfg(test)]
            uploads: 0,
        }
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// The view of uploaded image `id`, if it is uploaded.
    pub(crate) fn view(&self, id: u64) -> Option<&wgpu::TextureView> {
        self.textures.get(&id).map(|t| &t.view)
    }

    pub(crate) fn default_view(&self, which: DefaultTexture) -> &wgpu::TextureView {
        match which {
            DefaultTexture::White => &self.white.1,
            DefaultTexture::Normal => &self.normal.1,
            DefaultTexture::Orm => &self.orm.1,
        }
    }

    /// Uploads new or changed images and frees the ones no longer in
    /// `assets`. Unchanged images cost one map lookup each.
    #[profiling::function]
    pub(crate) fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: Option<&Assets<ImageData>>,
        stats: &mut UploadStats,
    ) {
        let Some(assets) = assets else {
            if !self.textures.is_empty() {
                self.textures.clear();
                self.generation += 1;
            }
            self.rejected.clear();
            return;
        };
        for (id, version, image) in assets.iter() {
            if self.textures.get(&id).is_some_and(|t| t.version == version)
                || self.rejected.get(&id) == Some(&version)
            {
                continue;
            }
            self.upload(device, queue, id, version, image, stats);
        }
        // Every asset is now uploaded or rejected, so extra entries belong to
        // removed images (or another world's store).
        if self.textures.len() + self.rejected.len() > assets.len() {
            let before = self.textures.len();
            self.textures
                .retain(|&id, _| assets.get_by_id(id).is_some());
            self.rejected
                .retain(|&id, _| assets.get_by_id(id).is_some());
            if self.textures.len() != before {
                self.generation += 1;
            }
        }
    }

    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        id: u64,
        version: u64,
        image: &ImageData,
        stats: &mut UploadStats,
    ) {
        #[cfg(test)]
        {
            self.uploads += 1;
        }
        if let Err(e) = image.validate() {
            tracing::warn!("image {id} is invalid and will not be used: {e}");
            if self.textures.remove(&id).is_some() {
                self.generation += 1;
            }
            self.rejected.insert(id, version);
            return;
        }
        self.rejected.remove(&id);
        let shape = (
            image.width,
            image.height,
            format_for(image.color_space),
            image.mips.len() as u32,
        );
        match self.textures.get_mut(&id) {
            // Same size, format and levels: rewrite in place, views stay valid.
            Some(t) if t.shape == shape => {
                write_levels(queue, &t._texture, image);
                t.version = version;
            }
            _ => {
                let texture = create_texture(device, &format!("image {id}"), image);
                write_levels(queue, &texture, image);
                let view = texture.create_view(&Default::default());
                self.textures.insert(
                    id,
                    GpuTexture {
                        _texture: texture,
                        view,
                        version,
                        shape,
                    },
                );
                self.generation += 1;
            }
        }
        stats.bytes += image.byte_len() as u64;
    }

    /// Uploaded images.
    pub(crate) fn len(&self) -> usize {
        self.textures.len()
    }
}

pub(crate) fn create_texture(
    device: &wgpu::Device,
    label: &str,
    image: &ImageData,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: image.width,
            height: image.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: image.mips.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: format_for(image.color_space),
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

pub(crate) fn write_levels(queue: &wgpu::Queue, texture: &wgpu::Texture, image: &ImageData) {
    for (level, data) in image.mips.iter().enumerate() {
        let (w, h) = image.level_size(level);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }
}
