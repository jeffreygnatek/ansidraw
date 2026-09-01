//! Load an .ANS, re-save it, reload, and verify the canvas is identical.
//! Usage: cargo run --example recode -- in.ans out.ans

use ansidraw::canvas::WIDTH;
use ansidraw::{ansi, sauce};
use std::path::Path;

fn main() {
    let mut args = std::env::args().skip(1);
    let inp = args.next().expect("usage: recode <in.ans> <out.ans>");
    let outp = args.next().expect("usage: recode <in.ans> <out.ans>");

    let meta = sauce::read(Path::new(&inp)).unwrap();
    match &meta {
        Some(s) => println!(
            "SAUCE: \"{}\" by {} / {} ({})",
            s.title, s.author, s.group, s.date
        ),
        None => println!("SAUCE: none"),
    }

    let a = ansi::load(Path::new(&inp)).unwrap();
    ansi::save(&a, Path::new(&outp), meta.as_ref()).unwrap();
    let b = ansi::load(Path::new(&outp)).unwrap();

    let rows = a.last_used_row().max(b.last_used_row()) + 1;
    let mut diffs = 0;
    for y in 0..rows {
        for x in 0..WIDTH {
            if a.get(x, y) != b.get(x, y) {
                if diffs < 5 {
                    println!("diff at {x},{y}: {:?} vs {:?}", a.get(x, y), b.get(x, y));
                }
                diffs += 1;
            }
        }
    }
    println!("{} rows used, {} cell diffs -> {}", rows, diffs, if diffs == 0 { "PASS" } else { "FAIL" });
    std::process::exit(if diffs == 0 { 0 } else { 1 });
}
