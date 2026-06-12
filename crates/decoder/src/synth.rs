//! Synthetic gesture generation for testing and tuning the decoder.
//!
//! Real users never trace the ideal path. To measure accuracy we take a word's
//! ideal trace and apply parametric, deterministic perturbations — Gaussian
//! jitter, corner-cutting (smoothing), and endpoint overshoot — to approximate
//! human swipes (ARCHITECTURE.md §4.6). Everything is seeded, so accuracy
//! numbers are reproducible. No external RNG dependency.

use crate::layout::KeyboardLayout;
use crate::template::ideal_trace;
use crate::trace::{Point, Trace};

/// A small, fast, deterministic PRNG (xorshift64*). Not cryptographic.
#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        // Avoid the all-zero state.
        Rng {
            state: seed ^ 0x9E37_79B9_7F4A_7C15,
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `[0, 1)`.
    pub fn next_f32(&mut self) -> f32 {
        // Top 24 bits → [0,1).
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }

    /// Standard normal via Box–Muller.
    pub fn gauss(&mut self) -> f32 {
        let u1 = (self.next_f32()).max(1e-7);
        let u2 = self.next_f32();
        (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
    }
}

/// Resample the ideal path to `n_points` and stamp constant-speed timestamps,
/// approximating a continuously sampled stroke. `None` if the word can't be
/// gestured.
pub fn sample_stroke(word: &str, layout: &KeyboardLayout, n_points: usize) -> Option<Trace> {
    let ideal = ideal_trace(word, layout)?;
    let mut rs = ideal.resample(n_points);
    // ~120 ms per key-unit of path, constant speed.
    let total = rs.path_length();
    let dur = (total * 120.0) as u32;
    let n = rs.points.len().max(1);
    for (i, p) in rs.points.iter_mut().enumerate() {
        p.t = (dur as usize * i / n) as u32;
    }
    Some(rs)
}

/// Add per-point Gaussian noise of standard deviation `sigma` (in key units).
pub fn jitter(trace: &Trace, rng: &mut Rng, sigma: f32) -> Trace {
    Trace::from_points(
        trace
            .points
            .iter()
            .map(|p| Point::new(p.x + rng.gauss() * sigma, p.y + rng.gauss() * sigma, p.t))
            .collect(),
    )
}

/// Corner-cutting: `passes` of a 3-tap moving average, endpoints fixed. Humans
/// round off sharp direction changes rather than hitting each centroid square.
pub fn smooth(trace: &Trace, passes: usize) -> Trace {
    let mut pts = trace.points.clone();
    for _ in 0..passes {
        if pts.len() < 3 {
            break;
        }
        let mut next = pts.clone();
        for i in 1..pts.len() - 1 {
            next[i].x = (pts[i - 1].x + pts[i].x + pts[i + 1].x) / 3.0;
            next[i].y = (pts[i - 1].y + pts[i].y + pts[i + 1].y) / 3.0;
        }
        pts = next;
    }
    Trace::from_points(pts)
}

/// Extend the first and last segments outward by `frac` of their length, as if
/// the user started/stopped slightly past the end keys.
pub fn overshoot(trace: &Trace, frac: f32) -> Trace {
    let mut pts = trace.points.clone();
    let n = pts.len();
    if n >= 2 && frac > 0.0 {
        let (a, b) = (pts[1], pts[0]);
        pts[0] = Point::new(b.x + (b.x - a.x) * frac, b.y + (b.y - a.y) * frac, b.t);
        let (c, d) = (pts[n - 2], pts[n - 1]);
        pts[n - 1] = Point::new(d.x + (d.x - c.x) * frac, d.y + (d.y - c.y) * frac, d.t);
    }
    Trace::from_points(pts)
}

/// Compose a human-like swipe for `word`. `level` in `[0, 1]` scales severity:
/// jitter sd up to ~0.18 key, up to 2 smoothing passes, up to 0.18 overshoot.
pub fn human_like(
    word: &str,
    layout: &KeyboardLayout,
    rng: &mut Rng,
    level: f32,
) -> Option<Trace> {
    let level = level.clamp(0.0, 1.0);
    let stroke = sample_stroke(word, layout, 48)?;
    let stroke = overshoot(&stroke, 0.18 * level);
    let stroke = smooth(&stroke, (2.0 * level).round() as usize);
    let stroke = jitter(&stroke, rng, 0.18 * level);
    Some(stroke)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn gauss_is_roughly_standard_normal() {
        let mut rng = Rng::new(7);
        let n = 20_000;
        let mut sum = 0.0f64;
        let mut sumsq = 0.0f64;
        for _ in 0..n {
            let g = rng.gauss() as f64;
            sum += g;
            sumsq += g * g;
        }
        let mean = sum / n as f64;
        let var = sumsq / n as f64 - mean * mean;
        assert!(mean.abs() < 0.05, "mean {mean}");
        assert!((var - 1.0).abs() < 0.1, "var {var}");
    }

    #[test]
    fn smoothing_keeps_endpoints() {
        let kb = KeyboardLayout::qwerty();
        let s = sample_stroke("hello", &kb, 40).unwrap();
        let sm = smooth(&s, 3);
        assert_eq!(s.first().unwrap().x, sm.first().unwrap().x);
        assert_eq!(s.last().unwrap().y, sm.last().unwrap().y);
    }

    #[test]
    fn human_like_level_zero_is_the_ideal() {
        let kb = KeyboardLayout::qwerty();
        let mut rng = Rng::new(1);
        let h = human_like("word", &kb, &mut rng, 0.0).unwrap();
        let ideal = sample_stroke("word", &kb, 48).unwrap();
        // level 0 => no jitter/smooth/overshoot, identical to the stroke.
        for (a, b) in h.points.iter().zip(&ideal.points) {
            assert!((a.x - b.x).abs() < 1e-5 && (a.y - b.y).abs() < 1e-5);
        }
    }
}
