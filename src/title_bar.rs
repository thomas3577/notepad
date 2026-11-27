//! Tabs in the title bar: the window keeps its native frame (shadow, resizing, Aero Snap,
//! Snap Layouts) but has no caption; egui paints the tab strip and the window buttons there.

use eframe::egui::{self, Color32, Rect, Sense, Stroke, pos2, vec2};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::{Mutex, MutexGuard};

type Hwnd = isize;

#[repr(C)]
#[derive(Default)]
struct Win32Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
struct TrackMouseEventParams {
    size: u32,
    flags: u32,
    hwnd: Hwnd,
    hover_time: u32,
}

type SubclassProc = unsafe extern "system" fn(Hwnd, u32, usize, isize, usize, usize) -> isize;

#[link(name = "comctl32")]
unsafe extern "system" {
    fn SetWindowSubclass(hwnd: Hwnd, proc: SubclassProc, id: usize, data: usize) -> i32;
    fn DefSubclassProc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn SetWindowPos(hwnd: Hwnd, after: Hwnd, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
    fn GetWindowRect(hwnd: Hwnd, rect: *mut Win32Rect) -> i32;
    fn GetClientRect(hwnd: Hwnd, rect: *mut Win32Rect) -> i32;
    fn ScreenToClient(hwnd: Hwnd, point: *mut Point) -> i32;
    fn IsZoomed(hwnd: Hwnd) -> i32;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    fn GetDpiForWindow(hwnd: Hwnd) -> u32;
    fn GetSystemMetricsForDpi(index: i32, dpi: u32) -> i32;
    fn TrackMouseEvent(params: *mut TrackMouseEventParams) -> i32;
}

const WM_NCCALCSIZE: u32 = 0x0083;
const WM_NCHITTEST: u32 = 0x0084;
const WM_NCMOUSEMOVE: u32 = 0x00A0;
const WM_NCLBUTTONDOWN: u32 = 0x00A1;
const WM_NCLBUTTONUP: u32 = 0x00A2;
const WM_NCLBUTTONDBLCLK: u32 = 0x00A3;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_NCMOUSELEAVE: u32 = 0x02A2;
const HTCLIENT: isize = 1;
const HTCAPTION: isize = 2;
const HTMAXBUTTON: isize = 9;
const HTTOP: isize = 12;

/// The caption as egui laid it out in the last frame (points), for the window procedure.
struct Caption {
    height: f32,
    /// Clickable parts of the caption; the rest of it drags the window.
    widgets: Vec<Rect>,
    /// Reported to Windows as the maximize button, which shows Snap Layouts on hover.
    max_button: Rect,
    max_hovered: bool,
    pixels_per_point: f32,
    ctx: Option<egui::Context>,
}

static CAPTION: Mutex<Caption> = Mutex::new(Caption {
    height: 0.0,
    widgets: Vec::new(),
    max_button: Rect::NOTHING,
    max_hovered: false,
    pixels_per_point: 1.0,
    ctx: None,
});

fn caption() -> MutexGuard<'static, Caption> {
    CAPTION.lock().unwrap_or_else(|e| e.into_inner())
}

fn hwnd(window: &impl HasWindowHandle) -> Option<Hwnd> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

/// winit switches the title bar with an undocumented call that Windows 11 applies only
/// partly (e.g. dark background with black text), so the documented attribute is set too.
/// It still colors the window border.
pub fn set_dark(window: &impl HasWindowHandle, dark: bool) {
    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(hwnd: isize, attribute: u32, value: *const core::ffi::c_void, size: u32) -> i32;
    }
    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;

    let Some(hwnd) = hwnd(window) else { return };
    let value = i32::from(dark);
    // SAFETY: valid window handle, and a pointer to a 4-byte BOOL as the attribute expects.
    unsafe {
        DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, (&raw const value).cast(), 4);
    }
}

/// Removes the native caption. Call once, before the window is shown.
pub fn install(window: &impl HasWindowHandle, ctx: &egui::Context) {
    let Some(hwnd) = hwnd(window) else { return };
    caption().ctx = Some(ctx.clone());
    // SAFETY: valid window handle owned by this thread; the subclass lives as long as the window.
    unsafe {
        let client_height = || {
            let mut r = Win32Rect::default();
            GetClientRect(hwnd, &mut r);
            r.bottom - r.top
        };
        let before = client_height();
        SetWindowSubclass(hwnd, subclass_proc, 1, 0);
        const SWP_NOSIZE: u32 = 0x1;
        const SWP_NOMOVE: u32 = 0x2;
        const SWP_NOZORDER: u32 = 0x4;
        const SWP_NOACTIVATE: u32 = 0x10;
        const SWP_FRAMECHANGED: u32 = 0x20;
        SetWindowPos(hwnd, 0, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        // eframe restores the saved client size; without the caption the client area grew by
        // its height, and the window would grow by that on every start.
        if IsZoomed(hwnd) == 0 {
            let grown = client_height() - before;
            let mut r = Win32Rect::default();
            GetWindowRect(hwnd, &mut r);
            SetWindowPos(hwnd, 0, 0, 0, r.right - r.left, r.bottom - r.top - grown, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }
}

/// Thickness of the resize border, which a maximized window has outside the screen.
unsafe fn frame_thickness(hwnd: Hwnd) -> i32 {
    const SM_CYFRAME: i32 = 33;
    const SM_CXPADDEDBORDER: i32 = 92;
    // SAFETY: plain queries on a valid window handle.
    unsafe {
        let dpi = GetDpiForWindow(hwnd);
        GetSystemMetricsForDpi(SM_CYFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi)
    }
}

fn set_max_hovered(hovered: bool) {
    let ctx = {
        let mut c = caption();
        if c.max_hovered == hovered {
            return;
        }
        c.max_hovered = hovered;
        c.ctx.clone()
    };
    if let Some(ctx) = ctx {
        ctx.request_repaint();
    }
}

unsafe extern "system" fn subclass_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize, _id: usize, _data: usize) -> isize {
    // SAFETY: called by Windows with valid arguments for `msg`.
    unsafe {
        match msg {
            WM_NCCALCSIZE if wparam != 0 => {
                // lparam points to NCCALCSIZE_PARAMS, whose first field is the new window rect
                let rect = lparam as *mut Win32Rect;
                let top = (*rect).top;
                let result = DefSubclassProc(hwnd, msg, wparam, lparam);
                (*rect).top = top + if IsZoomed(hwnd) != 0 { frame_thickness(hwnd) } else { 0 };
                return result;
            }
            WM_NCHITTEST => {
                let hit = DefSubclassProc(hwnd, msg, wparam, lparam);
                if hit != HTCLIENT {
                    return hit;
                }
                let mut p = Point { x: (lparam & 0xFFFF) as i16 as i32, y: ((lparam >> 16) & 0xFFFF) as i16 as i32 };
                ScreenToClient(hwnd, &mut p);
                if IsZoomed(hwnd) == 0 && p.y < frame_thickness(hwnd) {
                    return HTTOP;
                }
                let c = caption();
                let pos = pos2(p.x as f32, p.y as f32) / c.pixels_per_point;
                return if pos.y >= c.height || c.widgets.iter().any(|r| r.contains(pos)) {
                    HTCLIENT
                } else if c.max_button.contains(pos) {
                    HTMAXBUTTON
                } else {
                    HTCAPTION
                };
            }
            WM_NCMOUSEMOVE => {
                set_max_hovered(wparam as isize == HTMAXBUTTON);
                const TME_LEAVE: u32 = 0x2;
                const TME_NONCLIENT: u32 = 0x10;
                let mut t = TrackMouseEventParams { size: size_of::<TrackMouseEventParams>() as u32, flags: TME_LEAVE | TME_NONCLIENT, hwnd, hover_time: 0 };
                TrackMouseEvent(&mut t);
            }
            WM_NCMOUSELEAVE | WM_MOUSEMOVE => set_max_hovered(false),
            // DefWindowProc would paint a classic maximize button over ours
            WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if wparam as isize == HTMAXBUTTON => return 0,
            WM_NCLBUTTONUP if wparam as isize == HTMAXBUTTON => {
                const SW_MAXIMIZE: i32 = 3;
                const SW_RESTORE: i32 = 9;
                ShowWindow(hwnd, if IsZoomed(hwnd) != 0 { SW_RESTORE } else { SW_MAXIMIZE });
                return 0;
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, wparam, lparam)
    }
}

/// Records this frame's caption: its height and the clickable rects in it (points).
pub fn set_caption(ctx: &egui::Context, height: f32, widgets: Vec<Rect>, max_button: Rect) {
    let mut c = caption();
    (c.height, c.widgets, c.max_button, c.pixels_per_point) = (height, widgets, max_button, ctx.pixels_per_point());
}

/// Paints minimize, maximize/restore and close at the top right of the window.
/// Returns the rects of minimize and close (egui handles them) and of maximize (Windows does).
pub fn window_buttons(ui: &mut egui::Ui, height: f32) -> ([Rect; 2], Rect) {
    const W: f32 = 46.0;
    let right = ui.ctx().content_rect().right();
    let rect = |i: f32| Rect::from_min_size(pos2(right - W * i, 0.0), vec2(W, height));
    let (close, max, min) = (rect(1.0), rect(2.0), rect(3.0));
    let v = ui.visuals().clone();
    let painter = ui.painter().with_clip_rect(ui.ctx().content_rect());
    let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
    let stroke = |color| Stroke::new(1.0, color);

    let min_resp = ui.interact(min, egui::Id::unique("window-minimize"), Sense::click());
    min_resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Minimize"));
    if min_resp.hovered() {
        painter.rect_filled(min, 0.0, v.widgets.hovered.weak_bg_fill);
    }
    let c = min.center();
    painter.line_segment([c + vec2(-5.0, 0.0), c + vec2(5.0, 0.0)], stroke(v.text_color()));
    if min_resp.clicked() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    }

    if caption().max_hovered {
        painter.rect_filled(max, 0.0, v.widgets.hovered.weak_bg_fill);
    }
    let c = max.center();
    if maximized {
        let back = Rect::from_center_size(c + vec2(1.5, -1.5), vec2(8.0, 8.0));
        painter.line_segment([back.left_top(), back.right_top()], stroke(v.text_color()));
        painter.line_segment([back.right_top(), back.right_bottom()], stroke(v.text_color()));
        painter.rect_stroke(Rect::from_center_size(c + vec2(-1.0, 1.0), vec2(8.0, 8.0)), 1.0, stroke(v.text_color()), egui::StrokeKind::Middle);
    } else {
        painter.rect_stroke(Rect::from_center_size(c, vec2(10.0, 10.0)), 1.0, stroke(v.text_color()), egui::StrokeKind::Middle);
    }

    let close_resp = ui.interact(close, egui::Id::unique("window-close"), Sense::click());
    close_resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Close"));
    let color = if close_resp.hovered() {
        painter.rect_filled(close, 0.0, Color32::from_rgb(0xC4, 0x2B, 0x1C));
        Color32::WHITE
    } else {
        v.text_color()
    };
    let (c, r) = (close.center(), 5.0);
    painter.line_segment([c + vec2(-r, -r), c + vec2(r, r)], stroke(color));
    painter.line_segment([c + vec2(-r, r), c + vec2(r, -r)], stroke(color));
    if close_resp.clicked() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
    }

    ([min, close], max)
}
