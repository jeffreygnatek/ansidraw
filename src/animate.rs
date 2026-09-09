//! Animated GIF output. The canvas is already indexed to the xterm-256
//! palette, which is GIF's native color model — frames go straight from
//! cells to indexed pixels with palette256 as the global palette, no
//! quantization.
//!
//! Two shapes of animation:
//! - "reveal": one canvas appears cell-by-cell in reading order with a
//!   block cursor, like a BBS download at modem speed.
//! - frame stacks: arbitrary canvases (ANSImation), one GIF frame each.

use std::fs::File;
use std::io;
use std::path::Path;

use crate::canvas::{Canvas, Cell, WIDTH};
use crate::cp437;
use crate::render_png::{palette256, FONT};

fn err<E: std::error::Error + Send + Sync + 'static>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e)
}

/// Rasterize to palette-indexed pixels at a fixed row count.
fn render_indexed(canvas: &Canvas, scale: usize, rows: usize) -> Vec<u8> {
    let w = WIDTH * 8 * scale;
    let h = rows * 16 * scale;
    let mut img = vec![0u8; w * h];
    for cy in 0..rows {
        for cx in 0..WIDTH {
            let cell = canvas.get(cx, cy);
            let b = cp437::char_to_byte(cell.ch) as usize;
            let glyph = &FONT[b * 16..b * 16 + 16];
            for (gy, &bits) in glyph.iter().enumerate() {
                for gx in 0..8 {
                    let idx = if bits & (0x80 >> gx) != 0 { cell.fg } else { cell.bg };
                    for sy in 0..scale {
                        let py = (cy * 16 + gy) * scale + sy;
                        let base = py * w + (cx * 8 + gx) * scale;
                        img[base..base + scale].fill(idx);
                    }
                }
            }
        }
    }
    img
}

/// Encode frames as a looping GIF. `delay_cs` is per-frame in centiseconds;
/// the final frame holds for `hold_cs`. Returns (width, height, frames).
pub fn write_gif(
    frames: &[Canvas],
    path: &Path,
    scale: usize,
    delay_cs: u16,
    hold_cs: u16,
) -> io::Result<(usize, usize, usize)> {
    if frames.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "no frames"));
    }
    let scale = scale.clamp(1, 4);
    let rows = frames
        .iter()
        .map(|c| c.last_used_row() + 1)
        .max()
        .unwrap_or(1);
    let w = WIDTH * 8 * scale;
    let h = rows * 16 * scale;
    let mut pal = Vec::with_capacity(768);
    for i in 0..=255u8 {
        pal.extend(palette256(i));
    }
    let file = File::create(path)?;
    let mut enc = gif::Encoder::new(file, w as u16, h as u16, &pal).map_err(err)?;
    enc.set_repeat(gif::Repeat::Infinite).map_err(err)?;
    for (i, c) in frames.iter().enumerate() {
        let mut f = gif::Frame::<'_> {
            width: w as u16,
            height: h as u16,
            buffer: render_indexed(c, scale, rows).into(),
            ..gif::Frame::default()
        };
        f.delay = if i + 1 == frames.len() { hold_cs.max(delay_cs) } else { delay_cs };
        enc.write_frame(&f).map_err(err)?;
    }
    Ok((w, h, frames.len()))
}

/// Build reveal frames: the art appears `cells_per_frame` cells at a time in
/// reading order, with a block cursor at the write position.
pub fn reveal_frames(canvas: &Canvas, cells_per_frame: usize) -> Vec<Canvas> {
    let rows = canvas.last_used_row() + 1;
    let total = rows * WIDTH;
    let step = cells_per_frame.max(1);
    let mut frames = Vec::new();
    let mut shown = 0usize;
    while shown < total {
        shown = (shown + step).min(total);
        let mut f = Canvas::new(rows);
        for i in 0..shown {
            let (y, x) = (i / WIDTH, i % WIDTH);
            f.set(x, y, canvas.get(x, y));
        }
        if shown < total {
            let (y, x) = (shown / WIDTH, shown % WIDTH);
            f.set(x, y, Cell { ch: '█', fg: 7, bg: 0 });
        }
        frames.push(f);
    }
    frames
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gif_roundtrip() {
        let mut a = Canvas::new(2);
        a.set(0, 0, Cell { ch: '█', fg: 12, bg: 0 });
        a.set(0, 1, Cell { ch: '▒', fg: 7, bg: 0 });
        let mut b = Canvas::new(2);
        b.set(1, 1, Cell { ch: '█', fg: 208, bg: 0 });
        let p = std::env::temp_dir().join("ansidraw_anim.gif");
        let (w, h, n) = write_gif(&[a, b], &p, 1, 10, 100).unwrap();
        assert_eq!((w, h, n), (640, 32, 2));
        let data = std::fs::read(&p).unwrap();
        assert_eq!(&data[0..6], b"GIF89a");
        let mut opts = gif::DecodeOptions::new();
        opts.set_color_output(gif::ColorOutput::Indexed);
        let mut dec = opts.read_info(std::io::Cursor::new(&data)).unwrap();
        let mut count = 0;
        let mut last_delay = 0;
        while let Some(f) = dec.read_next_frame().unwrap() {
            count += 1;
            last_delay = f.delay;
        }
        assert_eq!(count, 2);
        assert_eq!(last_delay, 100, "final frame holds");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn reveal_builds_progressive_frames() {
        let mut c = Canvas::new(2);
        for x in 0..10 {
            c.set(x, 0, Cell { ch: 'x', fg: 10, bg: 0 });
            c.set(x, 1, Cell { ch: 'x', fg: 10, bg: 0 });
        }
        let frames = reveal_frames(&c, 80);
        assert_eq!(frames.len(), 2, "80 cells/frame over 2 rows");
        // first frame shows only row 0 content plus cursor
        assert_eq!(frames[0].get(0, 0).ch, 'x');
        assert!(frames[0].get(0, 1).ch == '█');
        assert_eq!(frames[1].get(9, 0).ch, 'x');
    }
}
