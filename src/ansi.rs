//! Save/load of .ANS files: CP437 bytes plus ANSI/VT100 SGR and cursor
//! escape sequences, the format BBS ANSI art has always used.

use std::fs;
use std::io;
use std::path::Path;

use crate::canvas::{Canvas, Cell, MAX_ROWS, WIDTH};
use crate::cp437;
use crate::sauce::{self, Sauce};

pub fn save(canvas: &Canvas, path: &Path, meta: Option<&Sauce>) -> io::Result<()> {
    save_with(canvas, path, meta, false)
}

/// With `utf8`, glyphs are written as UTF-8 instead of CP437 bytes — the
/// same escape codes, but modern terminals (`cat`) render it directly.
/// Classic CP437 is the authentic format scene viewers expect.
pub fn save_with(canvas: &Canvas, path: &Path, meta: Option<&Sauce>, utf8: bool) -> io::Result<()> {
    let mut out: Vec<u8> = Vec::new();
    out.extend(b"\x1b[0m");
    let last = canvas.last_used_row();
    for y in 0..=last {
        let end = canvas
            .line_bounds(y)
            .map(|(_, right)| right + 1)
            .unwrap_or(0);
        let mut cur: Option<(u8, u8)> = None;
        for x in 0..end {
            let c = canvas.get(x, y);
            if cur != Some((c.fg, c.bg)) {
                if c.fg < 16 && c.bg < 8 {
                    // classic 16-color SGR
                    let mut sgr = String::from("0");
                    if c.fg >= 8 {
                        sgr.push_str(";1");
                    }
                    sgr.push_str(&format!(";{};{}", 30 + c.fg % 8, 40 + c.bg));
                    out.extend(format!("\x1b[{}m", sgr).bytes());
                } else {
                    // xterm-256 SGR (38;5 / 48;5)
                    out.extend(
                        format!("\x1b[0;38;5;{};48;5;{}m", c.fg, c.bg).bytes(),
                    );
                }
                cur = Some((c.fg, c.bg));
            }
            if utf8 {
                let mut buf = [0u8; 4];
                out.extend(c.ch.encode_utf8(&mut buf).as_bytes());
            } else {
                out.push(cp437::char_to_byte(c.ch));
            }
        }
        out.extend(b"\x1b[0m\r\n");
    }
    if let Some(s) = meta {
        sauce::append(&mut out, s, WIDTH as u16, (last + 1) as u16);
    }
    fs::write(path, out)
}

pub fn load(path: &Path) -> io::Result<Canvas> {
    let data = fs::read(path)?;
    let mut canvas = Canvas::new(25);
    let (mut x, mut y) = (0usize, 0usize);
    let (mut saved_x, mut saved_y) = (0usize, 0usize);
    let mut fg: u8 = 7;
    let mut bg: u8 = 0;
    let mut bold = false;

    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        match b {
            0x1A => break, // SUB: EOF marker, SAUCE record follows
            0x0D => {
                x = 0;
                i += 1;
            }
            0x0A => {
                x = 0;
                y = (y + 1).min(MAX_ROWS - 1);
                i += 1;
            }
            0x1B if i + 1 < data.len() && data[i + 1] == b'[' => {
                let mut j = i + 2;
                let mut params: Vec<u16> = Vec::new();
                let mut num = String::new();
                let mut done = false;
                while j < data.len() && !done {
                    let c = data[j] as char;
                    if c.is_ascii_digit() {
                        num.push(c);
                    } else if c == ';' {
                        params.push(num.parse().unwrap_or(0));
                        num.clear();
                    } else if c == '?' || c == '=' {
                        // private-mode prefix; ignore
                    } else {
                        if !num.is_empty() {
                            params.push(num.parse().unwrap_or(0));
                            num.clear();
                        }
                        match c {
                            'm' => {
                                if params.is_empty() {
                                    params.push(0);
                                }
                                let mut k = 0;
                                while k < params.len() {
                                    match params[k] {
                                        0 => {
                                            fg = 7;
                                            bg = 0;
                                            bold = false;
                                        }
                                        1 => bold = true,
                                        22 => bold = false,
                                        5 | 25 => {} // blink: ignored
                                        30..=37 => fg = (params[k] - 30) as u8,
                                        39 => fg = 7,
                                        40..=47 => bg = (params[k] - 40) as u8,
                                        49 => bg = 0,
                                        // extended color: 38/48;5;N (256) or ;2;r;g;b (skip)
                                        p @ (38 | 48) => match params.get(k + 1) {
                                            Some(5) => {
                                                if let Some(&n) = params.get(k + 2) {
                                                    if p == 38 {
                                                        fg = n as u8;
                                                        bold = false;
                                                    } else {
                                                        bg = n as u8;
                                                    }
                                                }
                                                k += 2;
                                            }
                                            Some(2) => k += 4,
                                            _ => {}
                                        },
                                        _ => {}
                                    }
                                    k += 1;
                                }
                            }
                            'C' => {
                                let n = params.first().copied().unwrap_or(1).max(1) as usize;
                                x = (x + n).min(WIDTH - 1);
                            }
                            'D' => {
                                let n = params.first().copied().unwrap_or(1).max(1) as usize;
                                x = x.saturating_sub(n);
                            }
                            'A' => {
                                let n = params.first().copied().unwrap_or(1).max(1) as usize;
                                y = y.saturating_sub(n);
                            }
                            'B' => {
                                let n = params.first().copied().unwrap_or(1).max(1) as usize;
                                y = (y + n).min(MAX_ROWS - 1);
                            }
                            'H' | 'f' => {
                                let row = params.first().copied().unwrap_or(1).max(1) as usize;
                                let col = params.get(1).copied().unwrap_or(1).max(1) as usize;
                                y = (row - 1).min(MAX_ROWS - 1);
                                x = (col - 1).min(WIDTH - 1);
                            }
                            's' => {
                                saved_x = x;
                                saved_y = y;
                            }
                            'u' => {
                                x = saved_x;
                                y = saved_y;
                            }
                            'J' | 'K' | 'h' | 'l' | 't' => {} // clears/modes: ignored
                            _ => {}
                        }
                        done = true;
                    }
                    j += 1;
                }
                i = j;
            }
            0x09 => {
                x = ((x / 8) + 1) * 8;
                if x >= WIDTH {
                    x = WIDTH - 1;
                }
                i += 1;
            }
            _ => {
                let ch = cp437::byte_to_char(b);
                // bold-as-bright applies only to the classic 0-7 range
                let cell_fg = if fg < 8 && bold { fg + 8 } else { fg };
                canvas.set(x, y, Cell { ch, fg: cell_fg, bg });
                x += 1;
                if x >= WIDTH {
                    x = 0;
                    y = (y + 1).min(MAX_ROWS - 1);
                }
                i += 1;
            }
        }
    }
    Ok(canvas)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(name)
    }

    #[test]
    fn roundtrip() {
        let mut c = Canvas::new(5);
        c.set(0, 0, Cell { ch: '░', fg: 9, bg: 1 });
        c.set(1, 0, Cell { ch: '█', fg: 14, bg: 4 });
        c.set(5, 2, Cell { ch: 'A', fg: 7, bg: 0 });
        c.set(79, 3, Cell { ch: '▓', fg: 3, bg: 2 });
        let p = tmp("ansidraw_roundtrip.ans");
        save(&c, &p, None).unwrap();
        let l = load(&p).unwrap();
        for (x, y) in [(0, 0), (1, 0), (5, 2), (79, 3), (10, 1)] {
            assert_eq!(c.get(x, y), l.get(x, y), "cell {x},{y}");
        }
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn parses_classic_sequences() {
        // colored text, cursor-forward, positioning, and a SAUCE-style EOF
        let data = b"\x1b[0;1;31;44mAB\x1b[3CX\r\n\x1b[0mplain\x1b[2;1Hover\x1a\x00junk";
        let p = tmp("ansidraw_classic.ans");
        std::fs::write(&p, data).unwrap();
        let c = load(&p).unwrap();
        assert_eq!(c.get(0, 0), Cell { ch: 'A', fg: 9, bg: 4 });
        assert_eq!(c.get(1, 0), Cell { ch: 'B', fg: 9, bg: 4 });
        // 3 cells skipped by ESC[3C stay blank
        assert!(c.get(2, 0).is_blank());
        assert_eq!(c.get(5, 0).ch, 'X');
        // ESC[2;1H moved over the start of line 2 before "over" was written
        assert_eq!(c.get(0, 1), Cell { ch: 'o', fg: 7, bg: 0 });
        assert_eq!(c.get(4, 1).ch, 'n'); // "over" overwrote "plai", 'n' remains
        // nothing after the SUB byte is read
        assert!(c.get(0, 2).is_blank());
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn sauce_attached_and_ignored_by_loader() {
        let mut c = Canvas::new(3);
        c.set(0, 0, Cell { ch: '█', fg: 12, bg: 0 });
        c.set(0, 2, Cell { ch: 'x', fg: 7, bg: 0 });
        let meta = Sauce {
            title: "With Sauce".into(),
            author: "Jeff".into(),
            group: "grp".into(),
            ..Default::default()
        };
        let p = tmp("ansidraw_sauced.ans");
        save(&c, &p, Some(&meta)).unwrap();
        // loader stops at the EOF byte: canvas identical to a sauceless save
        let l = load(&p).unwrap();
        assert_eq!(l.get(0, 0), c.get(0, 0));
        assert_eq!(l.last_used_row(), 2);
        // and the record reads back
        let s = sauce::read(&p).unwrap().expect("sauce present");
        assert_eq!(s.title, "With Sauce");
        assert_eq!(s.author, "Jeff");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn roundtrip_256_colors() {
        let mut c = Canvas::new(2);
        c.set(0, 0, Cell { ch: '▀', fg: 208, bg: 52 }); // orange over dark red
        c.set(1, 0, Cell { ch: '█', fg: 231, bg: 0 }); // cube white
        c.set(2, 0, Cell { ch: 'x', fg: 12, bg: 4 }); // classic stays classic
        let p = tmp("ansidraw_256.ans");
        save(&c, &p, None).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("38;5;208"), "256-color SGR emitted");
        assert!(s.contains("[0;1;34;44m"), "classic cells keep classic SGR");
        let l = load(&p).unwrap();
        assert_eq!(l.get(0, 0), c.get(0, 0));
        assert_eq!(l.get(1, 0), c.get(1, 0));
        assert_eq!(l.get(2, 0), c.get(2, 0));
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn cp437_roundtrip_all_bytes() {
        for b in 1..=255u8 {
            let ch = crate::cp437::byte_to_char(b);
            assert_eq!(crate::cp437::char_to_byte(ch), b, "byte {b:#04x}");
        }
    }
}
