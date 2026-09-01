//! Generates demo.ans — view it with `cat demo.ans` (or open it in ansidraw).

use ansidraw::ansi;
use ansidraw::canvas::{Canvas, Cell};
use ansidraw::charset;

fn main() {
    let mut c = Canvas::new(12);
    let put = |c: &mut Canvas, x: usize, y: usize, s: &str, fg: u8, bg: u8| {
        for (i, ch) in s.chars().enumerate() {
            c.set(x + i, y, Cell { ch, fg, bg });
        }
    };

    // gradient bar in every color
    for fg in 0..16u8 {
        let x = fg as usize * 5;
        put(&mut c, x, 1, "░▒▓█▀", fg, 0);
    }
    put(&mut c, 20, 3, "▄▄▄ ansidraw ▄▄▄", 12, 0);
    put(&mut c, 20, 4, "█ hello, 1994 █", 15, 4);
    put(&mut c, 20, 5, "▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀", 12, 0);

    // one row per charset
    for (i, (name, glyphs)) in charset::SETS.iter().enumerate().take(5) {
        let g: String = glyphs.iter().collect();
        put(&mut c, 4, 7 + i, &format!("{:6} {}", name, g), (i as u8 % 7) + 9, 0);
    }

    let meta = ansidraw::sauce::Sauce {
        title: "ansidraw demo".into(),
        author: "ansidraw".into(),
        group: "example".into(),
        ..Default::default()
    };
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "demo.ans".into());
    ansi::save(&c, path.as_ref() as &std::path::Path, Some(&meta)).unwrap();
    println!("wrote {path}");
}
