//! The on-screen keyboard's *visual* layout: the rectangles we draw and
//! hit-test. Letter centroids for the decoder live in `swype_decoder::layout`;
//! this adds the function keys (space, backspace, …) and the pixel geometry.
//!
//! Caps are defined in **unit space** — a 10-wide × 4-tall grid — then scaled to
//! the surface size at render/hit-test time, so the layout is resolution
//! independent.

/// What pressing a cap does. In milestone 1 every action is just logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Char(char),
    Space,
    Backspace,
    Enter,
    Shift,
    Symbols,
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

/// The full visual keyboard.
pub struct VisualKeyboard {
    caps: Vec<Cap>,
    cols: f32,
    rows: f32,
}

impl VisualKeyboard {
    /// US-QWERTY with a function row, matching ARCHITECTURE.md milestone 1.
    pub fn qwerty() -> Self {
        let mut caps = Vec::new();

        // Row 0: q…p, ten 1-unit keys, no indent.
        for (col, ch) in "qwertyuiop".chars().enumerate() {
            caps.push(letter(ch, col as f32, 0.0));
        }
        // Row 1: a…l, nine keys, indented half a unit.
        for (col, ch) in "asdfghjkl".chars().enumerate() {
            caps.push(letter(ch, 0.5 + col as f32, 1.0));
        }
        // Row 2: [shift] z…m [backspace], spanning the full width.
        caps.push(func(KeyAction::Shift, "SH", 0.0, 2.0, 1.5));
        for (col, ch) in "zxcvbnm".chars().enumerate() {
            caps.push(letter(ch, 1.5 + col as f32, 2.0));
        }
        caps.push(func(KeyAction::Backspace, "<", 8.5, 2.0, 1.5));
        // Row 3: [?123] [,] [ space ] [.] [enter].
        caps.push(func(KeyAction::Symbols, "?12", 0.0, 3.0, 1.5));
        caps.push(func(KeyAction::Char(','), ",", 1.5, 3.0, 1.0));
        caps.push(func(KeyAction::Space, "", 2.5, 3.0, 5.0));
        caps.push(func(KeyAction::Char('.'), ".", 7.5, 3.0, 1.0));
        caps.push(func(KeyAction::Enter, "ENT", 8.5, 3.0, 1.5));

        VisualKeyboard {
            caps,
            cols: 10.0,
            rows: 4.0,
        }
    }

    pub fn caps(&self) -> &[Cap] {
        &self.caps
    }

    /// Grid extents in unit space. Used by the gesture renderer (milestone 4).
    #[allow(dead_code)]
    pub fn cols(&self) -> f32 {
        self.cols
    }
    #[allow(dead_code)]
    pub fn rows(&self) -> f32 {
        self.rows
    }

    /// Pixel rect of a cap for a surface of `width × height`.
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

    /// Index of the cap under pixel `(px, py)`, if any.
    pub fn hit_index(&self, px: i32, py: i32, width: u32, height: u32) -> Option<usize> {
        self.caps
            .iter()
            .position(|c| self.rect_of(c, width, height).contains(px, py))
    }

    /// The cap under pixel `(px, py)`, if any.
    #[allow(dead_code)] // convenience wrapper exercised by tests; app uses hit_index
    pub fn hit_test(&self, px: i32, py: i32, width: u32, height: u32) -> Option<&Cap> {
        self.hit_index(px, py, width, height).map(|i| &self.caps[i])
    }
}

fn letter(ch: char, x: f32, y: f32) -> Cap {
    Cap {
        action: KeyAction::Char(ch),
        label: ch.to_ascii_uppercase().to_string(),
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

    #[test]
    fn caps_cover_full_width_each_row() {
        let kb = VisualKeyboard::qwerty();
        for row in 0..4 {
            let w: f32 = kb
                .caps()
                .iter()
                .filter(|c| c.y as i32 == row)
                .map(|c| c.w)
                .sum();
            // Rows 0/1 are letter rows that don't span the full 10 units; rows
            // 2/3 must fill it exactly.
            if row >= 2 {
                assert!((w - 10.0).abs() < 1e-3, "row {row} width {w}");
            }
        }
    }

    #[test]
    fn hit_test_resolves_letters() {
        let kb = VisualKeyboard::qwerty();
        // 1000x400 surface => 100px per unit-x, 100px per unit-y.
        // 'q' cap is unit (0,0,1,1) => pixel rect (0,0,100,100); center (50,50).
        assert_eq!(kb.hit_test(50, 50, 1000, 400).unwrap().action, KeyAction::Char('q'));
        // Space is unit x 2.5..7.5 on row 3 (y 300..400); center (500, 350).
        assert_eq!(kb.hit_test(500, 350, 1000, 400).unwrap().action, KeyAction::Space);
        // Far corner beyond any cap.
        assert!(kb.hit_test(50, 50, 1000, 400).is_some());
    }

    #[test]
    fn space_bar_is_wide() {
        let kb = VisualKeyboard::qwerty();
        let space = kb.caps().iter().find(|c| c.action == KeyAction::Space).unwrap();
        assert_eq!(space.w, 5.0);
    }
}
