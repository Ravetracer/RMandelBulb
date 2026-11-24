// SPDX-License-Identifier: LGPL-2.1-or-later

//! GPU renderer (wgpu compute): per-hybrid generated WGSL, progressive accumulation.

pub mod codegen;
pub mod renderer;

pub use renderer::{FrameRenderer, GpuContext, GpuRenderer, GridField, read_texture_rgba8, render_animation, render_blocking, render_blocking_hdr, render_progressive, render_tiled_hdr};
