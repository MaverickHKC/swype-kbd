//! Constants for the two text-injection channels (see ARCHITECTURE.md §3.3).
//!
//! The behavior lives on `App` in `main.rs`; this module holds the static data:
//! the embedded US xkb keymap and the evdev keycode tables used by the virtual
//! keyboard.

/// US-QWERTY xkb keymap, generated at dev time with
/// `xkbcli compile-keymap --layout us` and embedded so the runtime needs no
/// libxkbcommon. Uploaded to the virtual keyboard once at startup.
pub const KEYMAP: &str = include_str!("us_keymap.xkb");

// --- evdev keycodes (what zwp_virtual_keyboard_v1.key expects) -------------
// The compositor adds the conventional +8 offset before xkb lookup, so e.g.
// evdev 14 -> xkb <BKSP> (22) -> BackSpace.
pub const KEY_BACKSPACE: u32 = 14;
pub const KEY_ENTER: u32 = 28;

/// wl_keyboard key state.
pub const STATE_RELEASED: u32 = 0;
pub const STATE_PRESSED: u32 = 1;

/// wl_keyboard keymap format: XKB v1.
pub const KEYMAP_FORMAT_XKB_V1: u32 = 1;

/// Evdev keycode for an unshifted letter, or `None` if not a-z.
fn letter_code(c: char) -> Option<u32> {
    Some(match c.to_ascii_lowercase() {
        'q' => 16, 'w' => 17, 'e' => 18, 'r' => 19, 't' => 20,
        'y' => 21, 'u' => 22, 'i' => 23, 'o' => 24, 'p' => 25,
        'a' => 30, 's' => 31, 'd' => 32, 'f' => 33, 'g' => 34,
        'h' => 35, 'j' => 36, 'k' => 37, 'l' => 38,
        'z' => 44, 'x' => 45, 'c' => 46, 'v' => 47, 'b' => 48,
        'n' => 49, 'm' => 50,
        _ => return None,
    })
}

/// Map a printable ASCII character to `(evdev keycode, needs_shift)` on a US
/// layout, for the virtual-keyboard fallback used when no text-input is active.
/// Covers letters (either case), digits, space, and all US-shifted symbols.
pub fn char_to_key(c: char) -> Option<(u32, bool)> {
    if c.is_ascii_alphabetic() {
        return letter_code(c).map(|code| (code, c.is_ascii_uppercase()));
    }
    let pair = match c {
        '1' => (2, false),  '!' => (2, true),
        '2' => (3, false),  '@' => (3, true),
        '3' => (4, false),  '#' => (4, true),
        '4' => (5, false),  '$' => (5, true),
        '5' => (6, false),  '%' => (6, true),
        '6' => (7, false),  '^' => (7, true),
        '7' => (8, false),  '&' => (8, true),
        '8' => (9, false),  '*' => (9, true),
        '9' => (10, false), '(' => (10, true),
        '0' => (11, false), ')' => (11, true),
        '-' => (12, false), '_' => (12, true),
        '=' => (13, false), '+' => (13, true),
        '[' => (26, false), '{' => (26, true),
        ']' => (27, false), '}' => (27, true),
        ';' => (39, false), ':' => (39, true),
        '\'' => (40, false), '"' => (40, true),
        '`' => (41, false), '~' => (41, true),
        '\\' => (43, false), '|' => (43, true),
        ',' => (51, false), '<' => (51, true),
        '.' => (52, false), '>' => (52, true),
        '/' => (53, false), '?' => (53, true),
        ' ' => (57, false),
        _ => return None,
    };
    Some(pair)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keymap_is_embedded_and_valid_ish() {
        assert!(KEYMAP.starts_with("xkb_keymap"));
        assert!(KEYMAP.contains("<BKSP>"));
    }

    #[test]
    fn letters_digits_and_symbols_map() {
        assert_eq!(char_to_key('a'), Some((30, false)));
        assert_eq!(char_to_key('A'), Some((30, true)));
        assert_eq!(char_to_key('m'), Some((50, false)));
        assert_eq!(char_to_key('0'), Some((11, false)));
        assert_eq!(char_to_key(' '), Some((57, false)));
        // shifted symbols share the unshifted key's code
        assert_eq!(char_to_key('1').unwrap().0, char_to_key('!').unwrap().0);
        assert!(char_to_key('!').unwrap().1);
        assert_eq!(char_to_key('@'), Some((3, true)));
        assert_eq!(char_to_key('/'), Some((53, false)));
        assert_eq!(char_to_key('?'), Some((53, true)));
        assert_eq!(char_to_key('é'), None);
    }
}
