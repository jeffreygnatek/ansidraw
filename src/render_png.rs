//! Render the canvas to PNG pixels using the classic VGA 8x16 bitmap font
//! and the authentic 16-color VGA palette.
//!
//! Font bitmap (assets/vga8x16.bin) extracted from libansilove
//! (https://www.ansilove.org, BSD-2-Clause) — the same font 16colo.rs uses.

use std::fs;
use std::io;
use std::path::Path;

use crate::canvas::{Canvas, WIDTH};
use crate::cp437;

pub(crate) const FONT: &[u8; 4096] = include_bytes!("../assets/vga8x16.bin");
const CELL_W: usize = 8;
const CELL_H: usize = 16;

/// The VGA text-mode palette.
pub const PALETTE: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00], // black
    [0x00, 0x00, 0xAA], // blue
    [0x00, 0xAA, 0x00], // green
    [0x00, 0xAA, 0xAA], // cyan
    [0xAA, 0x00, 0x00], // red
    [0xAA, 0x00, 0xAA], // magenta
    [0xAA, 0x55, 0x00], // brown
    [0xAA, 0xAA, 0xAA], // light gray
    [0x55, 0x55, 0x55], // dark gray
    [0x55, 0x55, 0xFF], // light blue
    [0x55, 0xFF, 0x55], // light green
    [0x55, 0xFF, 0xFF], // light cyan
    [0xFF, 0x55, 0x55], // light red
    [0xFF, 0x55, 0xFF], // light magenta
    [0xFF, 0xFF, 0x55], // yellow
    [0xFF, 0xFF, 0xFF], // white
];

/// RGB for any xterm-256 palette index: 0-15 VGA, 16-231 the 6x6x6 color
/// cube, 232-255 the grayscale ramp.
pub fn palette256(i: u8) -> [u8; 3] {
    match i {
        0..=15 => PALETTE[i as usize],
        16..=231 => {
            let n = i as usize - 16;
            let l = |v: usize| if v == 0 { 0u8 } else { (55 + 40 * v) as u8 };
            [l(n / 36), l((n / 6) % 6), l(n % 6)]
        }
        _ => {
            let v = (8 + 10 * (i as usize - 232)) as u8;
            [v, v, v]
        }
    }
}

/// Rasterize to RGB8; returns (width, height, pixels).
pub fn render_rgb(canvas: &Canvas, scale: usize) -> (usize, usize, Vec<u8>) {
    let rows = canvas.last_used_row() + 1;
    let w = WIDTH * CELL_W * scale;
    let h = rows * CELL_H * scale;
    let mut img = vec![0u8; w * h * 3];
    for cy in 0..rows {
        for cx in 0..WIDTH {
            let cell = canvas.get(cx, cy);
            let b = cp437::char_to_byte(cell.ch) as usize;
            let glyph = &FONT[b * 16..b * 16 + 16];
            let fgc = palette256(cell.fg);
            let bgc = palette256(cell.bg);
            for gy in 0..CELL_H {
                let bits = glyph[gy];
                for gx in 0..CELL_W {
                    let c = if bits & (0x80 >> gx) != 0 { fgc } else { bgc };
                    for sy in 0..scale {
                        let py = (cy * CELL_H + gy) * scale + sy;
                        let base = (py * w + (cx * CELL_W + gx) * scale) * 3;
                        for sx in 0..scale {
                            let o = base + sx * 3;
                            img[o..o + 3].copy_from_slice(&c);
                        }
                    }
                }
            }
        }
    }
    (w, h, img)
}

/// Encode the canvas as a PNG in memory. `scale` is clamped to 1-8.
pub fn png_bytes(canvas: &Canvas, scale: usize) -> io::Result<Vec<u8>> {
    let scale = scale.clamp(1, 8);
    let (w, h, img) = render_rgb(canvas, scale);
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(io_err)?;
        writer.write_image_data(&img).map_err(io_err)?;
    }
    Ok(buf)
}

pub fn save_png(canvas: &Canvas, path: &Path, scale: usize) -> io::Result<(usize, usize)> {
    let scale = scale.clamp(1, 8);
    let rows = canvas.last_used_row() + 1;
    fs::write(path, png_bytes(canvas, scale)?)?;
    Ok((WIDTH * CELL_W * scale, rows * CELL_H * scale))
}

fn io_err<E: std::error::Error + Send + Sync + 'static>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::Cell;

    #[test]
    fn renders_expected_pixels() {
        let mut c = Canvas::new(1);
        c.set(0, 0, Cell { ch: '█', fg: 4, bg: 0 }); // full block, red on black
        c.set(1, 0, Cell { ch: ' ', fg: 7, bg: 1 }); // space on blue
        let (w, h, img) = render_rgb(&c, 1);
        assert_eq!((w, h), (WIDTH * 8, 16));
        // top-left pixel of the full block is pure VGA red
        assert_eq!(&img[0..3], &[0xAA, 0x00, 0x00]);
        // first pixel of cell 1 is VGA blue background
        let o = 8 * 3;
        assert_eq!(&img[o..o + 3], &[0x00, 0x00, 0xAA]);
    }

    #[test]
    fn png_magic_and_scale() {
        let c = Canvas::new(2);
        let bytes = png_bytes(&c, 3).unwrap();
        assert_eq!(&bytes[0..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    }
}
