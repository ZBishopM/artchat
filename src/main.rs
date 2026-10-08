// Sin consola en el .exe de release: es una aplicación de ventana.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod canvas;
mod network;
mod settings;
mod theme;
mod types;

/// El icono de la ventana (el .exe lleva el suyo, incrustado por `build.rs`).
fn icono() -> Option<egui::IconData> {
    let mut r = png::Decoder::new(&include_bytes!("../assets/icon.png")[..]).read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    buf.truncate(info.buffer_size());
    Some(egui::IconData { rgba: buf, width: info.width, height: info.height })
}

fn main() -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([640.0, 420.0])
        .with_title("ArtChat");
    if let Some(i) = icono() {
        viewport = viewport.with_icon(i);
    }
    eframe::run_native(
        "ArtChat",
        eframe::NativeOptions { viewport, ..Default::default() },
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
