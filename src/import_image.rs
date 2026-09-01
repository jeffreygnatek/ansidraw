//! Convert a PNG image into canvas cells using the half-block technique:
//! each cell holds two vertically stacked "pixels" via ▀/▄/█, quantized to
//! the 16-color VGA palette (backgrounds limited to the dark 8, as ANSI
//! demands — the bright pixel of each pair goes to the foreground).

use std::io;
use std::path::Path;

use crate::canvas::{Canvas, Cell, MAX_ROWS, WIDTH};
use crate::render_png::{palette256, PALETTE};

fn err<E: std::error::Error + Send + Sync + 'static>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

fn dist(a: [u8; 3], b: [u8; 3]) -> u32 {
    let dr = a[0] as i32 - b[0] as i32;
    let dg = a[1] as i32 - b[1] as i32;
    let db = a[2] as i32 - b[2] as i32;
    (dr * dr + dg * dg + db * db) as u32
}

/// Nearest palette index among the first `limit` colors (16 for fg, 8 for bg).
fn nearest(c: [u8; 3], limit: usize) -> u8 {
    let mut best = 0;
    let mut bd = u32::MAX;
    for (i, p) in PALETTE[..limit].iter().enumerate() {
        let d = dist(c, *p);
        if d < bd {
            bd = d;
            best = i as u8;
        }
    }
    best
}

/// 256-color pair: any color can be a background, so no degrade logic.
fn pair_to_cell_256(top: [u8; 3], bot: [u8; 3], pal: &[[u8; 3]; 256]) -> Cell {
    let near = |c: [u8; 3]| -> u8 {
        let mut best = 0;
        let mut bd = u32::MAX;
        for (i, p) in pal.iter().enumerate() {
            let d = dist(c, *p);
            if d < bd {
                bd = d;
                best = i as u8;
            }
        }
        best
    };
    let (t, b) = (near(top), near(bot));
    if t == b {
        if t == 0 {
            Cell::default()
        } else {
            Cell { ch: '█', fg: t, bg: 0 }
        }
    } else {
        Cell { ch: '▀', fg: t, bg: b }
    }
}

/// Pick the best cell for a (top, bottom) pixel pair.
fn pair_to_cell(top: [u8; 3], bot: [u8; 3]) -> Cell {
    // full block: one fg covers both pixels
    let avg = [
        ((top[0] as u16 + bot[0] as u16) / 2) as u8,
        ((top[1] as u16 + bot[1] as u16) / 2) as u8,
        ((top[2] as u16 + bot[2] as u16) / 2) as u8,
    ];
    let f = nearest(avg, 16);
    let full = (
        dist(top, PALETTE[f as usize]) + dist(bot, PALETTE[f as usize]),
        Cell { ch: '█', fg: f, bg: 0 },
    );
    // upper half: top in fg (0-15), bottom in bg (0-7)
    let (uf, ub) = (nearest(top, 16), nearest(bot, 8));
    let upper = (
        dist(top, PALETTE[uf as usize]) + dist(bot, PALETTE[ub as usize]),
        Cell { ch: '▀', fg: uf, bg: ub },
    );
    // lower half: bottom in fg, top in bg
    let (lf, lb) = (nearest(bot, 16), nearest(top, 8));
    let lower = (
        dist(bot, PALETTE[lf as usize]) + dist(top, PALETTE[lb as usize]),
        Cell { ch: '▄', fg: lf, bg: lb },
    );
    let best = [full, upper, lower]
        .into_iter()
        .min_by_key(|(e, _)| *e)
        .unwrap()
        .1;
    if best.fg == 0 && best.bg == 0 {
        Cell::default() // pure black -> blank cell
    } else {
        best
    }
}

/// Load a PNG and convert it to a canvas `cols` cells wide (rows follow the
/// image's aspect ratio; half-block pixels are treated as square). With
/// `dither`, a Bayer offset is applied before quantization so smooth
/// gradients become mixed-color patterns instead of hard bands. With `ink`,
/// any sample box where enough source pixels are near-black snaps to black,
/// preserving thin outline contours that box-averaging would wash out.
/// `crop` is an optional (x, y, w, h) source-pixel rectangle to convert
/// instead of the full frame. With `colors256`, quantization targets the
/// full xterm-256 palette (and any color may be a background).
pub fn import_png(
    path: &Path,
    cols: usize,
    dither: bool,
    ink: bool,
    crop: Option<(usize, usize, usize, usize)>,
    colors256: bool,
) -> io::Result<Canvas> {
    let cols = cols.clamp(1, WIDTH);
    let file = std::fs::File::open(path)?;
    let mut dec = png::Decoder::new(file);
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().map_err(err)?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(err)?;
    let (sw, sh) = (info.width as usize, info.height as usize);
    let data = &buf[..info.buffer_size()];

    // normalize to RGB, compositing alpha over black
    let rgb: Vec<[u8; 3]> = match info.color_type {
        png::ColorType::Rgb => data.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
        png::ColorType::Rgba => data
            .chunks_exact(4)
            .map(|c| {
                let a = c[3] as u32;
                [
                    (c[0] as u32 * a / 255) as u8,
                    (c[1] as u32 * a / 255) as u8,
                    (c[2] as u32 * a / 255) as u8,
                ]
            })
            .collect(),
        png::ColorType::Grayscale => data.iter().map(|&g| [g, g, g]).collect(),
        png::ColorType::GrayscaleAlpha => data
            .chunks_exact(2)
            .map(|c| {
                let v = (c[0] as u32 * c[1] as u32 / 255) as u8;
                [v, v, v]
            })
            .collect(),
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported PNG color type {other:?}"),
            ))
        }
    };
    if rgb.len() < sw * sh {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "short PNG data"));
    }

    // source window (crop or full frame); `stride` stays the full row width
    let stride = sw;
    let (ox, oy, sw, sh) = match crop {
        Some((cx, cy, cw, ch)) => {
            let cx = cx.min(sw.saturating_sub(1));
            let cy = cy.min(sh.saturating_sub(1));
            (cx, cy, cw.clamp(1, sw - cx), ch.clamp(1, sh - cy))
        }
        None => (0, 0, sw, sh),
    };

    // target pixel grid: cols wide, aspect-preserving height (max 2 per row)
    let pw = cols;
    let ph = ((sh as f64 / sw as f64) * pw as f64)
        .round()
        .clamp(2.0, (MAX_ROWS * 2) as f64) as usize;

    // box-average sampler; with `ink`, boxes crossed by near-black outline
    // pixels snap to black instead of averaging the line away
    let sample = |px: usize, py: usize| -> [u8; 3] {
        let x0 = px * sw / pw;
        let x1 = (((px + 1) * sw).div_ceil(pw)).min(sw).max(x0 + 1);
        let y0 = py * sh / ph;
        let y1 = (((py + 1) * sh).div_ceil(ph)).min(sh).max(y0 + 1);
        let (mut r, mut g, mut b, mut n, mut dark) = (0u64, 0u64, 0u64, 0u64, 0u64);
        for y in y0..y1 {
            for x in x0..x1 {
                let p = rgb[(oy + y) * stride + (ox + x)];
                if p[0] < 80 && p[1] < 80 && p[2] < 80 {
                    dark += 1;
                }
                r += p[0] as u64;
                g += p[1] as u64;
                b += p[2] as u64;
                n += 1;
            }
        }
        if ink && dark * 4 >= n {
            return [0, 0, 0];
        }
        [(r / n) as u8, (g / n) as u8, (b / n) as u8]
    };

    // dither amplitude matches the palette's step size: coarse for 16 colors,
    // gentle for the fine-grained 256 palette
    let amp = if colors256 { 24.0 } else { 96.0 };
    let shade = |c: [u8; 3], x: usize, y: usize| -> [u8; 3] {
        if !dither {
            return c;
        }
        let t = crate::halfblock::bayer(x, y) - 0.5;
        let adj = |v: u8| (v as f64 + t * amp).clamp(0.0, 255.0) as u8;
        [adj(c[0]), adj(c[1]), adj(c[2])]
    };

    let pal256: [[u8; 3]; 256] = std::array::from_fn(|i| palette256(i as u8));

    let rows = ph.div_ceil(2);
    let mut canvas = Canvas::new(rows);
    for cy in 0..rows {
        for x in 0..pw {
            let top = shade(sample(x, cy * 2), x, cy * 2);
            let bot = if cy * 2 + 1 < ph {
                shade(sample(x, cy * 2 + 1), x, cy * 2 + 1)
            } else {
                [0, 0, 0]
            };
            let cell = if colors256 {
                pair_to_cell_256(top, bot, &pal256)
            } else {
                pair_to_cell(top, bot)
            };
            canvas.set(x, cy, cell);
        }
    }
    Ok(canvas)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 32x32 white PNG with a 2px black vertical line: box-averaged at width
    /// 8 the line washes to gray, but ink mode must keep it black.
    #[test]
    fn ink_mode_preserves_thin_outlines() {
        let (sw, sh) = (32usize, 32usize);
        let mut img = vec![255u8; sw * sh * 3];
        for y in 0..sh {
            for x in 14..16 {
                let o = (y * sw + x) * 3;
                img[o..o + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
        let path = std::env::temp_dir().join("ansidraw_ink_test.png");
        let mut buf = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut buf, sw as u32, sh as u32);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&img).unwrap();
        }
        std::fs::write(&path, buf).unwrap();

        let plain = import_png(&path, 8, false, false, None, false).unwrap();
        let inked = import_png(&path, 8, false, true, None, false).unwrap();
        // the line lands in pixel column 3 (14/32 * 8); cell column 3
        let p = plain.get(3, 1);
        let i = inked.get(3, 1);
        assert!(!p.is_blank() && p.fg != 0, "plain import averages the line to gray");
        // pure black encodes as the blank cell (black background)
        assert!(i.is_blank(), "ink import must snap the line to black");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn pair_quantization() {
        // both VGA yellow -> full block yellow
        let c = pair_to_cell([255, 255, 85], [255, 255, 85]);
        assert_eq!((c.ch, c.fg), ('█', 14));
        // bright top over dark bottom -> upper half, bright fg allowed
        let c = pair_to_cell([255, 255, 255], [0, 0, 170]);
        assert_eq!((c.ch, c.fg, c.bg), ('▀', 15, 1));
        // bright bottom over dark top -> lower half so the bright color survives
        let c = pair_to_cell([0, 0, 0], [85, 255, 85]);
        assert_eq!((c.ch, c.fg, c.bg), ('▄', 10, 0));
        // pure black -> blank
        assert!(pair_to_cell([0, 0, 0], [0, 0, 0]).is_blank());
    }
}
