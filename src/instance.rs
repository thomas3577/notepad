//! Single instance: a second start hands its files to the running Notepad (as new tabs)
//! and exits, so there is one window and one session.

use interprocess::local_socket::{GenericNamespaced, Listener, ListenerOptions, Stream, prelude::*};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

pub enum Start {
    /// We are the first Notepad: `serve` this listener.
    Primary(Listener),
    /// The files went to the running Notepad: exit.
    Forwarded,
    /// The pipe is unusable: run on our own.
    Alone,
}

/// Per user, so two people on one machine (Remote Desktop) each get their own Notepad.
fn pipe_name() -> String {
    let user: String = std::env::var("USERNAME").unwrap_or_default().chars().filter(|c| c.is_alphanumeric()).collect();
    format!("rust-notepad-{user}.sock")
}

/// `files` must be absolute: the running Notepad has a different working directory.
pub fn start(files: &[PathBuf]) -> Start {
    start_named(&pipe_name(), files)
}

fn start_named(name: &str, files: &[PathBuf]) -> Start {
    // ponytail: no lock between "connect" and "create"; two starts at the same moment
    // just retry a few times.
    for _ in 0..5 {
        let Ok(ns) = name.to_ns_name::<GenericNamespaced>() else { return Start::Alone };
        if let Ok(mut conn) = Stream::connect(ns) {
            allow_foreground();
            let msg = files.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>().join("\n");
            // dropping `conn` closes it, which ends the message
            return if conn.write_all(msg.as_bytes()).is_ok() { Start::Forwarded } else { Start::Alone };
        }
        let Ok(ns) = name.to_ns_name::<GenericNamespaced>() else { return Start::Alone };
        match ListenerOptions::new().name(ns).create_sync() {
            Ok(listener) => return Start::Primary(listener),
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => return Start::Alone,
        }
    }
    Start::Alone
}

/// Receive handed-over file lists on a background thread; `wake` repaints the window.
/// An empty list means "just bring the window to the front".
pub fn serve(listener: Listener, wake: impl Fn() + Send + 'static) -> Receiver<Vec<PathBuf>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for mut conn in listener.incoming().filter_map(Result::ok) {
            let mut msg = String::new();
            if conn.read_to_string(&mut msg).is_err() {
                continue;
            }
            let files = msg.lines().filter(|l| !l.is_empty()).map(PathBuf::from).collect();
            if tx.send(files).is_err() {
                break; // window closed
            }
            wake();
        }
    });
    rx
}

/// Windows only lets the process the user just started take the foreground (that's us,
/// the second start); pass that right on so the running Notepad can come to the front.
#[cfg(windows)]
fn allow_foreground() {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn AllowSetForegroundWindow(process_id: u32) -> i32;
    }
    const ASFW_ANY: u32 = u32::MAX;
    // SAFETY: plain Win32 call without pointers.
    unsafe {
        AllowSetForegroundWindow(ASFW_ANY);
    }
}

#[cfg(not(windows))]
fn allow_foreground() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_start_hands_files_to_the_first() {
        let name = format!("rust-notepad-test-{}.sock", std::process::id());
        let Start::Primary(listener) = start_named(&name, &[]) else { panic!("first start must be primary") };
        let rx = serve(listener, || {});

        let files = vec![PathBuf::from("C:\\a b.txt"), PathBuf::from("D:\\Grüße.txt")];
        assert!(matches!(start_named(&name, &files), Start::Forwarded));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), files);

        // start without files: only "come to the front"
        assert!(matches!(start_named(&name, &[]), Start::Forwarded));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Vec::<PathBuf>::new());
    }
}
