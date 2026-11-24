// SPDX-License-Identifier: LGPL-2.1-or-later

//! RMandelbulb3D core: scene model, formula language and library, hybrid evaluation, CPU
//! renderer and file formats.

pub mod animation;
pub mod compose;
pub mod explore;
pub mod flare;
pub mod formula;
pub mod fractal;
pub mod gradient;
pub mod io;
pub mod maps;
pub mod mesh;
pub mod mesh_export;
pub mod palette;
pub mod math;
pub mod render;
pub mod scene;
pub mod texture;
pub mod video;

pub use fractal::Program;
pub use math::{Quat, V3, v3};
pub use scene::Scene;
