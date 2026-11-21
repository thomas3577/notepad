//! Detecting, decoding and encoding text file encodings.

use std::borrow::Cow;

#[derive(Clone, Copy, PartialEq, Default, Debug, serde::Serialize, serde::Deserialize)]
pub enum Enc {
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

/// Decoded text plus whether invalid bytes had to be replaced with U+FFFD.
pub type Decoded = (String, Enc, bool);

fn units(b: &[u8], f: fn([u8; 2]) -> u16) -> Vec<u16> {
    b.as_chunks::<2>().0.iter().map(|&c| f(c)).collect()
}

/// Lossy UTF-16 decode; reports invalid surrogates and a dangling odd byte.
fn utf16(b: &[u8], f: fn([u8; 2]) -> u16) -> (String, bool) {
    let u = units(b, f);
    let (mut s, mut lossy) = match String::from_utf16(&u) {
        Ok(s) => (s, false),
        Err(_) => (String::from_utf16_lossy(&u), true),
    };
    if b.len() % 2 == 1 {
        s.push(char::REPLACEMENT_CHARACTER);
        lossy = true;
    }
    (s, lossy)
}

/// UTF-16 without BOM: in Latin-script text nearly every high byte is 0.
// ponytail: NUL-byte heuristic, misses BOM-less UTF-16 that is mostly non-Latin (e.g. CJK).
fn sniff_utf16(b: &[u8]) -> Option<Decoded> {
    if b.len() < 2 || b.len() % 2 == 1 {
        return None;
    }
    let pairs = b.len() / 2;
    let zeros = |start: usize| b.iter().skip(start).step_by(2).filter(|&&c| c == 0).count();
    let (even, odd) = (zeros(0), zeros(1));
    // more than half of one byte column is zero, under a tenth of the other
    let (f, enc): (fn([u8; 2]) -> u16, _) = if odd > pairs / 2 && even < pairs / 10 {
        (u16::from_le_bytes, Enc::Utf16Le)
    } else if even > pairs / 2 && odd < pairs / 10 {
        (u16::from_be_bytes, Enc::Utf16Be)
    } else {
        return None;
    };
    String::from_utf16(&units(b, f)).ok().map(|s| (s, enc, false))
}

impl Enc {
    pub const ALL: [Enc; 5] = [Enc::Utf8, Enc::Utf8Bom, Enc::Utf16Le, Enc::Utf16Be, Enc::Win1252];

    pub fn name(self) -> &'static str {
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
    pub fn decode(b: &[u8]) -> Decoded {
        if let Some(r) = b.strip_prefix(b"\xEF\xBB\xBF") {
            let s = String::from_utf8_lossy(r);
            let lossy = matches!(s, Cow::Owned(_));
            (s.into_owned(), Enc::Utf8Bom, lossy)
        } else if let Some(r) = b.strip_prefix(b"\xFF\xFE") {
            let (s, lossy) = utf16(r, u16::from_le_bytes);
            (s, Enc::Utf16Le, lossy)
        } else if let Some(r) = b.strip_prefix(b"\xFE\xFF") {
            let (s, lossy) = utf16(r, u16::from_be_bytes);
            (s, Enc::Utf16Be, lossy)
        } else if let Some(hit) = sniff_utf16(b) {
            // must run before the UTF-8 check: ASCII in UTF-16 is also valid UTF-8
            hit
        } else if let Ok(s) = std::str::from_utf8(b) {
            (s.into(), Enc::Utf8, false)
        } else {
            let s = b.iter().map(|&c| if (0x80..0xA0).contains(&c) { W1252[(c - 0x80) as usize] } else { c as char });
            (s.collect(), Enc::Win1252, false)
        }
    }

    /// None = text contains characters this encoding can't represent.
    pub fn encode(self, s: &str) -> Option<Cow<'_, [u8]>> {
        Some(match self {
            Enc::Utf8 => Cow::Borrowed(s.as_bytes()),
            Enc::Utf8Bom => Cow::Owned([b"\xEF\xBB\xBF", s.as_bytes()].concat()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodings_roundtrip() {
        let all: Vec<u8> = (0..=255).collect();
        let (s, e, lossy) = Enc::decode(&all);
        assert_eq!((e, lossy), (Enc::Win1252, false));
        assert_eq!(Enc::Win1252.encode(&s).unwrap(), all.as_slice());
        assert_eq!(Enc::decode(b"\xE4\x80").0, "ä€");
        assert_eq!(Enc::Win1252.encode("😀"), None);

        let t = "Grüße 😀\nzwei";
        for e in Enc::ALL.into_iter().filter(|&e| e != Enc::Win1252) {
            assert_eq!(Enc::decode(&e.encode(t).unwrap()), (t.to_string(), e, false), "{e:?}");
        }
    }

    #[test]
    fn invalid_bytes_are_reported() {
        assert_eq!(Enc::decode(b"\xEF\xBB\xBFok\xFF"), ("ok\u{FFFD}".into(), Enc::Utf8Bom, true));
        // UTF-16 LE with BOM and a dangling odd byte
        assert_eq!(Enc::decode(b"\xFF\xFEa\0b"), ("a\u{FFFD}".into(), Enc::Utf16Le, true));
        // unpaired surrogate
        assert!(Enc::decode(b"\xFF\xFE\x00\xD8").2);
    }

    #[test]
    fn utf16_without_bom() {
        let t = "Hello Grüße\r\nline 2";
        let le: Vec<u8> = t.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let be: Vec<u8> = t.encode_utf16().flat_map(u16::to_be_bytes).collect();
        assert_eq!(Enc::decode(&le), (t.to_string(), Enc::Utf16Le, false));
        assert_eq!(Enc::decode(&be), (t.to_string(), Enc::Utf16Be, false));
        // plain text and binary-ish data are not mistaken for UTF-16
        assert_eq!(Enc::decode(b"Hello world!").1, Enc::Utf8);
        assert_eq!(Enc::decode(b"a\0\0b").1, Enc::Utf8);
    }
}
