//! El lienzo: teselas de píxeles en la CPU, creadas solo donde se dibuja, con coordenadas
//! enteras con signo. No hay borde: se puede dibujar y desplazarse hacia cualquier lado.
//!
//! Antes era una sola textura de 0 a 4096 puntos (64 MiB de RAM y de GPU a escala 1, 256 MiB a
//! escala 2) y egui hace `assert!` en release si una textura supera `GL_MAX_TEXTURE_SIZE`, algo
//! que en una GPU con 8192 de límite pasaba justo con la escala 2. Ahora cada tesela es de
//! 256x256 texels (256 KiB), así que:
//!  - la memoria es proporcional a lo dibujado, no a lo recorrido;
//!  - ninguna textura se acerca a un límite de la GPU;
//!  - solo se guardan en disco las teselas que cambiaron (un PNG por tesela).
//!
//! Cada segmento se rasteriza una vez (cápsula con borde suavizado, como el `lineCap = "round"`
//! del canvas web). Borrar es `destination-out`: el píxel vuelve a ser transparente y se ve el
//! color de fondo, así que cambiar el fondo no destruye el dibujo.
//!
//! Límite: ±`LIM_PTS` puntos. Los vértices de egui son `f32`: a 200 000 el paso es ~0,016 px,
//! todavía por debajo de un píxel; más lejos los trazos empezarían a temblar.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use egui::{Color32, ColorImage, Context, Painter, Pos2, Rect, TextureHandle, TextureOptions, Vec2};

/// Hasta dónde llega el lienzo, a cada lado del origen, en puntos lógicos.
pub const LIM_PTS: f32 = 200_000.0;
/// Lado de una tesela, en texels.
const TEX: usize = 256;
/// Tope de teselas vivas (2048 x 256 KiB = 512 MiB): un trazo que pidiera más se recorta en vez
/// de comerse la memoria.
const MAX_TESELAS: usize = 2048;

type Clave = (i32, i32);

struct Tesela {
    px: Vec<Color32>,
    tex: Option<TextureHandle>,
    /// Caja sucia pendiente de subir a la GPU: x0, y0, x1, y1 (el final no incluido).
    sucia: Option<[usize; 4]>,
}

impl Tesela {
    fn nueva() -> Self {
        Self { px: vec![Color32::TRANSPARENT; TEX * TEX], tex: None, sucia: None }
    }
}

pub struct Canvas {
    /// Texels por punto lógico (2 en pantallas de alta densidad, para que el trazo no salga
    /// borroso).
    scale: f32,
    teselas: HashMap<Clave, Tesela>,
    /// Teselas con cambios que no están en disco.
    a_guardar: HashSet<Clave>,
    /// Se vació el lienzo: al guardar hay que borrar también lo que había en disco.
    borrado: bool,
}

impl Canvas {
    pub fn new(scale: f32) -> Self {
        Self { scale, teselas: HashMap::new(), a_guardar: HashSet::new(), borrado: false }
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Lado de una tesela en puntos lógicos.
    pub fn tesela_pts(&self) -> f32 {
        TEX as f32 / self.scale
    }

    /// El rectángulo, en puntos, que cubren las teselas que existen (de tesela en tesela).
    pub fn limites(&self) -> Option<Rect> {
        let tp = self.tesela_pts();
        let mut it = self.teselas.keys();
        let &(x, y) = it.next()?;
        let (mut x0, mut y0, mut x1, mut y1) = (x, y, x, y);
        for &(x, y) in it {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        Some(Rect::from_min_max(
            Pos2::new(x0 as f32 * tp, y0 as f32 * tp),
            Pos2::new((x1 + 1) as f32 * tp, (y1 + 1) as f32 * tp),
        ))
    }

    pub fn clear(&mut self) {
        self.teselas.clear();
        self.a_guardar.clear();
        self.borrado = true;
    }

    /// Un segmento redondeado de `size` puntos de grosor. `erase` = deja transparente.
    pub fn segment(&mut self, a: Pos2, b: Pos2, size: f32, color: Color32, erase: bool) {
        // `!(x <= LIM)` y no `x > LIM`: así también se descarta NaN.
        if !(a.x.abs() <= LIM_PTS && a.y.abs() <= LIM_PTS && b.x.abs() <= LIM_PTS && b.y.abs() <= LIM_PTS) {
            return;
        }
        let s = self.scale;
        let r = (size * 0.5 * s).max(0.5);
        let (ax, ay, bx, by) = (a.x * s, a.y * s, b.x * s, b.y * s);
        let tf = TEX as f32;
        let (dx, dy) = (bx - ax, by - ay);
        let l2 = dx * dx + dy * dy;
        // Distancia de un punto al segmento.
        let dist = |px: f32, py: f32| {
            let t = if l2 < 1e-6 { 0.0 } else { (((px - ax) * dx + (py - ay) * dy) / l2).clamp(0.0, 1.0) };
            let (cx, cy) = (ax + t * dx - px, ay + t * dy - py);
            (cx * cx + cy * cy).sqrt()
        };
        let (minx, maxx, miny, maxy) = (ax.min(bx) - r - 1.0, ax.max(bx) + r + 2.0, ay.min(by) - r - 1.0, ay.max(by) + r + 2.0);
        let src = [color.r() as f32, color.g() as f32, color.b() as f32];

        for ty in (miny / tf).floor() as i32..=(maxy / tf).floor() as i32 {
            for tx in (minx / tf).floor() as i32..=(maxx / tf).floor() as i32 {
                // Una diagonal larga tiene una caja enorme y casi toda vacía: se descartan las
                // teselas cuyo centro queda más lejos del segmento que su media diagonal.
                let (cx, cy) = ((tx as f32 + 0.5) * tf, (ty as f32 + 0.5) * tf);
                if dist(cx, cy) > r + 2.0 + tf * 0.7072 {
                    continue;
                }
                let clave = (tx, ty);
                let es_nueva = !self.teselas.contains_key(&clave);
                if es_nueva {
                    // Borrar donde no hay nada no hace nada, y no se crea para eso.
                    if erase || self.teselas.len() >= MAX_TESELAS {
                        continue;
                    }
                    self.teselas.insert(clave, Tesela::nueva());
                }
                let t = self.teselas.get_mut(&clave).expect("recién creada o existente");
                let (ox, oy) = (tx as f32 * tf, ty as f32 * tf);
                let lim = |v: f32| (v.max(0.0) as usize).min(TEX);
                let (x0, x1) = (lim((minx - ox).floor()), lim((maxx - ox).ceil()));
                let (y0, y1) = (lim((miny - oy).floor()), lim((maxy - oy).ceil()));
                let mut tocada = false;
                for y in y0..y1 {
                    let py = oy + y as f32 + 0.5;
                    for x in x0..x1 {
                        let cob = (r + 0.5 - dist(ox + x as f32 + 0.5, py)).clamp(0.0, 1.0);
                        if cob <= 0.0 {
                            continue;
                        }
                        tocada = true;
                        let d = &mut t.px[y * TEX + x];
                        let (dr, dg, db, da) = (d.r() as f32, d.g() as f32, d.b() as f32, d.a() as f32);
                        let k = 1.0 - cob;
                        *d = if erase {
                            Color32::from_rgba_premultiplied((dr * k) as u8, (dg * k) as u8, (db * k) as u8, (da * k) as u8)
                        } else {
                            // fuente opaca sobre lo que hubiera: out = src*cob + dst*(1-cob), ya premultiplicado
                            Color32::from_rgba_premultiplied(
                                (src[0] * cob + dr * k).round() as u8,
                                (src[1] * cob + dg * k).round() as u8,
                                (src[2] * cob + db * k).round() as u8,
                                (255.0 * cob + da * k).round() as u8,
                            )
                        };
                    }
                }
                if tocada {
                    t.sucia = Some(match t.sucia {
                        None => [x0, y0, x1, y1],
                        Some([a, b, c, d]) => [a.min(x0), b.min(y0), c.max(x1), d.max(y1)],
                    });
                    self.a_guardar.insert(clave);
                } else if es_nueva {
                    self.teselas.remove(&clave);
                }
            }
        }
    }

    /// Sube a la GPU lo que cambió. Una vez por fotograma, no por segmento.
    pub fn flush(&mut self, ctx: &Context) {
        for (&(tx, ty), t) in self.teselas.iter_mut() {
            match (&mut t.tex, t.sucia.take()) {
                (None, _) => {
                    let img = ColorImage { size: [TEX, TEX], pixels: t.px.clone() };
                    t.tex = Some(ctx.load_texture(format!("lienzo{tx},{ty}"), img, TextureOptions::LINEAR));
                }
                (Some(h), Some([x0, y0, x1, y1])) => {
                    let mut pixels = Vec::with_capacity((x1 - x0) * (y1 - y0));
                    for y in y0..y1 {
                        pixels.extend_from_slice(&t.px[y * TEX + x0..y * TEX + x1]);
                    }
                    h.set_partial([x0, y0], ColorImage { size: [x1 - x0, y1 - y0], pixels }, TextureOptions::LINEAR);
                }
                (Some(_), None) => {}
            }
        }
    }

    /// Pinta las teselas visibles. `origen` es dónde cae el (0,0) del lienzo en la pantalla y
    /// `zoom`, cuántas veces más grande (1 = normal; el minimapa usa uno pequeño).
    pub fn paint(&self, painter: &Painter, origen: Pos2, zoom: f32) {
        let clip = painter.clip_rect();
        let lado = self.tesela_pts() * zoom;
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        for (&(tx, ty), t) in &self.teselas {
            if let Some(h) = &t.tex {
                let r = Rect::from_min_size(origen + Vec2::new(tx as f32, ty as f32) * lado, Vec2::splat(lado));
                if r.intersects(clip) {
                    painter.image(h.id(), r, uv, Color32::WHITE);
                }
            }
        }
    }

    // ---- disco ---------------------------------------------------------------------------

    pub fn hay_cambios(&self) -> bool {
        self.borrado || !self.a_guardar.is_empty()
    }

    /// Copia de lo que cambió, para escribirlo en otro hilo: comprimir PNG en el hilo de la
    /// interfaz es un tirón. Deja el lienzo como «guardado».
    pub fn foto(&mut self) -> Foto {
        let borrar_todo = std::mem::take(&mut self.borrado);
        let claves: Vec<Clave> = self.a_guardar.drain().collect();
        let teselas = claves.into_iter().filter_map(|k| self.teselas.get(&k).map(|t| (k, t.px.clone()))).collect();
        Foto { teselas, borrar_todo }
    }

    /// Carga las teselas guardadas en `dir` (`x_y.png`). Lo que no sea una tesela válida se ignora.
    pub fn cargar(&mut self, dir: &Path) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            if self.teselas.len() >= MAX_TESELAS {
                break;
            }
            let ruta = e.path();
            let Some(clave) = ruta.file_stem().and_then(|n| n.to_str()).and_then(parse_clave) else { continue };
            if ruta.extension().and_then(|x| x.to_str()) != Some("png") {
                continue;
            }
            if let Some(px) = std::fs::read(&ruta).ok().and_then(|b| decodifica(&b)) {
                self.teselas.insert(clave, Tesela { px, tex: None, sucia: None });
            }
        }
    }

    #[cfg(test)]
    pub fn pixel(&self, x: i32, y: i32) -> Color32 {
        let t = TEX as i32;
        match self.teselas.get(&(x.div_euclid(t), y.div_euclid(t))) {
            Some(tes) => tes.px[y.rem_euclid(t) as usize * TEX + x.rem_euclid(t) as usize],
            None => Color32::TRANSPARENT,
        }
    }

    #[cfg(test)]
    pub fn n_teselas(&self) -> usize {
        self.teselas.len()
    }
}

fn parse_clave(nombre: &str) -> Option<Clave> {
    let (x, y) = nombre.split_once('_')?;
    Some((x.parse().ok()?, y.parse().ok()?))
}

pub struct Foto {
    teselas: Vec<(Clave, Vec<Color32>)>,
    borrar_todo: bool,
}

impl Foto {
    /// Escribe las teselas cambiadas (PNG, sin premultiplicar, que es lo que espera el formato).
    pub fn guardar(&self, dir: &Path) {
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        if self.borrar_todo {
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    if e.path().extension().and_then(|x| x.to_str()) == Some("png") {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            }
        }
        for ((x, y), px) in &self.teselas {
            if let Some(png) = codifica(px) {
                let destino = dir.join(format!("{x}_{y}.png"));
                let tmp = destino.with_extension("tmp");
                if std::fs::write(&tmp, png).is_ok() {
                    let _ = std::fs::rename(tmp, destino);
                }
            }
        }
    }
}

fn codifica(px: &[Color32]) -> Option<Vec<u8>> {
    let mut rgba = Vec::with_capacity(px.len() * 4);
    for c in px {
        let a = c.a();
        let un = |v: u8| if a == 0 { 0 } else { ((v as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 };
        rgba.extend_from_slice(&[un(c.r()), un(c.g()), un(c.b()), a]);
    }
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, TEX as u32, TEX as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    enc.write_header().ok()?.write_image_data(&rgba).ok()?;
    Some(out)
}

fn decodifica(bytes: &[u8]) -> Option<Vec<Color32>> {
    let mut r = png::Decoder::new(bytes).read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight || (info.width, info.height) != (TEX as u32, TEX as u32) {
        return None;
    }
    // Premultiplicar igual que `codifica` divide (en bruto, sin pasar por el espacio lineal que
    // usa `from_rgba_unmultiplied`): el lienzo mezcla en bruto y los bordes suavizados saldrían
    // de otro color.
    let pre = |v: u8, a: u8| ((v as u32 * a as u32 + 127) / 255) as u8;
    Some(
        buf[..TEX * TEX * 4]
            .chunks_exact(4)
            .map(|p| Color32::from_rgba_premultiplied(pre(p[0], p[3]), pre(p[1], p[3]), pre(p[2], p[3]), p[3]))
            .collect(),
    )
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

    fn tinta() -> Color32 {
        Color32::from_rgb(224, 163, 92)
    }

    #[test]
    fn un_trazo_pinta_su_color_en_el_centro_y_nada_lejos() {
        let mut c = Canvas::new(1.0);
        c.segment(Pos2::new(20.0, 20.0), Pos2::new(100.0, 20.0), 8.0, tinta(), false);
        assert_eq!(c.pixel(60, 20), tinta());
        assert_eq!(c.pixel(60, 40), Color32::TRANSPARENT);
        // el remate es redondo (radio 4): a 3 px del extremo todavía hay tinta, a 10 ya no
        assert!(c.pixel(22, 20).a() > 200 && c.pixel(17, 20).a() > 100);
        assert_eq!(c.pixel(110, 20), Color32::TRANSPARENT);
    }

    #[test]
    fn el_borrador_deja_transparente_y_no_toca_lo_demas() {
        let mut c = Canvas::new(1.0);
        c.segment(Pos2::new(10.0, 10.0), Pos2::new(120.0, 10.0), 10.0, Color32::WHITE, false);
        c.segment(Pos2::new(60.0, 4.0), Pos2::new(60.0, 16.0), 6.0, Color32::BLACK, true);
        assert_eq!(c.pixel(60, 10), Color32::TRANSPARENT);
        assert_eq!(c.pixel(100, 10), Color32::WHITE);
    }

    #[test]
    fn no_hay_borde_y_la_memoria_sigue_lo_dibujado() {
        let mut c = Canvas::new(1.0);
        // hacia arriba y a la izquierda del origen, y muy lejos hacia el otro lado
        c.segment(Pos2::new(-300.0, -300.0), Pos2::new(-300.0, -300.0), 10.0, Color32::WHITE, false);
        c.segment(Pos2::new(150_000.0, 90_000.0), Pos2::new(150_000.0, 90_000.0), 10.0, Color32::WHITE, false);
        assert_eq!(c.pixel(-300, -300), Color32::WHITE);
        assert_eq!(c.pixel(150_000, 90_000), Color32::WHITE);
        assert_eq!(c.n_teselas(), 2, "dos puntos lejanos son dos teselas, no el hueco entre ellos");
        // borrar donde no hay nada no crea teselas
        c.segment(Pos2::new(5000.0, 5000.0), Pos2::new(5100.0, 5000.0), 20.0, Color32::BLACK, true);
        assert_eq!(c.n_teselas(), 2);
        // más allá del límite o con NaN se ignora
        c.segment(Pos2::new(LIM_PTS + 1.0, 0.0), Pos2::new(0.0, 0.0), 5.0, Color32::WHITE, false);
        c.segment(Pos2::new(f32::NAN, 0.0), Pos2::new(0.0, 0.0), 5.0, Color32::WHITE, false);
        assert_eq!(c.n_teselas(), 2);
    }

    #[test]
    fn una_diagonal_larga_solo_toca_las_teselas_por_donde_pasa() {
        let mut c = Canvas::new(1.0);
        c.segment(Pos2::new(0.0, 0.0), Pos2::new(20_000.0, 20_000.0), 4.0, Color32::WHITE, false);
        // ~20 000 / 256 = 78 teselas de diagonal (más las vecinas del cruce), no 78 x 78
        assert!(c.n_teselas() > 70 && c.n_teselas() < 240, "{}", c.n_teselas());
        assert_eq!(c.pixel(10_000, 10_000), Color32::WHITE);
    }

    #[test]
    fn a_disco_van_solo_las_teselas_que_cambiaron_y_vuelven_iguales() {
        let dir = std::env::temp_dir().join(format!("artchat-teselas-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut c = Canvas::new(1.0);
        c.segment(Pos2::new(10.0, 10.0), Pos2::new(80.0, 40.0), 7.0, Color32::from_rgb(169, 181, 106), false);
        c.segment(Pos2::new(-400.0, 700.0), Pos2::new(-380.0, 700.0), 7.0, Color32::WHITE, false);
        assert!(c.hay_cambios());
        c.foto().guardar(&dir);
        assert!(!c.hay_cambios());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2, "una tesela por trazo");
        let mut d = Canvas::new(1.0);
        d.cargar(&dir);
        assert_eq!(d.n_teselas(), c.n_teselas());
        assert_eq!(d.pixel(45, 25), c.pixel(45, 25));
        assert_eq!(d.pixel(-390, 700), Color32::WHITE);
        // un borde suavizado (alfa parcial) también vuelve igual, con redondeo de un nivel
        let (a, b) = (c.pixel(45, 29), d.pixel(45, 29));
        assert!((a.a() as i32 - b.a() as i32).abs() <= 1 && (a.g() as i32 - b.g() as i32).abs() <= 2);
        // vaciar borra también lo que había en disco
        c.clear();
        c.foto().guardar(&dir);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn los_limites_cubren_las_teselas() {
        let mut c = Canvas::new(1.0);
        assert!(c.limites().is_none());
        c.segment(Pos2::new(-10.0, 10.0), Pos2::new(300.0, 10.0), 4.0, Color32::WHITE, false);
        let l = c.limites().unwrap();
        assert!(l.min.x <= -10.0 && l.max.x >= 300.0 && l.min.y <= 10.0 && l.max.y >= 10.0);
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
