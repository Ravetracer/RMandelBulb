// SPDX-License-Identifier: LGPL-2.1-or-later

//! RMandelbulb3D: 3D fractal explorer and renderer.

// Release builds on Windows start without a console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod doc;
mod export;
mod formulas_panel;
mod gizmos;
mod help;
mod icons;
mod inspector;
mod layers;
mod prefs;
mod render_view;
mod theme;
mod timeline;
mod viewport;
mod widgets;

use std::sync::Arc;

fn main() -> eframe::Result {
    let initial = std::env::args().nth(1).map(std::path::PathBuf::from);

    let mut wgpu_options = egui_wgpu::WgpuConfiguration::default();
    if let egui_wgpu::WgpuSetup::CreateNew(setup) = &mut wgpu_options.wgpu_setup {
        setup.device_descriptor = Arc::new(|adapter: &wgpu::Adapter| wgpu::DeviceDescriptor {
            label: Some("rmandelbulb3d device"),
            required_limits: rmb_gpu::GpuContext::required_limits(adapter),
            ..Default::default()
        });
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RMandelbulb3D")
            .with_inner_size([1480.0, 900.0])
            .with_min_inner_size([960.0, 600.0])
            .with_drag_and_drop(true)
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).unwrap_or_default()),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options,
        ..Default::default()
    };
    eframe::run_native("RMandelbulb3D", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, initial)))))
}
