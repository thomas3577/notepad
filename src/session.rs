//! Open tabs (including unsaved text) restored on the next start, like Windows Notepad.
//!
//! Stored in `%LOCALAPPDATA%\Notepad\session.ron`, written crash-safe with `write_atomic`:
//! for unsaved text this file is the only copy.

use crate::encoding::Enc;
use crate::files::write_atomic;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Default, PartialEq, Debug)]
pub struct Session {
    pub tabs: Vec<SavedTab>,
    pub active: usize,
    pub zoom: f32,
    pub no_wrap: bool,
    /// Most recent first. (serde default: sessions from older versions lack the newer fields)
    #[serde(default)]
    pub recent: Vec<PathBuf>,
    #[serde(default)]
    pub theme: eframe::egui::ThemePreference,
    #[serde(default)]
    pub hide_status_bar: bool,
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub struct SavedTab {
    pub path: Option<PathBuf>,
    pub enc: Enc,
    pub crlf: bool,
    /// Unsaved text (changed or Untitled tab). None: unchanged file, reloaded from disk.
    pub text: Option<String>,
}

/// Folder for the session (and eframe's window position): %LOCALAPPDATA%\Notepad.
pub fn dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Notepad"))
}

fn file() -> Option<PathBuf> {
    dir().map(|d| d.join("session.ron"))
}

/// None if there is no (readable) session.
pub fn load() -> Option<Session> {
    ron::from_str(&std::fs::read_to_string(file()?).ok()?).ok()
}

/// Serialized form, so callers can skip writing an unchanged session.
pub fn to_string(s: &Session) -> String {
    ron::to_string(s).expect("session is serializable")
}

pub fn store(serialized: &str) -> std::io::Result<()> {
    let file = file().ok_or_else(|| std::io::Error::other("LOCALAPPDATA is not set"))?;
    std::fs::create_dir_all(file.parent().expect("has parent"))?;
    write_atomic(&file, serialized.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let s = Session {
            tabs: vec![
                SavedTab { path: Some("C:\\a.txt".into()), enc: Enc::Win1252, crlf: true, text: None },
                SavedTab { path: None, enc: Enc::Utf8, crlf: false, text: Some("Grüße \"quoted\"\n😀".into()) },
            ],
            active: 1,
            zoom: 1.5,
            no_wrap: true,
            recent: vec![r"C:\b.txt".into()],
            theme: eframe::egui::ThemePreference::Dark,
            hide_status_bar: true,
        };
        assert_eq!(ron::from_str::<Session>(&to_string(&s)).unwrap(), s);
    }

    #[test]
    fn reads_session_of_older_version() {
        let s: Session = ron::from_str("(tabs:[],active:0,zoom:1.0,no_wrap:false)").unwrap();
        assert!(s.recent.is_empty());
    }
}
