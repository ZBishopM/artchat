//! Los tres sonidos del cliente Tauri (aviso de chat, alguien entra, zumbido), sin traer un
//! motor de audio.
//!
//! Windows ya sabe reproducir MP3 con MCI (`winmm`), así que basta soltar los archivos
//! incrustados en una carpeta temporal y pedírselos. Un `rodio` + `symphonia` para tres
//! sonidos de 60 KB en total habría duplicado el tiempo de compilación y sumado megas al
//! `.exe`. Cualquier fallo se ignora: un sonido que no suena no es motivo para tumbar nada.

use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq)]
pub enum Sonido {
    Aviso,
    Entra,
    Zumbido,
}

impl Sonido {
    fn alias(self) -> &'static str {
        match self {
            Sonido::Aviso => "artchat_aviso",
            Sonido::Entra => "artchat_entra",
            Sonido::Zumbido => "artchat_zumbido",
        }
    }
    fn archivo(self) -> &'static str {
        match self {
            Sonido::Aviso => "notify.mp3",
            Sonido::Entra => "connected.mp3",
            Sonido::Zumbido => "buzz.mp3",
        }
    }
    fn bytes(self) -> &'static [u8] {
        match self {
            Sonido::Aviso => include_bytes!("../assets/notify.mp3"),
            Sonido::Entra => include_bytes!("../assets/connected.mp3"),
            Sonido::Zumbido => include_bytes!("../assets/buzz.mp3"),
        }
    }
    /// Volumen de cada uno, de 0 a 1000 (el cliente Tauri usaba 0,5 / 0,5 / 0,7).
    fn volumen(self) -> u32 {
        match self {
            Sonido::Zumbido => 700,
            _ => 500,
        }
    }
}

#[derive(Default)]
pub struct Sfx {
    abiertos: Vec<Sonido>,
}

impl Sfx {
    pub fn play(&mut self, s: Sonido) {
        #[cfg(windows)]
        {
            if !self.abiertos.contains(&s) {
                let dir: PathBuf = std::env::temp_dir().join("ArtChat");
                let ruta = dir.join(s.archivo());
                let _ = std::fs::create_dir_all(&dir);
                if std::fs::metadata(&ruta).map(|m| m.len()).unwrap_or(0) != s.bytes().len() as u64 {
                    let _ = std::fs::write(&ruta, s.bytes());
                }
                let a = s.alias();
                mci(&format!("open \"{}\" type mpegvideo alias {a}", ruta.display()));
                mci(&format!("setaudio {a} volume to {}", s.volumen()));
                self.abiertos.push(s);
            }
            mci(&format!("play {} from 0", s.alias()));
        }
        #[cfg(not(windows))]
        let _ = s;
    }
}

#[cfg(windows)]
fn mci(cmd: &str) {
    #[link(name = "winmm")]
    extern "system" {
        fn mciSendStringW(cmd: *const u16, ret: *mut u16, len: u32, callback: isize) -> u32;
    }
    let w: Vec<u16> = cmd.encode_utf16().chain(Some(0)).collect();
    unsafe {
        mciSendStringW(w.as_ptr(), std::ptr::null_mut(), 0, 0);
    }
}
