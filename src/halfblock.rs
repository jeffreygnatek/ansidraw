//! Half-block pixel space: treat the canvas as a WIDTH x (2*MAX_ROWS) grid of
//! square pixels, two per text cell, packed into ▀/▄/█ glyphs. Plotting a
//! pixel preserves the other half of the cell, so pixel art can be built
//! incrementally in any order.
//!
//! ANSI constraint: a cell's background can only be colors 0-7, so when two
//! *different bright* colors (8-15) share a cell vertically, the bottom one
//! is degraded to its nearest dark neighbor. Align bright color boundaries
//! to even pixel rows to avoid this.

use crate::canvas::{Canvas, Cell, MAX_ROWS, WIDTH};
use crate::render_png::PALETTE;

pub const PIX_W: usize = WIDTH;
pub const PIX_H: usize = MAX_ROWS * 2;

fn dist(a: [u8; 3], b: [u8; 3]) -> u32 {
    let dr = a[0] as i32 - b[0] as i32;
    let dg = a[1] as i32 - b[1] as i32;
    let db = a[2] as i32 - b[2] as i32;
    (dr * dr + dg * dg + db * db) as u32
}

/// Nearest dark (bg-capable) color to the given palette color.
fn nearest_dark(c: u8) -> u8 {
    let p = PALETTE[(c & 15) as usize];
    let mut best = 0u8;
    let mut bd = u32::MAX;
    for (i, q) in PALETTE[..8].iter().enumerate() {
        let d = dist(p, *q);
        if d < bd {
            bd = d;
            best = i as u8;
        }
    }
    best
}

/// Decode a cell into its (top, bottom) pixel colors. Non-block glyphs are
/// treated as their background color.
pub fn cell_pixels(cell: Cell) -> (u8, u8) {
    match cell.ch {
        '▀' => (cell.fg, cell.bg),
        '▄' => (cell.bg, cell.fg),
        '█' => (cell.fg, cell.fg),
        _ => (cell.bg, cell.bg),
    }
}

/// Encode a (top, bottom) pixel pair as the best available cell.
pub fn pixels_to_cell(top: u8, bottom: u8) -> Cell {
    let (top, bottom) = (top & 15, bottom & 15);
    if top == bottom {
        return if top == 0 {
            Cell::default()
        } else {
            Cell { ch: '█', fg: top, bg: 0 }
        };
    }
    if bottom < 8 {
        Cell { ch: '▀', fg: top, bg: bottom }
    } else if top < 8 {
        Cell { ch: '▄', fg: bottom, bg: top }
    } else {
        // two different brights: bottom loses, dimmed into the background
        Cell { ch: '▀', fg: top, bg: nearest_dark(bottom) }
    }
}

pub fn set_pixel(canvas: &mut Canvas, x: usize, y: usize, color: u8) {
    if x >= PIX_W || y >= PIX_H {
        return;
    }
    let cy = y / 2;
    let (mut t, mut b) = cell_pixels(canvas.get(x, cy));
    if y % 2 == 0 {
        t = color;
    } else {
        b = color;
    }
    canvas.set(x, cy, pixels_to_cell(t, b));
}

/// 4x4 Bayer ordered-dither matrix (values 0-15).
pub const BAYER4: [[u8; 4]; 4] = [
    [0, 8, 2, 10],
    [12, 4, 14, 6],
    [3, 11, 1, 9],
    [15, 7, 13, 5],
];

/// Bayer threshold in 0.0-1.0 for a pixel position.
pub fn bayer(x: usize, y: usize) -> f64 {
    (BAYER4[y % 4][x % 4] as f64 + 0.5) / 16.0
}

/// Fill a pixel rect with an ordered-dither mix of two colors; `mix` is the
/// fraction of `c2` (0.0 = all c1, 1.0 = all c2). Fakes gradient steps the
/// 16-color palette can't reach.
pub fn dither_rect(
    canvas: &mut Canvas,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    c1: u8,
    c2: u8,
    mix: f64,
) {
    for py in y..(y + h).min(PIX_H) {
        for px in x..(x + w).min(PIX_W) {
            let c = if mix > bayer(px, py) { c2 } else { c1 };
            set_pixel(canvas, px, py, c);
        }
    }
}

pub fn fill_rect(canvas: &mut Canvas, x: usize, y: usize, w: usize, h: usize, color: u8) {
    for py in y..(y + h).min(PIX_H) {
        for px in x..(x + w).min(PIX_W) {
            set_pixel(canvas, px, py, color);
        }
    }
}

/// Bresenham line in pixel space.
pub fn line(canvas: &mut Canvas, x0: i64, y0: i64, x1: i64, y1: i64, color: u8) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let (mut x, mut y) = (x0, y0);
    loop {
        if x >= 0 && y >= 0 {
            set_pixel(canvas, x as usize, y as usize, color);
        }
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

/// Ellipse in pixel space; filled, or a ~1px outline band. `y_clip` limits
/// drawing to pixel rows within (min, max) inclusive — useful for arcs.
pub fn ellipse(
    canvas: &mut Canvas,
    cx: i64,
    cy: i64,
    rx: i64,
    ry: i64,
    color: u8,
    fill: bool,
    y_clip: Option<(i64, i64)>,
) {
    let (rx, ry) = (rx.max(1), ry.max(1));
    let inner = 1.0 - 2.0 / rx.min(ry) as f64;
    for y in (cy - ry)..=(cy + ry) {
        if let Some((lo, hi)) = y_clip {
            if y < lo || y > hi {
                continue;
            }
        }
        for x in (cx - rx)..=(cx + rx) {
            if x < 0 || y < 0 {
                continue;
            }
            let nx = (x - cx) as f64 / rx as f64;
            let ny = (y - cy) as f64 / ry as f64;
            let d = nx * nx + ny * ny;
            let hit = if fill { d <= 1.0 } else { d <= 1.0 && d >= inner * inner };
            if hit {
                set_pixel(canvas, x as usize, y as usize, color);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_roundtrip_preserves_other_half() {
        let mut c = Canvas::new(2);
        set_pixel(&mut c, 3, 0, 14); // top: yellow
        set_pixel(&mut c, 3, 1, 4); // bottom: dark red
        let cell = c.get(3, 0);
        assert_eq!(cell_pixels(cell), (14, 4));
        assert_eq!(cell, Cell { ch: '▀', fg: 14, bg: 4 });
        // overwrite just the top; bottom must survive
        set_pixel(&mut c, 3, 0, 15);
        assert_eq!(cell_pixels(c.get(3, 0)), (15, 4));
    }

    #[test]
    fn same_color_pair_is_full_block() {
        assert_eq!(pixels_to_cell(9, 9), Cell { ch: '█', fg: 9, bg: 0 });
        assert!(pixels_to_cell(0, 0).is_blank());
    }

    #[test]
    fn bright_bottom_degrades_to_dark() {
        let cell = pixels_to_cell(15, 12); // white over light red
        assert_eq!(cell.ch, '▀');
        assert_eq!(cell.fg, 15);
        assert!(cell.bg < 8, "bg must be dark, got {}", cell.bg);
    }

    #[test]
    fn dither_mixes_both_colors() {
        let mut c = Canvas::new(4);
        dither_rect(&mut c, 0, 0, 8, 8, 1, 3, 0.5);
        let mut seen = [false; 16];
        for y in 0..4 {
            for x in 0..8 {
                let (t, b) = cell_pixels(c.get(x, y));
                seen[t as usize] = true;
                seen[b as usize] = true;
            }
        }
        assert!(seen[1] && seen[3], "both colors should appear at mix 0.5");
    }

    #[test]
    fn shapes_stay_in_bounds() {
        let mut c = Canvas::new(4);
        line(&mut c, -5, -5, 100, 20, 10);
        ellipse(&mut c, 0, 0, 10, 10, 12, true, None);
        fill_rect(&mut c, 70, 0, 40, 40, 9); // clips at 80
        assert!(c.get(79, 0).fg == 9 || c.get(79, 0).ch == '█');
    }
}
