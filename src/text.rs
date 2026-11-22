//! Text positions and search/replace. Positions are char indices (what egui's cursor uses).

pub fn char_to_byte(t: &str, c: usize) -> usize {
    t.char_indices().nth(c).map_or(t.len(), |(i, _)| i)
}

/// 1-based line and column of char index `idx`.
pub fn line_col(text: &str, idx: usize) -> (usize, usize) {
    let before = &text[..char_to_byte(text, idx)];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    (line, col)
}

/// Char index of the start of 1-based `line`; past the last line: the start of the last line.
pub fn line_start(text: &str, line: usize) -> usize {
    let mut idx = 0;
    for l in text.split('\n').take(line.max(1) - 1) {
        idx += l.chars().count() + 1;
    }
    idx.min(text.chars().count() - text.rsplit('\n').next().unwrap_or("").chars().count())
}

// ponytail: per-char case folding, so "ß" never matches "SS"; good enough for a notepad.
fn char_eq(a: char, b: char, match_case: bool) -> bool {
    a == b || (!match_case && a.to_lowercase().eq(b.to_lowercase()))
}

/// Byte length of the match if `t` starts with `needle`.
fn match_len(t: &str, needle: &str, match_case: bool) -> Option<usize> {
    let mut it = t.char_indices();
    for n in needle.chars() {
        let (_, c) = it.next()?;
        if !char_eq(c, n, match_case) {
            return None;
        }
    }
    Some(it.next().map_or(t.len(), |(i, _)| i))
}

/// Next match at/after char index `from`, wrapping around. Returns a char range.
// ponytail: naive O(text × needle) scan, fine for notepad-sized files.
pub fn find(text: &str, needle: &str, from: usize, match_case: bool) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    let hit = |&(_, (b, _)): &(usize, (usize, char))| match_len(&text[b..], needle, match_case).is_some();
    let starts = || text.char_indices().enumerate();
    let (i, _) = starts().skip(from).find(hit).or_else(|| starts().take(from).find(hit))?;
    Some((i, i + needle.chars().count()))
}

/// None if there is nothing to replace.
pub fn replace_all(text: &str, needle: &str, replacement: &str, match_case: bool) -> Option<String> {
    if needle.is_empty() {
        return None;
    }
    let (mut out, mut rest, mut hit) = (String::with_capacity(text.len()), text, false);
    while let Some(c) = rest.chars().next() {
        if let Some(len) = match_len(rest, needle, match_case) {
            out.push_str(replacement);
            (rest, hit) = (&rest[len..], true);
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    hit.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_and_column() {
        assert_eq!(line_col("", 0), (1, 1));
        assert_eq!(line_col("ab\ncäd", 1), (1, 2));
        assert_eq!(line_col("ab\ncäd", 3), (2, 1));
        assert_eq!(line_col("ab\ncäd", 5), (2, 3));
        assert_eq!(line_col("ab\n", 3), (2, 1));
    }

    #[test]
    fn start_of_line() {
        assert_eq!(line_start("ab\ncäd\nx", 1), 0);
        assert_eq!(line_start("ab\ncäd\nx", 2), 3);
        assert_eq!(line_start("ab\ncäd\nx", 3), 7);
        assert_eq!(line_start("ab\ncäd\nx", 99), 7);
        assert_eq!(line_start("ab\n", 2), 3);
        assert_eq!(line_start("", 0), 0);
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(find("Grüße GRÜN", "grü", 1, false), Some((6, 9)));
        assert_eq!(find("Grüße GRÜN", "grü", 1, true), None);
        assert_eq!(find("abc", "C", 0, false), Some((2, 3)));
        assert_eq!(replace_all("Foo foo FOO", "foo", "x", false).as_deref(), Some("x x x"));
        assert_eq!(replace_all("Foo foo FOO", "foo", "x", true).as_deref(), Some("Foo x FOO"));
        assert_eq!(replace_all("äbc äbc", "ÄB", "x", false).as_deref(), Some("xc xc"));
        assert_eq!(replace_all("abc", "x", "y", false), None);
    }

    #[test]
    fn find_wraps_and_counts_chars() {
        assert_eq!(find("äb äb", "äb", 0, true), Some((0, 2)));
        assert_eq!(find("äb äb", "äb", 1, true), Some((3, 5)));
        assert_eq!(find("äb äb", "äb", 4, true), Some((0, 2)));
        assert_eq!(find("äb äb", "äb", 99, true), Some((0, 2)));
        assert_eq!(find("abc", "x", 0, true), None);
        assert_eq!(find("abc", "", 0, true), None);
    }
}
