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

/// Curvature floor (1/key-unit) so straight segments move at a finite top speed
/// rather than infinitely fast. Larger → a flatter speed profile.
const KAPPA_FLOOR: f32 = 0.15;

/// Re-stamp `trace` with a curvature-aware velocity profile. Tangential speed
/// follows the 2/3 power law of human movement (v ∝ curvature^(-1/3)), so the
/// swipe slows through the sharp corners at letter centroids and accelerates on
/// the straights between them — the dwell pattern real swipes show and the
/// velocity-weighted decoder keys on. Geometry is untouched; only `t` changes,
/// and total duration is rescaled to match `sample_stroke`'s ~120 ms/key-unit so
/// absolute times stay realistic.
pub fn stamp_velocity(trace: &Trace) -> Trace {
    let pts = trace.points.clone();
    let n = pts.len();
    if n < 3 {
        return Trace::from_points(pts);
    }
    // Per-point speed from the local turning angle (a curvature proxy, dθ/ds).
    let top_speed = KAPPA_FLOOR.powf(-1.0 / 3.0);
    let speed: Vec<f32> = (0..n)
        .map(|i| {
            if i == 0 || i == n - 1 {
                return top_speed; // endpoints: no corner, full speed
            }
            let (ax, ay) = (pts[i].x - pts[i - 1].x, pts[i].y - pts[i - 1].y);
            let (bx, by) = (pts[i + 1].x - pts[i].x, pts[i + 1].y - pts[i].y);
            let la = (ax * ax + ay * ay).sqrt().max(1e-6);
            let lb = (bx * bx + by * by).sqrt().max(1e-6);
            let cos = ((ax * bx + ay * by) / (la * lb)).clamp(-1.0, 1.0);
            let kappa = cos.acos() / (0.5 * (la + lb));
            (kappa + KAPPA_FLOOR).powf(-1.0 / 3.0)
        })
        .collect();
    // Integrate dt = ds / v_avg, then rescale to the target total duration.
    let mut acc = vec![0.0f32; n];
    for i in 1..n {
        let (dx, dy) = (pts[i].x - pts[i - 1].x, pts[i].y - pts[i - 1].y);
        let ds = (dx * dx + dy * dy).sqrt();
        let v = 0.5 * (speed[i - 1] + speed[i]);
        acc[i] = acc[i - 1] + ds / v.max(1e-6);
    }
    let target = trace.path_length() * 120.0;
    let scale = if acc[n - 1] > 1e-6 { target / acc[n - 1] } else { 0.0 };
    let mut out = pts;
    for (i, p) in out.iter_mut().enumerate() {
        p.t = (acc[i] * scale) as u32;
    }
    Trace::from_points(out)
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
    // Stamp a realistic velocity profile on the clean path before jittering, so
    // the timing reflects the intended dwell-at-corners, not the noise.
    let stroke = stamp_velocity(&stroke);
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
    fn velocity_slows_at_corners() {
        // An L-shape: straight along x, a sharp 90° turn, straight along y. All
        // segments are unit length, so timing differences come only from speed.
        let t = Trace::from_points(vec![
            Point::new(0.0, 0.0, 0),
            Point::new(1.0, 0.0, 0),
            Point::new(2.0, 0.0, 0),
            Point::new(2.0, 1.0, 0),
            Point::new(2.0, 2.0, 0),
        ]);
        let v = stamp_velocity(&t);
        let dt = |i: usize| v.points[i + 1].t as i64 - v.points[i - 1].t as i64;
        // The corner (index 2) is traversed slower than a straight stretch.
        assert!(dt(2) > dt(1), "corner dt {} should exceed straight dt {}", dt(2), dt(1));
        // Times are monotonic and geometry is untouched.
        assert!(v.points.windows(2).all(|w| w[1].t >= w[0].t));
        assert_eq!((v.points[2].x, v.points[2].y), (2.0, 0.0));
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
