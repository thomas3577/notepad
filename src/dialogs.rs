//! Native message and file dialogs (rfd), attached to the main window.

use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

/// Parent window for native dialogs, so they stay on top of Notepad (None before the window exists).
pub type Parent<'a> = Option<&'a eframe::Frame>;

fn message(p: Parent) -> MessageDialog {
    let d = MessageDialog::new().set_title("Notepad");
    match p {
        Some(f) => d.set_parent(f),
        None => d,
    }
}

pub fn file_dialog(p: Parent) -> FileDialog {
    match p {
        Some(f) => FileDialog::new().set_parent(f),
        None => FileDialog::new(),
    }
}

pub fn error(p: Parent, msg: String) {
    message(p).set_level(MessageLevel::Error).set_description(msg).show();
}

pub fn warn(p: Parent, msg: String) {
    message(p).set_level(MessageLevel::Warning).set_description(msg).show();
}

pub fn ask(p: Parent, msg: &str) -> bool {
    message(p).set_description(msg).set_buttons(MessageButtons::YesNo).show() == MessageDialogResult::Yes
}
