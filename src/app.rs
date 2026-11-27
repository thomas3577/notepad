//! The Notepad window: tabs, menus, find bar, editor, status bar.

use crate::alt_tap::AltTap;
use crate::dialogs::{Parent, ask, ask_yes_no_cancel, error, file_dialog};
use crate::doc::{self, Doc};
use crate::encoding::Enc;
use crate::session::{self, SavedTab, Session};
use crate::text::{char_to_byte, find, line_col, line_start, replace_all};
use crate::update::{Status, Updater};
use eframe::egui;
use egui::text::{CCursor, CCursorRange};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

/// Upper bound for egui's undo history, which stores a full copy of the text per step.
const UNDO_BUDGET_BYTES: usize = 64 * 1024 * 1024;

#[derive(Default)]
pub struct Notepad {
    /// Open tabs; never empty.
    docs: Vec<Doc>,
    active: usize,
    next_id: u64,
    /// Tab whose editor got keyboard focus last; switching tabs moves focus to the new one.
    focused_doc: Option<u64>,
    find_open: bool,
    find_focus: bool,
    needle: String,
    match_case: bool,
    replacement: String,
    not_found: bool,
    no_wrap: bool,
    last_title: String,
    hide_status_bar: bool,
    /// Alt tapped on its own opens the ☰ menu.
    alt_tap: AltTap,
    /// Text zoom factor, 1.0 = 100 %.
    zoom: f32,
    /// Files from the command line, opened on the first frame so dialogs have a parent window.
    pending_open: Vec<PathBuf>,
    /// Last session, restored on the first frame (before `pending_open`).
    restore: Option<Session>,
    /// File lists handed over by later starts of Notepad (single instance).
    incoming: Option<Receiver<Vec<PathBuf>>>,
    /// Last written session, to skip writing an unchanged one.
    last_session: String,
    session_error_shown: bool,
    updater: Option<Updater>,
    /// Window had focus last frame; regaining it checks for files changed by other programs.
    was_focused: bool,
    /// Go to Line dialog is open, with what was typed.
    goto: Option<String>,
    /// Recently opened or saved files, most recent first.
    recent: Vec<PathBuf>,
    theme: egui::ThemePreference,
    /// Whether the title bar was last made dark.
    dark_title_bar: Option<bool>,
    /// Selection kept while the right mouse button is down on it (egui would collapse it).
    right_click_sel: Option<CCursorRange>,
    /// Editor selection as of the last frame it had focus. egui collapses the selection in
    /// every frame the editor is unfocused, e.g. when a context menu item is clicked.
    editor_sel: Option<CCursorRange>,
}

const MAX_RECENT: usize = 10;

fn menu_id() -> egui::Id {
    egui::Id::unique("menu")
}

impl Notepad {
    /// One empty tab. `restore` and `open` (e.g. from the command line) are loaded on the
    /// first frame; `incoming` delivers files from later starts.
    pub fn new(
        open: Vec<PathBuf>,
        restore: Option<Session>,
        incoming: Option<Receiver<Vec<PathBuf>>>,
        updater: Option<Updater>,
    ) -> Self {
        Notepad {
            docs: vec![Doc::new(0, None)],
            next_id: 1,
            zoom: 1.0,
            pending_open: open,
            restore,
            incoming,
            updater,
            ..Default::default()
        }
    }

    /// Tabs to keep: changed and Untitled tabs with their text, unchanged files by path only.
    fn session(&self) -> Session {
        let keep = |d: &&Doc| !d.is_blank();
        Session {
            tabs: self
                .docs
                .iter()
                .filter(keep)
                .map(|d| SavedTab {
                    path: d.path.clone(),
                    enc: d.enc,
                    crlf: d.crlf,
                    text: (d.dirty || d.path.is_none()).then(|| d.text.clone()),
                })
                .collect(),
            active: self.docs[..self.active].iter().filter(keep).count(),
            zoom: self.zoom,
            no_wrap: self.no_wrap,
            recent: self.recent.clone(),
            theme: self.theme,
            hide_status_bar: self.hide_status_bar,
        }
    }

    fn restore_session(&mut self, s: Session, p: Parent) {
        let mut docs = Vec::new();
        for tab in s.tabs {
            let id = self.take_id();
            let doc = match (tab.text, tab.path) {
                (Some(text), path) => {
                    // ponytail: changes to the file while Notepad was closed go unnoticed (store mtime if that matters)
                    let mtime = path.as_deref().and_then(doc::mtime);
                    Some(Doc { text, enc: tab.enc, crlf: tab.crlf, dirty: true, mtime, ..Doc::new(id, path) })
                }
                // unchanged file: read it again (it may have changed); skip it if it is gone
                (None, Some(path)) if path.exists() => Doc::load(id, path, p),
                (None, _) => None,
            };
            docs.extend(doc);
        }
        if !docs.is_empty() {
            self.active = s.active.min(docs.len() - 1);
            self.docs = docs;
        }
        (self.zoom, self.no_wrap) = (if s.zoom > 0.0 { s.zoom } else { 1.0 }, s.no_wrap);
        (self.recent, self.theme, self.hide_status_bar) = (s.recent, s.theme, s.hide_status_bar);
    }

    fn doc(&self) -> &Doc {
        &self.docs[self.active]
    }

    fn doc_mut(&mut self) -> &mut Doc {
        &mut self.docs[self.active]
    }

    fn take_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id - 1
    }

    fn new_tab(&mut self) {
        let id = self.take_id();
        self.docs.push(Doc::new(id, None));
        self.active = self.docs.len() - 1;
    }

    /// Open in a new tab; switches to the tab if the file is already open, and reuses
    /// an empty Untitled tab.
    fn open_path(&mut self, path: PathBuf, p: Parent) {
        if let Some(i) = self.docs.iter().position(|d| d.path.as_ref() == Some(&path)) {
            self.active = i;
            self.add_recent(&path);
            return;
        }
        let id = self.take_id();
        if let Some(doc) = Doc::load(id, path.clone(), p) {
            self.add_recent(&path);
            if self.doc().is_blank() {
                *self.doc_mut() = doc;
            } else {
                self.docs.push(doc);
                self.active = self.docs.len() - 1;
            }
        }
    }

    fn open(&mut self, p: Parent) {
        for path in file_dialog(p).pick_files().unwrap_or_default() {
            self.open_path(path, p);
        }
    }

    fn add_recent(&mut self, path: &Path) {
        self.recent.retain(|r| r != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(MAX_RECENT);
    }

    /// true if the file was written.
    fn save_doc(&mut self, i: usize, save_as: bool, p: Parent) -> bool {
        let saved = self.docs[i].save(save_as, p);
        if let Some(path) = self.docs[i].path.clone().filter(|_| saved) {
            self.add_recent(&path);
        }
        saved
    }

    fn save(&mut self, save_as: bool, p: Parent) {
        self.save_doc(self.active, save_as, p);
    }

    /// Saves every changed tab; stops when a save is cancelled or fails.
    fn save_all(&mut self, p: Parent) {
        for i in 0..self.docs.len() {
            if !self.docs[i].dirty {
                continue;
            }
            if self.docs[i].path.is_none() {
                self.active = i; // the Save As dialog is for this tab
            }
            if !self.save_doc(i, false, p) {
                return;
            }
        }
    }

    /// Files changed by another program: unchanged tabs are reloaded, changed ones only
    /// after asking (once per change).
    fn check_external_changes(&mut self, p: Parent) {
        for d in &mut self.docs {
            let Some(path) = d.path.clone() else { continue };
            let now = doc::mtime(&path);
            if now.is_none() || now == d.mtime {
                continue; // unchanged, or deleted: keep the text, saving writes it again
            }
            d.mtime = now;
            if d.dirty && !ask(p, &format!("{} was changed by another program.\nReload it and lose your changes?", d.name())) {
                continue;
            }
            if let Some(new) = Doc::load(d.id, path, p) {
                *d = new;
            }
        }
    }

    /// Asks to save a changed tab first; the last tab is replaced by an empty one.
    /// false if the user cancelled.
    fn close_tab(&mut self, i: usize, ctx: &egui::Context, p: Parent) -> bool {
        let d = &mut self.docs[i];
        if d.dirty {
            match ask_yes_no_cancel(p, &format!("Save changes to {}?", d.name())) {
                Some(true) if d.save(false, p) => {}
                Some(false) => {}
                _ => return false,
            }
        }
        // drop egui's state for this editor (its undo history holds copies of the text)
        let id = self.docs.remove(i).editor_id();
        ctx.data_mut(|data| data.remove::<egui::text_edit::TextEditState>(id));
        if self.docs.is_empty() {
            self.new_tab();
        } else if self.active > i || self.active == self.docs.len() {
            self.active -= 1;
        }
        true
    }

    /// Closes all tabs but `keep`; stops when the user cancels.
    fn close_other_tabs(&mut self, keep: usize, ctx: &egui::Context, p: Parent) {
        let keep_id = self.docs[keep].id;
        let others: Vec<u64> = self.docs.iter().map(|d| d.id).filter(|&id| id != keep_id).collect();
        for id in others {
            let i = self.docs.iter().position(|d| d.id == id).expect("still open");
            if !self.close_tab(i, ctx, p) {
                break;
            }
        }
        self.active = self.docs.iter().position(|d| d.id == keep_id).expect("never closed");
    }

    fn go_to_line(&mut self, ctx: &egui::Context, line: usize) {
        let a = line_start(&self.doc().text, line);
        self.select(ctx, a, a);
        self.focused_doc = None; // focus the editor again
    }

    fn next_tab(&mut self, step: isize) {
        let n = self.docs.len() as isize;
        self.active = (self.active as isize + step).rem_euclid(n) as usize;
    }

    fn selection(&self, ctx: &egui::Context) -> Option<(usize, usize)> {
        let r = egui::TextEdit::load_state(ctx, self.doc().editor_id())?.cursor.char_range()?;
        let (a, b) = (r.primary.index, r.secondary.index);
        Some((a.min(b).into(), a.max(b).into()))
    }

    /// Select a range without moving keyboard focus (the find bar keeps it, so a second
    /// Enter searches again instead of overwriting the match).
    fn select(&mut self, ctx: &egui::Context, a: usize, b: usize) {
        let id = self.doc().editor_id();
        let mut s = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
        s.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(a), CCursor::new(b))));
        s.store(ctx, id);
        self.doc_mut().scroll_to = Some(a);
    }

    fn find_next(&mut self, ctx: &egui::Context) {
        if self.needle.is_empty() {
            (self.find_open, self.find_focus) = (true, true);
            return;
        }
        let from = self.selection(ctx).map_or(0, |(_, b)| b);
        let hit = find(&self.doc().text, &self.needle, from, self.match_case);
        self.not_found = hit.is_none();
        if let Some((a, b)) = hit {
            self.select(ctx, a, b);
        }
    }

    fn replace(&mut self, ctx: &egui::Context) {
        if let Some((a, b)) = self.selection(ctx) {
            let (needle, replacement, match_case) = (self.needle.clone(), self.replacement.clone(), self.match_case);
            let d = self.doc_mut();
            let (ba, bb) = (char_to_byte(&d.text, a), char_to_byte(&d.text, b));
            if a != b && find(&d.text[ba..bb], &needle, 0, match_case) == Some((0, b - a)) {
                d.text.replace_range(ba..bb, &replacement);
                d.dirty = true;
                let end = a + replacement.chars().count();
                self.select(ctx, end, end);
            }
        }
        self.find_next(ctx);
    }

    fn replace_all(&mut self) {
        let d = &mut self.docs[self.active];
        match replace_all(&d.text, &self.needle, &self.replacement, self.match_case) {
            Some(t) => (d.text, d.dirty, self.not_found) = (t, true, false),
            None => self.not_found = true,
        }
    }

    /// File/Edit/View/Encoding menus of the ☰ popup.
    fn menus(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, p: Parent) {
        ui.menu_button("File", |ui| {
            if ui.button("New Tab (Ctrl+N)").clicked() { self.new_tab(); }
            if ui.button("Open… (Ctrl+O)").clicked() { self.open(p); }
            if ui.button("Save (Ctrl+S)").clicked() { self.save(false, p); }
            if ui.button("Save As… (Ctrl+Shift+S)").clicked() { self.save(true, p); }
            if ui.button("Save All (Ctrl+Alt+S)").clicked() { self.save_all(p); }
            ui.separator();
            ui.menu_button("Recent Files", |ui| {
                let mut open = None;
                for path in &self.recent {
                    if ui.button(path.display().to_string()).clicked() { open = Some(path.clone()); }
                }
                if self.recent.is_empty() {
                    ui.weak("No recent files");
                } else {
                    ui.separator();
                    if ui.button("Clear List").clicked() { self.recent.clear(); }
                }
                if let Some(path) = open {
                    if path.is_file() {
                        self.open_path(path, p);
                    } else {
                        self.recent.retain(|r| *r != path);
                        error(p, format!("File not found: {}", path.display()));
                    }
                }
            });
            ui.separator();
            if ui.button("Close Tab (Ctrl+W)").clicked() { self.close_tab(self.active, ctx, p); }
        });
        ui.menu_button("Edit", |ui| {
            if ui.button("Find/Replace (Ctrl+F)").clicked() {
                (self.find_open, self.find_focus) = (true, true);
            }
            if ui.button("Find Next (F3)").clicked() { self.find_next(ctx); }
            if ui.button("Go to Line… (Ctrl+G)").clicked() { self.goto = Some(String::new()); }
        });
        ui.menu_button("View", |ui| {
            let mut status_bar = !self.hide_status_bar;
            if ui.checkbox(&mut status_bar, "Status Bar").changed() { self.hide_status_bar = !status_bar; }
            let mut wrap = !self.no_wrap;
            if ui.checkbox(&mut wrap, "Word Wrap").changed() { self.no_wrap = !wrap; }
            ui.separator();
            if ui.button("Zoom In (Ctrl+Plus)").clicked() { self.zoom += 0.1; }
            if ui.button("Zoom Out (Ctrl+Minus)").clicked() { self.zoom -= 0.1; }
            if ui.button("Reset Zoom (Ctrl+0)").clicked() { self.zoom = 1.0; }
            ui.separator();
            ui.menu_button("Theme", |ui| {
                use egui::ThemePreference as T;
                for (theme, name) in [(T::System, "System"), (T::Light, "Light"), (T::Dark, "Dark")] {
                    ui.radio_value(&mut self.theme, theme, name);
                }
            });
        });
        ui.menu_button("Encoding", |ui| {
            let d = self.doc_mut();
            enc_menu(ui, d);
            ui.separator();
            eol_menu(ui, d);
        });
        // the status bar shows the update otherwise
        if let Some(updater) = self.updater.as_ref().filter(|_| self.hide_status_bar) {
            match updater.status() {
                Status::UpToDate => {}
                Status::Available(v) => {
                    ui.separator();
                    if ui.button(format!("Update to v{v}")).on_hover_text("Download and install; Notepad restarts with all tabs").clicked() {
                        updater.install(v);
                    }
                }
                Status::Downloading => {
                    ui.separator();
                    ui.add_enabled(false, egui::Button::new("Updating..."));
                }
                Status::Failed(v, e) => {
                    ui.separator();
                    let text = egui::RichText::new("Update failed, retry").color(ui.visuals().warn_fg_color);
                    if ui.button(text).on_hover_text(e).clicked() {
                        updater.install(v);
                    }
                }
            }
        }
    }

    /// Zoom presets plus in/out/reset, shared by the status bar dropdown.
    fn zoom_menu(&mut self, ui: &mut egui::Ui) {
        for pct in [50, 75, 100, 125, 150, 200, 300, 400] {
            let z = pct as f32 / 100.0;
            if ui.radio((self.zoom - z).abs() < 0.005, format!("{pct}%")).clicked() { self.zoom = z; }
        }
        ui.separator();
        if ui.button("Zoom In (Ctrl+Plus)").clicked() { self.zoom += 0.1; }
        if ui.button("Zoom Out (Ctrl+Minus)").clicked() { self.zoom -= 0.1; }
        if ui.button("Reset Zoom (Ctrl+0)").clicked() { self.zoom = 1.0; }
    }

    /// The title bar: ☰ menu, tabs, window buttons; the free space in between drags the window.
    fn caption(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, p: Parent) {
        const H: f32 = 32.0;
        let (buttons, max_button) = crate::title_bar::window_buttons(ui, H);
        ui.horizontal(|ui| {
            // ☰ is painted: egui's default fonts don't contain the glyph
            let (rect, _) = ui.allocate_exact_size(egui::vec2(36.0, H), egui::Sense::hover());
            let resp = ui.interact(rect, menu_id(), egui::Sense::click());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Menu"));
            let v = ui.style().interact(&resp);
            let c = rect.center() + egui::vec2(0.0, -1.0);
            if resp.hovered() {
                ui.painter().rect_filled(egui::Rect::from_center_size(c, egui::vec2(30.0, 26.0)), 6.0, v.weak_bg_fill);
            }
            for dy in [-4.0, 0.0, 4.0] {
                let y = c.y + dy;
                ui.painter().line_segment([egui::pos2(c.x - 7.0, y), egui::pos2(c.x + 7.0, y)], egui::Stroke::new(1.3, v.fg_stroke.color));
            }
            let update_pending = self.hide_status_bar
                && self.updater.as_ref().is_some_and(|u| matches!(u.status(), Status::Available(_) | Status::Failed(..)));
            if update_pending {
                ui.painter().circle_filled(c + egui::vec2(8.0, -6.0), 3.0, ui.visuals().selection.bg_fill);
            }
            let resp = resp.on_hover_text(if update_pending { "Menu (Alt): update available" } else { "Menu (Alt)" });
            egui::Popup::menu(&resp).show(|ui| self.menus(ui, ctx, p));

            // keep some free caption to grab the window by, however many tabs are open
            let tabs_width = buttons[0].left() - ui.cursor().left() - 48.0;
            let tabs = self.tab_bar(ui, ctx, p, H, tabs_width.max(0.0));

            let height = ui.max_rect().bottom();
            // an open menu may reach into the caption, which must not drag the window then
            let widgets = if egui::Popup::is_any_open(ctx) {
                vec![egui::Rect::EVERYTHING]
            } else {
                vec![rect, tabs, buttons[0], buttons[1]]
            };
            crate::title_bar::set_caption(ctx, height, widgets, max_button);
        });
    }

    /// Tabs like Windows 11 Notepad: the active tab has the editor's color and merges into it;
    /// × inside the tab, a dot instead while it has unsaved changes. Icons are painted
    /// (egui's default fonts lack ✕ and ☰).
    /// Returns the area the tabs take.
    fn tab_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, p: Parent, h: f32, max_width: f32) -> egui::Rect {
        use egui::{Color32, CornerRadius, Rect, Role, Sense, Stroke, WidgetInfo, pos2, vec2};
        let (mut switch_to, mut close, mut close_others, mut save_as, mut add) = (None, None, None, None, false);
        let several = self.docs.len() > 1;
        let v = ui.visuals().clone();
        let font = egui::TextStyle::Body.resolve(ui.style());
        let cross = |painter: &egui::Painter, c: egui::Pos2, color: Color32| {
            let (r, stroke) = (3.5, Stroke::new(1.3, color));
            painter.line_segment([c + vec2(-r, -r), c + vec2(r, r)], stroke);
            painter.line_segment([c + vec2(-r, r), c + vec2(r, -r)], stroke);
        };

        let area = egui::ScrollArea::horizontal()
            .max_width(max_width)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (i, d) in self.docs.iter().enumerate() {
                        let active = i == self.active;
                        let name = d.name();
                        let short: String = if name.chars().count() > 28 {
                            name.chars().take(27).chain(['…']).collect()
                        } else {
                            name.to_string()
                        };
                        let galley = ui.painter().layout_no_wrap(short, font.clone(), Color32::PLACEHOLDER);
                        let w = (12.0 + galley.size().x + 10.0 + 20.0 + 6.0).max(96.0);
                        let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
                        resp.widget_info(|| WidgetInfo::selected(Role::Tab, true, active, &name));
                        let close_rect = Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), vec2(20.0, 20.0));
                        let close_resp = ui.interact(close_rect, ui.scope_id().with(("close-tab", d.id)), Sense::click());
                        close_resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, "Close tab"));
                        let hovered = resp.hovered() || close_resp.hovered();

                        let painter = ui.painter();
                        if active {
                            let top = CornerRadius { nw: 7, ne: 7, sw: 0, se: 0 };
                            painter.rect_filled(rect, top, v.panel_fill);
                        } else if hovered {
                            painter.rect_filled(rect.shrink2(vec2(0.0, 3.0)).translate(vec2(0.0, -1.0)), 6.0, v.widgets.hovered.weak_bg_fill);
                        }
                        let text_color = if active { v.strong_text_color() } else { v.text_color() };
                        painter.galley(pos2(rect.left() + 12.0, rect.center().y - galley.size().y / 2.0), galley, text_color);

                        if close_resp.hovered() {
                            painter.rect_filled(close_rect, 4.0, v.widgets.hovered.bg_fill);
                        }
                        if d.dirty && !hovered {
                            painter.circle_filled(close_rect.center(), 3.5, text_color); // unsaved
                        } else if active || hovered {
                            cross(painter, close_rect.center(), text_color);
                        }

                        let hover = d.path.as_ref().map_or("Untitled".into(), |p| p.display().to_string());
                        let resp = resp.on_hover_text(hover);
                        let close_resp = close_resp.on_hover_text("Close Tab (Ctrl+W)");
                        resp.context_menu(|ui| {
                            if let Some(path) = &d.path {
                                if ui.button("Copy Path").clicked() {
                                    ui.ctx().copy_text(path.display().to_string());
                                }
                                if ui.button("Reveal in File Explorer").clicked() {
                                    reveal_in_explorer(path);
                                }
                                ui.separator();
                            }
                            if ui.button("Save As…").clicked() {
                                save_as = Some(i);
                            }
                            ui.separator();
                            if ui.button("Close Tab").clicked() {
                                close = Some(i);
                            }
                            if ui.add_enabled(several, egui::Button::new("Close Other Tabs")).clicked() {
                                close_others = Some(i);
                            }
                        });
                        if close_resp.clicked() || resp.middle_clicked() || close_resp.middle_clicked() {
                            close = Some(i);
                        } else if resp.clicked() {
                            switch_to = Some(i);
                        }
                    }

                    // + : new tab
                    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, h), Sense::click());
                    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, "New tab"));
                    let c = rect.center() + vec2(0.0, -1.0);
                    if resp.hovered() {
                        ui.painter().rect_filled(Rect::from_center_size(c, vec2(26.0, 24.0)), 6.0, v.widgets.hovered.weak_bg_fill);
                    }
                    let stroke = Stroke::new(1.4, v.text_color());
                    ui.painter().line_segment([c + vec2(-5.5, 0.0), c + vec2(5.5, 0.0)], stroke);
                    ui.painter().line_segment([c + vec2(0.0, -5.5), c + vec2(0.0, 5.5)], stroke);
                    if resp.on_hover_text("New Tab (Ctrl+N)").clicked() {
                        add = true;
                    }
                });
            });
        if let Some(i) = close {
            self.close_tab(i, ctx, p);
        } else if let Some(i) = close_others {
            self.close_other_tabs(i, ctx, p);
        } else if let Some(i) = save_as {
            self.active = i; // the Save As dialog is for this tab
            self.save(true, p);
        } else if let Some(i) = switch_to {
            self.active = i;
        }
        if add {
            self.new_tab();
        }
        let mut rect = area.inner_rect;
        rect.set_width(rect.width().min(area.content_size.x));
        rect
    }

    fn title(&self) -> String {
        let d = self.doc();
        format!("{}{} - Notepad", if d.dirty { "*" } else { "" }, d.name())
    }
}

/// Opens File Explorer with the file selected.
fn enc_menu(ui: &mut egui::Ui, d: &mut Doc) {
    for e in Enc::ALL {
        if ui.radio_value(&mut d.enc, e, e.name()).changed() { d.dirty = true; }
    }
}

fn eol_menu(ui: &mut egui::Ui, d: &mut Doc) {
    if ui.radio_value(&mut d.crlf, true, "Windows (CRLF)").changed()
        | ui.radio_value(&mut d.crlf, false, "Unix (LF)").changed()
    {
        d.dirty = true;
    }
}

fn reveal_in_explorer(path: &std::path::Path) {
    use std::os::windows::process::CommandExt;
    // raw_arg: Explorer parses `/select,"path"` itself and mishandles Rust's argument quoting.
    let arg = format!("/select,\"{}\"", path.display());
    if let Err(e) = std::process::Command::new("explorer.exe").raw_arg(arg).spawn() {
        error(None, format!("Could not open File Explorer: {e}"));
    }
}

impl eframe::App for Notepad {
    /// Called by eframe on exit and every `auto_save_interval`.
    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        let s = session::to_string(&self.session());
        if s == self.last_session {
            return;
        }
        match session::store(&s) {
            Ok(()) => self.last_session = s,
            // once per run: repeating this every few seconds would make Notepad unusable
            Err(e) if !self.session_error_shown => {
                self.session_error_shown = true;
                error(None, format!("Could not save the session (open tabs and unsaved changes): {e}"));
            }
            Err(_) => {}
        }
    }

    fn auto_save_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(5) // cheap: unchanged sessions are not written
    }

    /// eframe only keeps the window position; egui's memory would include every tab's
    /// undo history (full copies of the text).
    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let p = Some(&*frame);

        if let Some(s) = self.restore.take() {
            self.restore_session(s, p);
        }
        for path in std::mem::take(&mut self.pending_open) {
            self.open_path(path, p);
        }
        // Files from a later start of Notepad: open them and come to the front
        let handed_over: Vec<Vec<PathBuf>> = self.incoming.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        if !handed_over.is_empty() {
            for path in handed_over.into_iter().flatten() {
                self.open_path(path, p);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }

        // Back from another program: reload files it changed
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if focused && !self.was_focused {
            self.check_external_changes(p);
        }
        self.was_focused = focused;

        if ctx.options(|o| o.theme_preference) != self.theme {
            ctx.set_theme(self.theme);
        }
        let dark = ctx.global_style().visuals.dark_mode;
        if self.dark_title_bar != Some(dark) {
            self.dark_title_bar = Some(dark);
            crate::title_bar::set_dark(frame, dark);
        }

        // Esc closes Go to Line, else the find bar; otherwise menus and popups need it
        let esc_ours = self.goto.is_some() || self.find_open;
        let [save_all, save_as, prev_tab, n, t, o, s, w, next_tab, f, h, g, f3, esc, plus, equals, minus, zero] = ctx.input_mut(|i| {
            use egui::{Key, KeyboardShortcut as K, Modifiers as M};
            [
                // Shift/Alt variants first: egui's Ctrl+X would also match Ctrl+Shift+X
                K::new(M::COMMAND | M::ALT, Key::S),
                K::new(M::COMMAND | M::SHIFT, Key::S),
                K::new(M::COMMAND | M::SHIFT, Key::Tab),
                K::new(M::COMMAND, Key::N),
                K::new(M::COMMAND, Key::T),
                K::new(M::COMMAND, Key::O),
                K::new(M::COMMAND, Key::S),
                K::new(M::COMMAND, Key::W),
                K::new(M::COMMAND, Key::Tab),
                K::new(M::COMMAND, Key::F),
                K::new(M::COMMAND, Key::H),
                K::new(M::COMMAND, Key::G),
                K::new(M::NONE, Key::F3),
                if esc_ours { K::new(M::NONE, Key::Escape) } else { K::new(M::NONE, Key::F24) },
                K::new(M::COMMAND, Key::Plus),
                K::new(M::COMMAND, Key::Equals),
                K::new(M::COMMAND, Key::Minus),
                K::new(M::COMMAND, Key::Num0),
            ]
            .map(|k| i.consume_shortcut(&k))
        });
        if save_all { self.save_all(p); }
        if save_as { self.save(true, p); }
        if n || t { self.new_tab(); }
        if o { self.open(p); }
        if s { self.save(false, p); }
        if w { self.close_tab(self.active, &ctx, p); }
        if next_tab { self.next_tab(1); }
        if prev_tab { self.next_tab(-1); }
        if f || h { (self.find_open, self.find_focus) = (true, true); }
        if f3 { self.find_next(&ctx); }
        if g { self.goto = Some(String::new()); }
        if esc && self.goto.is_some() {
            self.goto = None;
            self.focused_doc = None;
        } else if esc && self.find_open {
            self.find_open = false;
            self.focused_doc = None; // give the editor its focus back
        }

        // Alt pressed and released on its own opens the menu (like Windows Explorer's menu bar).
        // (every event must reach feed(), so no short-circuiting `any`)
        let mut tapped = false;
        ctx.input(|i| {
            for e in &i.events {
                tapped |= self.alt_tap.feed(e);
            }
        });
        if tapped {
            egui::Popup::toggle_id(&ctx, menu_id().with("popup"));
        }
        if plus || equals { self.zoom += 0.1; }
        if minus { self.zoom -= 0.1; }
        if zero { self.zoom = 1.0; }
        self.zoom *= ctx.input(|i| i.zoom_delta()); // Ctrl + mouse wheel

        // Files dropped onto the window
        for path in ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect::<Vec<_>>()) {
            self.open_path(path, p);
        }

        // No "discard changes?" on close: the session keeps them (eframe calls save() on exit).

        // Tab strip in the title bar, a bit darker than the editor, so the active tab (editor color) stands out
        let strip = {
            let (c, f) = (ui.visuals().panel_fill, if ui.visuals().dark_mode { 0.72 } else { 0.92 });
            egui::Color32::from_rgb((c.r() as f32 * f) as u8, (c.g() as f32 * f) as u8, (c.b() as f32 * f) as u8)
        };
        egui::Panel::top("tabs")
            .frame(egui::Frame::NONE.fill(strip).inner_margin(egui::Margin { left: 6, right: 6, top: 5, bottom: 0 }))
            .show_separator_line(false)
            .show(ui, |ui| self.caption(ui, &ctx, p));

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
                    // × (U+00D7): egui's default fonts have no ✕
                    if ui.button("×").clicked() {
                        self.find_open = false;
                        self.focused_doc = None;
                    }
                });
            });
        }

        if let Some(input) = &mut self.goto {
            let (mut go, mut close) = (false, false);
            let modal = egui::Modal::new(egui::Id::unique("goto")).show(&ctx, |ui| {
                ui.label("Go to line:");
                ui.text_edit_singleline(input).request_focus();
                go = ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    go |= ui.button("Go").clicked();
                    close = ui.button("Cancel").clicked();
                });
            });
            let line = input.trim().parse::<usize>().ok().filter(|_| go);
            if go || close || modal.should_close() {
                self.goto = None;
                self.focused_doc = None;
            }
            if let Some(line) = line {
                self.go_to_line(&ctx, line);
            }
        }

        self.zoom = self.zoom.clamp(0.5, 4.0);

        if !self.hide_status_bar {
            egui::Panel::bottom("status").show(ui, |ui| {
                let cursor = egui::TextEdit::load_state(&ctx, self.doc().editor_id())
                    .and_then(|s| s.cursor.char_range())
                    .map_or(0, |r| r.primary.index.into());
                let d = self.doc();
                let (ln, col) = line_col(&d.text, cursor);
                let (zoom_text, enc_text) = (format!("{:.0}%", self.zoom * 100.0), d.enc.name());
                let eol_text = if d.crlf { "CRLF" } else { "LF" };
                // Flat button that opens a dropdown; background only on hover, like the ☰ button.
                let dropdown = |ui: &mut egui::Ui, text: &str, f: &mut dyn FnMut(&mut egui::Ui)| {
                    ui.label("|");
                    let button = egui::Button::new(text).frame_when_inactive(false);
                    egui::menu::MenuButton::from_button(button).ui(ui, |ui| f(ui));
                };
                ui.horizontal(|ui| {
                    ui.label(format!("Ln {ln}, Col {col}"));
                    dropdown(ui, &zoom_text, &mut |ui| self.zoom_menu(ui));
                    dropdown(ui, enc_text, &mut |ui| enc_menu(ui, self.doc_mut()));
                    dropdown(ui, eol_text, &mut |ui| eol_menu(ui, self.doc_mut()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.weak(concat!("v", env!("CARGO_PKG_VERSION")));
                        // right to left: these appear left of the version
                        let Some(updater) = &self.updater else { return };
                        match updater.status() {
                            Status::UpToDate => {}
                            Status::Available(v) => {
                                let link = ui.link(format!("Update to v{v}"));
                                if link.on_hover_text("Download and install; Notepad restarts with all tabs").clicked() {
                                    updater.install(v);
                                }
                            }
                            Status::Downloading => {
                                ui.weak("Updating...");
                                ui.spinner();
                            }
                            Status::Failed(v, e) => {
                                let text = egui::RichText::new("Update failed, retry").color(ui.visuals().warn_fg_color);
                                if ui.link(text).on_hover_text(e).clicked() {
                                    updater.install(v);
                                }
                            }
                        }
                    });
                });
            });
        }

        let editor_id = self.doc().editor_id();

        // New document in this tab: fresh undo history (sized so large files don't keep
        // ~100 full copies of the text)
        let reset = std::mem::take(&mut self.doc_mut().reset_editor);
        if reset {
            let max_undos = (UNDO_BUDGET_BYTES / self.doc().text.len().max(1)).clamp(2, 100);
            let mut state = egui::text_edit::TextEditState::default();
            // cursor at the start, like Windows Notepad (egui would put it at the end)
            state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(0))));
            state.set_undoer(egui::util::undoer::Undoer::with_settings(egui::util::undoer::Settings {
                max_undos,
                ..Default::default()
            }));
            state.store(&ctx, editor_id);
        }
        // New or switched tab: ready to type right away, like Windows Notepad
        if self.focused_doc != Some(self.doc().id) {
            self.focused_doc = Some(self.doc().id);
            ctx.memory_mut(|m| m.request_focus(editor_id));
        }

        // While the editor is not focused (e.g. typing in the find bar), egui hides the
        // selection, so we paint the selected range (the current match) ourselves.
        let highlight = (!ctx.memory(|m| m.has_focus(editor_id)))
            .then(|| self.selection(&ctx))
            .flatten()
            .filter(|(a, b)| a != b)
            .map(|(a, b)| (char_to_byte(&self.doc().text, a), char_to_byte(&self.doc().text, b)));

        let (zoom, no_wrap) = (self.zoom, self.no_wrap);
        let right_click_sel = &mut self.right_click_sel;
        let editor_sel = &mut self.editor_sel;
        let d = &mut self.docs[self.active];
        egui::CentralPanel::default().show(ui, |ui| {
            // per tab, so every tab keeps its own scroll position
            let scroll = if no_wrap { egui::ScrollArea::both() } else { egui::ScrollArea::vertical() };
            let mut scroll = scroll.id_salt(("scroll", d.id));
            if reset {
                scroll = scroll.scroll_offset(egui::Vec2::ZERO);
            }
            scroll.show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    let mut font = egui::TextStyle::Monospace.resolve(ui.style());
                    font.size *= zoom;
                    let color = ui.visuals().weak_text_color();
                    let lines = d.text.matches('\n').count() + 1;
                    let w = ui.painter().layout_no_wrap(lines.to_string(), font.clone(), color).size().x;
                    let (gutter, _) = ui.allocate_exact_size(egui::vec2(w + 8.0, 0.0), egui::Sense::hover());

                    // Own layouter: zoomed font, optional wrapping, find highlight.
                    let editor_font = font.clone();
                    let mut layouter = move |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                        let text = buf.as_str();
                        let fmt = egui::TextFormat::simple(editor_font.clone(), ui.visuals().text_color());
                        let mut job = egui::text::LayoutJob::default();
                        job.wrap.max_width = if no_wrap { f32::INFINITY } else { wrap_width };
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

                    let before = egui::TextEdit::load_state(ui.ctx(), editor_id).and_then(|s| s.cursor.char_range());
                    let out = egui::TextEdit::multiline(&mut d.text)
                        .id(editor_id)
                        .layouter(&mut layouter)
                        .frame(egui::Frame::NONE)
                        .desired_width(f32::INFINITY)
                        .min_size(ui.available_size())
                        .show(ui);
                    if out.response.changed() { d.dirty = true; }

                    // egui moves the cursor on any mouse button. Like Windows, a right-click inside the
                    // selection keeps it (restored while the button is down); elsewhere it moves the cursor.
                    let (pressed, down) = ui.input(|i| {
                        let b = egui::PointerButton::Secondary;
                        (i.pointer.button_pressed(b), i.pointer.button_down(b) || i.pointer.button_released(b))
                    });
                    if pressed {
                        *right_click_sel = before.filter(|old| {
                            let (a, b) = (old.primary.index.min(old.secondary.index), old.primary.index.max(old.secondary.index));
                            a != b && out.cursor_range.is_some_and(|new| (a..=b).contains(&new.primary.index))
                        });
                    } else if !down {
                        *right_click_sel = None;
                    }
                    let mut cursor_range = out.cursor_range;
                    if let Some(sel) = *right_click_sel
                        && out.cursor_range != Some(sel)
                    {
                        let mut state = out.state.clone();
                        state.cursor.set_char_range(Some(sel));
                        state.store(ui.ctx(), editor_id);
                        cursor_range = Some(sel);
                        ui.ctx().request_repaint(); // replace this frame's collapsed cursor right away
                    }

                    if ui.memory(|m| m.has_focus(editor_id)) {
                        *editor_sel = cursor_range;
                    }

                    // Right-click menu. egui-winit turns these commands into the same Cut/Copy/Paste
                    // events as the keyboard shortcuts (next frame, to the focused editor; undo works).
                    // Each item first puts the selection back and focuses the editor (see `editor_sel`).
                    let has_sel = editor_sel.is_some_and(|r| r.primary != r.secondary);
                    let refocus = |state: &mut egui::text_edit::TextEditState| {
                        state.cursor.set_char_range(*editor_sel);
                        ctx.memory_mut(|m| m.request_focus(editor_id));
                    };
                    out.response.context_menu(|ui| {
                        use egui::ViewportCommand as V;
                        for (label, redo) in [("Undo (Ctrl+Z)", false), ("Redo (Ctrl+Y)", true)] {
                            if !ui.button(label).clicked() {
                                continue;
                            }
                            let mut state = out.state.clone();
                            refocus(&mut state);
                            // ponytail: clones the undo history (up to UNDO_BUDGET_BYTES) once per click
                            let mut undoer = state.undoer();
                            let cur = (state.cursor.char_range().unwrap_or_default(), d.text.clone());
                            let step = if redo { undoer.redo(&cur) } else { undoer.undo(&cur) };
                            if let Some((range, text)) = step {
                                (d.text, d.dirty) = (text.clone(), true);
                                state.cursor.set_char_range(Some(*range));
                            }
                            state.set_undoer(undoer);
                            state.store(&ctx, editor_id);
                        }
                        ui.separator();
                        let items = [("Cut (Ctrl+X)", V::RequestCut, has_sel), ("Copy (Ctrl+C)", V::RequestCopy, has_sel), ("Paste (Ctrl+V)", V::RequestPaste, true)];
                        for (label, cmd, enabled) in items {
                            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                                let mut state = out.state.clone();
                                refocus(&mut state);
                                state.store(&ctx, editor_id);
                                ctx.send_viewport_cmd(cmd);
                            }
                        }
                        ui.separator();
                        if ui.button("Select All (Ctrl+A)").clicked() {
                            let mut state = out.state.clone();
                            let all = CCursorRange::two(CCursor::new(0), CCursor::new(d.text.chars().count()));
                            state.cursor.set_char_range(Some(all));
                            state.store(&ctx, editor_id);
                            ctx.memory_mut(|m| m.request_focus(editor_id));
                        }
                    });

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

                    if let Some(i) = d.scroll_to.take() {
                        let r = out.galley.pos_from_cursor(CCursor::new(i)).translate(out.galley_pos.to_vec2());
                        ui.scroll_to_rect(r, Some(egui::Align::Center));
                    }
                });
            });
        });

        // An Untitled tab emptied again has nothing to save
        for d in &mut self.docs {
            if d.path.is_none() && d.text.is_empty() {
                d.dirty = false;
            }
        }

        // Only on change: every viewport command triggers a repaint, which would loop forever.
        let title = self.title();
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_tabs_keeps_a_sensible_active_tab() {
        let ctx = egui::Context::default();
        let mut app = Notepad::new(vec![], None, None, None);
        app.new_tab();
        app.new_tab(); // ids 0, 1, 2; active = 2
        let ids = |a: &Notepad| a.docs.iter().map(|d| d.id).collect::<Vec<_>>();

        app.close_tab(0, &ctx, None); // closing a tab left of the active one
        assert_eq!((ids(&app), app.doc().id), (vec![1, 2], 2));

        app.close_tab(app.active, &ctx, None); // closing the active, last tab
        assert_eq!((ids(&app), app.doc().id), (vec![1], 1));

        app.close_tab(0, &ctx, None); // closing the only tab leaves a fresh empty one
        assert_eq!(app.docs.len(), 1);
        assert!(app.doc().is_blank() && app.doc().id != 1);

        app.new_tab();
        app.new_tab();
        app.next_tab(1); // wraps around
        assert_eq!(app.active, 0);
        app.next_tab(-1);
        assert_eq!(app.active, 2);
    }

    #[test]
    fn session_keeps_unsaved_text() {
        let mut app = Notepad::new(vec![], None, None, None); // tab 0: blank, not kept
        app.new_tab(); // Untitled with text
        app.doc_mut().text = "draft".into();
        app.doc_mut().dirty = true;
        app.new_tab(); // changed file
        let d = app.doc_mut();
        (d.path, d.text, d.dirty) = (Some("C:\\x\\changed.txt".into()), "new".into(), true);
        app.new_tab(); // unchanged file: path only (this one no longer exists)
        app.doc_mut().path = Some("C:\\does-not-exist\\clean.txt".into());
        (app.active, app.zoom) = (2, 1.5);

        let s = app.session();
        let texts: Vec<_> = s.tabs.iter().map(|t| t.text.as_deref()).collect();
        assert_eq!(texts, [Some("draft"), Some("new"), None]);
        assert_eq!(s.active, 1); // index among the kept tabs

        let mut restored = Notepad::new(vec![], None, None, None);
        restored.restore_session(s, None);
        let tabs: Vec<_> = restored.docs.iter().map(|d| (d.text.as_str(), d.dirty, d.path.is_some())).collect();
        assert_eq!(tabs, [("draft", true, false), ("new", true, true)]); // missing file skipped
        assert_eq!((restored.active, restored.zoom), (1, 1.5));
    }

    /// The context menu's Copy relies on this: after a click outside the editor (the menu
    /// item) egui has collapsed the selection, so it is put back before the Copy event arrives.
    #[test]
    fn copy_after_click_elsewhere_needs_the_selection_restored() {
        let ctx = egui::Context::default();
        let id = egui::Id::unique("test-editor");
        let mut text = String::from("hello world");
        let hello = CCursorRange::two(CCursor::new(0), CCursor::new(5));
        // one frame; `restore`: put the selection back and focus the editor (the menu item's job)
        let frame = |text: &mut String, events: Vec<egui::Event>, restore: bool| {
            let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0)));
            let mut out = ctx.run_ui(egui::RawInput { screen_rect, events, ..Default::default() }, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let out = egui::TextEdit::multiline(text).id(id).show(ui);
                    if restore {
                        let mut state = out.state;
                        state.cursor.set_char_range(Some(hello));
                        state.store(ui.ctx(), id);
                        ui.memory_mut(|m| m.request_focus(id));
                    }
                });
            });
            out.textures_delta.clear();
            out.platform_output.commands
        };
        frame(&mut text, vec![], false);
        ctx.memory_mut(|m| m.request_focus(id));
        frame(&mut text, vec![], true);

        // click outside the editor, as on a menu item
        let at = egui::pos2(790.0, 590.0);
        let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(&mut text, vec![egui::Event::PointerMoved(at), button(true)], false);
        frame(&mut text, vec![button(false)], false);
        let collapsed = egui::TextEdit::load_state(&ctx, id).unwrap().cursor.char_range().unwrap();
        assert_eq!(collapsed.primary, collapsed.secondary, "egui collapses the selection (else this workaround is moot)");

        frame(&mut text, vec![], true);
        let commands = frame(&mut text, vec![egui::Event::Copy], false);
        assert!(commands.contains(&egui::OutputCommand::CopyText("hello".into())), "{commands:?}");
    }
}
