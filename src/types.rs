//! El protocolo, tal cual lo habla `art-chat-server/chat-server.js` y el cliente Tauri.
//!
//! La primera versión de este cliente inventó otro (`stroke.points`), y así no entiende al
//! servidor ni a los demás clientes. Aquí no se inventa nada: cada campo sale del servidor
//! (`msg.x0`, `msg.erase`, `msg.senderId`...) o de `+page.svelte`.

use serde::{Deserialize, Serialize};

/// Un segmento de trazo. Es lo que viaja en `draw` y lo que guarda el historial del servidor
/// (`x0 y0 x1 y1 color size erase`; los efímeros no se guardan).
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct Seg {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub size: Option<f32>,
    #[serde(default)]
    pub erase: bool,
    #[serde(default)]
    pub ephemeral: bool,
    /// Milisegundos que tarda en desvanecerse un trazo efímero.
    #[serde(default)]
    pub duration: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct User {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub nickname: String,
    #[serde(default = "color_blanco")]
    pub color: String,
    #[serde(default = "online")]
    pub status: String,
}

fn color_blanco() -> String {
    "#ffffff".into()
}
fn online() -> String {
    "online".into()
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HistMsg {
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub content: String,
}

/// Lo que llega del servidor. Todo lo desconocido cae en `Other`: el servidor reenvía "tal
/// cual" cualquier tipo que no conoce, y un cliente nuevo no debe romperse por eso.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type")]
pub enum In {
    #[serde(rename = "history")]
    History {
        #[serde(default)]
        strokes: Vec<Seg>,
        #[serde(default)]
        messages: Vec<HistMsg>,
    },
    #[serde(rename = "users_update")]
    UsersUpdate {
        #[serde(default)]
        users: Vec<User>,
    },
    #[serde(rename = "draw")]
    Draw {
        #[serde(default, rename = "senderId")]
        sender_id: String,
        #[serde(flatten)]
        seg: Seg,
    },
    #[serde(rename = "chat")]
    Chat {
        #[serde(default, rename = "senderId")]
        sender_id: String,
        #[serde(default)]
        nickname: Option<String>,
        #[serde(default)]
        content: String,
    },
    #[serde(rename = "clear")]
    Clear {
        #[serde(default, rename = "senderId")]
        sender_id: String,
    },
    #[serde(rename = "buzz")]
    Buzz {
        #[serde(default, rename = "senderId")]
        sender_id: String,
        #[serde(default)]
        nickname: Option<String>,
    },
    #[serde(other)]
    Other,
}

/// Lo que se manda. Los nombres de campo son los del cliente Tauri.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type")]
pub enum Out {
    #[serde(rename = "join")]
    Join {
        #[serde(rename = "senderId")]
        sender_id: String,
        nickname: String,
        color: String,
    },
    #[serde(rename = "status")]
    Status {
        #[serde(rename = "senderId")]
        sender_id: String,
        status: String,
    },
    #[serde(rename = "draw")]
    Draw {
        #[serde(rename = "senderId")]
        sender_id: String,
        #[serde(flatten)]
        seg: Seg,
    },
    #[serde(rename = "chat")]
    Chat {
        #[serde(rename = "senderId")]
        sender_id: String,
        nickname: String,
        content: String,
    },
    #[serde(rename = "buzz")]
    Buzz {
        #[serde(rename = "senderId")]
        sender_id: String,
        nickname: String,
    },
    #[serde(rename = "clear")]
    Clear {
        #[serde(rename = "senderId")]
        sender_id: String,
    },
}

/// `#rgb` o `#rrggbb` -> bytes. `None` si no es un color.
pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    let p = |a: &str| u8::from_str_radix(a, 16).ok();
    match h.len() {
        6 => Some([p(&h[0..2])?, p(&h[2..4])?, p(&h[4..6])?]),
        3 => {
            let d = |i: usize| p(&h[i..i + 1]).map(|v| v * 17);
            Some([d(0)?, d(1)?, d(2)?])
        }
        _ => None,
    }
}

pub fn to_hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entra(v: serde_json::Value) -> In {
        serde_json::from_value(v).expect("el servidor manda esto")
    }

    #[test]
    fn historial_como_lo_manda_el_servidor() {
        // `sendHistory`: strokes guardados por `addStroke` (sin senderId) y mensajes con hora.
        let m = entra(json!({
            "type": "history",
            "strokes": [{"x0": 10, "y0": 20.5, "x1": 11, "y1": 22, "color": "#ff0000", "size": 5, "erase": false}],
            "messages": [{"nickname": "Ana", "content": "hola", "timestamp": 1760000000000i64}]
        }));
        let In::History { strokes, messages } = m else { panic!("no es history") };
        assert_eq!(strokes[0].x1, 11.0);
        assert_eq!(strokes[0].color.as_deref(), Some("#ff0000"));
        assert_eq!(messages[0].nickname, "Ana");
    }

    #[test]
    fn trazo_de_otro_cliente() {
        let m = entra(json!({
            "type": "draw", "senderId": "abc123", "x0": 1, "y0": 2, "x1": 3, "y1": 4,
            "color": "#00ff00", "size": 8, "ephemeral": true, "duration": 1500, "erase": false
        }));
        let In::Draw { sender_id, seg } = m else { panic!("no es draw") };
        assert_eq!(sender_id, "abc123");
        assert!(seg.ephemeral);
        assert_eq!(seg.duration, Some(1500.0));
    }

    #[test]
    fn usuarios_chat_clear_buzz_y_lo_desconocido() {
        let u = entra(json!({"type": "users_update", "users": [
            {"id": "a", "nickname": "Ana", "status": "busy", "color": "#e0a35c"}, {"nickname": "sin id"}]}));
        let In::UsersUpdate { users } = u else { panic!() };
        assert_eq!(users[0].status, "busy");
        assert_eq!(users[1].status, "online"); // por defecto
        assert!(matches!(entra(json!({"type": "chat", "senderId": "a", "nickname": "Ana", "content": "x"})), In::Chat { .. }));
        assert!(matches!(entra(json!({"type": "clear", "senderId": "a"})), In::Clear { .. }));
        assert!(matches!(entra(json!({"type": "buzz", "senderId": "a", "nickname": "Ana"})), In::Buzz { .. }));
        assert_eq!(entra(json!({"type": "algo-nuevo", "x": 1})), In::Other);
    }

    #[test]
    fn lo_que_se_manda_lleva_los_nombres_del_cliente_tauri() {
        let j = serde_json::to_value(Out::Join { sender_id: "id1".into(), nickname: "Ana".into(), color: "#fff".into() }).unwrap();
        assert_eq!(j, json!({"type": "join", "senderId": "id1", "nickname": "Ana", "color": "#fff"}));

        let d = serde_json::to_value(Out::Draw {
            sender_id: "id1".into(),
            seg: Seg { x0: 1.0, y0: 2.0, x1: 3.0, y1: 4.0, color: Some("#fff".into()), size: Some(5.0), erase: true, ephemeral: false, duration: Some(0.0) },
        })
        .unwrap();
        for k in ["type", "senderId", "x0", "y0", "x1", "y1", "color", "size", "ephemeral", "duration", "erase"] {
            assert!(d.get(k).is_some(), "falta {k}: {d}");
        }
        assert_eq!(d["erase"], json!(true));
    }

    #[test]
    fn colores() {
        assert_eq!(parse_hex("#e0a35c"), Some([224, 163, 92]));
        assert_eq!(parse_hex("fff"), Some([255, 255, 255]));
        assert_eq!(parse_hex("#12"), None);
        assert_eq!(to_hex([224, 163, 92]), "#e0a35c");
    }
}
