//! Convert a .ANS file to a PNG image.
//! Usage: ans2png <in.ans> [out.png] [scale 1-8, default 4]

use std::path::PathBuf;
use std::process::exit;

use ansidraw::{ansi, render_png};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(inp) = args.next() else {
        eprintln!("usage: ans2png <in.ans> [out.png] [scale 1-8, default 4]");
        exit(2);
    };
    let inp = PathBuf::from(inp);
    let outp = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| inp.with_extension("png"));
    let scale: usize = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);

    let canvas = match ansi::load(&inp) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error loading {}: {e}", inp.display());
            exit(1);
        }
    };
    match render_png::save_png(&canvas, &outp, scale) {
        Ok((w, h)) => println!("wrote {} ({w}x{h})", outp.display()),
        Err(e) => {
            eprintln!("error writing {}: {e}", outp.display());
            exit(1);
        }
    }
}
