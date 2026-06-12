//! A minimal software rasterizer that writes `Argb8888` pixels into a byte
//! slice borrowed from a `wl_shm` buffer. No GPU, no allocations in the hot
//! path — just rectangle fills and bitmap-font blits (see ARCHITECTURE.md §2).

use crate::font;

/// A color as `0xAARRGGBB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color(pub u32);

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color(0xFF00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | b as u32)
    }
    #[inline]
    fn parts(self) -> (u8, u8, u8, u8) {
        let v = self.0;
        (
            (v >> 24) as u8, // a
            (v >> 16) as u8, // r
            (v >> 8) as u8,  // g
            v as u8,         // b
        )
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

    /// Single-pixel plot. Used by the gesture-trail renderer (milestone 4).
    #[allow(dead_code)]
    #[inline]
    fn put(&mut self, x: usize, y: usize, c: Color) {
        if x >= self.width || y >= self.height {
            return;
        }
        let (a, r, g, b) = c.parts();
        let i = (y * self.width + x) * 4;
        self.buf[i] = b;
        self.buf[i + 1] = g;
        self.buf[i + 2] = r;
        self.buf[i + 3] = a;
    }

    /// Fill the whole canvas with one color.
    pub fn clear(&mut self, c: Color) {
        let (a, r, g, b) = c.parts();
        for px in self.buf.chunks_exact_mut(4) {
            px[0] = b;
            px[1] = g;
            px[2] = r;
            px[3] = a;
        }
    }

    /// Fill an axis-aligned rectangle, clipped to the canvas.
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

    /// Stroke a rectangle outline `t` pixels thick (inset). Used by the
    /// suggestion bar / pressed-key outline in later milestones.
    #[allow(dead_code)]
    pub fn stroke_rect(&mut self, x: i32, y: i32, w: i32, h: i32, t: i32, c: Color) {
        self.fill_rect(x, y, w, t, c); // top
        self.fill_rect(x, y + h - t, w, t, c); // bottom
        self.fill_rect(x, y, t, h, c); // left
        self.fill_rect(x + w - t, y, t, h, c); // right
    }

    /// Draw a `thick`-pixel line from `(x0,y0)` to `(x1,y1)` (Bresenham, square
    /// caps). Used for the live gesture trail.
    pub fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, thick: i32, c: Color) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        let (mut x, mut y) = (x0, y0);
        let h = thick.max(1);
        loop {
            self.fill_rect(x - h / 2, y - h / 2, h, h, c);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// Draw one bitmap glyph at integer `scale`, top-left at `(x, y)`.
    pub fn blit_glyph(&mut self, ch: char, x: i32, y: i32, scale: usize, c: Color) {
        let g = font::glyph_or_box(ch);
        for (ry, row) in g.iter().enumerate() {
            for col in 0..font::GLYPH_W {
                // bit (GLYPH_W-1 - col) is the leftmost column
                if row & (1 << (font::GLYPH_W - 1 - col)) != 0 {
                    let px = x + (col * scale) as i32;
                    let py = y + (ry * scale) as i32;
                    self.fill_rect(px, py, scale as i32, scale as i32, c);
                }
            }
        }
    }

    /// Draw a left-aligned string at `(x, y)` with 1-scaled-px glyph spacing.
    pub fn draw_text(&mut self, text: &str, x: i32, y: i32, scale: usize, c: Color) {
        let advance = (font::GLYPH_W * scale + scale) as i32;
        let mut cx = x;
        for ch in text.chars() {
            self.blit_glyph(ch, cx, y, scale, c);
            cx += advance;
        }
    }

    /// Draw a string centered within the rectangle `(x, y, w, h)`.
    pub fn draw_text_centered(&mut self, text: &str, x: i32, y: i32, w: i32, h: i32, scale: usize, c: Color) {
        let tw = font::text_width(text, scale) as i32;
        let th = (font::GLYPH_H * scale) as i32;
        self.draw_text(text, x + (w - tw) / 2, y + (h - th) / 2, scale, c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_rect_clips_and_writes_bgra() {
        let mut buf = vec![0u8; 4 * 4 * 4]; // 4x4 px
        {
            let mut cv = Canvas::new(&mut buf, 4, 4);
            // Rect spans pixels (0,0)..(1,1) after clipping the negative origin.
            cv.fill_rect(-1, -1, 3, 3, Color::rgb(0x12, 0x34, 0x56));
        }
        // pixel (0,0) should be B,G,R,A = 56,34,12,FF
        assert_eq!(&buf[0..4], &[0x56, 0x34, 0x12, 0xFF]);
        // pixel (1,1) inside the rect too
        let i = (1 * 4 + 1) * 4;
        assert_eq!(&buf[i..i + 4], &[0x56, 0x34, 0x12, 0xFF]);
        // pixel (2,2) untouched (rect ends before it)
        let i = (2 * 4 + 2) * 4;
        assert_eq!(&buf[i..i + 4], &[0, 0, 0, 0]);
    }

    #[test]
    fn glyph_blit_marks_pixels() {
        let mut buf = vec![0u8; 8 * 8 * 4];
        {
            let mut cv = Canvas::new(&mut buf, 8, 8);
            cv.blit_glyph('-', 0, 0, 1, Color::rgb(255, 255, 255));
        }
        // '-' has its bar on row 3 (full 5 px). Row 0 should be empty.
        let row0_lit = (0..5).any(|x| buf[(x) * 4 + 3] != 0);
        let row3_lit = (0..5).all(|x| buf[(3 * 8 + x) * 4 + 3] != 0);
        assert!(!row0_lit, "row 0 should be blank for '-'");
        assert!(row3_lit, "row 3 should be the bar for '-'");
    }
}
