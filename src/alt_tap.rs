//! Alt pressed and released on its own (toggles the menu bar).

use eframe::egui;

/// Detects Alt pressed and released on its own, fed with egui events in order.
/// Alt+F4, Alt+Tab, Alt+click and AltGr (German @ € { } [ ] \ |) don't count.
#[derive(Default)]
pub struct AltTap {
    held: bool,
    /// Alt went down and nothing else happened since.
    alone: bool,
}

impl AltTap {
    /// true when this event completes an Alt tap.
    pub fn feed(&mut self, e: &egui::Event) -> bool {
        use egui::{Event, Key};
        match e {
            Event::ModifiersChanged(m) => {
                let tap = !m.alt && self.held && self.alone;
                if m.alt && !self.held {
                    self.alone = true;
                }
                if m.ctrl || m.shift || m.command {
                    self.alone = false; // AltGr arrives as Ctrl+Alt
                }
                self.held = m.alt;
                tap
            }
            // egui reports the Alt key itself as a key event; that's not "another key"
            Event::Key { key: Key::AltLeft | Key::AltRight, .. } => false,
            Event::Key { pressed: true, .. }
            | Event::Text(_)
            | Event::PointerButton { pressed: true, .. }
            | Event::WindowFocused(false) => {
                self.alone = false;
                false
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alt_tap() {
        use egui::{Event, Key, Modifiers};
        let key = |key, pressed| Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers: Modifiers::NONE };
        let alt = Event::ModifiersChanged(Modifiers::ALT);
        let none = Event::ModifiersChanged(Modifiers::NONE);
        let taps = |events: &[Event]| {
            let mut t = AltTap::default();
            events.iter().filter(|e| t.feed(e)).count()
        };
        // the sequence egui 0.37 really sends for Alt down/up (seen in a debug log)
        let tap = [alt.clone(), key(Key::AltLeft, true), none.clone(), key(Key::AltLeft, false)];
        assert_eq!(taps(&tap), 1);
        assert_eq!(taps(&[&tap[..], &tap[..]].concat()), 2);
        // Alt+F4 / Alt+Tab (focus lost) / AltGr+Q
        assert_eq!(taps(&[alt.clone(), key(Key::F4, true), none.clone()]), 0);
        assert_eq!(taps(&[alt.clone(), Event::WindowFocused(false), none.clone()]), 0);
        assert_eq!(taps(&[Event::ModifiersChanged(Modifiers::CTRL | Modifiers::ALT), Event::Text("@".into()), none]), 0);
    }
}
