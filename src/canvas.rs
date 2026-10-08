//! El lienzo: una capa de píxeles en la CPU que se sube a la GPU solo por donde cambia.
//!
//! La primera versión guardaba cada trazo como un vector de formas y los volvía a pintar
//! enteros en CADA fotograma: con unos miles de segmentos el dibujo cuesta más que la
//! interfaz, y el borrador pintaba el color de fondo encima en vez de borrar. Aquí:
//!  - cada segmento se rasteriza UNA vez (cápsula con borde suavizado, igual que el
//!    `lineCap = "round"` del canvas web);
//!  - borrar es de verdad (`destination-out`): el píxel vuelve a ser transparente y se ve el
//!    color de fondo, así que cambiar el fondo no destruye el dibujo;
//!  - la textura se actualiza con la caja que tocó el segmento, no entera;
//!  - el lienzo crece con la ventana y no encoge, como el `bufferCanvas` del cliente Tauri.

use std::time::{Duration, Instant};

use egui::{Color32, ColorImage, Context, Painter, Pos2, Rect, TextureHandle, TextureOptions, Vec2};

/// Lado máximo del lienzo en puntos lógicos. Un trazo fuera de esto se recorta.
pub const MAX_PTS: f32 = 4096.0;
/// Se crece de 128 en 128 texels: así arrastrar el borde de la ventana no realoca por píxel.
const PASO: usize = 128;

pub struct Canvas {
    /// Texels por punto lógico (2 en pantallas de alta densidad, para que el trazo no salga
    /// borroso).
    scale: f32,
    w: usize,
    h: usize,
    px: Vec<Color32>,
    tex: Option<TextureHandle>,
    /// Caja sucia pendiente de subir: x0, y0, x1, y1 (el final no incluido).
    sucia: Option<[usize; 4]>,
    /// Hay cambios que no están en disco.
    pub por_guardar: bool,
}

impl Canvas {
    pub fn new(scale: f32) -> Self {
        Self { scale, w: 0, h: 0, px: Vec::new(), tex: None, sucia: None, por_guardar: false }
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Tamaño en puntos lógicos.
    pub fn size_pts(&self) -> Vec2 {
        Vec2::new(self.w as f32 / self.scale, self.h as f32 / self.scale)
    }

    /// Que quepa (w, h) puntos. Solo crece.
    pub fn ensure(&mut self, w_pts: f32, h_pts: f32) {
        let tope = (MAX_PTS * self.scale) as usize;
        let redondea = |pts: f32| (((pts.max(1.0) * self.scale).ceil() as usize).div_ceil(PASO) * PASO).min(tope);
        let (nw, nh) = (redondea(w_pts).max(self.w), redondea(h_pts).max(self.h));
        if (nw, nh) == (self.w, self.h) {
            return;
        }
        let mut px = vec![Color32::TRANSPARENT; nw * nh];
        for y in 0..self.h {
            px[y * nw..y * nw + self.w].copy_from_slice(&self.px[y * self.w..(y + 1) * self.w]);
        }
        self.px = px;
        self.w = nw;
        self.h = nh;
        self.tex = None; // textura nueva, entera, en el próximo `flush`
        self.sucia = None;
    }

    /// Que quepa el punto (más un margen), para trazos que vienen de ventanas más grandes.
    pub fn ensure_point(&mut self, p: Pos2) {
        let s = self.size_pts();
        if p.x + 1.0 > s.x || p.y + 1.0 > s.y {
            self.ensure(p.x + 100.0, p.y + 100.0);
        }
    }

    pub fn clear(&mut self) {
        self.px.fill(Color32::TRANSPARENT);
        self.sucia = if self.w > 0 { Some([0, 0, self.w, self.h]) } else { None };
        self.por_guardar = true;
    }

    /// Un segmento redondeado de `size` puntos de grosor. `erase` = deja transparente.
    pub fn segment(&mut self, a: Pos2, b: Pos2, size: f32, color: Color32, erase: bool) {
        if self.w == 0 {
            return;
        }
        let s = self.scale;
        let r = (size * 0.5 * s).max(0.5);
        let (ax, ay, bx, by) = (a.x * s, a.y * s, b.x * s, b.y * s);
        let lim = |v: f32, max: usize| (v.floor().max(0.0) as usize).min(max);
        let x0 = lim(ax.min(bx) - r - 1.0, self.w);
        let y0 = lim(ay.min(by) - r - 1.0, self.h);
        let x1 = lim(ax.max(bx) + r + 2.0, self.w);
        let y1 = lim(ay.max(by) + r + 2.0, self.h);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let (dx, dy) = (bx - ax, by - ay);
        let l2 = dx * dx + dy * dy;
        let src = [color.r() as f32, color.g() as f32, color.b() as f32];
        for y in y0..y1 {
            let py = y as f32 + 0.5;
            for x in x0..x1 {
                let pxx = x as f32 + 0.5;
                let t = if l2 < 1e-6 { 0.0 } else { (((pxx - ax) * dx + (py - ay) * dy) / l2).clamp(0.0, 1.0) };
                let (cx, cy) = (ax + t * dx - pxx, ay + t * dy - py);
                let cob = (r + 0.5 - (cx * cx + cy * cy).sqrt()).clamp(0.0, 1.0);
                if cob <= 0.0 {
                    continue;
                }
                let d = &mut self.px[y * self.w + x];
                let (dr, dg, db, da) = (d.r() as f32, d.g() as f32, d.b() as f32, d.a() as f32);
                *d = if erase {
                    let k = 1.0 - cob;
                    Color32::from_rgba_premultiplied((dr * k) as u8, (dg * k) as u8, (db * k) as u8, (da * k) as u8)
                } else {
                    // fuente opaca sobre lo que hubiera: out = src*cob + dst*(1-cob), ya premultiplicado
                    let k = 1.0 - cob;
                    Color32::from_rgba_premultiplied(
                        (src[0] * cob + dr * k).round() as u8,
                        (src[1] * cob + dg * k).round() as u8,
                        (src[2] * cob + db * k).round() as u8,
                        (255.0 * cob + da * k).round() as u8,
                    )
                };
            }
        }
        self.sucia = Some(match self.sucia {
            None => [x0, y0, x1, y1],
            Some([a, b, c, d]) => [a.min(x0), b.min(y0), c.max(x1), d.max(y1)],
        });
        self.por_guardar = true;
    }

    /// Sube a la GPU lo que cambió. Una vez por fotograma, no por segmento.
    pub fn flush(&mut self, ctx: &Context) {
        if self.w == 0 {
            return;
        }
        match (&mut self.tex, self.sucia.take()) {
            (None, _) => {
                let img = ColorImage { size: [self.w, self.h], pixels: self.px.clone() };
                self.tex = Some(ctx.load_texture("lienzo", img, TextureOptions::LINEAR));
            }
            (Some(t), Some([x0, y0, x1, y1])) => {
                let mut pixels = Vec::with_capacity((x1 - x0) * (y1 - y0));
                for y in y0..y1 {
                    pixels.extend_from_slice(&self.px[y * self.w + x0..y * self.w + x1]);
                }
                t.set_partial([x0, y0], ColorImage { size: [x1 - x0, y1 - y0], pixels }, TextureOptions::LINEAR);
            }
            (Some(_), None) => {}
        }
    }

    pub fn textura(&self) -> Option<egui::TextureId> {
        self.tex.as_ref().map(|t| t.id())
    }

    /// Pinta el lienzo con su esquina superior izquierda en `origen`.
    pub fn paint(&self, painter: &Painter, origen: Pos2) {
        if let Some(t) = &self.tex {
            let r = Rect::from_min_size(origen, self.size_pts());
            painter.image(t.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
    }

    // ---- disco ---------------------------------------------------------------------------

    /// Copia de los píxeles para codificarlos en otro hilo: comprimir un PNG de 1080p cuesta
    /// decenas de milisegundos, y eso en el hilo de la interfaz es un tirón.
    pub fn foto(&self) -> Foto {
        Foto { w: self.w, h: self.h, px: self.px.clone() }
    }

    pub fn from_png(&mut self, bytes: &[u8]) -> Option<()> {
        let mut r = png::Decoder::new(bytes).read_info().ok()?;
        let mut buf = vec![0; r.output_buffer_size()];
        let info = r.next_frame(&mut buf).ok()?;
        if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
            return None;
        }
        let (w, h) = (info.width as usize, info.height as usize);
        if w == 0 || h == 0 || w > (MAX_PTS * self.scale) as usize || h > (MAX_PTS * self.scale) as usize {
            return None;
        }
        self.w = w;
        self.h = h;
        // Premultiplicar igual que `Foto::to_png` divide (en bruto, sin pasar por el espacio
        // lineal que usa `from_rgba_unmultiplied`): el lienzo mezcla en bruto y los bordes
        // suavizados saldrían de otro color.
        let pre = |v: u8, a: u8| ((v as u32 * a as u32 + 127) / 255) as u8;
        self.px = buf[..w * h * 4]
            .chunks_exact(4)
            .map(|p| Color32::from_rgba_premultiplied(pre(p[0], p[3]), pre(p[1], p[3]), pre(p[2], p[3]), p[3]))
            .collect();
        self.tex = None;
        self.sucia = None;
        self.por_guardar = false;
        Some(())
    }

    #[cfg(test)]
    pub fn pixel(&self, x: usize, y: usize) -> Color32 {
        self.px[y * self.w + x]
    }
}

pub struct Foto {
    w: usize,
    h: usize,
    px: Vec<Color32>,
}

impl Foto {
    /// PNG sin premultiplicar, que es lo que espera el formato.
    pub fn to_png(&self) -> Option<Vec<u8>> {
        if self.w == 0 {
            return None;
        }
        let mut rgba = Vec::with_capacity(self.px.len() * 4);
        for c in &self.px {
            let a = c.a();
            let un = |v: u8| if a == 0 { 0 } else { ((v as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 };
            rgba.extend_from_slice(&[un(c.r()), un(c.g()), un(c.b()), a]);
        }
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, self.w as u32, self.h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        enc.write_header().ok()?.write_image_data(&rgba).ok()?;
        Some(out)
    }
}

// ---- trazos efímeros ---------------------------------------------------------------------

/// Un segmento que se desvanece. Son pocos y viven poco, así que van como formas normales.
pub struct Efimero {
    pub a: Pos2,
    pub b: Pos2,
    pub size: f32,
    pub color: Color32,
    pub nacio: Instant,
    pub dura: Duration,
}

impl Efimero {
    /// 1.0 recién hecho, 0.0 desvanecido.
    pub fn vida(&self, ahora: Instant) -> f32 {
        1.0 - (ahora.saturating_duration_since(self.nacio).as_secs_f32() / self.dura.as_secs_f32().max(0.001))
    }

    pub fn paint(&self, painter: &Painter, origen: Pos2, ahora: Instant) {
        let v = self.vida(ahora);
        if v <= 0.0 {
            return;
        }
        let c = self.color.gamma_multiply(v);
        let (a, b) = (origen + self.a.to_vec2(), origen + self.b.to_vec2());
        if (self.a - self.b).length_sq() > 0.01 {
            painter.line_segment([a, b], egui::Stroke::new(self.size, c));
        }
        painter.circle_filled(b, self.size * 0.5, c);
        painter.circle_filled(a, self.size * 0.5, c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lienzo() -> Canvas {
        let mut c = Canvas::new(1.0);
        c.ensure(256.0, 128.0);
        c
    }

    #[test]
    fn un_trazo_pinta_su_color_en_el_centro_y_nada_lejos() {
        let mut c = lienzo();
        c.segment(Pos2::new(20.0, 20.0), Pos2::new(100.0, 20.0), 8.0, Color32::from_rgb(224, 163, 92), false);
        assert_eq!(c.pixel(60, 20), Color32::from_rgb(224, 163, 92));
        assert_eq!(c.pixel(60, 40), Color32::TRANSPARENT);
        // el remate es redondo (radio 4): a 3 px del extremo todavía hay tinta, a 10 ya no
        assert!(c.pixel(22, 20).a() > 200 && c.pixel(17, 20).a() > 100);
        assert_eq!(c.pixel(110, 20), Color32::TRANSPARENT);
    }

    #[test]
    fn el_borrador_deja_transparente_y_no_toca_lo_demas() {
        let mut c = lienzo();
        c.segment(Pos2::new(10.0, 10.0), Pos2::new(120.0, 10.0), 10.0, Color32::WHITE, false);
        c.segment(Pos2::new(60.0, 4.0), Pos2::new(60.0, 16.0), 6.0, Color32::BLACK, true);
        assert_eq!(c.pixel(60, 10), Color32::TRANSPARENT);
        assert_eq!(c.pixel(100, 10), Color32::WHITE);
    }

    #[test]
    fn el_lienzo_solo_crece_y_conserva_lo_pintado() {
        let mut c = lienzo();
        c.segment(Pos2::new(30.0, 30.0), Pos2::new(30.0, 30.0), 10.0, Color32::WHITE, false);
        c.ensure(100.0, 100.0); // más chico: no encoge
        assert!(c.size_pts().x >= 256.0);
        c.ensure(600.0, 300.0);
        assert!(c.size_pts().x >= 600.0 && c.size_pts().y >= 300.0);
        assert_eq!(c.pixel(30, 30), Color32::WHITE);
    }

    #[test]
    fn png_de_ida_y_vuelta() {
        let mut c = lienzo();
        c.segment(Pos2::new(10.0, 10.0), Pos2::new(80.0, 40.0), 7.0, Color32::from_rgb(169, 181, 106), false);
        let bytes = c.foto().to_png().unwrap();
        let mut d = Canvas::new(1.0);
        d.from_png(&bytes).unwrap();
        assert_eq!((d.w, d.h), (c.w, c.h));
        assert_eq!(d.pixel(45, 25), c.pixel(45, 25));
        // un borde suavizado (alfa parcial) también vuelve igual, con redondeo de un nivel
        let (a, b) = (c.pixel(45, 29), d.pixel(45, 29));
        assert!((a.a() as i32 - b.a() as i32).abs() <= 1 && (a.g() as i32 - b.g() as i32).abs() <= 2);
    }

    #[test]
    fn el_efimero_se_desvanece_linealmente() {
        let t0 = Instant::now();
        let e = Efimero { a: Pos2::ZERO, b: Pos2::new(1.0, 1.0), size: 4.0, color: Color32::WHITE, nacio: t0, dura: Duration::from_secs(2) };
        assert!((e.vida(t0) - 1.0).abs() < 1e-3);
        assert!((e.vida(t0 + Duration::from_secs(1)) - 0.5).abs() < 1e-3);
        assert!(e.vida(t0 + Duration::from_secs(3)) < 0.0);
    }
}
