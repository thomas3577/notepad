#![windows_subsystem = "windows"]

use eframe::egui;
use egui::text::{CCursor, CCursorRange};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use std::path::PathBuf;

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

fn utf16(b: &[u8], f: fn([u8; 2]) -> u16) -> String {
    String::from_utf16_lossy(&b.chunks_exact(2).map(|c| f([c[0], c[1]])).collect::<Vec<_>>())
}

impl Enc {
    const ALL: [Enc; 5] = [Enc::Utf8, Enc::Utf8Bom, Enc::Utf16Le, Enc::Utf16Be, Enc::Win1252];

    fn name(self) -> &'static str {
        match self {
            Enc::Utf8 => "UTF-8",
            Enc::Utf8Bom => "UTF-8 mit BOM",
            Enc::Utf16Le => "UTF-16 LE",
            Enc::Utf16Be => "UTF-16 BE",
            Enc::Win1252 => "ANSI (Windows-1252)",
        }
    }

    /// BOM → that encoding; valid UTF-8 → UTF-8; anything else → Windows-1252 (never fails).
    // ponytail: no BOM-less UTF-16 detection, such files open as Windows-1252.
    fn decode(b: &[u8]) -> (String, Enc) {
        if let Some(r) = b.strip_prefix(b"\xEF\xBB\xBF") {
            (String::from_utf8_lossy(r).into(), Enc::Utf8Bom)
        } else if let Some(r) = b.strip_prefix(b"\xFF\xFE") {
            (utf16(r, u16::from_le_bytes), Enc::Utf16Le)
        } else if let Some(r) = b.strip_prefix(b"\xFE\xFF") {
            (utf16(r, u16::from_be_bytes), Enc::Utf16Be)
        } else if let Ok(s) = std::str::from_utf8(b) {
            (s.into(), Enc::Utf8)
        } else {
            let s = b.iter().map(|&c| if (0x80..0xA0).contains(&c) { W1252[(c - 0x80) as usize] } else { c as char });
            (s.collect(), Enc::Win1252)
        }
    }

    /// None = text contains characters this encoding can't represent.
    fn encode(self, s: &str) -> Option<Vec<u8>> {
        Some(match self {
            Enc::Utf8 => s.as_bytes().to_vec(),
            Enc::Utf8Bom => [b"\xEF\xBB\xBF", s.as_bytes()].concat(),
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

/// Next match at/after char index `from`, wrapping around. Returns a char range.
// ponytail: case-sensitive only.
fn find(text: &str, needle: &str, from: usize) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    let start = char_to_byte(text, from);
    let b = text[start..].find(needle).map(|i| i + start).or_else(|| text.find(needle))?;
    let a = text[..b].chars().count();
    Some((a, a + needle.chars().count()))
}

// ---------- App ----------

fn editor_id() -> egui::Id {
    egui::Id::unique("editor")
}

fn error(msg: String) {
    MessageDialog::new().set_level(MessageLevel::Error).set_description(msg).show();
}

fn ask(msg: &str) -> bool {
    MessageDialog::new().set_title("Notepad").set_description(msg).set_buttons(MessageButtons::YesNo).show()
        == MessageDialogResult::Yes
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
    replacement: String,
    not_found: bool,
    scroll_to: Option<usize>,
}

impl Notepad {
    /// true = ok to throw away current text
    fn confirm_discard(&self) -> bool {
        !self.dirty || ask("Ungespeicherte Änderungen verwerfen?")
    }

    fn new_file(&mut self) {
        if self.confirm_discard() {
            self.text.clear();
            (self.path, self.dirty, self.enc, self.crlf) = (None, false, Enc::default(), false);
        }
    }

    fn open(&mut self) {
        if !self.confirm_discard() {
            return;
        }
        let Some(path) = FileDialog::new().pick_file() else { return };
        match std::fs::read(&path) {
            Ok(bytes) => {
                let (text, enc) = Enc::decode(&bytes);
                self.crlf = text.contains("\r\n");
                self.text = text.replace("\r\n", "\n");
                (self.path, self.dirty, self.enc) = (Some(path), false, enc);
            }
            Err(e) => error(format!("Öffnen fehlgeschlagen: {e}")),
        }
    }

    fn save(&mut self, save_as: bool) {
        let path = match (&self.path, save_as) {
            (Some(p), false) => p.clone(),
            _ => match FileDialog::new().set_file_name("unbenannt.txt").save_file() {
                Some(p) => p,
                None => return,
            },
        };
        let text = if self.crlf { self.text.replace('\n', "\r\n") } else { self.text.clone() };
        let bytes = match self.enc.encode(&text) {
            Some(b) => b,
            None if ask(&format!(
                "Der Text enthält Zeichen, die in {} nicht darstellbar sind.\nStattdessen als UTF-8 speichern?",
                self.enc.name()
            )) =>
            {
                self.enc = Enc::Utf8;
                text.into_bytes()
            }
            None => return,
        };
        match std::fs::write(&path, bytes) {
            Ok(()) => (self.path, self.dirty) = (Some(path), false),
            Err(e) => error(format!("Speichern fehlgeschlagen: {e}")),
        }
    }

    fn selection(ctx: &egui::Context) -> Option<(usize, usize)> {
        let r = egui::TextEdit::load_state(ctx, editor_id())?.cursor.char_range()?;
        let (a, b) = (r.primary.index, r.secondary.index);
        Some((a.min(b).into(), a.max(b).into()))
    }

    fn select(&mut self, ctx: &egui::Context, a: usize, b: usize) {
        let mut s = egui::TextEdit::load_state(ctx, editor_id()).unwrap_or_default();
        s.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(a), CCursor::new(b))));
        s.store(ctx, editor_id());
        ctx.memory_mut(|m| m.request_focus(editor_id()));
        self.scroll_to = Some(a);
    }

    fn find_next(&mut self, ctx: &egui::Context) {
        let from = Self::selection(ctx).map_or(0, |(_, b)| b);
        let hit = find(&self.text, &self.needle, from);
        self.not_found = hit.is_none();
        if let Some((a, b)) = hit {
            self.select(ctx, a, b);
        }
    }

    fn replace(&mut self, ctx: &egui::Context) {
        if let Some((a, b)) = Self::selection(ctx) {
            let (ba, bb) = (char_to_byte(&self.text, a), char_to_byte(&self.text, b));
            if a != b && self.text[ba..bb] == self.needle {
                self.text.replace_range(ba..bb, &self.replacement);
                self.dirty = true;
                let end = a + self.replacement.chars().count();
                self.select(ctx, end, end);
            }
        }
        self.find_next(ctx);
    }

    fn replace_all(&mut self) {
        if !self.needle.is_empty() && self.text.contains(&self.needle) {
            self.text = self.text.replace(&self.needle, &self.replacement);
            self.dirty = true;
        }
    }

    fn title(&self) -> String {
        let name = self.path.as_ref().and_then(|p| p.file_name()).map_or("Unbenannt".into(), |n| n.to_string_lossy());
        format!("{}{name} - Notepad", if self.dirty { "*" } else { "" })
    }
}

impl eframe::App for Notepad {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        let [n, o, s, f, h, f3, esc] = ctx.input_mut(|i| {
            use egui::{Key, KeyboardShortcut as K, Modifiers as M};
            [
                K::new(M::COMMAND, Key::N),
                K::new(M::COMMAND, Key::O),
                K::new(M::COMMAND, Key::S),
                K::new(M::COMMAND, Key::F),
                K::new(M::COMMAND, Key::H),
                K::new(M::NONE, Key::F3),
                K::new(M::NONE, Key::Escape),
            ]
            .map(|k| i.consume_shortcut(&k))
        });
        if n { self.new_file(); }
        if o { self.open(); }
        if s { self.save(false); }
        if f || h { (self.find_open, self.find_focus) = (true, true); }
        if f3 { self.find_next(&ctx); }
        if esc { self.find_open = false; }

        // Close guard
        if ctx.input(|i| i.viewport().close_requested()) && !self.confirm_discard() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }

        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("Datei", |ui| {
                    if ui.button("Neu (Ctrl+N)").clicked() { self.new_file(); }
                    if ui.button("Öffnen… (Ctrl+O)").clicked() { self.open(); }
                    if ui.button("Speichern (Ctrl+S)").clicked() { self.save(false); }
                    if ui.button("Speichern unter…").clicked() { self.save(true); }
                });
                ui.menu_button("Bearbeiten", |ui| {
                    if ui.button("Suchen/Ersetzen (Ctrl+F)").clicked() {
                        (self.find_open, self.find_focus) = (true, true);
                    }
                    if ui.button("Weitersuchen (F3)").clicked() { self.find_next(&ctx); }
                });
                ui.menu_button("Kodierung", |ui| {
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
                    ui.label("Suchen:");
                    let r = ui.text_edit_singleline(&mut self.needle);
                    if std::mem::take(&mut self.find_focus) { r.request_focus(); }
                    if r.changed() { self.not_found = false; }
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.label("Ersetzen:");
                    ui.text_edit_singleline(&mut self.replacement);
                    if ui.button("Weitersuchen").clicked() || enter { self.find_next(&ctx); }
                    if ui.button("Ersetzen").clicked() { self.replace(&ctx); }
                    if ui.button("Alle ersetzen").clicked() { self.replace_all(); }
                    if self.not_found { ui.colored_label(ui.visuals().warn_fg_color, "Nicht gefunden"); }
                    if ui.button("✕").clicked() { self.find_open = false; }
                });
            });
        }

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.label(format!("{}  |  {}", self.enc.name(), if self.crlf { "CRLF" } else { "LF" }));
        });

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    let font = egui::TextStyle::Monospace.resolve(ui.style());
                    let color = ui.visuals().weak_text_color();
                    let lines = self.text.matches('\n').count() + 1;
                    let w = ui.painter().layout_no_wrap(lines.to_string(), font.clone(), color).size().x;
                    let (gutter, _) = ui.allocate_exact_size(egui::vec2(w + 8.0, 0.0), egui::Sense::hover());

                    let out = egui::TextEdit::multiline(&mut self.text)
                        .id(editor_id())
                        .font(egui::TextStyle::Monospace)
                        .frame(egui::Frame::NONE)
                        .desired_width(f32::INFINITY)
                        .min_size(ui.available_size())
                        .show(ui);
                    if out.response.changed() { self.dirty = true; }

                    // Line numbers: one per logical line, at the first wrapped row of that line.
                    let mut line = 1;
                    let mut starts_line = true;
                    for row in &out.galley.rows {
                        if starts_line {
                            let y = out.galley_pos.y + row.pos.y;
                            ui.painter().text(egui::pos2(gutter.right() - 6.0, y), egui::Align2::RIGHT_TOP, line, font.clone(), color);
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

        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.title()));
    }
}

fn main() -> eframe::Result {
    eframe::run_native(
        "Notepad",
        eframe::NativeOptions::default(),
        Box::new(|_| Ok(Box::<Notepad>::default())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodings_roundtrip() {
        let all: Vec<u8> = (0..=255).collect();
        let (s, e) = Enc::decode(&all);
        assert_eq!(e, Enc::Win1252);
        assert_eq!(Enc::Win1252.encode(&s).unwrap(), all);
        assert_eq!(Enc::decode(b"\xE4\x80").0, "ä€");
        assert_eq!(Enc::Win1252.encode("😀"), None);

        let t = "Grüße 😀\nzwei";
        for e in Enc::ALL.into_iter().filter(|&e| e != Enc::Win1252) {
            assert_eq!(Enc::decode(&e.encode(t).unwrap()), (t.to_string(), e), "{e:?}");
        }
    }

    #[test]
    fn find_wraps_and_counts_chars() {
        assert_eq!(find("äb äb", "äb", 0), Some((0, 2)));
        assert_eq!(find("äb äb", "äb", 1), Some((3, 5)));
        assert_eq!(find("äb äb", "äb", 4), Some((0, 2)));
        assert_eq!(find("abc", "x", 0), None);
        assert_eq!(find("abc", "", 0), None);
    }
}
