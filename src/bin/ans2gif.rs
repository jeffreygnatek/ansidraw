//! Render a .ANS file as a looping "BBS download" reveal GIF.
//! Usage: ans2gif <in.ans> [out.gif] [scale 1-4] [cells-per-frame]

use std::path::PathBuf;
use std::process::exit;

use ansidraw::{animate, ansi};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(inp) = args.next() else {
        eprintln!("usage: ans2gif <in.ans> [out.gif] [scale 1-4] [cells-per-frame]");
        exit(2);
    };
    let inp = PathBuf::from(inp);
    let outp = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| inp.with_extension("gif"));
    let scale: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let speed: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(80);

    let canvas = match ansi::load(&inp) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error loading {}: {e}", inp.display());
            exit(1);
        }
    };
    let frames = animate::reveal_frames(&canvas, speed);
    match animate::write_gif(&frames, &outp, scale, 4, 200) {
        Ok((w, h, n)) => println!("wrote {} ({w}x{h}, {n} frames)", outp.display()),
        Err(e) => {
            eprintln!("error writing {}: {e}", outp.display());
            exit(1);
        }
    }
}
