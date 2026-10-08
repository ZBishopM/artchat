//! Preferencias en `%APPDATA%\ArtChat\settings.json` (lo que el cliente Tauri guardaba en
//! `localStorage`), y la ruta del lienzo guardado.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::theme;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub id: String,
    pub nick: String,
    pub sound: bool,
    pub color: [u8; 3],
    pub bg: [u8; 3],
    pub size: f32,
    pub fade: bool,
    /// Lo que tarda un trazo efímero en desvanecerse.
    pub fade_ms: f32,
    pub online: [u8; 3],
    pub busy: [u8; 3],
    pub chat_collapsed: bool,
}

/// Un u64 distinto en cada ejecución, sin traer `rand`: `RandomState` se siembra desde el
/// sistema operativo.
pub fn azar() -> u64 {
    RandomState::new().build_hasher().finish()
}

impl Default for Settings {
    fn default() -> Self {
        let n = azar();
        Self {
            id: format!("{:09x}", n & 0xf_ffff_ffff),
            nick: format!("Artista_{}", 100 + (n >> 40) % 900),
            sound: true,
            color: theme::ACCENT,
            bg: theme::BAR_BG,
            size: 5.0,
            fade: false,
            fade_ms: 3000.0,
            online: theme::ACCENT_OK,
            busy: theme::ACCENT_WARN,
            chat_collapsed: false,
        }
    }
}

fn dir() -> Option<PathBuf> {
    let d = PathBuf::from(std::env::var_os("APPDATA")?).join("ArtChat");
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

impl Settings {
    pub fn load() -> Self {
        dir()
            .and_then(|d| std::fs::read_to_string(d.join("settings.json")).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let (Some(d), Ok(t)) = (dir(), serde_json::to_string_pretty(self)) {
            let tmp = d.join("settings.json.tmp");
            if std::fs::write(&tmp, t).is_ok() {
                let _ = std::fs::rename(tmp, d.join("settings.json"));
            }
        }
    }
}

/// El lienzo guardado. La escala va en el nombre: un PNG hecho a 2 texels por punto no sirve
/// en una pantalla de 1.
pub fn ruta_lienzo(escala: f32) -> Option<PathBuf> {
    Some(dir()?.join(format!("lienzo-x{}.png", escala as u32)))
}
