//! Ideal gesture templates: the geometric path a perfect swipe for a word would
//! follow, plus the precomputed channels the decoder compares against.
//!
//! For a word, the ideal path is the polyline through the centroid of each
//! letter ([`ideal_trace`]). It is resampled to `n` equidistant points (the
//! location channel) and additionally translated-to-centroid + scale-normalized
//! (the shape channel). Both are cached per word so decoding is a flat numeric
//! pass (see ARCHITECTURE.md §4.2).

use crate::layout::KeyboardLayout;
use crate::trace::{Point, Trace};

const EPS: f32 = 1e-6;

/// A precomputed template for one dictionary word.
#[derive(Debug, Clone)]
pub struct Template {
    /// Index into the owning [`crate::Dictionary`].
    pub word_index: usize,
    /// Log-frequency weight copied from the dictionary (the prior).
    pub ln_freq: f32,
    /// Location-channel points in keyboard units, length `n`.
    pub loc: Vec<[f32; 2]>,
    /// Shape-channel points (centroid-centered, scale-normalized), length `n`.
    pub shape: Vec<[f32; 2]>,
    /// First and last location points, used by the pruning gate.
    pub start: [f32; 2],
    pub end: [f32; 2],
}

/// The geometric ideal path for `word`: a polyline through letter centroids.
///
/// Returns `None` if the word has fewer than two layout letters or collapses to
/// zero length (e.g. a repeated single letter) — such words are tap-typed, not
/// gestured.
pub fn ideal_trace(word: &str, layout: &KeyboardLayout) -> Option<Trace> {
    let mut pts = Vec::new();
    for ch in word.chars() {
        let (x, y) = layout.centroid_of(ch)?;
        pts.push(Point::new(x, y, 0));
    }
    if pts.len() < 2 {
        return None;
    }
    let trace = Trace::from_points(pts);
    if trace.path_length() <= EPS {
        return None;
    }
    Some(trace)
}

/// Translate a resampled trace to its centroid and scale so its longer bounding
/// box side is 1 — the scale/translation-invariant shape channel.
pub fn normalize_shape(resampled: &Trace) -> Vec<[f32; 2]> {
    let (cx, cy) = resampled.centroid();
    let (minx, miny, maxx, maxy) = resampled.bbox();
    let scale = 1.0 / (maxx - minx).max(maxy - miny).max(EPS);
    resampled
        .points
        .iter()
        .map(|p| [(p.x - cx) * scale, (p.y - cy) * scale])
        .collect()
}

impl Template {
    /// Build a template for `word`, or `None` if it cannot be gestured.
    pub fn build(
        word_index: usize,
        word: &str,
        ln_freq: f32,
        layout: &KeyboardLayout,
        n: usize,
    ) -> Option<Template> {
        let ideal = ideal_trace(word, layout)?;
        let rs = ideal.resample(n);
        let loc: Vec<[f32; 2]> = rs.points.iter().map(|p| [p.x, p.y]).collect();
        let shape = normalize_shape(&rs);
        let start = loc[0];
        let end = loc[loc.len() - 1];
        Some(Template {
            word_index,
            ln_freq,
            loc,
            shape,
            start,
            end,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ideal_trace_visits_centroids() {
        let kb = KeyboardLayout::qwerty();
        let t = ideal_trace("cat", &kb).unwrap();
        assert_eq!(t.len(), 3);
        assert_eq!((t.points[0].x, t.points[0].y), kb.centroid_of('c').unwrap());
        assert_eq!((t.points[2].x, t.points[2].y), kb.centroid_of('t').unwrap());
    }

    #[test]
    fn single_letter_and_repeats_are_rejected() {
        let kb = KeyboardLayout::qwerty();
        assert!(ideal_trace("a", &kb).is_none());
        assert!(ideal_trace("aa", &kb).is_none()); // zero length
        assert!(ideal_trace("é", &kb).is_none()); // not on layout
    }

    #[test]
    fn template_channels_have_length_n() {
        let kb = KeyboardLayout::qwerty();
        let t = Template::build(0, "hello", -1.0, &kb, 64).unwrap();
        assert_eq!(t.loc.len(), 64);
        assert_eq!(t.shape.len(), 64);
        assert_eq!(t.start, t.loc[0]);
        assert_eq!(t.end, t.loc[63]);
    }

    #[test]
    fn shape_is_translation_and_scale_invariant() {
        // The same word gestured large vs small must produce ~identical shapes.
        let kb = KeyboardLayout::qwerty();
        let base = ideal_trace("word", &kb).unwrap().resample(50);
        let shape_a = normalize_shape(&base);

        // Scale the trace 3x and shift it; normalized shape should match.
        let scaled = Trace::from_points(
            base.points
                .iter()
                .map(|p| Point::new(p.x * 3.0 + 5.0, p.y * 3.0 - 2.0, p.t))
                .collect(),
        );
        let shape_b = normalize_shape(&scaled);
        for (a, b) in shape_a.iter().zip(&shape_b) {
            assert!((a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4);
        }
    }
}
