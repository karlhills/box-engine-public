//! Offscreen viewports: render targets of any size and explicit cameras, for
//! tools (the editor viewport) that draw the scene into a texture.

use box_core::transform::{GlobalTransform, Transform};

use crate::components::Camera;
use crate::renderer::Targets;

/// Storage format of a [`ViewportTarget`]'s colour texture. Its texels hold
/// sRGB-encoded values: the scene is tone mapped through an `Rgba8UnormSrgb`
/// view of the same texture, and [`ViewportTarget::view`] reads the encoded
/// bytes back unchanged. That is what egui-wgpu expects from a native
/// `Rgba8Unorm` texture.
pub const VIEWPORT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// A camera that is not an entity: a world transform (looking down its −Z)
/// plus projection parameters. `camera.active` is ignored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewCamera {
    pub transform: GlobalTransform,
    pub camera: Camera,
}

impl ViewCamera {
    /// A camera at a world-space `transform` (scale is ignored).
    pub fn new(transform: &Transform, camera: Camera) -> Self {
        Self {
            transform: GlobalTransform(transform.to_affine()),
            camera,
        }
    }
}

/// An offscreen render target with its own HDR, MSAA and depth buffers.
/// Create with [`Renderer::create_viewport_target`](crate::Renderer::create_viewport_target),
/// draw with [`Renderer::render_to`](crate::Renderer::render_to).
pub struct ViewportTarget {
    pub(crate) size: (u32, u32),
    pub(crate) targets: Targets,
    pub(crate) texture: wgpu::Texture,
    /// `Rgba8UnormSrgb` view; the tone-mapping pass writes through it.
    pub(crate) srgb_view: wgpu::TextureView,
    /// `Rgba8Unorm` view of the same texels, for sampling.
    pub(crate) view: wgpu::TextureView,
}

impl ViewportTarget {
    /// Size in physical pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The colour texture ([`VIEWPORT_FORMAT`]): `RENDER_ATTACHMENT`,
    /// `TEXTURE_BINDING` and `COPY_SRC`.
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// An `Rgba8Unorm` view holding sRGB-encoded colour. Register it with
    /// egui-wgpu as a native texture.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }
}
