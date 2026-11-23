// SPDX-License-Identifier: LGPL-2.1-or-later

//! Phosphor icons used in the UI.

pub use egui_phosphor::regular::{
    AIRPLANE_TILT as FLY, ARROW_U_UP_LEFT as UNDO, ARROW_U_UP_RIGHT as REDO, ARROWS_CLOCKWISE as REFRESH, ATOM, CAMERA, CARET_DOWN, CARET_RIGHT, CARET_UP,
    CLOUD_FOG as FOG, CUBE, DOWNLOAD_SIMPLE as EXPORT, GEAR, GLOBE_HEMISPHERE_WEST as ORBIT, INFO, LIGHTBULB as LIGHT, MAGNIFYING_GLASS, PALETTE,
    PLUS, TRASH, WARNING, X,
};

pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
}
