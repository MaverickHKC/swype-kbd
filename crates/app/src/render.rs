//! A small software rasterizer that writes `Argb8888` pixels into a byte slice
//! borrowed from a `wl_shm` buffer. No GPU. Beyond flat fills it does
//! anti-aliased rounded rectangles, TrueType text (via `ab_glyph`), and a
//! coverage-accumulated gesture trail, so the keyboard reads as a finished UI
//! rather than a bitmap mock-up (see ARCHITECTURE.md §2).

// Drawing primitives naturally take a geometry + color (+ alpha) argument list.
#![allow(clippy::too_many_arguments)]

use ab_glyph::{point, Font, PxScale, ScaleFont};

/// A color as `0xAARRGGBB`. The alpha byte is informational; compositing uses an
/// explicit coverage argument so callers control translucency per draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color(pub u32);

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color(0xFF00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | b as u32)
    }
    #[inline]
    fn parts(self) -> (u8, u8, u8, u8) {
        let v = self.0;
        ((v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8)
    }
}

/// A mutable view over a tightly-packed `Argb8888` framebuffer.
///
/// `wl_shm`'s `Argb8888` is 32-bit little-endian, i.e. bytes laid out B, G, R,
/// A. We write that order explicitly so the code is endianness-correct.
pub struct Canvas<'a> {
    buf: &'a mut [u8],
    width: usize,
    height: usize,
}

impl<'a> Canvas<'a> {
    /// `buf` must be at least `width * height * 4` bytes.
    pub fn new(buf: &'a mut [u8], width: usize, height: usize) -> Self {
        debug_assert!(buf.len() >= width * height * 4);
        Canvas { buf, width, height }
    }

    #[allow(dead_code)]
    pub fn width(&self) -> usize {
        self.width
    }
    #[allow(dead_code)]
    pub fn height(&self) -> usize {
        self.height
    }

    /// Fill the whole canvas with one (opaque) color.
    pub fn clear(&mut self, c: Color) {
        let (a, r, g, b) = c.parts();
        for px in self.buf.chunks_exact_mut(4) {
            px[0] = b;
            px[1] = g;
            px[2] = r;
            px[3] = a;
        }
    }

    /// Fill an axis-aligned rectangle with an opaque color, clipped to the canvas.
    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Color) {
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = ((x + w).max(0) as usize).min(self.width);
        let y1 = ((y + h).max(0) as usize).min(self.height);
        let (a, r, g, b) = c.parts();
        for yy in y0..y1 {
            let row = yy * self.width;
            for xx in x0..x1 {
                let i = (row + xx) * 4;
                self.buf[i] = b;
                self.buf[i + 1] = g;
                self.buf[i + 2] = r;
                self.buf[i + 3] = a;
            }
        }
    }

    /// Alpha-blend `c` over the pixel at `(x, y)` with coverage `cov` in `[0, 1]`.
    /// The surface stays opaque.
    #[inline]
    fn blend(&mut self, x: i32, y: i32, c: Color, cov: f32) {
        if cov <= 0.0 || x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return;
        }
        let a = cov.min(1.0);
        let inv = 1.0 - a;
        let (_, r, g, b) = c.parts();
        let i = (y as usize * self.width + x as usize) * 4;
        self.buf[i] = (b as f32 * a + self.buf[i] as f32 * inv) as u8;
        self.buf[i + 1] = (g as f32 * a + self.buf[i + 1] as f32 * inv) as u8;
        self.buf[i + 2] = (r as f32 * a + self.buf[i + 2] as f32 * inv) as u8;
        self.buf[i + 3] = 0xFF;
    }

    /// Fill a rounded rectangle with anti-aliased corners. Straight spans use the
    /// fast opaque path; only the four corner squares are blended per-pixel.
    pub fn fill_round_rect(&mut self, x: i32, y: i32, w: i32, h: i32, radius: f32, c: Color) {
        if w <= 0 || h <= 0 {
            return;
        }
        let r = radius.min(w as f32 / 2.0).min(h as f32 / 2.0).max(0.0);
        let ri = r.ceil() as i32;
        // Opaque interior: a tall middle band plus the two side bands, leaving the
        // four ri×ri corner squares for the anti-aliased arcs.
        self.fill_rect(x + ri, y, w - 2 * ri, h, c);
        self.fill_rect(x, y + ri, ri, h - 2 * ri, c);
        self.fill_rect(x + w - ri, y + ri, ri, h - 2 * ri, c);
        self.round_corners(x, y, w, h, ri, r, c, 1.0);
    }

    /// Like [`fill_round_rect`] but blended at constant `alpha` everywhere — for
    /// translucent fills such as key drop-shadows.
    pub fn fill_round_rect_alpha(
        &mut self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        radius: f32,
        c: Color,
        alpha: f32,
    ) {
        if w <= 0 || h <= 0 || alpha <= 0.0 {
            return;
        }
        let r = radius.min(w as f32 / 2.0).min(h as f32 / 2.0).max(0.0);
        let ri = r.ceil() as i32;
        self.fill_rect_alpha(x + ri, y, w - 2 * ri, h, c, alpha);
        self.fill_rect_alpha(x, y + ri, ri, h - 2 * ri, c, alpha);
        self.fill_rect_alpha(x + w - ri, y + ri, ri, h - 2 * ri, c, alpha);
        self.round_corners(x, y, w, h, ri, r, c, alpha);
    }

    /// Blend a constant-`alpha` rectangle, clipped to the canvas.
    fn fill_rect_alpha(&mut self, x: i32, y: i32, w: i32, h: i32, c: Color, alpha: f32) {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w).min(self.width as i32);
        let y1 = (y + h).min(self.height as i32);
        for py in y0..y1 {
            for px in x0..x1 {
                self.blend(px, py, c, alpha);
            }
        }
    }

    /// Paint the four anti-aliased quarter-disc corners of a rounded rect.
    #[allow(clippy::too_many_arguments)]
    fn round_corners(&mut self, x: i32, y: i32, w: i32, h: i32, ri: i32, r: f32, c: Color, alpha: f32) {
        self.fill_corner(x as f32 + r, y as f32 + r, x, y, ri, r, c, alpha);
        self.fill_corner((x + w) as f32 - r, y as f32 + r, x + w - ri, y, ri, r, c, alpha);
        self.fill_corner(x as f32 + r, (y + h) as f32 - r, x, y + h - ri, ri, r, c, alpha);
        self.fill_corner((x + w) as f32 - r, (y + h) as f32 - r, x + w - ri, y + h - ri, ri, r, c, alpha);
    }

    /// Anti-aliased quarter-disc covering an `ri×ri` box anchored at `(bx, by)`,
    /// centered on `(cx, cy)` with radius `r`, scaled by `alpha`.
    #[allow(clippy::too_many_arguments)]
    fn fill_corner(&mut self, cx: f32, cy: f32, bx: i32, by: i32, ri: i32, r: f32, c: Color, alpha: f32) {
        for py in by..by + ri {
            for px in bx..bx + ri {
                let dx = px as f32 + 0.5 - cx;
                let dy = py as f32 + 0.5 - cy;
                let cov = (r + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
                self.blend(px, py, c, cov * alpha);
            }
        }
    }

    /// Stroke a polyline as a round-capped ribbon of total width `width`. Coverage
    /// is accumulated (max) into a scratch buffer over the path's bounding box and
    /// blended once, so a translucent trail does not darken where segments overlap.
    pub fn stroke_trail(&mut self, pts: &[(f32, f32)], width: f32, c: Color, alpha: f32) {
        if pts.len() < 2 || width <= 0.0 {
            return;
        }
        let r = width / 2.0;
        let pad = r + 1.0;
        let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &(px, py) in pts {
            minx = minx.min(px);
            miny = miny.min(py);
            maxx = maxx.max(px);
            maxy = maxy.max(py);
        }
        let bx0 = ((minx - pad).floor() as i32).max(0);
        let by0 = ((miny - pad).floor() as i32).max(0);
        let bx1 = ((maxx + pad).ceil() as i32).min(self.width as i32);
        let by1 = ((maxy + pad).ceil() as i32).min(self.height as i32);
        if bx1 <= bx0 || by1 <= by0 {
            return;
        }
        let (bw, bh) = ((bx1 - bx0) as usize, (by1 - by0) as usize);
        let mut cov = vec![0f32; bw * bh];
        for seg in pts.windows(2) {
            let (x0, y0) = seg[0];
            let (x1, y1) = seg[1];
            let sx0 = ((x0.min(x1) - pad).floor() as i32).max(bx0);
            let sy0 = ((y0.min(y1) - pad).floor() as i32).max(by0);
            let sx1 = ((x0.max(x1) + pad).ceil() as i32).min(bx1);
            let sy1 = ((y0.max(y1) + pad).ceil() as i32).min(by1);
            for py in sy0..sy1 {
                for px in sx0..sx1 {
                    let d = dist_to_segment(px as f32 + 0.5, py as f32 + 0.5, x0, y0, x1, y1);
                    let cv = (r + 0.5 - d).clamp(0.0, 1.0);
                    if cv > 0.0 {
                        let idx = (py - by0) as usize * bw + (px - bx0) as usize;
                        if cv > cov[idx] {
                            cov[idx] = cv;
                        }
                    }
                }
            }
        }
        for j in 0..bh {
            for i in 0..bw {
                let cv = cov[j * bw + i];
                if cv > 0.0 {
                    self.blend(bx0 + i as i32, by0 + j as i32, c, cv * alpha);
                }
            }
        }
    }

    /// Draw `text` centered in the box `(x, y, w, h)` at pixel size `size`,
    /// anti-aliased through `font`.
    pub fn text_centered<F: Font>(
        &mut self,
        font: &F,
        text: &str,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        size: f32,
        c: Color,
    ) {
        let px = PxScale::from(size);
        let scaled = font.as_scaled(px);
        let mut width = 0.0f32;
        let mut prev = None;
        for ch in text.chars() {
            let id = font.glyph_id(ch);
            if let Some(p) = prev {
                width += scaled.kern(p, id);
            }
            width += scaled.h_advance(id);
            prev = Some(id);
        }
        // Center the ascent..descent band vertically; lay glyphs from the centered
        // start along the baseline.
        let (ascent, descent) = (scaled.ascent(), scaled.descent());
        let baseline = y as f32 + (h as f32 - (ascent - descent)) / 2.0 + ascent;
        let mut caret = x as f32 + (w as f32 - width) / 2.0;
        let mut prev = None;
        for ch in text.chars() {
            let id = font.glyph_id(ch);
            if let Some(p) = prev {
                caret += scaled.kern(p, id);
            }
            let glyph = id.with_scale_and_position(px, point(caret, baseline));
            if let Some(outline) = font.outline_glyph(glyph) {
                let bb = outline.px_bounds();
                outline.draw(|gx, gy, coverage| {
                    self.blend(bb.min.x as i32 + gx as i32, bb.min.y as i32 + gy as i32, c, coverage);
                });
            }
            caret += scaled.h_advance(id);
            prev = Some(id);
        }
    }
}

/// Euclidean distance from `(px, py)` to the segment `(x0,y0)-(x1,y1)`.
#[inline]
fn dist_to_segment(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> f32 {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let len2 = dx * dx + dy * dy;
    let t = if len2 <= f32::EPSILON {
        0.0
    } else {
        (((px - x0) * dx + (py - y0) * dy) / len2).clamp(0.0, 1.0)
    };
    let (cx, cy) = (x0 + t * dx, y0 + t * dy);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_rect_clips_and_writes_bgra() {
        let mut buf = vec![0u8; 4 * 4 * 4]; // 4x4 px
        {
            let mut cv = Canvas::new(&mut buf, 4, 4);
            cv.fill_rect(-1, -1, 3, 3, Color::rgb(0x12, 0x34, 0x56));
        }
        assert_eq!(&buf[0..4], &[0x56, 0x34, 0x12, 0xFF]);
        let i = (1 * 4 + 1) * 4;
        assert_eq!(&buf[i..i + 4], &[0x56, 0x34, 0x12, 0xFF]);
        let i = (2 * 4 + 2) * 4;
        assert_eq!(&buf[i..i + 4], &[0, 0, 0, 0]);
    }

    #[test]
    fn blend_half_coverage_is_midpoint() {
        let mut buf = vec![0u8; 4]; // 1x1
        {
            let mut cv = Canvas::new(&mut buf, 1, 1);
            cv.blend(0, 0, Color::rgb(0xFF, 0xFF, 0xFF), 0.5);
        }
        // black background blended halfway to white ~= 0x7F, opaque alpha.
        assert!((buf[0] as i32 - 0x7F).abs() <= 1);
        assert_eq!(buf[3], 0xFF);
    }

    #[test]
    fn round_rect_corner_is_softer_than_center() {
        let mut buf = vec![0u8; 20 * 20 * 4];
        {
            let mut cv = Canvas::new(&mut buf, 20, 20);
            cv.fill_round_rect(0, 0, 20, 20, 6.0, Color::rgb(0xFF, 0xFF, 0xFF));
        }
        // The extreme corner pixel is only partly covered; the center is solid.
        let corner = buf[(0 * 20 + 0) * 4];
        let center = buf[(10 * 20 + 10) * 4];
        assert!(corner < center, "corner {corner} should be softer than center {center}");
        assert_eq!(center, 0xFF);
    }

    #[test]
    fn dist_to_segment_endpoint_and_midline() {
        assert!((dist_to_segment(0.0, 3.0, 0.0, 0.0, 10.0, 0.0) - 3.0).abs() < 1e-5);
        assert!((dist_to_segment(-4.0, 0.0, 0.0, 0.0, 10.0, 0.0) - 4.0).abs() < 1e-5);
    }
}
