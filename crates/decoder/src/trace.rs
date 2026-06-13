//! Gesture traces: the raw `(x, y, t)` path captured from a swipe, and the
//! ideal paths generated from word templates. Both are normalized through the
//! same resampling so the decoder can compare them point-for-point.
//!
//! Coordinates are in layout units (see [`crate::layout`]); `t` is milliseconds
//! from the start of the gesture. The decoder's scoring (milestone 3) builds on
//! the geometry here.

/// One sampled point of a gesture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    /// Milliseconds since the gesture began. Ideal/template traces use 0.
    pub t: u32,
}

impl Point {
    #[inline]
    pub fn new(x: f32, y: f32, t: u32) -> Self {
        Point { x, y, t }
    }

    #[inline]
    fn dist(&self, o: &Point) -> f32 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
}

/// An ordered path of points — a captured swipe or a generated ideal trace.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Trace {
    pub points: Vec<Point>,
}

impl Trace {
    #[inline]
    pub fn new() -> Self {
        Trace { points: Vec::new() }
    }

    pub fn from_points(points: Vec<Point>) -> Self {
        Trace { points }
    }

    #[inline]
    pub fn push(&mut self, p: Point) {
        self.points.push(p);
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.points.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn first(&self) -> Option<Point> {
        self.points.first().copied()
    }

    pub fn last(&self) -> Option<Point> {
        self.points.last().copied()
    }

    /// Total arc length of the path in layout units.
    pub fn path_length(&self) -> f32 {
        self.points
            .windows(2)
            .map(|w| w[0].dist(&w[1]))
            .sum()
    }

    /// Resample the path to exactly `n` points spaced equally along its arc
    /// length. This is the alignment step that lets two traces be compared
    /// point-for-point regardless of original sampling rate or speed.
    ///
    /// Timestamps are linearly interpolated. A degenerate trace (0 or 1 point,
    /// or zero length) is returned padded to `n` copies of its first point.
    pub fn resample(&self, n: usize) -> Trace {
        assert!(n >= 2, "resample needs at least 2 target points");
        if self.points.len() < 2 {
            let p = self.points.first().copied().unwrap_or(Point::new(0.0, 0.0, 0));
            return Trace::from_points(vec![p; n]);
        }
        let total = self.path_length();
        if total <= f32::EPSILON {
            return Trace::from_points(vec![self.points[0]; n]);
        }

        // Classic $1-recognizer resampling: walk the polyline accumulating
        // distance; every time we cross an interval boundary, emit an
        // interpolated point and treat it as the new previous vertex.
        let interval = total / (n as f32 - 1.0);
        let mut work = self.points.clone();
        let mut out = Vec::with_capacity(n);
        out.push(work[0]);

        let mut acc = 0.0f32;
        let mut i = 1;
        while i < work.len() {
            let a = work[i - 1];
            let b = work[i];
            let d = a.dist(&b);
            if acc + d >= interval && d > f32::EPSILON {
                let f = (interval - acc) / d;
                let q = Point::new(
                    a.x + (b.x - a.x) * f,
                    a.y + (b.y - a.y) * f,
                    (a.t as f32 + (b.t as f32 - a.t as f32) * f).round() as u32,
                );
                out.push(q);
                work.insert(i, q); // q becomes the previous vertex for the rest
                acc = 0.0;
            } else {
                acc += d;
            }
            i += 1;
        }
        // Floating-point drift can leave us one short; clamp to exactly n.
        while out.len() < n {
            out.push(*self.points.last().unwrap());
        }
        out.truncate(n);
        *out.last_mut().unwrap() = *self.points.last().unwrap();
        Trace::from_points(out)
    }

    /// Denoise the path with `passes` of an endpoint-preserving 3-tap weighted
    /// average (kernel `[0.25, 0.5, 0.25]`). Real touch/pointer input is jittery;
    /// a light smoothing before resampling steadies both the decode and the
    /// rendered trail. The center-weighted kernel rounds high-frequency tremor
    /// while keeping the deliberate corners (key visits) the decoder leans on,
    /// and the fixed endpoints keep the start/end the prune gate depends on.
    /// `passes == 0` returns a no-op clone.
    pub fn smoothed(&self, passes: usize) -> Trace {
        let mut pts = self.points.clone();
        for _ in 0..passes {
            if pts.len() < 3 {
                break;
            }
            let prev = pts.clone();
            for i in 1..prev.len() - 1 {
                pts[i].x = 0.25 * prev[i - 1].x + 0.5 * prev[i].x + 0.25 * prev[i + 1].x;
                pts[i].y = 0.25 * prev[i - 1].y + 0.5 * prev[i].y + 0.25 * prev[i + 1].y;
            }
        }
        Trace::from_points(pts)
    }

    /// Centroid (mean position) of all points.
    pub fn centroid(&self) -> (f32, f32) {
        if self.points.is_empty() {
            return (0.0, 0.0);
        }
        let (sx, sy) = self
            .points
            .iter()
            .fold((0.0f32, 0.0f32), |(ax, ay), p| (ax + p.x, ay + p.y));
        let n = self.points.len() as f32;
        (sx / n, sy / n)
    }

    /// Axis-aligned bounding box as `(min_x, min_y, max_x, max_y)`.
    pub fn bbox(&self) -> (f32, f32, f32, f32) {
        let mut min = (f32::INFINITY, f32::INFINITY);
        let mut max = (f32::NEG_INFINITY, f32::NEG_INFINITY);
        for p in &self.points {
            min.0 = min.0.min(p.x);
            min.1 = min.1.min(p.y);
            max.0 = max.0.max(p.x);
            max.1 = max.1.max(p.y);
        }
        (min.0, min.1, max.0, max.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f32, y: f32) -> Point {
        Point::new(x, y, 0)
    }

    #[test]
    fn path_length_of_straight_line() {
        let t = Trace::from_points(vec![pt(0.0, 0.0), pt(3.0, 4.0)]);
        assert!((t.path_length() - 5.0).abs() < 1e-5);
    }

    #[test]
    fn resample_straight_line_is_equidistant() {
        let t = Trace::from_points(vec![pt(0.0, 0.0), pt(10.0, 0.0)]);
        let r = t.resample(11);
        assert_eq!(r.len(), 11);
        for (i, p) in r.points.iter().enumerate() {
            assert!((p.x - i as f32).abs() < 1e-4, "point {i} at {}", p.x);
            assert!(p.y.abs() < 1e-4);
        }
    }

    #[test]
    fn resample_preserves_endpoints() {
        let t = Trace::from_points(vec![pt(1.0, 2.0), pt(5.0, 1.0), pt(9.0, 7.0)]);
        let r = t.resample(50);
        assert_eq!(r.first().unwrap(), pt(1.0, 2.0));
        assert_eq!(r.last().unwrap(), pt(9.0, 7.0));
    }

    #[test]
    fn resample_corner_path_keeps_length() {
        // L-shape: right 4 then up 3, total length 7.
        let t = Trace::from_points(vec![pt(0.0, 0.0), pt(4.0, 0.0), pt(4.0, 3.0)]);
        let r = t.resample(29);
        // Resampled polyline length should match the original closely.
        assert!((r.path_length() - 7.0).abs() < 1e-2, "len {}", r.path_length());
    }

    #[test]
    fn smoothed_fixes_endpoints_and_reduces_a_spike() {
        // A straight line with one point spiked off-axis.
        let t = Trace::from_points(vec![pt(0.0, 0.0), pt(1.0, 5.0), pt(2.0, 0.0), pt(3.0, 0.0)]);
        let s = t.smoothed(1);
        // Endpoints are untouched.
        assert_eq!(s.first().unwrap(), pt(0.0, 0.0));
        assert_eq!(s.last().unwrap(), pt(3.0, 0.0));
        // The spike is pulled toward its neighbours (0.25*0 + 0.5*5 + 0.25*0).
        assert!((s.points[1].y - 2.5).abs() < 1e-5, "y = {}", s.points[1].y);
        // x of interior points is preserved on an evenly-spaced line.
        assert!((s.points[1].x - 1.0).abs() < 1e-5);
    }

    #[test]
    fn smoothed_zero_passes_is_identity() {
        let t = Trace::from_points(vec![pt(0.0, 0.0), pt(1.0, 5.0), pt(2.0, 0.0)]);
        assert_eq!(t.smoothed(0), t);
    }

    #[test]
    fn degenerate_trace_pads() {
        let t = Trace::from_points(vec![pt(2.0, 2.0)]);
        let r = t.resample(5);
        assert_eq!(r.len(), 5);
        assert!(r.points.iter().all(|p| *p == pt(2.0, 2.0)));
    }
}
