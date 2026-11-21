#![windows_subsystem = "windows"]

use eframe::egui;
use egui::text::{CCursor, CCursorRange};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
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

fn units(b: &[u8], f: fn([u8; 2]) -> u16) -> Vec<u16> {
    b.chunks_exact(2).map(|c| f([c[0], c[1]])).collect()
}

fn utf16(b: &[u8], f: fn([u8; 2]) -> u16) -> String {
    String::from_utf16_lossy(&units(b, f))
}

/// UTF-16 without BOM: in Latin-script text nearly every high byte is 0.
// ponytail: NUL-byte heuristic, misses BOM-less UTF-16 that is mostly non-Latin (e.g. CJK).
fn sniff_utf16(b: &[u8]) -> Option<(String, Enc)> {
    if b.len() < 2 || b.len() % 2 != 0 {
        return None;
    }
    let pairs = b.len() / 2;
    let zeros = |start: usize| b.iter().skip(start).step_by(2).filter(|&&c| c == 0).count();
    let (even, odd) = (zeros(0), zeros(1));
    let (f, enc): (fn([u8; 2]) -> u16, _) = if odd * 2 > pairs && even * 10 < pairs {
        (u16::from_le_bytes, Enc::Utf16Le)
    } else if even * 2 > pairs && odd * 10 < pairs {
        (u16::from_be_bytes, Enc::Utf16Be)
    } else {
        return None;
    };
    String::from_utf16(&units(b, f)).ok().map(|s| (s, enc))
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
    fn decode(b: &[u8]) -> (String, Enc) {
        if let Some(r) = b.strip_prefix(b"\xEF\xBB\xBF") {
            (String::from_utf8_lossy(r).into(), Enc::Utf8Bom)
        } else if let Some(r) = b.strip_prefix(b"\xFF\xFE") {
            (utf16(r, u16::from_le_bytes), Enc::Utf16Le)
        } else if let Some(r) = b.strip_prefix(b"\xFE\xFF") {
            (utf16(r, u16::from_be_bytes), Enc::Utf16Be)
        } else if let Some(hit) = sniff_utf16(b) {
            // must run before the UTF-8 check: ASCII in UTF-16 is also valid UTF-8
            hit
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

// ponytail: per-char case folding, so "ß" never matches "SS"; good enough for a notepad.
fn char_eq(a: char, b: char, match_case: bool) -> bool {
    a == b || (!match_case && a.to_lowercase().eq(b.to_lowercase()))
}

fn match_at(t: &[char], i: usize, n: &[char], match_case: bool) -> bool {
    t.len() >= i + n.len() && t[i..].iter().zip(n).all(|(&a, &b)| char_eq(a, b, match_case))
}

/// Next match at/after char index `from`, wrapping around. Returns a char range.
// ponytail: naive O(text × needle) scan, fine for notepad-sized files.
fn find(text: &str, needle: &str, from: usize, match_case: bool) -> Option<(usize, usize)> {
    let (t, n): (Vec<char>, Vec<char>) = (text.chars().collect(), needle.chars().collect());
    if n.is_empty() {
        return None;
    }
    let from = from.min(t.len());
    (from..t.len()).chain(0..from).find(|&i| match_at(&t, i, &n, match_case)).map(|i| (i, i + n.len()))
}

/// None if there is nothing to replace.
fn replace_all(text: &str, needle: &str, replacement: &str, match_case: bool) -> Option<String> {
    let (t, n): (Vec<char>, Vec<char>) = (text.chars().collect(), needle.chars().collect());
    if n.is_empty() {
        return None;
    }
    let (mut out, mut i, mut hit) = (String::with_capacity(text.len()), 0, false);
    while i < t.len() {
        if match_at(&t, i, &n, match_case) {
            out.push_str(replacement);
            (i, hit) = (i + n.len(), true);
        } else {
            out.push(t[i]);
            i += 1;
        }
    }
    hit.then_some(out)
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

/// 1-based line and column of char index `idx`.
fn line_col(text: &str, idx: usize) -> (usize, usize) {
    let before = &text[..char_to_byte(text, idx)];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    (line, col)
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
}

impl Notepad {
    /// true = ok to throw away current text
    fn confirm_discard(&self) -> bool {
        !self.dirty || ask("Discard unsaved changes?")
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
        if let Some(path) = FileDialog::new().pick_file() {
            self.load(path);
        }
    }

    fn load(&mut self, path: PathBuf) {
        match std::fs::read(&path) {
            Ok(bytes) => {
                let (text, enc) = Enc::decode(&bytes);
                self.crlf = text.contains("\r\n");
                self.text = text.replace("\r\n", "\n");
                (self.path, self.dirty, self.enc) = (Some(path), false, enc);
            }
            Err(e) => error(format!("Could not open file: {e}")),
        }
    }

    fn save(&mut self, save_as: bool) {
        let path = match (&self.path, save_as) {
            (Some(p), false) => p.clone(),
            _ => match FileDialog::new().set_file_name("untitled.txt").save_file() {
                Some(p) => p,
                None => return,
            },
        };
        let text = if self.crlf { self.text.replace('\n', "\r\n") } else { self.text.clone() };
        let bytes = match self.enc.encode(&text) {
            Some(b) => b,
            None if ask(&format!(
                "The text contains characters that cannot be represented in {}.\nSave as UTF-8 instead?",
                self.enc.name()
            )) =>
            {
                self.enc = Enc::Utf8;
                text.into_bytes()
            }
            None => return,
        };
        match write_atomic(&path, &bytes) {
            Ok(()) => (self.path, self.dirty) = (Some(path), false),
            Err(e) => error(format!("Could not save file: {e}")),
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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

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
                K::new(M::NONE, Key::Escape),
                K::new(M::COMMAND, Key::Plus),
                K::new(M::COMMAND, Key::Equals),
                K::new(M::COMMAND, Key::Minus),
                K::new(M::COMMAND, Key::Num0),
            ]
            .map(|k| i.consume_shortcut(&k))
        });
        if save_as { self.save(true); }
        if n { self.new_file(); }
        if o { self.open(); }
        if s { self.save(false); }
        if f || h { (self.find_open, self.find_focus) = (true, true); }
        if f3 { self.find_next(&ctx); }
        if esc { self.find_open = false; }
        if plus || equals { self.zoom += 0.1; }
        if minus { self.zoom -= 0.1; }
        if zero { self.zoom = 1.0; }
        self.zoom *= ctx.input(|i| i.zoom_delta()); // Ctrl + mouse wheel

        // File dropped onto the window
        if let Some(path) = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf())) {
            if self.confirm_discard() {
                self.load(path);
            }
        }

        // Close guard
        if ctx.input(|i| i.viewport().close_requested()) && !self.confirm_discard() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }

        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New (Ctrl+N)").clicked() { self.new_file(); }
                    if ui.button("Open… (Ctrl+O)").clicked() { self.open(); }
                    if ui.button("Save (Ctrl+S)").clicked() { self.save(false); }
                    if ui.button("Save As… (Ctrl+Shift+S)").clicked() { self.save(true); }
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
                    if std::mem::take(&mut self.find_focus) { r.request_focus(); }
                    if r.changed() { self.not_found = false; }
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
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
            ui.label(format!(
                "Ln {ln}, Col {col}  |  {:.0}%  |  {}  |  {}",
                self.zoom * 100.0,
                self.enc.name(),
                if self.crlf { "CRLF" } else { "LF" }
            ));
        });

        egui::CentralPanel::default().show(ui, |ui| {
            let scroll = if self.no_wrap { egui::ScrollArea::both() } else { egui::ScrollArea::vertical() };
            scroll.show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    let mut font = egui::TextStyle::Monospace.resolve(ui.style());
                    font.size *= self.zoom;
                    let color = ui.visuals().weak_text_color();
                    let lines = self.text.matches('\n').count() + 1;
                    let w = ui.painter().layout_no_wrap(lines.to_string(), font.clone(), color).size().x;
                    let (gutter, _) = ui.allocate_exact_size(egui::vec2(w + 8.0, 0.0), egui::Sense::hover());

                    // Own layouter: zoomed font, and no wrapping when Word Wrap is off.
                    let (editor_font, wrap) = (font.clone(), !self.no_wrap);
                    let mut layouter = move |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                        let width = if wrap { wrap_width } else { f32::INFINITY };
                        let job = egui::text::LayoutJob::simple(
                            buf.as_str().to_owned(),
                            editor_font.clone(),
                            ui.visuals().text_color(),
                            width,
                        );
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

fn main() -> eframe::Result {
    // "Open with" / drag onto the .exe passes the file path as first argument.
    let mut app = Notepad { zoom: 1.0, ..Default::default() };
    if let Some(path) = std::env::args_os().nth(1) {
        app.load(path.into());
    }
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
        assert_eq!(find("äb äb", "äb", 0, true), Some((0, 2)));
        assert_eq!(find("äb äb", "äb", 1, true), Some((3, 5)));
        assert_eq!(find("äb äb", "äb", 4, true), Some((0, 2)));
        assert_eq!(find("abc", "x", 0, true), None);
        assert_eq!(find("abc", "", 0, true), None);
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
        assert_eq!(Enc::decode(&le), (t.to_string(), Enc::Utf16Le));
        assert_eq!(Enc::decode(&be), (t.to_string(), Enc::Utf16Be));
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
        assert_eq!(replace_all("abc", "x", "y", false), None);
    }
}
