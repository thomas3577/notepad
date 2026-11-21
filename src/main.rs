#![windows_subsystem = "windows"]

mod alt_tap;
mod app;
mod dialogs;
mod doc;
mod encoding;
mod files;
mod text;

use eframe::egui;
use std::path::PathBuf;

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

    // "Open with" / dragging files onto the .exe passes their paths as arguments; one tab each.
    let app = app::Notepad::new(std::env::args_os().skip(1).map(PathBuf::from).collect());
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
