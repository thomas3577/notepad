//! One open document (one tab): text, file, encoding, line endings, load and save.

use crate::dialogs::{Parent, ask, error, file_dialog, warn};
use crate::encoding::Enc;
use crate::files::write_atomic;
use eframe::egui;
use std::borrow::Cow;
use std::path::PathBuf;

/// egui keeps far more memory per character than the file needs (layout data for every glyph).
/// Measured: a 5.6 MB file needs about 1.3 GB.
const MEMORY_PER_TEXT_BYTE: u64 = 240;
/// Ask before opening files whose estimated memory use exceeds this.
const LARGE_FILE_BYTES: u64 = 10 * 1024 * 1024;

/// Line ending for new documents: CRLF on Windows, like Windows Notepad.
const NEW_DOC_CRLF: bool = cfg!(windows);

pub struct Doc {
    /// Unique per tab; keys egui's editor state (undo, cursor) and scroll position.
    pub id: u64,
    pub text: String,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub enc: Enc,
    pub crlf: bool,
    /// Needs a fresh egui editor state (undo history sized for this text, scroll to top).
    pub reset_editor: bool,
    /// Char index to scroll into view on the next frame (find).
    pub scroll_to: Option<usize>,
}

impl Doc {
    /// Empty document; with a path it will be saved there (file name that doesn't exist yet).
    pub fn new(id: u64, path: Option<PathBuf>) -> Self {
        Doc {
            id,
            text: String::new(),
            path,
            dirty: false,
            enc: Enc::default(),
            crlf: NEW_DOC_CRLF,
            reset_editor: true,
            scroll_to: None,
        }
    }

    /// Read a file. None if it can't be opened or the user declined (errors are shown).
    pub fn load(id: u64, path: PathBuf, p: Parent) -> Option<Doc> {
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            // Like Windows Notepad: a new document that will be saved under this name.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(Doc::new(id, Some(path))),
            Err(e) => {
                error(p, format!("Could not open file: {e}"));
                return None;
            }
        };
        // Devices (CON), pipes and folders would hang or fail on read.
        if !meta.is_file() {
            error(p, format!("Not a regular file: {}", path.display()));
            return None;
        }
        if meta.len() > LARGE_FILE_BYTES {
            let mb = meta.len() as f64 / 1_048_576.0;
            let gb = (meta.len() * MEMORY_PER_TEXT_BYTE) as f64 / 1_073_741_824.0;
            if !ask(p, &format!("This file is {mb:.0} MB. Opening it will use about {gb:.1} GB of memory.\nOpen anyway?")) {
                return None;
            }
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                error(p, format!("Could not open file: {e}"));
                return None;
            }
        };
        let (text, enc, lossy) = Enc::decode(&bytes);
        drop(bytes);
        if lossy {
            warn(p, format!(
                "This file contains bytes that are not valid {}.\nThey are shown as \u{FFFD} and will be replaced if you save the file.",
                enc.name()
            ));
        }
        // no line break at all: use the platform default, so the first Enter matches it
        let crlf = if text.contains('\n') { text.contains("\r\n") } else { NEW_DOC_CRLF };
        let text = if crlf { text.replace("\r\n", "\n") } else { text };
        Some(Doc { text, enc, crlf, ..Doc::new(id, Some(path)) })
    }

    /// true if the file was written.
    pub fn save(&mut self, save_as: bool, p: Parent) -> bool {
        let path = match (&self.path, save_as) {
            (Some(path), false) => path.clone(),
            _ => {
                let d = file_dialog(p);
                let d = match &self.path {
                    Some(cur) => {
                        let d = d.set_file_name(cur.file_name().unwrap_or_default().to_string_lossy());
                        match cur.parent() {
                            Some(dir) => d.set_directory(dir),
                            None => d,
                        }
                    }
                    None => d.set_file_name("untitled.txt"),
                };
                match d.save_file() {
                    Some(path) => path,
                    None => return false,
                }
            }
        };
        let text: Cow<str> = if self.crlf { self.text.replace('\n', "\r\n").into() } else { (&self.text).into() };
        let (bytes, enc) = match self.enc.encode(&text) {
            Some(b) => (b, self.enc),
            None if ask(p, &format!(
                "The text contains characters that cannot be represented in {}.\nSave as UTF-8 instead?",
                self.enc.name()
            )) =>
            {
                (Cow::Borrowed(text.as_bytes()), Enc::Utf8)
            }
            None => return false,
        };
        match write_atomic(&path, &bytes) {
            // only now: a failed save must not silently change the encoding
            Ok(()) => {
                (self.path, self.dirty, self.enc) = (Some(path), false, enc);
                true
            }
            Err(e) => {
                error(p, format!("Could not save file: {e}"));
                false
            }
        }
    }

    /// File name, or "Untitled".
    pub fn name(&self) -> Cow<'_, str> {
        self.path.as_ref().and_then(|p| p.file_name()).map_or("Untitled".into(), |n| n.to_string_lossy())
    }

    /// Untitled, empty and unchanged: opening a file may reuse this tab.
    pub fn is_blank(&self) -> bool {
        self.path.is_none() && !self.dirty && self.text.is_empty()
    }

    pub fn editor_id(&self) -> egui::Id {
        egui::Id::unique(("editor", self.id))
    }
}
