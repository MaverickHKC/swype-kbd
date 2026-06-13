//! The on-screen keyboard's *visual* layout across input layers: lowercase and
//! uppercase letters plus two symbol pages. Each layer is a list of caps in
//! unit space (a 10-wide × 4-tall grid), scaled to the surface at draw/hit-test
//! time. Letter centroids for the decoder live in `swype_decoder::layout`.

/// What pressing a cap does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Char(char),
    Space,
    Backspace,
    Enter,
    /// Toggle shift (Off → one-shot → lock → Off).
    Shift,
    /// Switch to the lowercase letter layer.
    ToLetters,
    /// Switch to symbol page 1 (numbers + punctuation).
    ToSymbols1,
    /// Switch to symbol page 2 (brackets + math).
    ToSymbols2,
}

/// Which concrete layer is being displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerKind {
    LettersLower,
    LettersUpper,
    Symbols1,
    Symbols2,
}

/// One drawable/hit-testable key, positioned in unit space.
#[derive(Debug, Clone)]
pub struct Cap {
    pub action: KeyAction,
    pub label: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Pixel rectangle (top-left origin).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// The full visual keyboard: one cap list per layer.
pub struct VisualKeyboard {
    lower: Vec<Cap>,
    upper: Vec<Cap>,
    sym1: Vec<Cap>,
    sym2: Vec<Cap>,
    cols: f32,
    rows: f32,
}

impl VisualKeyboard {
    pub fn new() -> Self {
        VisualKeyboard {
            lower: letter_layer(false),
            upper: letter_layer(true),
            sym1: symbols1(),
            sym2: symbols2(),
            cols: 10.0,
            rows: 4.0,
        }
    }

    /// The caps for a given layer.
    pub fn caps(&self, kind: LayerKind) -> &[Cap] {
        match kind {
            LayerKind::LettersLower => &self.lower,
            LayerKind::LettersUpper => &self.upper,
            LayerKind::Symbols1 => &self.sym1,
            LayerKind::Symbols2 => &self.sym2,
        }
    }

    /// Grid extents in unit space. Used by the gesture renderer.
    #[allow(dead_code)]
    pub fn cols(&self) -> f32 {
        self.cols
    }
    #[allow(dead_code)]
    pub fn rows(&self) -> f32 {
        self.rows
    }

    /// Pixel rect of a cap for a surface region of `width × height`.
    pub fn rect_of(&self, cap: &Cap, width: u32, height: u32) -> Rect {
        let ux = width as f32 / self.cols;
        let uy = height as f32 / self.rows;
        Rect {
            x: (cap.x * ux).round() as i32,
            y: (cap.y * uy).round() as i32,
            w: (cap.w * ux).round() as i32,
            h: (cap.h * uy).round() as i32,
        }
    }

    /// Index (into `caps(kind)`) of the cap under pixel `(px, py)`, if any.
    pub fn hit_index(&self, kind: LayerKind, px: i32, py: i32, width: u32, height: u32) -> Option<usize> {
        self.caps(kind)
            .iter()
            .position(|c| self.rect_of(c, width, height).contains(px, py))
    }
}

impl Default for VisualKeyboard {
    fn default() -> Self {
        Self::new()
    }
}

// --- layer builders --------------------------------------------------------

fn letter_layer(upper: bool) -> Vec<Cap> {
    let mut caps = Vec::new();
    for (col, ch) in "qwertyuiop".chars().enumerate() {
        caps.push(letter(ch, upper, col as f32, 0.0));
    }
    for (col, ch) in "asdfghjkl".chars().enumerate() {
        caps.push(letter(ch, upper, 0.5 + col as f32, 1.0));
    }
    caps.push(func(KeyAction::Shift, "SH", 0.0, 2.0, 1.5));
    for (col, ch) in "zxcvbnm".chars().enumerate() {
        caps.push(letter(ch, upper, 1.5 + col as f32, 2.0));
    }
    caps.push(func(KeyAction::Backspace, "<", 8.5, 2.0, 1.5));
    bottom_row(&mut caps, KeyAction::ToSymbols1, "?12");
    caps
}

fn symbols1() -> Vec<Cap> {
    // row0 + row1 are full 10-wide; row2 is [page] + 7 + [backspace].
    symbol_layer("1234567890", "@#$&-+()/:", "*\"'_;!?", KeyAction::ToSymbols2, "#+=")
}

fn symbols2() -> Vec<Cap> {
    symbol_layer("[](){}<>|\\", "~`^=+-*/%_", "@#$&\";:", KeyAction::ToSymbols1, "123")
}

/// Build a symbol page from three character rows (10, 10, 7 chars) and the
/// label/action of the page-toggle key at the bottom-left of row 2.
fn symbol_layer(row0: &str, row1: &str, row2mid: &str, toggle: KeyAction, toggle_label: &str) -> Vec<Cap> {
    let mut caps = Vec::new();
    for (col, ch) in row0.chars().enumerate() {
        caps.push(sym(ch, col as f32, 0.0));
    }
    for (col, ch) in row1.chars().enumerate() {
        caps.push(sym(ch, col as f32, 1.0));
    }
    caps.push(func(toggle, toggle_label, 0.0, 2.0, 1.5));
    for (col, ch) in row2mid.chars().enumerate() {
        caps.push(sym(ch, 1.5 + col as f32, 2.0));
    }
    caps.push(func(KeyAction::Backspace, "<", 8.5, 2.0, 1.5));
    bottom_row(&mut caps, KeyAction::ToLetters, "ABC");
    caps
}

/// The shared bottom row: [layer-switch] [,] [space] [.] [enter]. `switch` is
/// the action/label of the left-most key (to-symbols on letters, to-letters on
/// symbol pages).
fn bottom_row(caps: &mut Vec<Cap>, switch: KeyAction, switch_label: &str) {
    caps.push(func(switch, switch_label, 0.0, 3.0, 1.5));
    caps.push(func(KeyAction::Char(','), ",", 1.5, 3.0, 1.0));
    caps.push(func(KeyAction::Space, "", 2.5, 3.0, 5.0));
    caps.push(func(KeyAction::Char('.'), ".", 7.5, 3.0, 1.0));
    caps.push(func(KeyAction::Enter, "ENT", 8.5, 3.0, 1.5));
}

fn letter(ch: char, upper: bool, x: f32, y: f32) -> Cap {
    let typed = if upper { ch.to_ascii_uppercase() } else { ch };
    Cap {
        action: KeyAction::Char(typed),
        label: ch.to_ascii_uppercase().to_string(),
        x,
        y,
        w: 1.0,
        h: 1.0,
    }
}

fn sym(ch: char, x: f32, y: f32) -> Cap {
    Cap {
        action: KeyAction::Char(ch),
        label: ch.to_string(),
        x,
        y,
        w: 1.0,
        h: 1.0,
    }
}

fn func(action: KeyAction, label: &str, x: f32, y: f32, w: f32) -> Cap {
    Cap {
        action,
        label: label.to_string(),
        x,
        y,
        w,
        h: 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_width(caps: &[Cap], row: i32) -> f32 {
        caps.iter().filter(|c| c.y as i32 == row).map(|c| c.w).sum()
    }

    #[test]
    fn symbol_rows_fill_full_width() {
        let kb = VisualKeyboard::new();
        for kind in [LayerKind::Symbols1, LayerKind::Symbols2] {
            for row in 0..4 {
                let w = row_width(kb.caps(kind), row);
                assert!((w - 10.0).abs() < 1e-3, "{kind:?} row {row} width {w}");
            }
        }
    }

    #[test]
    fn lower_types_lowercase_upper_types_uppercase() {
        let kb = VisualKeyboard::new();
        let q_lower = &kb.caps(LayerKind::LettersLower)[0];
        let q_upper = &kb.caps(LayerKind::LettersUpper)[0];
        assert_eq!(q_lower.action, KeyAction::Char('q'));
        assert_eq!(q_upper.action, KeyAction::Char('Q'));
        // both show the same uppercase label
        assert_eq!(q_lower.label, "Q");
    }

    #[test]
    fn hit_test_resolves_per_layer() {
        let kb = VisualKeyboard::new();
        // 1000x400 surface. 'q' cap at unit (0,0) -> center (50,50).
        let i = kb.hit_index(LayerKind::LettersLower, 50, 50, 1000, 400).unwrap();
        assert_eq!(kb.caps(LayerKind::LettersLower)[i].action, KeyAction::Char('q'));
        // Symbols1 top-left is '1'.
        let j = kb.hit_index(LayerKind::Symbols1, 50, 50, 1000, 400).unwrap();
        assert_eq!(kb.caps(LayerKind::Symbols1)[j].action, KeyAction::Char('1'));
        // Space (unit x 2.5..7.5, row 3) on every layer.
        let s = kb.hit_index(LayerKind::Symbols2, 500, 350, 1000, 400).unwrap();
        assert_eq!(kb.caps(LayerKind::Symbols2)[s].action, KeyAction::Space);
    }

    #[test]
    fn page_switch_keys_exist() {
        let kb = VisualKeyboard::new();
        let has = |kind, a| kb.caps(kind).iter().any(|c| c.action == a);
        assert!(has(LayerKind::LettersLower, KeyAction::ToSymbols1));
        assert!(has(LayerKind::Symbols1, KeyAction::ToSymbols2));
        assert!(has(LayerKind::Symbols1, KeyAction::ToLetters));
        assert!(has(LayerKind::Symbols2, KeyAction::ToSymbols1));
        assert!(has(LayerKind::Symbols2, KeyAction::ToLetters));
    }

    #[test]
    fn every_cap_has_a_renderable_label_or_is_space() {
        use ab_glyph::Font;
        let font = ab_glyph::FontRef::try_from_slice(crate::FONT_BYTES).unwrap();
        let kb = VisualKeyboard::new();
        for kind in [
            LayerKind::LettersLower,
            LayerKind::LettersUpper,
            LayerKind::Symbols1,
            LayerKind::Symbols2,
        ] {
            for cap in kb.caps(kind) {
                if cap.action == KeyAction::Space {
                    continue;
                }
                for ch in cap.label.chars() {
                    // Glyph id 0 is .notdef — the font cannot render the char.
                    assert!(font.glyph_id(ch).0 != 0, "no glyph for {ch:?} in {kind:?}");
                }
            }
        }
    }
}
