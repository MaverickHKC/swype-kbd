//! A tiny embedded 5×7 bitmap font for key caps and labels.
//!
//! Zero dependencies by design (see ARCHITECTURE.md §2): key labels are short
//! ASCII strings, so a hand-authored bitmap is plenty and keeps the dependency
//! surface empty. Each glyph is 7 rows of 5 columns; in each row byte, bit 4 is
//! the leftmost column and bit 0 the rightmost.

/// Glyph cell dimensions in pixels-before-scaling.
pub const GLYPH_W: usize = 5;
pub const GLYPH_H: usize = 7;

/// Returns the 7-row bitmap for `ch`, or `None` if unglyphed.
///
/// Letters are rendered from their uppercase form. Unknown printable chars fall
/// back to a hollow box via [`glyph_or_box`].
pub fn glyph(ch: char) -> Option<[u8; 7]> {
    let c = ch.to_ascii_uppercase();
    let g: [u8; 7] = match c {
        ' ' => [0, 0, 0, 0, 0, 0, 0],
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        ',' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00100, 0b00100, 0b01000],
        '.' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100],
        '\'' => [0b00100, 0b00100, 0b01000, 0b00000, 0b00000, 0b00000, 0b00000],
        '?' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b00000, 0b00100],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100],
        '<' => [0b00010, 0b00100, 0b01000, 0b10000, 0b01000, 0b00100, 0b00010],
        '>' => [0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000],
        '-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        '@' => [0b01110, 0b10001, 0b10111, 0b10101, 0b10111, 0b10000, 0b01110],
        '#' => [0b01010, 0b01010, 0b11111, 0b01010, 0b11111, 0b01010, 0b01010],
        '$' => [0b00100, 0b01111, 0b10100, 0b01110, 0b00101, 0b11110, 0b00100],
        '&' => [0b01100, 0b10010, 0b10010, 0b01100, 0b10101, 0b10010, 0b01101],
        '+' => [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000],
        '(' => [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010],
        ')' => [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000],
        '/' => [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
        ':' => [0b00000, 0b01100, 0b01100, 0b00000, 0b01100, 0b01100, 0b00000],
        '*' => [0b00000, 0b00100, 0b10101, 0b01110, 0b10101, 0b00100, 0b00000],
        '"' => [0b01010, 0b01010, 0b01010, 0b00000, 0b00000, 0b00000, 0b00000],
        '_' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111],
        ';' => [0b00000, 0b01100, 0b01100, 0b00000, 0b01100, 0b00100, 0b01000],
        '~' => [0b00000, 0b00000, 0b01001, 0b10110, 0b00000, 0b00000, 0b00000],
        '`' => [0b01000, 0b00100, 0b00010, 0b00000, 0b00000, 0b00000, 0b00000],
        '|' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        '\\' => [0b10000, 0b01000, 0b01000, 0b00100, 0b00010, 0b00010, 0b00001],
        '^' => [0b00100, 0b01010, 0b10001, 0b00000, 0b00000, 0b00000, 0b00000],
        '=' => [0b00000, 0b00000, 0b11111, 0b00000, 0b11111, 0b00000, 0b00000],
        '%' => [0b11000, 0b11001, 0b00010, 0b00100, 0b01000, 0b10011, 0b00011],
        '{' => [0b00110, 0b01000, 0b01000, 0b11000, 0b01000, 0b01000, 0b00110],
        '}' => [0b01100, 0b00010, 0b00010, 0b00011, 0b00010, 0b00010, 0b01100],
        '[' => [0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110],
        ']' => [0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110],
        _ => return None,
    };
    Some(g)
}

/// Like [`glyph`] but renders a hollow box for unglyphed characters so missing
/// coverage is visible rather than blank.
pub fn glyph_or_box(ch: char) -> [u8; 7] {
    glyph(ch).unwrap_or([0b11111, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11111])
}

/// Pixel width of `text` rendered at integer `scale` with 1-px-scaled spacing.
pub fn text_width(text: &str, scale: usize) -> usize {
    let n = text.chars().count();
    if n == 0 {
        return 0;
    }
    n * GLYPH_W * scale + (n - 1) * scale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_all_label_chars() {
        // Everything the app draws on a cap or in a label must have a glyph.
        for ch in "abcdefghijklmnopqrstuvwxyz0123456789 ,.?!<>-'".chars() {
            assert!(glyph(ch).is_some(), "missing glyph for {ch:?}");
        }
        // Symbol-layer characters.
        for ch in "@#$&+()/:*\"_;~`|\\^=%{}[]".chars() {
            assert!(glyph(ch).is_some(), "missing glyph for {ch:?}");
        }
    }

    #[test]
    fn letters_are_case_insensitive() {
        assert_eq!(glyph('a'), glyph('A'));
    }

    #[test]
    fn text_width_is_consistent() {
        assert_eq!(text_width("", 2), 0);
        assert_eq!(text_width("A", 1), GLYPH_W);
        assert_eq!(text_width("AB", 1), GLYPH_W * 2 + 1);
    }

    /// Renders the glyph set as ASCII art when run with `--nocapture`, so the
    /// hand-authored bitmaps can be eyeballed for correctness.
    #[test]
    fn dump_glyphs() {
        for ch in "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789@#$&+()/:*\"_;~`|\\^=%{}[]".chars() {
            println!("\n[{ch}]");
            let g = glyph(ch).unwrap();
            for row in g {
                let mut line = String::new();
                for col in (0..GLYPH_W).rev() {
                    line.push(if row & (1 << col) != 0 { '#' } else { '.' });
                }
                println!("{line}");
            }
        }
    }
}
