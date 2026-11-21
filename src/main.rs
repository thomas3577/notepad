#![windows_subsystem = "windows"]

use eframe::egui;
use egui::text::{CCursor, CCursorRange};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use std::borrow::Cow;
use std::io::Write;
use std::path::{Path, PathBuf};

// ---------- Encoding ----------

#[derive(Clone, Copy, PartialEq, Default, Debug)]
enum Enc {
    #[default]
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Win1252,
}

/// Windows-1252 bytes 0x80..=0x9F (WHATWG mapping; undefined bytes map to themselves).
const W1252: [char; 32] = [
    '\u{20AC}', '\u{81}', '\u{201A}', '\u{192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{2C6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8D}', '\u{17D}', '\u{8F}',
    '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{2DC}', '\u{2122}', '\u{161}', '\u{203A}', '\u{153}', '\u{9D}', '\u{17E}', '\u{178}',
];

/// Decoded text plus whether invalid bytes had to be replaced with U+FFFD.
type Decoded = (String, Enc, bool);

fn units(b: &[u8], f: fn([u8; 2]) -> u16) -> Vec<u16> {
    b.as_chunks::<2>().0.iter().map(|&c| f(c)).collect()
}

/// Lossy UTF-16 decode; reports invalid surrogates and a dangling odd byte.
fn utf16(b: &[u8], f: fn([u8; 2]) -> u16) -> (String, bool) {
    let u = units(b, f);
    let (mut s, mut lossy) = match String::from_utf16(&u) {
        Ok(s) => (s, false),
        Err(_) => (String::from_utf16_lossy(&u), true),
    };
    if b.len() % 2 == 1 {
        s.push(char::REPLACEMENT_CHARACTER);
        lossy = true;
    }
    (s, lossy)
}

/// UTF-16 without BOM: in Latin-script text nearly every high byte is 0.
// ponytail: NUL-byte heuristic, misses BOM-less UTF-16 that is mostly non-Latin (e.g. CJK).
fn sniff_utf16(b: &[u8]) -> Option<Decoded> {
    if b.len() < 2 || b.len() % 2 == 1 {
        return None;
    }
    let pairs = b.len() / 2;
    let zeros = |start: usize| b.iter().skip(start).step_by(2).filter(|&&c| c == 0).count();
    let (even, odd) = (zeros(0), zeros(1));
    // more than half of one byte column is zero, under a tenth of the other
    let (f, enc): (fn([u8; 2]) -> u16, _) = if odd > pairs / 2 && even < pairs / 10 {
        (u16::from_le_bytes, Enc::Utf16Le)
    } else if even > pairs / 2 && odd < pairs / 10 {
        (u16::from_be_bytes, Enc::Utf16Be)
    } else {
        return None;
    };
    String::from_utf16(&units(b, f)).ok().map(|s| (s, enc, false))
}

impl Enc {
    const ALL: [Enc; 5] = [Enc::Utf8, Enc::Utf8Bom, Enc::Utf16Le, Enc::Utf16Be, Enc::Win1252];

    fn name(self) -> &'static str {
        match self {
            Enc::Utf8 => "UTF-8",
            Enc::Utf8Bom => "UTF-8 with BOM",
            Enc::Utf16Le => "UTF-16 LE",
            Enc::Utf16Be => "UTF-16 BE",
            Enc::Win1252 => "ANSI (Windows-1252)",
        }
    }

    /// BOM → that encoding; UTF-16 pattern → UTF-16; valid UTF-8 → UTF-8;
    /// anything else → Windows-1252 (never fails).
    /// BOM-less UTF-16 is saved with BOM, like Windows Notepad does.
    fn decode(b: &[u8]) -> Decoded {
        if let Some(r) = b.strip_prefix(b"\xEF\xBB\xBF") {
            let s = String::from_utf8_lossy(r);
            let lossy = matches!(s, Cow::Owned(_));
            (s.into_owned(), Enc::Utf8Bom, lossy)
        } else if let Some(r) = b.strip_prefix(b"\xFF\xFE") {
            let (s, lossy) = utf16(r, u16::from_le_bytes);
            (s, Enc::Utf16Le, lossy)
        } else if let Some(r) = b.strip_prefix(b"\xFE\xFF") {
            let (s, lossy) = utf16(r, u16::from_be_bytes);
            (s, Enc::Utf16Be, lossy)
        } else if let Some(hit) = sniff_utf16(b) {
            // must run before the UTF-8 check: ASCII in UTF-16 is also valid UTF-8
            hit
        } else if let Ok(s) = std::str::from_utf8(b) {
            (s.into(), Enc::Utf8, false)
        } else {
            let s = b.iter().map(|&c| if (0x80..0xA0).contains(&c) { W1252[(c - 0x80) as usize] } else { c as char });
            (s.collect(), Enc::Win1252, false)
        }
    }

    /// None = text contains characters this encoding can't represent.
    fn encode(self, s: &str) -> Option<Cow<'_, [u8]>> {
        Some(match self {
            Enc::Utf8 => Cow::Borrowed(s.as_bytes()),
            Enc::Utf8Bom => Cow::Owned([b"\xEF\xBB\xBF", s.as_bytes()].concat()),
            Enc::Utf16Le => [0xFF, 0xFE].into_iter().chain(s.encode_utf16().flat_map(u16::to_le_bytes)).collect(),
            Enc::Utf16Be => [0xFE, 0xFF].into_iter().chain(s.encode_utf16().flat_map(u16::to_be_bytes)).collect(),
            Enc::Win1252 => s
                .chars()
                .map(|c| match c as u32 {
                    0..0x80 | 0xA0..=0xFF => Some(c as u8),
                    _ => W1252.iter().position(|&w| w == c).map(|i| 0x80 + i as u8),
                })
                .collect::<Option<_>>()?,
        })
    }
}

// ---------- Search ----------

fn char_to_byte(t: &str, c: usize) -> usize {
    t.char_indices().nth(c).map_or(t.len(), |(i, _)| i)
}

// ponytail: per-char case folding, so "ß" never matches "SS"; good enough for a notepad.
fn char_eq(a: char, b: char, match_case: bool) -> bool {
    a == b || (!match_case && a.to_lowercase().eq(b.to_lowercase()))
}

/// Byte length of the match if `t` starts with `needle`.
fn match_len(t: &str, needle: &str, match_case: bool) -> Option<usize> {
    let mut it = t.char_indices();
    for n in needle.chars() {
        let (_, c) = it.next()?;
        if !char_eq(c, n, match_case) {
            return None;
        }
    }
    Some(it.next().map_or(t.len(), |(i, _)| i))
}

/// Next match at/after char index `from`, wrapping around. Returns a char range.
// ponytail: naive O(text × needle) scan, fine for notepad-sized files.
fn find(text: &str, needle: &str, from: usize, match_case: bool) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    let hit = |&(_, (b, _)): &(usize, (usize, char))| match_len(&text[b..], needle, match_case).is_some();
    let starts = || text.char_indices().enumerate();
    let (i, _) = starts().skip(from).find(hit).or_else(|| starts().take(from).find(hit))?;
    Some((i, i + needle.chars().count()))
}

/// None if there is nothing to replace.
fn replace_all(text: &str, needle: &str, replacement: &str, match_case: bool) -> Option<String> {
    if needle.is_empty() {
        return None;
    }
    let (mut out, mut rest, mut hit) = (String::with_capacity(text.len()), text, false);
    while let Some(c) = rest.chars().next() {
        if let Some(len) = match_len(rest, needle, match_case) {
            out.push_str(replacement);
            (rest, hit) = (&rest[len..], true);
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    hit.then_some(out)
}

// ---------- Files ----------

/// Write to a temp file next to the target, then rename over it, so a crash or
/// power loss mid-save never leaves a truncated file.
// ponytail: rename replaces symlinks/hardlinks with a plain file and the file gets
// the folder's default permissions; fine for a notepad.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".notepad-tmp");
    let tmp = PathBuf::from(tmp);
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// egui keeps far more memory per character than the file needs (layout data for every glyph).
/// Measured: a 5.6 MB file needs about 1.3 GB.
const MEMORY_PER_TEXT_BYTE: u64 = 240;
/// Ask before opening files whose estimated memory use exceeds this.
const LARGE_FILE_BYTES: u64 = 10 * 1024 * 1024;
/// Upper bound for egui's undo history, which stores a full copy of the text per step.
const UNDO_BUDGET_BYTES: usize = 64 * 1024 * 1024;

/// Line ending for new documents: CRLF on Windows, like Windows Notepad.
const NEW_DOC_CRLF: bool = cfg!(windows);

/// 1-based line and column of char index `idx`.
fn line_col(text: &str, idx: usize) -> (usize, usize) {
    let before = &text[..char_to_byte(text, idx)];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    (line, col)
}

// ---------- Dialogs ----------

/// Parent window for native dialogs, so they stay on top of Notepad (None before the window exists).
type Parent<'a> = Option<&'a eframe::Frame>;

fn message(p: Parent) -> MessageDialog {
    let d = MessageDialog::new().set_title("Notepad");
    match p {
        Some(f) => d.set_parent(f),
        None => d,
    }
}

fn file_dialog(p: Parent) -> FileDialog {
    match p {
        Some(f) => FileDialog::new().set_parent(f),
        None => FileDialog::new(),
    }
}

fn error(p: Parent, msg: String) {
    message(p).set_level(MessageLevel::Error).set_description(msg).show();
}

fn warn(p: Parent, msg: String) {
    message(p).set_level(MessageLevel::Warning).set_description(msg).show();
}

fn ask(p: Parent, msg: &str) -> bool {
    message(p).set_description(msg).set_buttons(MessageButtons::YesNo).show() == MessageDialogResult::Yes
}

// ---------- App ----------

fn editor_id() -> egui::Id {
    egui::Id::unique("editor")
}

#[derive(Default)]
struct Notepad {
    text: String,
    path: Option<PathBuf>,
    dirty: bool,
    enc: Enc,
    crlf: bool,
    find_open: bool,
    find_focus: bool,
    needle: String,
    match_case: bool,
    replacement: String,
    not_found: bool,
    scroll_to: Option<usize>,
    no_wrap: bool,
    last_title: String,
    /// Text zoom factor, 1.0 = 100 %.
    zoom: f32,
    /// New document: clear egui's editor state (undo history, cursor) and scroll position.
    reset_editor: bool,
    /// File from the command line, opened on the first frame so dialogs have a parent window.
    pending_open: Option<PathBuf>,
}

impl Notepad {
    /// true = ok to throw away current text
    fn confirm_discard(&self, p: Parent) -> bool {
        !self.dirty || ask(p, "Discard unsaved changes?")
    }

    fn set_document(&mut self, text: String, path: Option<PathBuf>, enc: Enc, crlf: bool) {
        (self.text, self.path, self.enc, self.crlf) = (text, path, enc, crlf);
        (self.dirty, self.not_found, self.scroll_to, self.reset_editor) = (false, false, None, true);
    }

    fn new_file(&mut self, p: Parent) {
        if self.confirm_discard(p) {
            self.set_document(String::new(), None, Enc::default(), NEW_DOC_CRLF);
        }
    }

    fn open(&mut self, p: Parent) {
        if !self.confirm_discard(p) {
            return;
        }
        if let Some(path) = file_dialog(p).pick_file() {
            self.load(path, p);
        }
    }

    fn load(&mut self, path: PathBuf, p: Parent) {
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            // Like Windows Notepad: a new document that will be saved under this name.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return self.set_document(String::new(), Some(path), Enc::default(), NEW_DOC_CRLF);
            }
            Err(e) => return error(p, format!("Could not open file: {e}")),
        };
        // Devices (CON), pipes and folders would hang or fail on read.
        if !meta.is_file() {
            return error(p, format!("Not a regular file: {}", path.display()));
        }
        if meta.len() > LARGE_FILE_BYTES {
            let mb = meta.len() as f64 / 1_048_576.0;
            let gb = (meta.len() * MEMORY_PER_TEXT_BYTE) as f64 / 1_073_741_824.0;
            if !ask(p, &format!("This file is {mb:.0} MB. Opening it will use about {gb:.1} GB of memory.\nOpen anyway?")) {
                return;
            }
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => return error(p, format!("Could not open file: {e}")),
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
        self.set_document(text, Some(path), enc, crlf);
    }

    fn save(&mut self, save_as: bool, p: Parent) {
        let path = match (&self.path, save_as) {
            (Some(path), false) => path.clone(),
            _ => {
                let mut d = file_dialog(p);
                d = match &self.path {
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
                    None => return,
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
            None => return,
        };
        match write_atomic(&path, &bytes) {
            // only now: a failed save must not silently change the encoding
            Ok(()) => (self.path, self.dirty, self.enc) = (Some(path), false, enc),
            Err(e) => error(p, format!("Could not save file: {e}")),
        }
    }

    fn selection(ctx: &egui::Context) -> Option<(usize, usize)> {
        let r = egui::TextEdit::load_state(ctx, editor_id())?.cursor.char_range()?;
        let (a, b) = (r.primary.index, r.secondary.index);
        Some((a.min(b).into(), a.max(b).into()))
    }

    /// Select a range without moving keyboard focus (the find bar keeps it, so a second
    /// Enter searches again instead of overwriting the match).
    fn select(&mut self, ctx: &egui::Context, a: usize, b: usize) {
        let mut s = egui::TextEdit::load_state(ctx, editor_id()).unwrap_or_default();
        s.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(a), CCursor::new(b))));
        s.store(ctx, editor_id());
        self.scroll_to = Some(a);
    }

    fn find_next(&mut self, ctx: &egui::Context) {
        if self.needle.is_empty() {
            (self.find_open, self.find_focus) = (true, true);
            return;
        }
        let from = Self::selection(ctx).map_or(0, |(_, b)| b);
        let hit = find(&self.text, &self.needle, from, self.match_case);
        self.not_found = hit.is_none();
        if let Some((a, b)) = hit {
            self.select(ctx, a, b);
        }
    }

    fn replace(&mut self, ctx: &egui::Context) {
        if let Some((a, b)) = Self::selection(ctx) {
            let (ba, bb) = (char_to_byte(&self.text, a), char_to_byte(&self.text, b));
            if a != b && find(&self.text[ba..bb], &self.needle, 0, self.match_case) == Some((0, b - a)) {
                self.text.replace_range(ba..bb, &self.replacement);
                self.dirty = true;
                let end = a + self.replacement.chars().count();
                self.select(ctx, end, end);
            }
        }
        self.find_next(ctx);
    }

    fn replace_all(&mut self) {
        match replace_all(&self.text, &self.needle, &self.replacement, self.match_case) {
            Some(t) => (self.text, self.dirty, self.not_found) = (t, true, false),
            None => self.not_found = true,
        }
    }

    fn title(&self) -> String {
        let name = self.path.as_ref().and_then(|p| p.file_name()).map_or("Untitled".into(), |n| n.to_string_lossy());
        format!("{}{name} - Notepad", if self.dirty { "*" } else { "" })
    }
}

impl eframe::App for Notepad {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let p = Some(&*frame);

        if let Some(path) = self.pending_open.take() {
            self.load(path, p);
        }

        let find_open = self.find_open;
        let [save_as, n, o, s, f, h, f3, esc, plus, equals, minus, zero] = ctx.input_mut(|i| {
            use egui::{Key, KeyboardShortcut as K, Modifiers as M};
            [
                // Ctrl+Shift+S first: egui's Ctrl+S would also match it (extra Shift is ignored)
                K::new(M::COMMAND | M::SHIFT, Key::S),
                K::new(M::COMMAND, Key::N),
                K::new(M::COMMAND, Key::O),
                K::new(M::COMMAND, Key::S),
                K::new(M::COMMAND, Key::F),
                K::new(M::COMMAND, Key::H),
                K::new(M::NONE, Key::F3),
                // Esc only belongs to the find bar while it is open; otherwise menus need it
                if find_open { K::new(M::NONE, Key::Escape) } else { K::new(M::NONE, Key::F24) },
                K::new(M::COMMAND, Key::Plus),
                K::new(M::COMMAND, Key::Equals),
                K::new(M::COMMAND, Key::Minus),
                K::new(M::COMMAND, Key::Num0),
            ]
            .map(|k| i.consume_shortcut(&k))
        });
        if save_as { self.save(true, p); }
        if n { self.new_file(p); }
        if o { self.open(p); }
        if s { self.save(false, p); }
        if f || h { (self.find_open, self.find_focus) = (true, true); }
        if f3 { self.find_next(&ctx); }
        if esc && find_open {
            self.find_open = false;
            ctx.memory_mut(|m| m.request_focus(editor_id()));
        }
        if plus || equals { self.zoom += 0.1; }
        if minus { self.zoom -= 0.1; }
        if zero { self.zoom = 1.0; }
        self.zoom *= ctx.input(|i| i.zoom_delta()); // Ctrl + mouse wheel

        // File dropped onto the window
        if let Some(path) = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()))
            && self.confirm_discard(p)
        {
            self.load(path, p);
        }

        // Close guard
        if ctx.input(|i| i.viewport().close_requested()) && !self.confirm_discard(p) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }

        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New (Ctrl+N)").clicked() { self.new_file(p); }
                    if ui.button("Open… (Ctrl+O)").clicked() { self.open(p); }
                    if ui.button("Save (Ctrl+S)").clicked() { self.save(false, p); }
                    if ui.button("Save As… (Ctrl+Shift+S)").clicked() { self.save(true, p); }
                });
                ui.menu_button("Edit", |ui| {
                    if ui.button("Find/Replace (Ctrl+F)").clicked() {
                        (self.find_open, self.find_focus) = (true, true);
                    }
                    if ui.button("Find Next (F3)").clicked() { self.find_next(&ctx); }
                });
                ui.menu_button("View", |ui| {
                    let mut wrap = !self.no_wrap;
                    if ui.checkbox(&mut wrap, "Word Wrap").changed() { self.no_wrap = !wrap; }
                    ui.separator();
                    if ui.button("Zoom In (Ctrl+Plus)").clicked() { self.zoom += 0.1; }
                    if ui.button("Zoom Out (Ctrl+Minus)").clicked() { self.zoom -= 0.1; }
                    if ui.button("Reset Zoom (Ctrl+0)").clicked() { self.zoom = 1.0; }
                });
                ui.menu_button("Encoding", |ui| {
                    for e in Enc::ALL {
                        if ui.radio_value(&mut self.enc, e, e.name()).changed() { self.dirty = true; }
                    }
                    ui.separator();
                    if ui.radio_value(&mut self.crlf, true, "Windows (CRLF)").changed()
                        | ui.radio_value(&mut self.crlf, false, "Unix (LF)").changed()
                    {
                        self.dirty = true;
                    }
                });
            });
        });

        if self.find_open {
            egui::Panel::top("find").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Find:");
                    let r = ui.text_edit_singleline(&mut self.needle);
                    if r.changed() { self.not_found = false; }
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    // Enter makes egui drop the focus; take it back so Enter again finds the next match
                    if std::mem::take(&mut self.find_focus) || enter { r.request_focus(); }
                    ui.label("Replace:");
                    ui.text_edit_singleline(&mut self.replacement);
                    if ui.checkbox(&mut self.match_case, "Match case").changed() { self.not_found = false; }
                    if ui.button("Find Next").clicked() || enter { self.find_next(&ctx); }
                    if ui.button("Replace").clicked() { self.replace(&ctx); }
                    if ui.button("Replace All").clicked() { self.replace_all(); }
                    if self.not_found { ui.colored_label(ui.visuals().warn_fg_color, "Not found"); }
                    if ui.button("✕").clicked() { self.find_open = false; }
                });
            });
        }

        self.zoom = self.zoom.clamp(0.5, 4.0);

        egui::Panel::bottom("status").show(ui, |ui| {
            let cursor = egui::TextEdit::load_state(&ctx, editor_id())
                .and_then(|s| s.cursor.char_range())
                .map_or(0, |r| r.primary.index.into());
            let (ln, col) = line_col(&self.text, cursor);
            ui.horizontal(|ui| {
                ui.label(format!(
                    "Ln {ln}, Col {col}  |  {:.0}%  |  {}  |  {}",
                    self.zoom * 100.0,
                    self.enc.name(),
                    if self.crlf { "CRLF" } else { "LF" }
                ));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(concat!("v", env!("CARGO_PKG_VERSION")));
                });
            });
        });

        // New document: fresh undo history (sized so large files don't keep ~100 full copies)
        let reset = std::mem::take(&mut self.reset_editor);
        if reset {
            let max_undos = (UNDO_BUDGET_BYTES / self.text.len().max(1)).clamp(2, 100);
            let mut state = egui::text_edit::TextEditState::default();
            state.set_undoer(egui::util::undoer::Undoer::with_settings(egui::util::undoer::Settings {
                max_undos,
                ..Default::default()
            }));
            state.store(&ctx, editor_id());
            // ready to type right away, like Windows Notepad
            ctx.memory_mut(|m| m.request_focus(editor_id()));
        }

        // While the editor is not focused (e.g. typing in the find bar), egui hides the
        // selection, so we paint the selected range (the current match) ourselves.
        let highlight = (!ctx.memory(|m| m.has_focus(editor_id())))
            .then(|| Self::selection(&ctx))
            .flatten()
            .filter(|(a, b)| a != b)
            .map(|(a, b)| (char_to_byte(&self.text, a), char_to_byte(&self.text, b)));

        egui::CentralPanel::default().show(ui, |ui| {
            let mut scroll = if self.no_wrap { egui::ScrollArea::both() } else { egui::ScrollArea::vertical() };
            if reset {
                scroll = scroll.scroll_offset(egui::Vec2::ZERO);
            }
            scroll.show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    let mut font = egui::TextStyle::Monospace.resolve(ui.style());
                    font.size *= self.zoom;
                    let color = ui.visuals().weak_text_color();
                    let lines = self.text.matches('\n').count() + 1;
                    let w = ui.painter().layout_no_wrap(lines.to_string(), font.clone(), color).size().x;
                    let (gutter, _) = ui.allocate_exact_size(egui::vec2(w + 8.0, 0.0), egui::Sense::hover());

                    // Own layouter: zoomed font, optional wrapping, find highlight.
                    let (editor_font, wrap) = (font.clone(), !self.no_wrap);
                    let mut layouter = move |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                        let text = buf.as_str();
                        let fmt = egui::TextFormat::simple(editor_font.clone(), ui.visuals().text_color());
                        let mut job = egui::text::LayoutJob::default();
                        job.wrap.max_width = if wrap { wrap_width } else { f32::INFINITY };
                        match highlight {
                            Some((a, b)) if b <= text.len() && text.is_char_boundary(a) && text.is_char_boundary(b) => {
                                let marked = egui::TextFormat { background: ui.visuals().selection.bg_fill, ..fmt.clone() };
                                job.append(&text[..a], 0.0, fmt.clone());
                                job.append(&text[a..b], 0.0, marked);
                                job.append(&text[b..], 0.0, fmt);
                            }
                            _ => job.append(text, 0.0, fmt),
                        }
                        ui.ctx().fonts_mut(|f| f.layout_job(job))
                    };

                    let out = egui::TextEdit::multiline(&mut self.text)
                        .id(editor_id())
                        .layouter(&mut layouter)
                        .frame(egui::Frame::NONE)
                        .desired_width(f32::INFINITY)
                        .min_size(ui.available_size())
                        .show(ui);
                    if out.response.changed() { self.dirty = true; }

                    // Line numbers: one per logical line, at the first wrapped row of that line.
                    // Only rows inside the visible area are painted (large files).
                    let clip = ui.clip_rect();
                    let mut line = 1;
                    let mut starts_line = true;
                    for row in &out.galley.rows {
                        let y = out.galley_pos.y + row.pos.y;
                        if y > clip.bottom() {
                            break;
                        }
                        if starts_line {
                            if y + row.rect().height() >= clip.top() {
                                ui.painter().text(egui::pos2(gutter.right() - 6.0, y), egui::Align2::RIGHT_TOP, line, font.clone(), color);
                            }
                            line += 1;
                        }
                        starts_line = row.ends_with_newline;
                    }

                    if let Some(i) = self.scroll_to.take() {
                        let r = out.galley.pos_from_cursor(CCursor::new(i)).translate(out.galley_pos.to_vec2());
                        ui.scroll_to_rect(r, Some(egui::Align::Center));
                    }
                });
            });
        });

        // Only on change: every viewport command triggers a repaint, which would loop forever.
        let title = self.title();
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }
}

/// Remove the current directory from the DLL search path. Opening a .txt from Downloads
/// makes that the working directory, and a planted DLL there must never be loaded.
#[cfg(windows)]
fn harden_dll_search() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetDllDirectoryW(path: *const u16) -> i32;
    }
    // SAFETY: passes a valid NUL-terminated (empty) wide string; only affects DLL lookup.
    unsafe {
        SetDllDirectoryW([0u16].as_ptr());
    }
}

fn main() -> eframe::Result {
    #[cfg(windows)]
    harden_dll_search();

    // "Open with" / drag onto the .exe passes the file path as first argument.
    let app = Notepad {
        zoom: 1.0,
        crlf: NEW_DOC_CRLF,
        reset_editor: true,
        pending_open: std::env::args_os().nth(1).map(PathBuf::from),
        ..Default::default()
    };
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).expect("valid icon png");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_icon(icon),
        ..Default::default()
    };
    eframe::run_native(
        "Notepad",
        options,
        Box::new(|cc| {
            // We zoom only the text (like Windows Notepad), not egui's whole UI.
            cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
            Ok(Box::new(app))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodings_roundtrip() {
        let all: Vec<u8> = (0..=255).collect();
        let (s, e, lossy) = Enc::decode(&all);
        assert_eq!((e, lossy), (Enc::Win1252, false));
        assert_eq!(Enc::Win1252.encode(&s).unwrap(), all.as_slice());
        assert_eq!(Enc::decode(b"\xE4\x80").0, "ä€");
        assert_eq!(Enc::Win1252.encode("😀"), None);

        let t = "Grüße 😀\nzwei";
        for e in Enc::ALL.into_iter().filter(|&e| e != Enc::Win1252) {
            assert_eq!(Enc::decode(&e.encode(t).unwrap()), (t.to_string(), e, false), "{e:?}");
        }
    }

    #[test]
    fn invalid_bytes_are_reported() {
        assert_eq!(Enc::decode(b"\xEF\xBB\xBFok\xFF"), ("ok\u{FFFD}".into(), Enc::Utf8Bom, true));
        // UTF-16 LE with BOM and a dangling odd byte
        assert_eq!(Enc::decode(b"\xFF\xFEa\0b"), ("a\u{FFFD}".into(), Enc::Utf16Le, true));
        // unpaired surrogate
        assert!(Enc::decode(b"\xFF\xFE\x00\xD8").2);
    }

    #[test]
    fn atomic_save_overwrites_and_cleans_up() {
        let dir = std::env::temp_dir().join(format!("notepad-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        write_atomic(&file, b"first").unwrap();
        write_atomic(&file, b"second").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"second");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "temp file left behind");
        // failure (target is a directory) must not leave a temp file either
        assert!(write_atomic(&dir, b"x").is_err());
        assert!(!dir.with_extension("notepad-tmp").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn line_and_column() {
        assert_eq!(line_col("", 0), (1, 1));
        assert_eq!(line_col("ab\ncäd", 1), (1, 2));
        assert_eq!(line_col("ab\ncäd", 3), (2, 1));
        assert_eq!(line_col("ab\ncäd", 5), (2, 3));
        assert_eq!(line_col("ab\n", 3), (2, 1));
    }

    #[test]
    fn utf16_without_bom() {
        let t = "Hello Grüße\r\nline 2";
        let le: Vec<u8> = t.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let be: Vec<u8> = t.encode_utf16().flat_map(u16::to_be_bytes).collect();
        assert_eq!(Enc::decode(&le), (t.to_string(), Enc::Utf16Le, false));
        assert_eq!(Enc::decode(&be), (t.to_string(), Enc::Utf16Be, false));
        // plain text and binary-ish data are not mistaken for UTF-16
        assert_eq!(Enc::decode(b"Hello world!").1, Enc::Utf8);
        assert_eq!(Enc::decode(b"a\0\0b").1, Enc::Utf8);
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(find("Grüße GRÜN", "grü", 1, false), Some((6, 9)));
        assert_eq!(find("Grüße GRÜN", "grü", 1, true), None);
        assert_eq!(find("abc", "C", 0, false), Some((2, 3)));
        assert_eq!(replace_all("Foo foo FOO", "foo", "x", false).as_deref(), Some("x x x"));
        assert_eq!(replace_all("Foo foo FOO", "foo", "x", true).as_deref(), Some("Foo x FOO"));
        assert_eq!(replace_all("äbc äbc", "ÄB", "x", false).as_deref(), Some("xc xc"));
        assert_eq!(replace_all("abc", "x", "y", false), None);
    }

    #[test]
    fn find_wraps_and_counts_chars() {
        assert_eq!(find("äb äb", "äb", 0, true), Some((0, 2)));
        assert_eq!(find("äb äb", "äb", 1, true), Some((3, 5)));
        assert_eq!(find("äb äb", "äb", 4, true), Some((0, 2)));
        assert_eq!(find("äb äb", "äb", 99, true), Some((0, 2)));
        assert_eq!(find("abc", "x", 0, true), None);
        assert_eq!(find("abc", "", 0, true), None);
    }
}
