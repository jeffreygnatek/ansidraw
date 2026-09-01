//! SAUCE v00 records — "Standard Architecture for Universal Comment
//! Extensions" (Tasmaniac / ACiD, spec shipped with ACiDDraw as SAUCE.DOC).
//!
//! A SAUCE'd file ends with: EOF byte 0x1A, an optional COMNT block
//! ("COMNT" + n*64 bytes), and the 128-byte record itself.

use std::fs;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cp437;

pub const REC_LEN: usize = 128;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sauce {
    pub title: String,  // max 35
    pub author: String, // max 20
    pub group: String,  // max 20
    /// CCYYMMDD as read from the file; regenerated on save.
    pub date: String,
    pub comments: Vec<String>, // 64 chars each
    /// ANSiFlags bit 0: iCE colors.
    pub ice: bool,
}

fn field_to_string(bytes: &[u8]) -> String {
    let s: String = bytes.iter().map(|&b| cp437::byte_to_char(b)).collect();
    s.trim_end().to_string()
}

fn push_field(out: &mut Vec<u8>, s: &str, len: usize) {
    let mut n = 0;
    for ch in s.chars().take(len) {
        out.push(cp437::char_to_byte(ch));
        n += 1;
    }
    out.resize(out.len() + (len - n), b' ');
}

/// Extract the SAUCE record from raw file data, if present.
pub fn parse(data: &[u8]) -> Option<Sauce> {
    if data.len() < REC_LEN {
        return None;
    }
    let rec = &data[data.len() - REC_LEN..];
    if &rec[0..7] != b"SAUCE00" {
        return None;
    }
    let n_comments = rec[104] as usize;
    let mut comments = Vec::new();
    if n_comments > 0 {
        let blk = 5 + 64 * n_comments;
        let end = data.len() - REC_LEN;
        if end >= blk && &data[end - blk..end - blk + 5] == b"COMNT" {
            for i in 0..n_comments {
                let s = end - blk + 5 + i * 64;
                comments.push(field_to_string(&data[s..s + 64]));
            }
        }
    }
    Some(Sauce {
        title: field_to_string(&rec[7..42]),
        author: field_to_string(&rec[42..62]),
        group: field_to_string(&rec[62..82]),
        date: field_to_string(&rec[82..90]),
        comments,
        ice: rec[105] & 1 != 0,
    })
}

/// Read just the SAUCE record of a file on disk.
pub fn read(path: &Path) -> io::Result<Option<Sauce>> {
    Ok(parse(&fs::read(path)?))
}

/// Append EOF byte, comment block, and SAUCE record to a serialized image.
/// `width`/`lines` are the character dimensions (TInfo1/TInfo2).
pub fn append(out: &mut Vec<u8>, s: &Sauce, width: u16, lines: u16) {
    let filesize = out.len() as u32;
    out.push(0x1A);
    let comments: Vec<&String> = s.comments.iter().take(255).collect();
    if !comments.is_empty() {
        out.extend(b"COMNT");
        for c in &comments {
            push_field(out, c, 64);
        }
    }
    out.extend(b"SAUCE00");
    push_field(out, &s.title, 35);
    push_field(out, &s.author, 20);
    push_field(out, &s.group, 20);
    push_field(out, &today_yyyymmdd(), 8);
    out.extend(filesize.to_le_bytes());
    out.push(1); // DataType: Character
    out.push(1); // FileType: ANSI
    out.extend(width.to_le_bytes());
    out.extend(lines.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out.push(comments.len() as u8);
    out.push(if s.ice { 1 } else { 0 });
    out.extend([0u8; 22]);
}

/// Current UTC date as CCYYMMDD (civil-from-days, no chrono needed).
fn today_yyyymmdd() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let z = (secs / 86400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}{:02}{:02}", y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut data = b"art bytes here".to_vec();
        let s = Sauce {
            title: "Test Piece".into(),
            author: "Jeff".into(),
            group: "ansidraw".into(),
            date: String::new(),
            comments: vec!["first comment".into(), "second".into()],
            ice: true,
        };
        append(&mut data, &s, 80, 25);
        let p = parse(&data).expect("sauce should parse");
        assert_eq!(p.title, "Test Piece");
        assert_eq!(p.author, "Jeff");
        assert_eq!(p.group, "ansidraw");
        assert_eq!(p.comments, vec!["first comment".to_string(), "second".to_string()]);
        assert!(p.ice);
        assert_eq!(p.date.len(), 8);
        // record is fixed-size at the tail
        assert_eq!(
            data.len(),
            "art bytes here".len() + 1 + 5 + 64 * 2 + REC_LEN
        );
    }

    #[test]
    fn absent() {
        assert!(parse(b"no sauce here").is_none());
        assert!(parse(&[0u8; 200]).is_none());
    }
}
