//! La ventana: el lienzo a sangre y, encima, tres islas flotantes (herramientas arriba a la
//! izquierda, usuarios arriba a la derecha, chat abajo a la izquierda), que es la
//! disposición del cliente Tauri con la identidad de windots.

use std::time::{Duration, Instant};

use egui::emath::TSTransform;
use egui::text::{LayoutJob, TextFormat};
use egui::{
    pos2, vec2, Align, Align2, Color32, Context, CursorIcon, FontId, Frame, Id, LayerId, Layout, Margin, Order, Pos2,
    Response, RichText, Rounding, ScrollArea, Sense, Shadow, Stroke, TextEdit, Ui,
};

use crate::audio::{Sfx, Sonido};
use crate::canvas::{Canvas, Efimero};
use crate::network::{self, Event, Net};
use crate::settings::{self, Settings};
use crate::theme::{self, col, col_a, icono, ACCENT, ACCENT_OK, ACCENT_WARN, ATARDECER, BAR_BG, HIGHLIGHT, SUBTEXT, SURFACE, TEXT};
use crate::types::{parse_hex, to_hex, In, Out, Seg, User};

const URL: &str = "wss://artchat.danassistantassistant.website";
/// Lo que se espera entre un zumbido y el siguiente (el cliente Tauri: 3 s).
const ENFRIAR_ZUMBIDO: Duration = Duration::from_secs(3);
const DURA_SACUDIDA: f32 = 0.5;
const MAX_EFIMEROS: usize = 30_000;

#[derive(Clone, PartialEq)]
enum Conn {
    Conectando,
    Conectado,
    Error,
}

#[derive(Clone, Copy, PartialEq)]
enum Tipo {
    Yo,
    Otro,
    Sistema,
    Historial,
}

struct Linea {
    tipo: Tipo,
    nick: String,
    texto: String,
}

pub struct App {
    s: Settings,
    guardado: Settings,
    sucio_desde: Option<Instant>,

    net: Net,
    conn: Conn,
    /// Por qué se cayó, para la ayuda del indicador.
    motivo: String,
    usuarios: Vec<User>,
    mi_estado: String,
    join_enviado: Option<Instant>,
    nick_enviado: String,
    /// El historial se aplica una vez por conexión: el servidor lo manda con cada `join`, y
    /// repetirlo duplicaría los mensajes.
    historia_aplicada: bool,

    lienzo: Canvas,
    listo: bool,
    efimeros: Vec<Efimero>,
    borrador: bool,
    /// Último punto del trazo en curso, en coordenadas del lienzo.
    trazando: Option<Pos2>,
    /// Esquina superior izquierda de la ventana sobre el lienzo, en coordenadas del lienzo (las
    /// del protocolo). El panorámico mueve esto; los trazos salen ya en coordenadas del lienzo.
    vista: egui::Vec2,
    paneando: bool,
    /// Lo que abarca el minimapa, fijo mientras se arrastra en él.
    mini_fijo: Option<egui::Rect>,
    mini_arrastra: bool,
    /// Tamaño del área de dibujo en el último fotograma, para el minimapa.
    ventana: egui::Vec2,
    lienzo_guardado: Instant,

    lineas: Vec<Linea>,
    entrada: String,
    zumbido_libre: Instant,
    sacudida: Option<Instant>,

    sfx: Sfx,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        let url = std::env::var("ARTCHAT_URL").unwrap_or_else(|_| URL.to_string());
        Self::con(&cc.egui_ctx, Settings::load(), url)
    }

    /// Aparte de `new` para poder crearla sin ventana (las pruebas).
    fn con(ctx: &Context, s: Settings, url: String) -> Self {
        let ctx = ctx.clone();
        Self {
            guardado: s.clone(),
            sucio_desde: None,
            net: network::spawn(url, move || ctx.request_repaint()),
            conn: Conn::Conectando,
            motivo: String::new(),
            usuarios: Vec::new(),
            mi_estado: "online".into(),
            join_enviado: None,
            nick_enviado: s.nick.clone(),
            historia_aplicada: false,
            lienzo: Canvas::new(1.0),
            listo: false,
            efimeros: Vec::new(),
            borrador: false,
            trazando: None,
            vista: egui::Vec2::ZERO,
            paneando: false,
            mini_fijo: None,
            mini_arrastra: false,
            ventana: egui::Vec2::ZERO,
            lienzo_guardado: Instant::now(),
            lineas: Vec::new(),
            entrada: String::new(),
            zumbido_libre: Instant::now(),
            sacudida: None,
            sfx: Sfx::default(),
            s,
        }
    }

    // ---- red ------------------------------------------------------------------------------

    fn enviar(&self, o: Out) {
        let _ = self.net.tx.send(o);
    }

    fn enviar_join(&mut self) {
        self.enviar(Out::Join { sender_id: self.s.id.clone(), nickname: self.s.nick.clone(), color: to_hex(self.s.color) });
        self.nick_enviado = self.s.nick.clone();
        self.join_enviado = Some(Instant::now());
    }

    fn sonar(&mut self, s: Sonido) {
        if self.s.sound {
            self.sfx.play(s);
        }
    }

    fn red(&mut self) {
        while let Ok(ev) = self.net.rx.try_recv() {
            match ev {
                Event::Connecting => {
                    self.conn = Conn::Conectando;
                    self.historia_aplicada = false;
                }
                Event::Connected => {
                    self.conn = Conn::Conectado;
                    self.mi_estado = "online".into(); // el servidor registra a todos "online"
                    self.enviar_join();
                }
                Event::Down(motivo) => {
                    self.conn = Conn::Error;
                    self.motivo = motivo;
                    self.usuarios.clear();
                }
                Event::Msg(m) => self.mensaje(m),
            }
        }
    }

    fn mensaje(&mut self, m: In) {
        match m {
            In::UsersUpdate { users } => {
                if users.len() > self.usuarios.len() {
                    self.sonar(Sonido::Entra);
                }
                self.usuarios = users;
                // Si llegan usuarios, estamos conectados aunque se nos haya pasado el evento.
                if !self.usuarios.is_empty() {
                    self.conn = Conn::Conectado;
                }
            }
            In::History { strokes, messages } => {
                if self.historia_aplicada {
                    return;
                }
                self.historia_aplicada = true;
                for s in &strokes {
                    self.aplicar(s, true);
                }
                self.lineas.retain(|l| l.tipo != Tipo::Historial);
                let hist = messages.into_iter().map(|h| Linea { tipo: Tipo::Historial, nick: h.nickname, texto: h.content });
                self.lineas.splice(0..0, hist.collect::<Vec<_>>());
            }
            In::Draw { sender_id, seg } => {
                if sender_id != self.s.id {
                    self.aplicar(&seg, false);
                }
            }
            In::Chat { sender_id, nickname, content } => {
                if sender_id != self.s.id {
                    let nick = nickname.filter(|n| !n.is_empty()).unwrap_or_else(|| "Anónimo".into());
                    self.lineas.push(Linea { tipo: Tipo::Otro, nick, texto: content });
                    self.sonar(Sonido::Aviso);
                }
            }
            In::Clear { sender_id } => {
                if sender_id != self.s.id {
                    self.lienzo.clear();
                    self.efimeros.clear();
                }
            }
            In::Buzz { sender_id, nickname } => {
                if sender_id != self.s.id {
                    let nick = nickname.filter(|n| !n.is_empty()).unwrap_or_else(|| "Alguien".into());
                    self.lineas.push(Linea { tipo: Tipo::Sistema, nick: String::new(), texto: format!("{} {nick} te envió un zumbido", icono::CAMPANA) });
                    self.sonar(Sonido::Zumbido);
                    self.sacudida = Some(Instant::now());
                }
            }
            In::Other => {}
        }
    }

    /// Un segmento de otro cliente (o del historial) al lienzo o a los efímeros.
    fn aplicar(&mut self, s: &Seg, historial: bool) {
        let (a, b) = (pos2(s.x0, s.y0), pos2(s.x1, s.y1));
        if ![a.x, a.y, b.x, b.y].iter().all(|v| v.is_finite()) {
            return;
        }
        let size = s.size.filter(|v| v.is_finite()).unwrap_or(5.0).clamp(0.5, 200.0);
        let color = s.color.as_deref().and_then(parse_hex).map(col).unwrap_or(Color32::from_rgb(0, 255, 0));
        if s.ephemeral && !historial {
            let ms = s.duration.filter(|d| d.is_finite() && *d > 0.0).unwrap_or(1000.0).min(3_600_000.0);
            if self.efimeros.len() >= MAX_EFIMEROS {
                self.efimeros.drain(..1000);
            }
            self.efimeros.push(Efimero { a, b, size, color, nacio: Instant::now(), dura: Duration::from_millis(ms as u64) });
        } else {
            self.lienzo.segment(a, b, size, color, s.erase);
        }
    }

    // ---- dibujo ---------------------------------------------------------------------------

    /// Un segmento del trazo del usuario: se pinta aquí y se manda al servidor, igual que hacía
    /// el cliente Tauri con cada `mousemove`.
    fn trazar(&mut self, a: Pos2, b: Pos2) {
        let size = self.s.size;
        let efimero = self.s.fade && !self.borrador;
        if self.borrador {
            self.lienzo.segment(a, b, size, Color32::BLACK, true);
        } else if efimero {
            self.efimeros.push(Efimero { a, b, size, color: col(self.s.color), nacio: Instant::now(), dura: Duration::from_millis(self.s.fade_ms as u64) });
        } else {
            self.lienzo.segment(a, b, size, col(self.s.color), false);
        }
        self.enviar(Out::Draw {
            sender_id: self.s.id.clone(),
            seg: Seg {
                x0: a.x,
                y0: a.y,
                x1: b.x,
                y1: b.y,
                color: Some(to_hex(self.s.color)),
                size: Some(size),
                erase: self.borrador,
                ephemeral: efimero,
                duration: Some(if efimero { self.s.fade_ms } else { 0.0 }),
            },
        });
    }

    fn limpiar(&mut self) {
        self.lienzo.clear();
        self.efimeros.clear();
        self.enviar(Out::Clear { sender_id: self.s.id.clone() });
    }

    fn lienzo_ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let origen = resp.rect.min;
        self.ventana = resp.rect.size();

        let teclado_libre = !ctx.wants_keyboard_input();
        let (pulsado, abajo, medio, medio_pulsado, espacio, rueda, delta, eventos, ptr) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.middle_down(),
                i.pointer.button_pressed(egui::PointerButton::Middle),
                i.key_down(egui::Key::Space),
                i.smooth_scroll_delta,
                i.pointer.delta(),
                i.events.clone(),
                i.pointer.hover_pos(),
            )
        });
        let espacio = espacio && teclado_libre;

        // Panorámico: botón del medio, o espacio + arrastrar, o la rueda (con Mayús, de lado).
        let arrastre = medio || (espacio && abajo);
        if !arrastre {
            self.paneando = false;
        } else if (medio_pulsado || pulsado) && resp.hovered() {
            self.paneando = true;
        }
        if self.paneando {
            self.vista -= delta;
            self.trazando = None;
        } else if resp.hovered() {
            self.vista -= rueda;
        }
        self.vista = limita_vista(self.vista);

        let vista = self.vista;
        let local = move |p: Pos2| p - origen.to_vec2() + vista;
        if pulsado && resp.hovered() && !espacio {
            if let Some(p) = ptr.map(local) {
                self.trazar(p, p); // el punto de la pulsación, como el `arc` del cliente Tauri
                self.trazando = Some(p);
            }
        }
        if !abajo || self.paneando {
            self.trazando = None;
        } else if let Some(mut ult) = self.trazando {
            // Cada movimiento que llegó en el fotograma, no solo el último: así el trazo no
            // sale a tramos rectos cuando el ratón va más rápido que la pantalla.
            for e in &eventos {
                match e {
                    egui::Event::PointerMoved(p) => {
                        let p = local(*p);
                        if p != ult {
                            self.trazar(ult, p);
                            ult = p;
                        }
                    }
                    egui::Event::PointerGone => {
                        self.trazando = None;
                        break;
                    }
                    _ => {}
                }
            }
            if self.trazando.is_some() {
                self.trazando = Some(ult);
            }
        }

        self.lienzo.flush(&ctx);
        painter.rect_filled(resp.rect, 0.0, col(self.s.bg));
        // Al píxel físico: las teselas van pegadas, y con una posición fraccionaria el filtro
        // lineal dejaría una rendija en cada unión.
        let pp = ctx.pixels_per_point();
        let origen_mundo = Pos2::new(((origen.x - self.vista.x) * pp).round() / pp, ((origen.y - self.vista.y) * pp).round() / pp);
        self.lienzo.paint(&painter, origen_mundo, 1.0);
        let ahora = Instant::now();
        self.efimeros.retain(|e| e.vida(ahora) > 0.0);
        for e in &self.efimeros {
            e.paint(&painter, origen_mundo, ahora);
        }
        if !self.efimeros.is_empty() {
            ctx.request_repaint();
        }

        // El cursor es un aro del tamaño del pincel: se ve dónde va a caer el trazo.
        if self.paneando {
            ctx.set_cursor_icon(CursorIcon::Grabbing);
        } else if resp.hovered() && espacio {
            ctx.set_cursor_icon(CursorIcon::Grab);
        } else if resp.hovered() {
            ctx.set_cursor_icon(CursorIcon::None);
            if let Some(p) = ptr {
                let r = (self.s.size * 0.5).max(1.5);
                let tinta = if self.borrador { col(ACCENT_WARN) } else { col_a(TEXT, 0.9) };
                painter.circle_stroke(p, r + 0.75, Stroke::new(1.0_f32, Color32::from_black_alpha(150)));
                painter.circle_stroke(p, r, Stroke::new(1.0_f32, tinta));
            }
        }
    }

    // ---- islas ----------------------------------------------------------------------------

    fn isla() -> Frame {
        Frame::none()
            .fill(col(SURFACE))
            .rounding(Rounding::same(theme::RADIO_ISLA))
            .stroke(Stroke::new(1.0_f32, col(HIGHLIGHT)))
            .shadow(Shadow { offset: vec2(0.0, 5.0), blur: 18.0, spread: 0.0, color: Color32::from_black_alpha(110) })
            .inner_margin(Margin::symmetric(12.0, 9.0))
    }

    fn barra(&mut self, ctx: &Context) {
        // Hasta el panel de usuarios (166 + 24 de margen), con 14 de aire a cada lado y los 24
        // de margen de la propia isla.
        let ancho = (ctx.screen_rect().width() - 190.0 - 14.0 * 3.0 - 24.0).max(260.0);
        egui::Area::new(Id::new("barra")).order(Order::Foreground).fixed_pos(pos2(14.0, 14.0)).show(ctx, |ui| {
            Self::isla().show(ui, |ui| {
                ui.set_max_width(ancho);
                ui.horizontal_wrapped(|ui| {
                    let nick = ui.add(
                        TextEdit::singleline(&mut self.s.nick)
                            .desired_width(104.0)
                            .char_limit(24)
                            .hint_text("Tu nick")
                            .text_color(col(ACCENT)),
                    );
                    if nick.lost_focus() {
                        self.confirmar_nick();
                    }
                    let (g, tip) = if self.s.sound { (icono::SONIDO, "Silenciar") } else { (icono::SILENCIO, "Activar sonido") };
                    if boton_icono(ui, g, !self.s.sound, true, tip).clicked() {
                        self.s.sound = !self.s.sound;
                    }
                    ui.separator();

                    ui.color_edit_button_srgb(&mut self.s.color).on_hover_text("Color del pincel");
                    for c in ATARDECER.iter().chain([&theme::SUBTEXT]) {
                        if punto_color(ui, *c, self.s.color == *c && !self.borrador).clicked() {
                            self.s.color = *c;
                            self.borrador = false;
                        }
                    }
                    ui.color_edit_button_srgb(&mut self.s.bg).on_hover_text("Color de fondo");
                    ui.separator();

                    // Un solo widget (con su valor dentro): `horizontal_wrapped` envuelve widgets
                    // sueltos, no bloques anidados, que se salen por el borde en vez de bajar.
                    ui.add(egui::Slider::new(&mut self.s.size, 1.0..=50.0).max_decimals(0)).on_hover_text("Grosor");
                    ui.separator();

                    let (g, tip) = if self.borrador { (icono::BORRADOR, "Volver a dibujar") } else { (icono::BORRADOR, "Borrador") };
                    if boton_icono(ui, g, self.borrador, true, tip).clicked() {
                        self.borrador = !self.borrador;
                    }
                    if boton_icono(ui, icono::PAPELERA, false, true, "Borrar todo (para todos)").clicked() {
                        self.limpiar();
                    }
                    ui.separator();

                    if boton_icono(ui, icono::VARITA, self.s.fade, true, "Trazos que se desvanecen").clicked() {
                        self.s.fade = !self.s.fade;
                    }
                    if self.s.fade {
                        ui.add(
                            egui::Slider::new(&mut self.s.fade_ms, 500.0..=360_000.0)
                                .logarithmic(true)
                                .custom_formatter(|v, _| tiempo(v as f32))
                                .custom_parser(|t| destiempo(t)),
                        )
                        .on_hover_text("Cuánto tarda en desvanecerse");
                    }
                    ui.separator();
                    self.indicador(ui);
                });
            });
        });
    }

    fn confirmar_nick(&mut self) {
        let n = self.s.nick.trim().to_string();
        if n.is_empty() {
            self.s.nick = self.nick_enviado.clone();
        } else {
            self.s.nick = n;
            // El servidor solo se entera del nombre nuevo con un `join` (o con el siguiente
            // mensaje); el historial que contesta se ignora (`historia_aplicada`).
            if self.s.nick != self.nick_enviado && self.conn == Conn::Conectado {
                self.enviar_join();
            }
        }
    }

    fn indicador(&self, ui: &mut Ui) {
        let (color, texto) = match self.conn {
            Conn::Conectado => (ACCENT_OK, "Conectado"),
            Conn::Conectando => (ACCENT, "Conectando..."),
            Conn::Error => (ACCENT_WARN, "Sin conexión"),
        };
        let mut a = 1.0;
        if self.conn == Conn::Conectando {
            // Pulso, como el amarillo del cliente Tauri.
            a = 0.6 + 0.4 * (ui.input(|i| i.time) as f32 * 4.0).sin();
            ui.ctx().request_repaint();
        }
        // Punto y texto en UN widget, para que envuelvan juntos.
        let mut j = LayoutJob::default();
        j.append("\u{f111}  ", 0.0, TextFormat { font_id: FontId::proportional(10.0), color: col_a(color, a), ..Default::default() });
        j.append(texto, 0.0, TextFormat { font_id: FontId::proportional(11.5), color: col(SUBTEXT), ..Default::default() });
        let r = ui.label(j);
        if self.conn == Conn::Error && !self.motivo.is_empty() {
            r.on_hover_text(format!("{} (reintentando)", self.motivo));
        }
    }

    fn usuarios_ui(&mut self, ctx: &Context) {
        egui::Area::new(Id::new("usuarios"))
            .order(Order::Foreground)
            .anchor(Align2::RIGHT_TOP, vec2(-14.0, 14.0))
            .show(ctx, |ui| {
                Self::isla().show(ui, |ui| {
                    ui.set_width(166.0);
                    ui.label(RichText::new(format!("USUARIOS ({})", self.usuarios.len().max(1))).small().color(col(SUBTEXT)));
                    ui.add_space(2.0);

                    // Yo: un clic cambia el estado.
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
                    let alfa = if resp.hovered() { 0.20 } else { 0.11 };
                    ui.painter().rect_filled(rect, Rounding::same(theme::RADIO_FILA), col_a(ACCENT, alfa));
                    let c = self.color_estado(&self.mi_estado);
                    punto_brillante(ui, pos2(rect.left() + 14.0, rect.center().y), c);
                    ui.painter().text(
                        pos2(rect.left() + 28.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        format!("{} (Tú)", self.s.nick),
                        FontId::proportional(14.0),
                        col(TEXT),
                    );
                    if resp.on_hover_text("Clic para cambiar de estado").on_hover_cursor(CursorIcon::PointingHand).clicked() {
                        self.mi_estado = if self.mi_estado == "online" { "busy".into() } else { "online".into() };
                        self.enviar(Out::Status { sender_id: self.s.id.clone(), status: self.mi_estado.clone() });
                    }

                    ScrollArea::vertical().id_salt("usuarios").max_height(240.0).show(ui, |ui| {
                        for u in self.usuarios.iter().filter(|u| u.id != self.s.id) {
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(vec2(14.0, 20.0), Sense::hover());
                                punto_brillante(ui, r.center(), self.color_estado(&u.status));
                                let c = parse_hex(&u.color).map(col).unwrap_or(col(TEXT));
                                ui.add(egui::Label::new(RichText::new(&u.nickname).color(c)).truncate());
                            });
                        }
                    });

                    // Los colores de los dos estados, como en el cliente Tauri.
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.color_edit_button_srgb(&mut self.s.online).on_hover_text("Color de «en línea»");
                        ui.color_edit_button_srgb(&mut self.s.busy).on_hover_text("Color de «ocupado»");
                        ui.label(RichText::new("estados").small().color(col(SUBTEXT)));
                    });
                });
            });
    }

    fn color_estado(&self, estado: &str) -> Color32 {
        col(if estado == "online" { self.s.online } else { self.s.busy })
    }

    fn chat_ui(&mut self, ctx: &Context) {
        let area = egui::Area::new(Id::new("chat")).order(Order::Foreground).anchor(Align2::LEFT_BOTTOM, vec2(14.0, -14.0));
        if self.s.chat_collapsed {
            area.show(ctx, |ui| {
                Self::isla().inner_margin(Margin::same(5.0)).show(ui, |ui| {
                    if boton_icono(ui, icono::CHAT, false, true, "Mostrar el chat").clicked() {
                        self.s.chat_collapsed = false;
                    }
                });
            });
            return;
        }
        area.show(ctx, |ui| {
            Self::isla().show(ui, |ui| {
                ui.set_width(310.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("CHAT").small().color(col(SUBTEXT)));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if boton_icono(ui, icono::MENOS, false, true, "Ocultar el chat").clicked() {
                            self.s.chat_collapsed = true;
                        }
                    });
                });
                ScrollArea::vertical().id_salt("chat").max_height(150.0).min_scrolled_height(150.0).stick_to_bottom(true).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("Bienvenido al Art-Chat").color(col(ACCENT)));
                    ui.add_space(4.0);
                    for l in &self.lineas {
                        ui.label(linea(l));
                    }
                });
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    let ancho = ui.available_width() - 2.0 * 30.0 - 2.0 * ui.spacing().item_spacing.x;
                    let campo = ui.add(TextEdit::singleline(&mut self.entrada).desired_width(ancho).hint_text("Escribe algo..."));
                    let enter = campo.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (boton_icono(ui, icono::ENVIAR, false, true, "Enviar").clicked() || enter) && !self.entrada.trim().is_empty() {
                        let texto = std::mem::take(&mut self.entrada).trim().to_string();
                        self.enviar(Out::Chat { sender_id: self.s.id.clone(), nickname: self.s.nick.clone(), content: texto.clone() });
                        self.lineas.push(Linea { tipo: Tipo::Yo, nick: "Yo".into(), texto });
                        campo.request_focus();
                    } else if enter {
                        campo.request_focus();
                    }
                    let libre = Instant::now() >= self.zumbido_libre;
                    let tip = if libre { "Enviar un zumbido" } else { "Espera..." };
                    if boton_icono(ui, icono::CAMPANA, false, libre, tip).clicked() && libre {
                        self.enviar(Out::Buzz { sender_id: self.s.id.clone(), nickname: self.s.nick.clone() });
                        self.lineas.push(Linea { tipo: Tipo::Sistema, nick: String::new(), texto: format!("{} Enviaste un zumbido", icono::CAMPANA) });
                        self.zumbido_libre = Instant::now() + ENFRIAR_ZUMBIDO;
                        ui.ctx().request_repaint_after(ENFRIAR_ZUMBIDO);
                    }
                });
            });
        });
    }

    /// El minimapa: todo el lienzo en pequeño con el recuadro de lo que se ve. Un clic o un
    /// arrastre centra la vista donde se toca.
    fn minimapa(&mut self, ctx: &Context) {
        let win = self.ventana;
        // En una ventana diminuta taparía el chat.
        if win.x < 560.0 {
            return;
        }
        // Lo que abarca: lo dibujado y lo que se ve ahora, con un poco de aire. Mientras se arrastra
        // en el minimapa se congela, o el mapa se re-escalaría bajo el dedo a cada movimiento.
        let visible = egui::Rect::from_min_size(Pos2::ZERO + self.vista, win);
        let ext = match self.mini_fijo {
            Some(r) if self.mini_arrastra => r,
            _ => self.lienzo.limites().map_or(visible, |l| l.union(visible)).expand(win.x.min(win.y) * 0.1),
        };
        self.mini_fijo = Some(ext);
        let k = (180.0 / ext.width()).min(120.0 / ext.height());
        let mut arrastra = false;
        egui::Area::new(Id::new("minimapa")).order(Order::Foreground).anchor(Align2::RIGHT_BOTTOM, vec2(-14.0, -14.0)).show(ctx, |ui| {
            Self::isla().inner_margin(Margin::same(6.0)).show(ui, |ui| {
                let (rect, resp) = ui.allocate_exact_size(ext.size() * k, Sense::click_and_drag());
                let p = ui.painter_at(rect);
                p.rect_filled(rect, Rounding::same(4.0), col(self.s.bg));
                // El (0,0) del lienzo cae a `-ext.min * k` de la esquina del minimapa.
                self.lienzo.paint(&p, rect.min - ext.min.to_vec2() * k, k);
                let visto = egui::Rect::from_min_size(rect.min + (visible.min - ext.min) * k, win * k);
                p.rect(visto, Rounding::same(2.0), col_a(ACCENT, 0.14), Stroke::new(1.0_f32, col(ACCENT)));
                if resp.is_pointer_button_down_on() {
                    arrastra = true;
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let mundo = ext.min + (pos - rect.min) / k;
                        self.vista = limita_vista(mundo.to_vec2() - win * 0.5);
                    }
                }
                resp.on_hover_text("Minimapa: clic o arrastre para moverte\nPanorámica: botón central, Espacio + arrastrar o la rueda")
                    .on_hover_cursor(CursorIcon::PointingHand);
            });
        });
        self.mini_arrastra = arrastra;
    }

    /// El zumbido sacude la interfaz medio segundo (el `buzz-shake` del cliente Tauri). Se
    /// sacude el contenido, no la ventana: con un gestor de ventanas en mosaico la ventana
    /// no se deja mover.
    fn sacudir(&mut self, ctx: &Context) {
        let ids = [Id::new("barra"), Id::new("usuarios"), Id::new("chat"), Id::new("minimapa")];
        let pon = |dx: f32| {
            let t = TSTransform::from_translation(vec2(dx, 0.0));
            ctx.set_transform_layer(LayerId::background(), t);
            for id in ids {
                ctx.set_transform_layer(LayerId::new(Order::Foreground, id), t);
            }
        };
        let Some(t0) = self.sacudida else { return };
        let t = t0.elapsed().as_secs_f32();
        if t < DURA_SACUDIDA {
            pon((t * 70.0).sin() * 10.0 * (1.0 - t / DURA_SACUDIDA));
            ctx.request_repaint();
        } else {
            pon(0.0);
            self.sacudida = None;
        }
    }

    // ---- ciclo ----------------------------------------------------------------------------

    /// Primera vez: ahora ya se conoce la densidad de la pantalla, y con ella la resolución del
    /// lienzo; si hay uno guardado de la sesión anterior se recupera.
    fn arranque(&mut self, ctx: &Context) {
        self.listo = true;
        let escala = if ctx.pixels_per_point() >= 1.5 { 2.0 } else { 1.0 };
        self.lienzo = Canvas::new(escala);
        if let Some(dir) = settings::dir_lienzo(escala) {
            self.lienzo.cargar(&dir);
        }
    }

    fn guarda_lienzo(&mut self, ahora_mismo: bool) {
        let Some(dir) = settings::dir_lienzo(self.lienzo.scale()) else { return };
        if !self.lienzo.hay_cambios() {
            return;
        }
        let foto = self.lienzo.foto();
        let escribe = move || foto.guardar(&dir);
        // En segundo plano mientras se dibuja; al cerrar hay que esperar.
        if ahora_mismo {
            escribe();
        } else {
            std::thread::spawn(escribe);
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.ui(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.s.save();
        self.guarda_lienzo(true);
    }
}

impl App {
    /// Un fotograma. No toca `eframe::Frame`, así que se puede ejecutar sin ventana.
    fn ui(&mut self, ctx: &Context) {
        if !self.listo {
            self.arranque(ctx);
        }
        self.red();

        // Red de seguridad del cliente Tauri: si el servidor no nos lista, repetir el `join`.
        if self.conn == Conn::Conectado && !self.usuarios.iter().any(|u| u.id == self.s.id) {
            if self.join_enviado.map_or(true, |t| t.elapsed() > Duration::from_secs(2)) {
                self.enviar_join();
            }
            ctx.request_repaint_after(Duration::from_millis(2100));
        }

        self.sacudir(ctx);

        egui::CentralPanel::default().frame(Frame::none().fill(col(BAR_BG))).show(ctx, |ui| self.lienzo_ui(ui));
        self.barra(ctx);
        self.usuarios_ui(ctx);
        self.chat_ui(ctx);
        self.minimapa(ctx);

        // Preferencias: a disco 0,4 s después del último cambio, no en cada fotograma.
        if self.s != self.guardado {
            let desde = *self.sucio_desde.get_or_insert_with(Instant::now);
            if desde.elapsed() > Duration::from_millis(400) {
                self.s.save();
                self.guardado = self.s.clone();
                self.sucio_desde = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(450));
            }
        }
        // El lienzo, cada 20 s si cambió y no se está dibujando.
        if self.lienzo_guardado.elapsed() > Duration::from_secs(20) {
            self.lienzo_guardado = Instant::now();
            if self.trazando.is_none() {
                self.guarda_lienzo(false);
            }
        }
    }
}

// ---- piezas -------------------------------------------------------------------------------

/// Botón cuadrado con un glifo de Nerd Font. `activo` lo pinta en ámbar (modo encendido).
fn boton_icono(ui: &mut Ui, glifo: &str, activo: bool, habilitado: bool, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 28.0), if habilitado { Sense::click() } else { Sense::hover() });
    let encima = resp.hovered() && habilitado;
    let (fondo, borde, tinta) = if activo {
        (col(ACCENT), col(ACCENT), col(BAR_BG))
    } else if encima {
        (Color32::from_rgb(48, 40, 34), col(ACCENT), col(TEXT))
    } else {
        (Color32::from_rgb(34, 28, 24), col(HIGHLIGHT), col(if habilitado { TEXT } else { SUBTEXT }))
    };
    let tinta = if habilitado { tinta } else { tinta.gamma_multiply(0.5) };
    ui.painter().rect(rect, Rounding::same(theme::RADIO_FILA), fondo, Stroke::new(1.0_f32, borde));
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, glifo, FontId::proportional(15.0), tinta);
    let r = resp.on_hover_text(tip);
    if habilitado {
        r.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        r
    }
}

/// Muestra de color rápida: un disco que, si es el actual, lleva un aro.
fn punto_color(ui: &mut Ui, c: [u8; 3], actual: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(20.0, 28.0), Sense::click());
    let radio = if resp.hovered() { 8.0 } else { 7.0 };
    ui.painter().circle_filled(rect.center(), radio, col(c));
    if actual {
        ui.painter().circle_stroke(rect.center(), 9.5, Stroke::new(1.5_f32, col(TEXT)));
    }
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// El punto de estado con su halo (el `box-shadow` del cliente Tauri).
fn punto_brillante(ui: &Ui, centro: Pos2, c: Color32) {
    ui.painter().circle_filled(centro, 7.0, c.gamma_multiply(0.22));
    ui.painter().circle_filled(centro, 4.0, c);
}

/// Una línea del chat: nick con su color y el texto, que pasa a la línea de abajo si no cabe.
fn linea(l: &Linea) -> LayoutJob {
    let f = FontId::proportional(14.0);
    let fmt = |c: [u8; 3]| TextFormat { font_id: f.clone(), color: col(c), ..Default::default() };
    let mut j = LayoutJob::default();
    match l.tipo {
        Tipo::Sistema => j.append(&l.texto, 0.0, fmt(ACCENT)),
        Tipo::Historial => {
            j.append(&format!("{}: ", l.nick), 0.0, fmt(SUBTEXT));
            j.append(&l.texto, 0.0, fmt(SUBTEXT));
        }
        Tipo::Yo | Tipo::Otro => {
            j.append(&format!("{}: ", l.nick), 0.0, fmt(if l.tipo == Tipo::Yo { ACCENT_OK } else { ACCENT }));
            j.append(&l.texto, 0.0, fmt(TEXT));
        }
    }
    j
}

/// La vista no sale del lienzo posible (±`LIM_PTS` en cada eje; sin borde práctico).
fn limita_vista(v: egui::Vec2) -> egui::Vec2 {
    let l = crate::canvas::LIM_PTS;
    vec2(v.x.clamp(-l, l), v.y.clamp(-l, l))
}

/// Lo inverso de `tiempo`, para teclear el valor en el deslizador: «3», «3 s», «1,5 min».
fn destiempo(t: &str) -> Option<f64> {
    let t = t.trim().replace(',', ".");
    let (num, unidad) = match t.find(|c: char| c.is_alphabetic()) {
        Some(i) => (t[..i].trim(), t[i..].trim()),
        None => (t.as_str(), "s"),
    };
    let n: f64 = num.parse().ok()?;
    let k = match unidad {
        "s" => 1000.0,
        "min" | "m" => 60_000.0,
        "h" => 3_600_000.0,
        _ => return None,
    };
    Some(n * k)
}

/// 500 -> «0,5 s», 3000 -> «3 s», 90000 -> «1,5 min».
fn tiempo(ms: f32) -> String {
    let s = ms / 1000.0;
    if s < 1.0 {
        format!("{s:.1} s").replace('.', ",")
    } else if s < 60.0 {
        format!("{s:.0} s")
    } else if s < 3600.0 {
        format!("{:.1} min", s / 60.0).replace('.', ",")
    } else {
        format!("{:.1} h", s / 3600.0).replace('.', ",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event as Ev, PointerButton, RawInput, Rect};

    /// Un fotograma sin ventana, con estos eventos de puntero.
    fn fotograma(ctx: &Context, app: &mut App, events: Vec<Ev>) {
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))), events, ..Default::default() };
        let _ = ctx.run(raw, |c| app.ui(c));
    }

    /// Dibujar con el ratón pinta en el lienzo y manda un `draw` por segmento al servidor.
    ///   ARTCHAT_TEST_URL=ws://127.0.0.1:18480 cargo test -- --ignored
    #[test]
    #[ignore]
    fn dibujar_con_el_raton_pinta_y_manda_trazos() {
        let url = std::env::var("ARTCHAT_TEST_URL").expect("falta ARTCHAT_TEST_URL");
        // Sin tocar el lienzo ni las preferencias guardadas de quien ejecute la prueba.
        std::env::set_var("APPDATA", std::env::temp_dir().join("artchat-prueba"));
        let oyente = network::spawn(url.clone(), || {});
        let limite = Instant::now() + Duration::from_secs(5);
        while !matches!(oyente.rx.recv_timeout(Duration::from_millis(200)), Ok(Event::Connected)) {
            assert!(Instant::now() < limite, "el oyente no conecta");
        }
        let _ = oyente.tx.send(Out::Join { sender_id: "oyente".into(), nickname: "Oyente".into(), color: "#fff".into() });

        let ctx = Context::default();
        let mut app = App::con(&ctx, Settings::default(), url);
        while app.conn != Conn::Conectado {
            fotograma(&ctx, &mut app, vec![]);
            assert!(Instant::now() < limite + Duration::from_secs(5), "la app no conecta");
            std::thread::sleep(Duration::from_millis(30));
        }

        // egui decide qué está bajo el puntero con los widgets del fotograma ANTERIOR: primero
        // se mueve el puntero y se deja pasar un fotograma, y recién entonces se pulsa.
        let (a, mut p) = (pos2(600.0, 400.0), pos2(600.0, 400.0));
        fotograma(&ctx, &mut app, vec![Ev::PointerMoved(a)]);
        fotograma(&ctx, &mut app, vec![]);
        let boton = |pressed| Ev::PointerButton { pos: a, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        fotograma(&ctx, &mut app, vec![boton(true)]);
        for _ in 0..10 {
            p += vec2(12.0, 5.0);
            fotograma(&ctx, &mut app, vec![Ev::PointerMoved(p)]);
        }
        fotograma(&ctx, &mut app, vec![Ev::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Default::default() }]);

        // en el lienzo: tinta ámbar donde se pulsó y a mitad de camino
        let tinta = |x: i32, y: i32| app.lienzo.pixel(x, y);
        assert_eq!(tinta(600, 400), col(ACCENT), "no hay tinta donde se pulsó");
        assert!(tinta(660, 425).a() > 200, "no hay tinta a mitad del trazo");
        assert_eq!(tinta(600, 460).a(), 0, "hay tinta donde no se pasó");

        // y en el servidor: el punto de la pulsación y 10 segmentos, con el formato del cliente Tauri
        let mut recibidos = Vec::new();
        while let Ok(e) = oyente.rx.recv_timeout(Duration::from_millis(800)) {
            if let Event::Msg(In::Draw { sender_id, seg }) = e {
                assert_eq!(sender_id, app.s.id);
                recibidos.push(seg);
            }
        }
        assert_eq!(recibidos.len(), 11, "esperaba 1 punto + 10 segmentos: {recibidos:?}");
        let (primero, ultimo) = (&recibidos[0], recibidos.last().unwrap());
        assert_eq!((primero.x0, primero.y0, primero.x1, primero.y1), (600.0, 400.0, 600.0, 400.0));
        assert_eq!((ultimo.x1, ultimo.y1), (p.x, p.y));
        assert!(recibidos.iter().all(|s| !s.ephemeral && !s.erase && s.size == Some(5.0) && s.color.as_deref() == Some("#e0a35c")));
    }

    fn app_sin_red() -> (Context, App) {
        std::env::set_var("APPDATA", std::env::temp_dir().join("artchat-prueba-pan"));
        let ctx = Context::default();
        let app = App::con(&ctx, Settings::default(), "ws://127.0.0.1:1".into());
        (ctx, app)
    }

    /// El botón central arrastra la vista (y no dibuja); lo dibujado después sale en
    /// coordenadas del lienzo, no de la ventana.
    #[test]
    fn el_panoramico_mueve_la_vista_y_los_trazos_salen_en_coordenadas_del_lienzo() {
        let (ctx, mut app) = app_sin_red();
        let a = pos2(600.0, 400.0);
        fotograma(&ctx, &mut app, vec![Ev::PointerMoved(a)]);
        fotograma(&ctx, &mut app, vec![]);
        let medio = |p, pressed| Ev::PointerButton { pos: p, button: PointerButton::Middle, pressed, modifiers: Default::default() };
        fotograma(&ctx, &mut app, vec![medio(a, true)]);
        // arrastrar 100 a la izquierda y 50 hacia arriba: la vista avanza 100 y 50
        fotograma(&ctx, &mut app, vec![Ev::PointerMoved(pos2(500.0, 350.0))]);
        fotograma(&ctx, &mut app, vec![medio(pos2(500.0, 350.0), false)]);
        assert_eq!(app.vista, vec2(100.0, 50.0));
        assert!(app.lienzo.pixel(600, 400).a() == 0, "el panorámico no debe pintar");

        // ahora un clic con el pincel en (300,200) de la pantalla = (400,250) del lienzo
        let p = pos2(300.0, 200.0);
        fotograma(&ctx, &mut app, vec![Ev::PointerMoved(p)]);
        fotograma(&ctx, &mut app, vec![]);
        let izq = |pressed| Ev::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        fotograma(&ctx, &mut app, vec![izq(true)]);
        fotograma(&ctx, &mut app, vec![izq(false)]);
        assert_eq!(app.lienzo.pixel(400, 250), col(ACCENT));
        assert_eq!(app.lienzo.pixel(300, 200).a(), 0);
    }

    #[test]
    fn la_vista_no_tiene_borde_salvo_el_limite_de_precision() {
        let l = crate::canvas::LIM_PTS;
        assert_eq!(limita_vista(vec2(-50.0, 10.0)), vec2(-50.0, 10.0)); // hacia atrás del origen sí
        assert_eq!(limita_vista(vec2(-9e9, 9e9)), vec2(-l, l));
    }

    /// Se puede ir al otro lado del origen y dibujar allí: el trazo se guarda con coordenadas
    /// negativas, que es lo que verá cualquier otro cliente.
    #[test]
    fn se_dibuja_a_la_izquierda_y_arriba_del_origen() {
        let (ctx, mut app) = app_sin_red();
        let a = pos2(600.0, 400.0);
        fotograma(&ctx, &mut app, vec![Ev::PointerMoved(a)]);
        fotograma(&ctx, &mut app, vec![]);
        let medio = |p, pressed| Ev::PointerButton { pos: p, button: PointerButton::Middle, pressed, modifiers: Default::default() };
        fotograma(&ctx, &mut app, vec![medio(a, true)]);
        fotograma(&ctx, &mut app, vec![Ev::PointerMoved(pos2(900.0, 700.0))]); // arrastra 300 a la derecha y abajo
        fotograma(&ctx, &mut app, vec![medio(pos2(900.0, 700.0), false)]);
        assert_eq!(app.vista, vec2(-300.0, -300.0));

        let p = pos2(100.0, 100.0); // pantalla -> lienzo (-200,-200)
        fotograma(&ctx, &mut app, vec![Ev::PointerMoved(p)]);
        fotograma(&ctx, &mut app, vec![]);
        let izq = |pressed| Ev::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        fotograma(&ctx, &mut app, vec![izq(true)]);
        fotograma(&ctx, &mut app, vec![izq(false)]);
        assert_eq!(app.lienzo.pixel(-200, -200), col(ACCENT));
    }

    #[test]
    fn el_tiempo_de_desvanecido_se_lee_bien() {
        assert_eq!(tiempo(500.0), "0,5 s");
        assert_eq!(tiempo(3000.0), "3 s");
        assert_eq!(tiempo(90_000.0), "1,5 min");
        assert_eq!(tiempo(360_000.0), "6,0 min");
        // y se puede teclear lo mismo que se lee
        assert_eq!(destiempo("3"), Some(3000.0));
        assert_eq!(destiempo("0,5 s"), Some(500.0));
        assert_eq!(destiempo("1,5 min"), Some(90_000.0));
        assert_eq!(destiempo("2h"), Some(7_200_000.0));
        assert_eq!(destiempo("x"), None);
    }
}
