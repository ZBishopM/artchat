//! La identidad visual de windots (`crates/rice-common/src/theme.rs` y `ui.rs`): la paleta
//! cálida, JetBrains Mono Nerd Font y los radios de la barra y del lanzador.
//!
//! Los valores son los del repositorio, byte por byte, y se mantienen como `[u8; 3]` igual
//! que allí para que una herramienta que no sea egui pueda leerlos. La fuente va
//! incrustada: el original la busca en `C:\Windows\Fonts` y se queda sin iconos si no está,
//! algo que en un `.exe` portable que se ejecuta en otra máquina pasaría siempre.

use egui::{Color32, Context, FontData, FontDefinitions, FontFamily, FontId, Rounding, Shadow, Stroke, TextStyle, Vec2};

/// Fondo de la barra / la superficie más profunda.
pub const BAR_BG: [u8; 3] = [26, 22, 19];
/// Superficie elevada: la isla de la barra, el cuerpo del aviso.
pub const SURFACE: [u8; 3] = [40, 33, 28];
/// Borde superior de una superficie elevada.
pub const HIGHLIGHT: [u8; 3] = [60, 50, 43];
pub const TEXT: [u8; 3] = [233, 224, 214];
pub const SUBTEXT: [u8; 3] = [170, 154, 140];
/// Ámbar `#e0a35c`: el acento de la casa.
pub const ACCENT: [u8; 3] = [224, 163, 92];
/// Lima `#a9b56a`: guardado, éxito, en línea.
pub const ACCENT_OK: [u8; 3] = [169, 181, 106];
/// Terracota `#d08770`: avisos y fallos, ocupado.
pub const ACCENT_WARN: [u8; 3] = [208, 135, 112];
/// Degradado de atardecer de los espectros de cava (de abajo arriba): para los colores
/// rápidos del pincel.
pub const ATARDECER: [[u8; 3]; 4] = [[224, 163, 92], [216, 122, 84], [169, 181, 106], [233, 224, 214]];

/// Un color de la paleta como color de egui.
pub const fn col(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// Un color de la paleta con transparencia (0 a 1).
pub fn col_a(c: [u8; 3], a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], (a.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Radio de las islas flotantes (el lanzador usa 14).
pub const RADIO_ISLA: f32 = 14.0;
/// Radio de filas y botones (el lanzador usa 8).
pub const RADIO_FILA: f32 = 8.0;

/// Glifos de Nerd Font (los de la tabla de `rice-common::ui::icon_glyph` y vecinos).
pub mod icono {
    pub const BORRADOR: &str = "\u{f12d}";
    pub const PAPELERA: &str = "\u{f1f8}";
    pub const SONIDO: &str = "\u{f028}";
    pub const SILENCIO: &str = "\u{f026}";
    pub const CAMPANA: &str = "\u{f0f3}";
    pub const CHAT: &str = "\u{f075}";
    pub const ENVIAR: &str = "\u{f1d8}";
    pub const VARITA: &str = "\u{f0d0}";
    pub const MENOS: &str = "\u{f068}";
}

pub fn apply(ctx: &Context) {
    instala_fuente(ctx);

    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(col(TEXT));
    v.panel_fill = col(BAR_BG);
    v.window_fill = col(SURFACE);
    v.faint_bg_color = col(BAR_BG);
    v.extreme_bg_color = col(BAR_BG); // fondo de los campos de texto
    v.hyperlink_color = col(ACCENT);
    v.window_rounding = Rounding::same(RADIO_ISLA);
    v.window_stroke = Stroke::new(1.0_f32, col(HIGHLIGHT));
    v.menu_rounding = Rounding::same(RADIO_FILA + 2.0);
    v.popup_shadow = Shadow { offset: Vec2::new(0.0, 6.0), blur: 20.0, spread: 0.0, color: Color32::from_black_alpha(140) };
    v.window_shadow = v.popup_shadow;
    v.selection.bg_fill = col_a(ACCENT, 0.28);
    v.selection.stroke = Stroke::new(1.0_f32, col(ACCENT));
    v.slider_trailing_fill = true;

    let fila = Rounding::same(RADIO_FILA);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = col(SURFACE);
    w.noninteractive.weak_bg_fill = col(SURFACE);
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, col(HIGHLIGHT));
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, col(SUBTEXT));
    w.noninteractive.rounding = fila;

    // `weak_bg_fill` es el fondo de los botones; `bg_fill`, el de las guías de los deslizadores
    // y las casillas. Hay que fijar los dos o los botones salen con el gris de egui.
    w.inactive.bg_fill = col(BAR_BG);
    w.inactive.weak_bg_fill = Color32::from_rgb(34, 28, 24);
    w.inactive.bg_stroke = Stroke::new(1.0_f32, col(HIGHLIGHT));
    w.inactive.fg_stroke = Stroke::new(1.0_f32, col(TEXT));
    w.inactive.rounding = fila;

    w.hovered.bg_fill = Color32::from_rgb(48, 40, 34);
    w.hovered.weak_bg_fill = Color32::from_rgb(48, 40, 34);
    w.hovered.bg_stroke = Stroke::new(1.0_f32, col(ACCENT));
    w.hovered.fg_stroke = Stroke::new(1.5_f32, col(TEXT));
    w.hovered.rounding = fila;

    w.active.bg_fill = col(ACCENT);
    w.active.weak_bg_fill = col(ACCENT);
    w.active.bg_stroke = Stroke::new(1.0_f32, col(ACCENT));
    w.active.fg_stroke = Stroke::new(1.5_f32, col(BAR_BG));
    w.active.rounding = fila;

    w.open.bg_fill = col(SURFACE);
    w.open.weak_bg_fill = col(SURFACE);
    w.open.bg_stroke = Stroke::new(1.0_f32, col(ACCENT));
    w.open.rounding = fila;
    ctx.set_visuals(v);

    ctx.style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        s.spacing.button_padding = Vec2::new(10.0, 5.0);
        s.spacing.interact_size.y = 28.0;
        s.spacing.slider_width = 110.0;
        s.text_styles = [
            (TextStyle::Heading, FontId::new(17.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(11.5, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
        ]
        .into();
    });
}

/// JetBrains Mono Nerd Font como familia por defecto, con las de egui detrás por si falta algo.
fn instala_fuente(ctx: &Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "jbm".to_owned(),
        FontData::from_static(include_bytes!("../assets/JetBrainsMonoNerdFont-Regular.ttf")),
    );
    for fam in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts.families.entry(fam).or_default().insert(0, "jbm".to_owned());
    }
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_paleta_es_la_de_windots() {
        // Los mismos hex que documenta rice-common/theme.rs.
        assert_eq!(crate::types::parse_hex("#e0a35c"), Some(ACCENT));
        assert_eq!(crate::types::parse_hex("#a9b56a"), Some(ACCENT_OK));
        assert_eq!(BAR_BG, [26, 22, 19]);
        assert_eq!(SURFACE, [40, 33, 28]);
        assert_eq!(HIGHLIGHT, [60, 50, 43]);
        assert_eq!(TEXT, [233, 224, 214]);
        assert_eq!(SUBTEXT, [170, 154, 140]);
        assert_eq!(ACCENT_WARN, [208, 135, 112]);
    }
}
