//! The Notepad window: document state, menus, find bar, editor, status bar.

use crate::alt_tap::AltTap;
use crate::dialogs::{Parent, ask, error, file_dialog, warn};
use crate::encoding::Enc;
use crate::files::write_atomic;
use crate::text::{char_to_byte, find, line_col, replace_all};
use eframe::egui;
use egui::text::{CCursor, CCursorRange};
use std::borrow::Cow;
use std::path::PathBuf;

/// egui keeps far more memory per character than the file needs (layout data for every glyph).
/// Measured: a 5.6 MB file needs about 1.3 GB.
const MEMORY_PER_TEXT_BYTE: u64 = 240;
/// Ask before opening files whose estimated memory use exceeds this.
const LARGE_FILE_BYTES: u64 = 10 * 1024 * 1024;
/// Upper bound for egui's undo history, which stores a full copy of the text per step.
const UNDO_BUDGET_BYTES: usize = 64 * 1024 * 1024;

/// Line ending for new documents: CRLF on Windows, like Windows Notepad.
const NEW_DOC_CRLF: bool = cfg!(windows);

fn editor_id() -> egui::Id {
    egui::Id::unique("editor")
}

#[derive(Default)]
pub struct Notepad {
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
    /// Menu bar is hidden by default; Alt or View > Show Menu Bar shows it, ☰ always works.
    show_menu: bool,
    alt_tap: AltTap,
    /// Text zoom factor, 1.0 = 100 %.
    zoom: f32,
    /// New document: clear egui's editor state (undo history, cursor) and scroll position.
    reset_editor: bool,
    /// File from the command line, opened on the first frame so dialogs have a parent window.
    pending_open: Option<PathBuf>,
}

impl Notepad {
    /// Empty document; `open` (e.g. from the command line) is loaded on the first frame.
    pub fn new(open: Option<PathBuf>) -> Self {
        Self { zoom: 1.0, crlf: NEW_DOC_CRLF, reset_editor: true, pending_open: open, ..Default::default() }
    }

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

    /// File/Edit/View/Encoding menus, used by the menu bar and the ☰ popup.
    fn menus(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, p: Parent) {
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
            if ui.button("Find Next (F3)").clicked() { self.find_next(ctx); }
        });
        ui.menu_button("View", |ui| {
            ui.checkbox(&mut self.show_menu, "Show Menu Bar (Alt)");
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

        // Esc closes the find bar, else the menu bar; otherwise menus and popups need it
        let esc_ours = self.find_open || self.show_menu;
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
                if esc_ours { K::new(M::NONE, Key::Escape) } else { K::new(M::NONE, Key::F24) },
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
        if esc && self.find_open {
            self.find_open = false;
            ctx.memory_mut(|m| m.request_focus(editor_id()));
        } else if esc {
            self.show_menu = false;
        }

        // Alt pressed and released on its own toggles the menu bar (like Windows Explorer).
        // (every event must reach feed(), so no short-circuiting `any`)
        let mut tapped = false;
        ctx.input(|i| {
            for e in &i.events {
                tapped |= self.alt_tap.feed(e);
            }
        });
        if tapped {
            self.show_menu = !self.show_menu;
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

        if self.show_menu {
            egui::Panel::top("menu").show(ui, |ui| {
                egui::MenuBar::new().ui(ui, |ui| self.menus(ui, &ctx, p));
            });
        }

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
                // Same menus as the menu bar, as a popup with submenus. The ☰ icon is painted:
                // egui's default fonts don't contain the ☰ glyph.
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(22.0, 18.0), egui::Sense::click());
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Menu"));
                let v = ui.style().interact(&resp);
                ui.painter().rect_filled(rect, 3.0, v.weak_bg_fill);
                for dy in [-4.0, 0.0, 4.0] {
                    let y = rect.center().y + dy;
                    let x = rect.center().x;
                    ui.painter().line_segment([egui::pos2(x - 6.0, y), egui::pos2(x + 6.0, y)], egui::Stroke::new(1.5, v.fg_stroke.color));
                }
                let resp = resp.on_hover_text("Menu (Alt)");
                egui::Popup::menu(&resp).show(|ui| self.menus(ui, &ctx, p));
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
