//! Keyboard geometry shared by the decoder and the renderer.
//!
//! Coordinates are in **layout units** where one key is `1.0 × 1.0`. The origin
//! is the top-left of the key field; `x` grows right, `y` grows down. Absolute
//! scale is irrelevant to the shape channel (it normalizes) and consistent for
//! the location channel because templates and input both derive from the same
//! centroids here.
//!
//! Only the 26 letters carry semantic centroids the decoder cares about; the
//! app overlays its own function keys (space, backspace, …) on top of this
//! model.

/// A single character key with its position in layout units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Key {
    /// The character this key produces (always lowercase for letters).
    pub ch: char,
    /// Center x in layout units.
    pub cx: f32,
    /// Center y in layout units.
    pub cy: f32,
}

impl Key {
    /// Center point as an `(x, y)` pair.
    #[inline]
    pub fn centroid(&self) -> (f32, f32) {
        (self.cx, self.cy)
    }
}

/// The set of character keys and the field extent, in layout units.
#[derive(Debug, Clone)]
pub struct KeyboardLayout {
    keys: Vec<Key>,
    width: f32,
    height: f32,
}

impl KeyboardLayout {
    /// The standard three-row US-QWERTY letter layout.
    ///
    /// Row offsets follow a physical keyboard: the home row is indented half a
    /// key, the bottom row a key and a half. Row centers sit at y = 0.5, 1.5,
    /// 2.5 so the field is 10 wide × 3 tall in layout units.
    pub fn qwerty() -> Self {
        const ROWS: [&str; 3] = ["qwertyuiop", "asdfghjkl", "zxcvbnm"];
        // Left indent of each row's first key, in layout units.
        const INDENT: [f32; 3] = [0.0, 0.5, 1.5];

        let mut keys = Vec::with_capacity(26);
        for (row, (letters, indent)) in ROWS.iter().zip(INDENT).enumerate() {
            let cy = row as f32 + 0.5;
            for (col, ch) in letters.chars().enumerate() {
                let cx = indent + col as f32 + 0.5;
                keys.push(Key { ch, cx, cy });
            }
        }
        KeyboardLayout {
            keys,
            width: 10.0,
            height: 3.0,
        }
    }

    /// All character keys, in reading order (top-left to bottom-right).
    #[inline]
    pub fn keys(&self) -> &[Key] {
        &self.keys
    }

    /// Field width in layout units.
    #[inline]
    pub fn width(&self) -> f32 {
        self.width
    }

    /// Field height in layout units.
    #[inline]
    pub fn height(&self) -> f32 {
        self.height
    }

    /// Centroid of a given letter, if present in the layout.
    pub fn centroid_of(&self, ch: char) -> Option<(f32, f32)> {
        let ch = ch.to_ascii_lowercase();
        self.keys
            .iter()
            .find(|k| k.ch == ch)
            .map(|k| (k.cx, k.cy))
    }

    /// The letter key whose center is nearest to `(x, y)` (squared-distance).
    /// Returns `None` only if the layout is empty.
    pub fn nearest_key(&self, x: f32, y: f32) -> Option<&Key> {
        self.keys.iter().min_by(|a, b| {
            let da = (a.cx - x).powi(2) + (a.cy - y).powi(2);
            let db = (b.cx - x).powi(2) + (b.cy - y).powi(2);
            da.total_cmp(&db)
        })
    }
}

impl Default for KeyboardLayout {
    fn default() -> Self {
        Self::qwerty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qwerty_has_26_unique_letters() {
        let kb = KeyboardLayout::qwerty();
        assert_eq!(kb.keys().len(), 26);
        let mut seen = std::collections::HashSet::new();
        for k in kb.keys() {
            assert!(k.ch.is_ascii_lowercase());
            assert!(seen.insert(k.ch), "duplicate key {}", k.ch);
        }
    }

    #[test]
    fn known_centroids() {
        let kb = KeyboardLayout::qwerty();
        // 'q' is the top-left key: indent 0, col 0 -> (0.5, 0.5).
        assert_eq!(kb.centroid_of('q'), Some((0.5, 0.5)));
        // 'a' is home-row first key: indent 0.5 -> (1.0, 1.5).
        assert_eq!(kb.centroid_of('a'), Some((1.0, 1.5)));
        // 'z' is bottom-row first key: indent 1.5 -> (2.0, 2.5).
        assert_eq!(kb.centroid_of('z'), Some((2.0, 2.5)));
        // case-insensitive
        assert_eq!(kb.centroid_of('Q'), kb.centroid_of('q'));
        assert_eq!(kb.centroid_of('é'), None);
    }

    #[test]
    fn nearest_key_snaps_to_centroid() {
        let kb = KeyboardLayout::qwerty();
        // A point just right of 'q's center still resolves to 'q'.
        assert_eq!(kb.nearest_key(0.6, 0.5).unwrap().ch, 'q');
        // Dead center of 'g' (home row, indent .5 + col 4 + .5 = 5.0, y 1.5).
        assert_eq!(kb.nearest_key(5.0, 1.5).unwrap().ch, 'g');
    }
}
