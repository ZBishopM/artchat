//! La conexión con el servidor, en su propio hilo con un runtime de tokio.
//!
//! Dos cosas que el cliente Tauri no hacía y aquí sí:
//!  - **Reconecta.** Aquel se conectaba una vez al arrancar; si el servidor reiniciaba o
//!    nginx cerraba el socket por inactividad, había que cerrar y abrir la aplicación.
//!  - **No sondea.** La primera versión de este cliente miraba la cola cada 5 ms. Ahora el
//!    hilo duerme hasta que llega algo, de cualquiera de los dos lados, y despierta a la
//!    interfaz con `wake` solo cuando hay algo que enseñar.

use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::types::{In, Out};

#[derive(Debug, Clone)]
pub enum Event {
    Connecting,
    Connected,
    /// Se cayó (o no llegó a conectar). Va con el motivo; el hilo ya está reintentando.
    Down(String),
    Msg(In),
}

pub struct Net {
    pub tx: UnboundedSender<Out>,
    pub rx: Receiver<Event>,
}

/// nginx cierra un WebSocket que no dice nada durante 60 s (`proxy_read_timeout`). Un ping del
/// cliente lo contesta el servidor con un pong, que cuenta como tráfico.
const PING: Duration = Duration::from_secs(25);
const ESPERA_MIN: Duration = Duration::from_secs(1);
const ESPERA_MAX: Duration = Duration::from_secs(10);

pub fn spawn(url: String, wake: impl Fn() + Send + 'static) -> Net {
    let (tx, mut out_rx) = unbounded_channel::<Out>();
    let (ev_tx, rx) = channel::<Event>();

    std::thread::Builder::new()
        .name("artchat-red".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime de tokio");
            rt.block_on(async move {
                let emit = |e: Event| {
                    let ok = ev_tx.send(e).is_ok();
                    wake();
                    ok
                };
                let mut espera = ESPERA_MIN;
                loop {
                    if !emit(Event::Connecting) {
                        return; // la interfaz ya no existe
                    }
                    // Lo que se encoló mientras no había conexión es viejo: al reconectar,
                    // `join` y el historial dejan a todos al día.
                    while out_rx.try_recv().is_ok() {}

                    let motivo = match connect_async(url.as_str()).await {
                        Err(e) => format!("no conecta: {e}"),
                        Ok((ws, _)) => {
                            espera = ESPERA_MIN;
                            if !emit(Event::Connected) {
                                return;
                            }
                            let (mut write, mut read) = ws.split();
                            let mut ping = tokio::time::interval(PING);
                            ping.tick().await; // el primero sale al instante
                            let fin = loop {
                                tokio::select! {
                                    entra = read.next() => match entra {
                                        Some(Ok(Message::Text(t))) => {
                                            if let Ok(m) = serde_json::from_str::<In>(&t) {
                                                if !emit(Event::Msg(m)) {
                                                    return;
                                                }
                                            }
                                        }
                                        Some(Ok(Message::Close(_))) | None => break "el servidor cerró la conexión".to_string(),
                                        Some(Err(e)) => break format!("error en el socket: {e}"),
                                        Some(Ok(_)) => {}
                                    },
                                    sale = out_rx.recv() => match sale {
                                        Some(m) => {
                                            if let Ok(s) = serde_json::to_string(&m) {
                                                if let Err(e) = write.send(Message::Text(s.into())).await {
                                                    break format!("no se pudo enviar: {e}");
                                                }
                                            }
                                        }
                                        None => return, // se soltó el emisor: la aplicación terminó
                                    },
                                    _ = ping.tick() => {
                                        if let Err(e) = write.send(Message::Ping(Vec::new().into())).await {
                                            break format!("no se pudo enviar: {e}");
                                        }
                                    }
                                }
                            };
                            fin
                        }
                    };
                    if !emit(Event::Down(motivo)) {
                        return;
                    }
                    tokio::time::sleep(espera).await;
                    espera = (espera * 2).min(ESPERA_MAX);
                }
            });
        })
        .expect("hilo de red");

    Net { tx, rx }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Seg;
    use std::time::Instant;

    /// El primer evento que cumpla `f`, o `None` a los 6 s.
    fn espera(n: &Net, f: impl Fn(&Event) -> bool) -> Option<Event> {
        let limite = Instant::now() + Duration::from_secs(6);
        while let Some(resto) = limite.checked_duration_since(Instant::now()) {
            match n.rx.recv_timeout(resto) {
                Ok(e) if f(&e) => return Some(e),
                Ok(_) => {}
                Err(_) => return None,
            }
        }
        None
    }

    fn entra(url: &str, id: &str) -> Net {
        let n = spawn(url.to_string(), || {});
        espera(&n, |e| matches!(e, Event::Connected)).expect("conecta");
        let _ = n.tx.send(Out::Join { sender_id: id.into(), nickname: id.into(), color: "#e0a35c".into() });
        n
    }

    fn historial(n: &Net) -> (Vec<Seg>, Vec<crate::types::HistMsg>) {
        match espera(n, |e| matches!(e, Event::Msg(In::History { .. }))) {
            Some(Event::Msg(In::History { strokes, messages })) => (strokes, messages),
            _ => panic!("no llegó el historial"),
        }
    }

    /// Contra un servidor de verdad (`node art-chat-server/chat-server.js`, con `history.json` limpio):
    ///   ARTCHAT_TEST_URL=ws://127.0.0.1:8099 cargo test -- --ignored
    #[test]
    #[ignore]
    fn dos_clientes_y_uno_tardio_contra_el_servidor() {
        let url = std::env::var("ARTCHAT_TEST_URL").expect("falta ARTCHAT_TEST_URL");
        let a = entra(&url, "aaa");
        historial(&a);
        let b = entra(&url, "bbb");
        historial(&b);
        // `users_update` con los dos
        let Some(Event::Msg(In::UsersUpdate { users })) = espera(&a, |e| matches!(e, Event::Msg(In::UsersUpdate { users }) if users.len() == 2)) else {
            panic!("A no ve a B")
        };
        assert!(users.iter().any(|u| u.id == "bbb" && u.status == "online"));

        // un trazo permanente, uno efímero y un mensaje de A
        let seg = |x: f32, ef: bool| Seg { x0: x, y0: 2.0, x1: x + 1.0, y1: 4.0, color: Some("#e0a35c".into()), size: Some(6.0), erase: false, ephemeral: ef, duration: Some(if ef { 800.0 } else { 0.0 }) };
        a.tx.send(Out::Draw { sender_id: "aaa".into(), seg: seg(10.0, false) }).unwrap();
        a.tx.send(Out::Draw { sender_id: "aaa".into(), seg: seg(50.0, true) }).unwrap();
        a.tx.send(Out::Chat { sender_id: "aaa".into(), nickname: "Ana".into(), content: "hola desde egui".into() }).unwrap();

        let Some(Event::Msg(In::Draw { sender_id, seg: v })) = espera(&b, |e| matches!(e, Event::Msg(In::Draw { .. }))) else { panic!("B no ve el trazo") };
        assert_eq!((sender_id.as_str(), v.x0, v.x1, v.color.as_deref()), ("aaa", 10.0, 11.0, Some("#e0a35c")));
        let Some(Event::Msg(In::Draw { seg: v, .. })) = espera(&b, |e| matches!(e, Event::Msg(In::Draw { .. }))) else { panic!("B no ve el efímero") };
        assert!(v.ephemeral && v.duration == Some(800.0));
        let Some(Event::Msg(In::Chat { content, nickname, .. })) = espera(&b, |e| matches!(e, Event::Msg(In::Chat { .. }))) else { panic!("B no ve el chat") };
        assert_eq!((content.as_str(), nickname.as_deref()), ("hola desde egui", Some("Ana")));

        // el servidor no devuelve a quien manda: A no recibe su propio trazo
        assert!(espera(&a, |e| matches!(e, Event::Msg(In::Draw { .. }))).is_none());

        // uno que llega tarde recibe lo permanente y lo dicho, y no lo efímero
        let c = entra(&url, "ccc");
        let (trazos, mensajes) = historial(&c);
        assert!(trazos.iter().any(|s| s.x0 == 10.0), "falta el trazo permanente: {trazos:?}");
        assert!(trazos.iter().all(|s| s.x0 != 50.0), "el efímero no se guarda");
        assert!(mensajes.iter().any(|m| m.content == "hola desde egui"));

        // clear: llega a los demás y vacía el historial
        a.tx.send(Out::Clear { sender_id: "aaa".into() }).unwrap();
        assert!(espera(&b, |e| matches!(e, Event::Msg(In::Clear { .. }))).is_some(), "B no ve el clear");
        let d = entra(&url, "ddd");
        assert!(historial(&d).0.is_empty(), "el servidor no vació el historial");
    }
}
