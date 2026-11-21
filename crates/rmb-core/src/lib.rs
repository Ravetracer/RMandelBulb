// SPDX-License-Identifier: LGPL-2.1-or-later

//! RMandelbulb3D core: scene model, formula language and library, hybrid evaluation, CPU
//! renderer and file formats.

pub mod formula;
pub mod fractal;
pub mod gradient;
pub mod io;
pub mod math;
pub mod render;
pub mod scene;

pub use fractal::Program;
pub use math::{V3, v3};
pub use scene::Scene;
