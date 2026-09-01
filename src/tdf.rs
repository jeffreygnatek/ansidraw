//! TheDraw font (.TDF) support — the classic ANSI-scene format for big
//! colored letterforms ("oldschool BBS fonts"). A file holds one or more
//! fonts; each glyph is a small grid of CP437 chars with DOS attributes.
//!
//! Supported font types: 1 (block) and 2 (color). Outline fonts (0) use a
//! separate border-translation scheme and are skipped.

use std::fs;
use std::io;
use std::path::Path;

use crate::canvas::{Canvas, Cell, MAX_ROWS, WIDTH};
use crate::cp437;

const FILE_MAGIC: &[u8] = b"\x13TheDraw FONTS file\x1a";
const FONT_MAGIC: [u8; 4] = [0x55, 0xAA, 0x00, 0xFF];

#[derive(Clone)]
pub struct Glyph {
    pub width: usize,
    pub height: usize,
    /// rows of (glyph char, DOS attribute)
    pub rows: Vec<Vec<(char, u8)>>,
}

pub struct TdfFont {
    pub name: String,
    pub kind: u8, // 1 block, 2 color
    pub spacing: usize,
    glyphs: Vec<Option<Glyph>>, // indexed by ascii - 33, for '!'..='~'
}

impl TdfFont {
    pub fn glyph(&self, ch: char) -> Option<&Glyph> {
        let lookup = |c: char| -> Option<&Glyph> {
            let i = (c as u32).checked_sub(33)? as usize;
            self.glyphs.get(i)?.as_ref()
        };
        lookup(ch)
            .or_else(|| lookup(ch.to_ascii_uppercase()))
            .or_else(|| lookup(ch.to_ascii_lowercase()))
    }

    /// Average glyph width (used for the space character).
    pub fn space_width(&self) -> usize {
        let ws: Vec<usize> = self.glyphs.iter().flatten().map(|g| g.width).collect();
        if ws.is_empty() {
            4
        } else {
            (ws.iter().sum::<usize>() / ws.len()).max(2)
        }
    }
}

/// Parse every supported font in a .TDF file.
pub fn load(path: &Path) -> io::Result<Vec<TdfFont>> {
    let data = fs::read(path)?;
    if data.len() < 24 || &data[..FILE_MAGIC.len()] != FILE_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a TheDraw FONTS file",
        ));
    }
    let mut fonts = Vec::new();
    let mut pos = FILE_MAGIC.len();
    while pos + 4 <= data.len() {
        if data[pos..pos + 4] != FONT_MAGIC {
            pos += 1; // resync
            continue;
        }
        let p = pos + 4;
        if p + 12 + 1 + 4 + 1 + 1 + 2 + 188 > data.len() {
            break;
        }
        let namelen = (data[p] as usize).min(12);
        let name: String = data[p + 1..p + 1 + namelen]
            .iter()
            .map(|&b| cp437::byte_to_char(b))
            .collect();
        let q = p + 1 + 12 + 4; // skip name field + 4 reserved bytes
        let kind = data[q];
        let spacing = data[q + 1] as usize;
        let blocksize = u16::from_le_bytes([data[q + 2], data[q + 3]]) as usize;
        let offsets_at = q + 4;
        let data_at = offsets_at + 94 * 2;
        let next = data_at + blocksize;
        if data_at > data.len() {
            break;
        }
        if kind == 1 || kind == 2 {
            let mut glyphs: Vec<Option<Glyph>> = vec![None; 94];
            for (gi, glyph) in glyphs.iter_mut().enumerate() {
                let o = offsets_at + gi * 2;
                let off = u16::from_le_bytes([data[o], data[o + 1]]) as usize;
                if off == 0xFFFF {
                    continue;
                }
                *glyph = parse_glyph(&data[data_at..data.len().min(next)], off, kind);
            }
            fonts.push(TdfFont { name, kind, spacing, glyphs });
        }
        pos = next;
    }
    Ok(fonts)
}

fn parse_glyph(block: &[u8], off: usize, kind: u8) -> Option<Glyph> {
    if off + 2 > block.len() {
        return None;
    }
    let width = block[off] as usize;
    let height = block[off + 1] as usize;
    if width == 0 || width > 80 || height == 0 || height > 50 {
        return None;
    }
    let mut rows: Vec<Vec<(char, u8)>> = vec![Vec::new()];
    let mut i = off + 2;
    loop {
        let b = *block.get(i)?;
        match b {
            0x00 => break,
            0x0D => {
                rows.push(Vec::new());
                i += 1;
            }
            _ => {
                let attr = if kind == 2 {
                    let a = *block.get(i + 1)?;
                    i += 2;
                    a
                } else {
                    i += 1;
                    0x0F // block fonts: white on black
                };
                rows.last_mut().unwrap().push((cp437::byte_to_char(b), attr));
            }
        }
    }
    while rows.last().is_some_and(|r| r.is_empty()) {
        rows.pop();
    }
    Some(Glyph { width, height, rows })
}

/// Stamp `text` onto the canvas at cell (x, y). Space cells within glyphs are
/// transparent so banners layer over art. Returns (width, height) used.
pub fn stamp(canvas: &mut Canvas, font: &TdfFont, text: &str, x: usize, y: usize) -> (usize, usize) {
    let mut cx = x;
    let mut max_h = 0;
    for ch in text.chars() {
        if ch == ' ' {
            cx += font.space_width();
            continue;
        }
        let Some(g) = font.glyph(ch) else { continue };
        for (ry, row) in g.rows.iter().enumerate() {
            for (rx, &(gc, attr)) in row.iter().enumerate() {
                let (px, py) = (cx + rx, y + ry);
                if px >= WIDTH || py >= MAX_ROWS {
                    continue;
                }
                let fg = attr & 0x0F;
                let bg = (attr >> 4) & 0x07;
                if gc == ' ' && bg == 0 {
                    continue; // transparent
                }
                canvas.set(px, py, Cell { ch: gc, fg, bg });
            }
        }
        max_h = max_h.max(g.rows.len());
        cx += g.width + font.spacing;
        if cx >= WIDTH {
            break;
        }
    }
    (cx.saturating_sub(x), max_h)
}

/// Width in cells that `text` would occupy, for centering.
pub fn measure(font: &TdfFont, text: &str) -> usize {
    let mut w = 0;
    for ch in text.chars() {
        if ch == ' ' {
            w += font.space_width();
        } else if let Some(g) = font.glyph(ch) {
            w += g.width + font.spacing;
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn font_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fonts")
    }

    #[test]
    fn parses_shipped_fonts() {
        for name in ["graffiti", "cartoon", "neon", "fire"] {
            let p = font_dir().join(format!("{name}.tdf"));
            if !p.exists() {
                continue;
            }
            let fonts = load(&p).unwrap();
            assert!(!fonts.is_empty(), "{name}: no fonts parsed");
            let f = &fonts[0];
            let g = f.glyph('A').expect("glyph A");
            assert!(g.width > 0 && !g.rows.is_empty());
        }
    }

    #[test]
    fn stamps_text() {
        let p = font_dir().join("graffiti.tdf");
        if !p.exists() {
            return;
        }
        let fonts = load(&p).unwrap();
        let mut c = Canvas::new(25);
        let (w, h) = stamp(&mut c, &fonts[0], "AB", 2, 1);
        assert!(w > 4 && h > 2, "banner has real size, got {w}x{h}");
        let non_blank = (0..w + 2)
            .flat_map(|x| (0..h + 1).map(move |y| (x, y)))
            .filter(|&(x, y)| !c.get(x, y + 1).is_blank())
            .count();
        assert!(non_blank > 10, "banner drew cells");
    }
}
