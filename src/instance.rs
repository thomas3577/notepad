//! Single instance: a second start hands its files to the running Notepad (as new tabs)
//! and exits, so there is one window and one session.

use interprocess::local_socket::{GenericNamespaced, Listener, ListenerOptions, Stream, prelude::*};
use std::io::{self, BufRead, BufReader, Read, Write};
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
        match try_forward(name, files) {
            Some(true) => return Start::Forwarded,
            // A Notepad is running but didn't confirm: open the files ourselves rather
            // than lose them (worst case they are open twice).
            Some(false) => return Start::Alone,
            None => {} // nobody listening yet
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

/// None: no Notepad is listening. Some(true): handed over and confirmed.
/// Some(false): a Notepad is there but the hand-over failed or took over 5 s.
fn try_forward(name: &str, files: &[PathBuf]) -> Option<bool> {
    let (tx, rx) = mpsc::channel();
    let (name, files) = (name.to_owned(), files.to_vec());
    // On a thread, because a busy pipe makes connect wait forever (WaitNamedPipe) and a hung
    // Notepad never sends its confirmation; this start must not hang invisibly.
    std::thread::spawn(move || {
        let result = (|| {
            let conn = Stream::connect(name.to_ns_name::<GenericNamespaced>().ok()?).ok()?;
            allow_foreground();
            Some(hand_over(conn, &files).is_ok())
        })();
        let _ = tx.send(result);
    });
    rx.recv_timeout(Duration::from_secs(5)).unwrap_or(Some(false))
}

/// Protocol: one path per line, then an empty line; the running Notepad answers with
/// one byte. The sender must keep the connection open until then: Windows drops a pipe
/// connection that was closed before the server accepted it, and with it the files
/// (e.g. several .txt files opened from Explorer at once start several Notepads).
fn hand_over(mut conn: Stream, files: &[PathBuf]) -> io::Result<()> {
    let mut msg: String = files.iter().map(|p| format!("{}\n", p.to_string_lossy())).collect();
    msg.push('\n');
    conn.write_all(msg.as_bytes())?;
    conn.read_exact(&mut [0u8]) // waits; try_forward limits that to 5 s
}

/// Receive handed-over file lists on a background thread; `wake` repaints the window.
/// An empty list means "just bring the window to the front".
pub fn serve(listener: Listener, wake: impl Fn() + Send + 'static) -> Receiver<Vec<PathBuf>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for conn in listener.incoming().filter_map(Result::ok) {
            let mut conn = BufReader::new(conn);
            let mut files = Vec::new();
            let mut line = String::new();
            let complete = loop {
                line.clear();
                match conn.read_line(&mut line) {
                    Ok(0) | Err(_) => break false, // sender gone before the end marker
                    Ok(_) if line.trim_end_matches(['\r', '\n']).is_empty() => break true,
                    Ok(_) => files.push(PathBuf::from(line.trim_end_matches(['\r', '\n']))),
                }
            };
            if !complete {
                continue;
            }
            let _ = conn.get_mut().write_all(b"k"); // the sender may exit now
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

    /// A running Notepad that never answers must not make a new start hang invisibly.
    #[test]
    fn hung_notepad_does_not_block_a_start() {
        let name = format!("rust-notepad-test-hung-{}.sock", std::process::id());
        let Start::Primary(_listener) = start_named(&name, &[]) else { panic!("first start must be primary") };
        // never served: like a Notepad whose pipe thread hangs
        let t = std::time::Instant::now();
        assert!(matches!(start_named(&name, &[PathBuf::from("C:\\x.txt")]), Start::Alone));
        assert!(t.elapsed() < Duration::from_secs(8));
    }

    /// Several .txt files opened from Explorer at once start several Notepads at the same
    /// moment; every file must arrive (this lost files before the acknowledgement).
    #[test]
    fn simultaneous_starts_lose_no_files() {
        let name = format!("rust-notepad-test-burst-{}.sock", std::process::id());
        let Start::Primary(listener) = start_named(&name, &[]) else { panic!("first start must be primary") };
        let rx = serve(listener, || {});

        let starts: Vec<_> = (0..8)
            .map(|i| {
                let name = name.clone();
                std::thread::spawn(move || matches!(start_named(&name, &[PathBuf::from(format!("C:\\{i}.txt"))]), Start::Forwarded))
            })
            .collect();
        assert!(starts.into_iter().all(|t| t.join().unwrap()), "every start hands over");

        let mut got: Vec<String> = (0..8)
            .map(|_| rx.recv_timeout(Duration::from_secs(5)).expect("all files arrive")[0].to_string_lossy().into())
            .collect();
        got.sort();
        assert_eq!(got, (0..8).map(|i| format!("C:\\{i}.txt")).collect::<Vec<_>>());
    }
}
