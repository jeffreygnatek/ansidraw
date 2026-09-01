//! MCP server exposing ansidraw's canvas as drawing tools, so an AI agent
//! can compose ANSI art and save real .ANS files (with SAUCE credits).
//!
//! Speaks MCP's stdio transport: newline-delimited JSON-RPC 2.0.
//! Register with e.g.:  claude mcp add ansidraw -- /path/to/ansidraw-mcp

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use serde_json::{json, Value};

use ansidraw::canvas::{Canvas, Cell, MAX_ROWS, WIDTH};
use ansidraw::sauce::Sauce;
use ansidraw::{ansi, halfblock, import_image, render_png, sauce, tdf};

struct State {
    canvas: Canvas,
    sauce: Sauce,
}

fn main() {
    let stdin = io::stdin();
    let mut state = State {
        canvas: Canvas::new(25),
        sauce: Sauce::default(),
    };

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let id = msg.get("id").cloned().filter(|v| !v.is_null());
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));

        let result: Option<Result<Value, (i64, String)>> = match method {
            "initialize" => {
                let ver = params
                    .get("protocolVersion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("2024-11-05");
                Some(Ok(json!({
                    "protocolVersion": ver,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "ansidraw", "version": env!("CARGO_PKG_VERSION") }
                })))
            }
            "ping" => Some(Ok(json!({}))),
            "tools/list" => Some(Ok(json!({ "tools": tool_defs() }))),
            "tools/call" => Some(Ok(tool_call(&mut state, &params))),
            m if m.starts_with("notifications/") => None,
            _ => id
                .as_ref()
                .map(|_| Err((-32601, format!("method not found: {method}")))),
        };

        if let (Some(res), Some(id)) = (result, id) {
            let resp = match res {
                Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
                Err((code, message)) => json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": { "code": code, "message": message }
                }),
            };
            println!("{resp}");
            io::stdout().flush().ok();
        }
    }
}

// ------------------------------------------------------------------- tools

fn tool_defs() -> Value {
    let color_desc = "0-15 or a name (black, blue, green, cyan, red, magenta, \
                      brown, gray, darkgray, lightblue, lightgreen, lightcyan, \
                      lightred, lightmagenta, yellow, white)";
    json!([
        {
            "name": "new_canvas",
            "description": "Start a fresh 80-column canvas, discarding the current one.",
            "inputSchema": { "type": "object", "properties": {
                "rows": { "type": "integer", "description": "Initial rows (default 25, max 1000)" }
            }}
        },
        {
            "name": "put_text",
            "description": "Write text onto the canvas at column x, row y (0-based). \
                Text may be multi-line (\\n starts the next row at the same x). Use CP437 \
                glyphs for art: ░▒▓█ ▀▄▌▐ ─│┌┐└┘├┤┬┴┼ ═║╔╗╚╝╠╣╦╩╬ ■·∙○ etc. Rows extend \
                automatically; anything past column 79 is clipped. Later calls overdraw \
                earlier ones, so build art in layers.",
            "inputSchema": { "type": "object", "properties": {
                "x": { "type": "integer" },
                "y": { "type": "integer" },
                "text": { "type": "string" },
                "fg": { "description": format!("Foreground color, {color_desc} (default gray)") },
                "bg": { "description": "Background color 0-7 or name (default black)" }
            }, "required": ["x", "y", "text"] }
        },
        {
            "name": "fill_rect",
            "description": "Fill a rectangle with one character and color.",
            "inputSchema": { "type": "object", "properties": {
                "x": { "type": "integer" }, "y": { "type": "integer" },
                "w": { "type": "integer" }, "h": { "type": "integer" },
                "ch": { "type": "string", "description": "Single character (default space)" },
                "fg": { "description": format!("Foreground color, {color_desc}") },
                "bg": { "description": "Background color 0-7 or name" }
            }, "required": ["x", "y", "w", "h"] }
        },
        {
            "name": "draw_box",
            "description": "Draw a rectangular frame (border only).",
            "inputSchema": { "type": "object", "properties": {
                "x": { "type": "integer" }, "y": { "type": "integer" },
                "w": { "type": "integer", "description": "Width >= 2" },
                "h": { "type": "integer", "description": "Height >= 2" },
                "style": { "type": "string", "enum": ["single", "double", "block"], "description": "Line style (default single)" },
                "fg": { "description": format!("Foreground color, {color_desc}") },
                "bg": { "description": "Background color 0-7 or name" }
            }, "required": ["x", "y", "w", "h"] }
        },
        {
            "name": "banner",
            "description": "Stamp graffiti-style scene lettering onto the canvas using a \
                TheDraw font (.tdf) — the classic ANSI-art logo format. Glyphs carry their \
                own colors and layer transparently over existing art. Fonts ship in the \
                ansidraw fonts/ directory (graffiti, cartoon, neon, fire, block3dx, dream, \
                metalix, iceblock). Most fonts are CAPS-only (lowercase falls back). Use \
                list_fonts to see what a file contains and how wide text will be.",
            "inputSchema": { "type": "object", "properties": {
                "font": { "type": "string", "description": "Path to a .tdf file, or a bare name like \"graffiti\" resolved from the fonts/ dir" },
                "text": { "type": "string" },
                "x": { "type": "integer", "description": "Left cell column (omit with center=true)" },
                "y": { "type": "integer", "description": "Top cell row" },
                "center": { "type": "boolean", "description": "Center horizontally on the 80-col canvas" },
                "index": { "type": "integer", "description": "Which font within the file (default 0)" }
            }, "required": ["font", "text", "y"] }
        },
        {
            "name": "list_fonts",
            "description": "List the fonts inside a .tdf file (name, type, letter spacing) and \
                measure how many columns a given text would occupy in each.",
            "inputSchema": { "type": "object", "properties": {
                "font": { "type": "string", "description": "Path to a .tdf file or bare name from fonts/" },
                "text": { "type": "string", "description": "Optional text to measure" }
            }, "required": ["font"] }
        },
        {
            "name": "render",
            "description": "Return the current canvas as text so you can inspect what has \
                been drawn. Plain glyphs by default; set colors=true for a per-cell color \
                listing appended after the art.",
            "inputSchema": { "type": "object", "properties": {
                "colors": { "type": "boolean", "description": "Also list color runs per row" }
            }}
        },
        {
            "name": "pixel_set",
            "description": "Plot individual pixels in PIXEL SPACE: the canvas doubles as an \
                80-wide grid of square half-block pixels, TWO per text row (pixel y = 2x the \
                text row; up to 2000 tall). The server packs pixels into ▀▄█ cells \
                automatically and preserves the other half of each cell, so plot in any \
                order. Prefer pixel tools for image-style art; use put_text for lettering \
                and ░▒▓ textures. NOTE: backgrounds can only be dark colors 0-7, so when two \
                different bright colors (8-15) stack in one cell the lower one dims — align \
                bright color boundaries to even pixel rows.",
            "inputSchema": { "type": "object", "properties": {
                "pixels": { "type": "array", "description": "List of [x, y, color] triples; color 0-15 or a name",
                    "items": { "type": "array" } }
            }, "required": ["pixels"] }
        },
        {
            "name": "pixel_rect",
            "description": "Fill a rectangle in half-block PIXEL SPACE (see pixel_set: 80 wide, 2 pixels per text row).",
            "inputSchema": { "type": "object", "properties": {
                "x": { "type": "integer" }, "y": { "type": "integer", "description": "In pixels (2 per text row)" },
                "w": { "type": "integer" }, "h": { "type": "integer" },
                "color": { "description": "0-15 or a color name" },
                "color2": { "description": "Optional second color: fill becomes an ordered-dither mix — fake gradient steps by varying mix across several rects" },
                "mix": { "type": "number", "description": "Fraction of color2, 0.0-1.0 (default 0.5)" }
            }, "required": ["x", "y", "w", "h", "color"] }
        },
        {
            "name": "pixel_line",
            "description": "Draw a 1px line in half-block PIXEL SPACE (see pixel_set).",
            "inputSchema": { "type": "object", "properties": {
                "x0": { "type": "integer" }, "y0": { "type": "integer" },
                "x1": { "type": "integer" }, "y1": { "type": "integer" },
                "color": { "description": "0-15 or a color name" }
            }, "required": ["x0", "y0", "x1", "y1", "color"] }
        },
        {
            "name": "pixel_ellipse",
            "description": "Draw an ellipse in half-block PIXEL SPACE (see pixel_set) — filled \
                by default, or a ~1px outline with fill=false. Layer offset filled ellipses \
                (dark base, lighter inner, white speck) for cartoon-style shading.",
            "inputSchema": { "type": "object", "properties": {
                "cx": { "type": "integer" }, "cy": { "type": "integer" },
                "rx": { "type": "integer" }, "ry": { "type": "integer" },
                "color": { "description": "0-15 or a color name" },
                "fill": { "type": "boolean", "description": "true = filled (default), false = outline" },
                "clip_y_min": { "type": "integer", "description": "Only draw pixel rows >= this (for arcs/occlusion)" },
                "clip_y_max": { "type": "integer", "description": "Only draw pixel rows <= this" }
            }, "required": ["cx", "cy", "rx", "ry", "color"] }
        },
        {
            "name": "import_image",
            "description": "Convert a PNG image file into the canvas (replaces it) using \
                half-block ▀▄█ pixels quantized to the 16-color VGA palette. Each text row \
                holds two image rows, and height follows the image's aspect ratio. After \
                importing, inspect with render_png and touch up with the drawing tools.",
            "inputSchema": { "type": "object", "properties": {
                "path": { "type": "string", "description": "Path to a .png file" },
                "width": { "type": "integer", "description": "Target width in columns, 1-80 (default 80)" },
                "dither": { "type": "boolean", "description": "Bayer-dither smooth gradients instead of hard color bands (default false; use for photos/gradients, NOT flat cel art)" },
                "ink": { "type": "boolean", "description": "Preserve thin black outlines: sample boxes crossed by near-black contour pixels snap to black instead of averaging to gray (default false; use for line art/cartoons)" },
                "crop_x": { "type": "integer", "description": "Optional source crop rectangle (pixels): left edge" },
                "crop_y": { "type": "integer", "description": "Crop top edge" },
                "crop_w": { "type": "integer", "description": "Crop width" },
                "crop_h": { "type": "integer", "description": "Crop height" },
                "colors": { "type": "integer", "enum": [16, 256], "description": "Palette: 16 = classic VGA (default, works in every viewer), 256 = xterm-256 (real skin tones and smooth ramps; needs a modern terminal, saves with 38;5/48;5 codes)" }
            }, "required": ["path"] }
        },
        {
            "name": "render_png",
            "description": "Render the canvas to a PNG image (classic VGA font + palette) \
                and return it so you can SEE the art with real colors — use this to judge \
                composition. Optionally also writes the PNG to a file.",
            "inputSchema": { "type": "object", "properties": {
                "path": { "type": "string", "description": "Optional .png file path to also save to" },
                "scale": { "type": "integer", "description": "Pixel scale 1-8 (default 2; use 4+ for high-res export)" }
            }}
        },
        {
            "name": "save",
            "description": "Save the canvas as a real .ANS file (CP437 + ANSI escapes). \
                Optionally set SAUCE credits; if any of title/author/group is given (or was \
                loaded), a SAUCE record is attached.",
            "inputSchema": { "type": "object", "properties": {
                "path": { "type": "string", "description": ".ans file path" },
                "title": { "type": "string", "description": "SAUCE title (max 35 chars)" },
                "author": { "type": "string", "description": "SAUCE author (max 20 chars)" },
                "group": { "type": "string", "description": "SAUCE group (max 20 chars)" },
                "utf8": { "type": "boolean", "description": "Write glyphs as UTF-8 instead of CP437 bytes — renders directly with cat in modern terminals (default false = authentic CP437 for scene viewers)" }
            }, "required": ["path"] }
        },
        {
            "name": "load",
            "description": "Load an existing .ANS file into the canvas (replaces it). \
                Reads SAUCE credits too, if present.",
            "inputSchema": { "type": "object", "properties": {
                "path": { "type": "string" }
            }, "required": ["path"] }
        }
    ])
}

fn ok_text(s: String) -> Value {
    json!({ "content": [{ "type": "text", "text": s }] })
}

fn err_text(s: String) -> Value {
    json!({ "content": [{ "type": "text", "text": s }], "isError": true })
}

fn tool_call(state: &mut State, params: &Value) -> Value {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    match name {
        "new_canvas" => {
            let rows = get_usize(&args, "rows").unwrap_or(25).clamp(1, MAX_ROWS);
            state.canvas = Canvas::new(rows);
            state.sauce = Sauce::default();
            ok_text(format!("New {WIDTH}x{rows} canvas."))
        }
        "put_text" => match put_text(state, &args) {
            Ok(s) => ok_text(s),
            Err(e) => err_text(e),
        },
        "fill_rect" => match fill_rect(state, &args) {
            Ok(s) => ok_text(s),
            Err(e) => err_text(e),
        },
        "draw_box" => match draw_box(state, &args) {
            Ok(s) => ok_text(s),
            Err(e) => err_text(e),
        },
        "banner" => {
            let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let y = get_usize(&args, "y").unwrap_or(0);
            let idx = get_usize(&args, "index").unwrap_or(0);
            let path = resolve_font(args.get("font").and_then(|v| v.as_str()).unwrap_or(""));
            match tdf::load(&path) {
                Ok(fonts) if fonts.is_empty() => err_text("no usable fonts in file".into()),
                Ok(fonts) => {
                    let f = &fonts[idx.min(fonts.len() - 1)];
                    let w = tdf::measure(f, text);
                    let x = if args.get("center").and_then(|v| v.as_bool()).unwrap_or(false) {
                        WIDTH.saturating_sub(w) / 2
                    } else {
                        get_usize(&args, "x").unwrap_or(0)
                    };
                    let (used_w, used_h) = tdf::stamp(&mut state.canvas, f, text, x, y);
                    ok_text(format!(
                        "Stamped \"{text}\" in {} at {x},{y} ({used_w}x{used_h} cells).",
                        f.name
                    ))
                }
                Err(e) => err_text(format!("font load failed ({}): {e}", path.display())),
            }
        }
        "list_fonts" => {
            let path = resolve_font(args.get("font").and_then(|v| v.as_str()).unwrap_or(""));
            let text = args.get("text").and_then(|v| v.as_str());
            match tdf::load(&path) {
                Ok(fonts) if fonts.is_empty() => err_text("no usable fonts in file".into()),
                Ok(fonts) => {
                    let mut out = String::new();
                    for (i, f) in fonts.iter().enumerate() {
                        out.push_str(&format!(
                            "{i}: {} (type {}, spacing {})",
                            f.name, f.kind, f.spacing
                        ));
                        if let Some(t) = text {
                            out.push_str(&format!(" — \"{t}\" = {} cols", tdf::measure(f, t)));
                        }
                        out.push('\n');
                    }
                    ok_text(out)
                }
                Err(e) => err_text(format!("font load failed ({}): {e}", path.display())),
            }
        }
        "render" => {
            let colors = args.get("colors").and_then(|v| v.as_bool()).unwrap_or(false);
            ok_text(render(&state.canvas, colors))
        }
        "pixel_set" => {
            let Some(arr) = args.get("pixels").and_then(|v| v.as_array()) else {
                return err_text("pixels is required (array of [x, y, color])".into());
            };
            let mut n = 0;
            for p in arr {
                let Some(t) = p.as_array() else { continue };
                let (Some(x), Some(y)) = (
                    t.first().and_then(|v| v.as_u64()),
                    t.get(1).and_then(|v| v.as_u64()),
                ) else {
                    continue;
                };
                match parse_color(t.get(2), 16) {
                    Ok(Some(c)) => {
                        halfblock::set_pixel(&mut state.canvas, x as usize, y as usize, c);
                        n += 1;
                    }
                    Ok(None) => {}
                    Err(e) => return err_text(e),
                }
            }
            ok_text(format!("Set {n} pixels."))
        }
        "pixel_rect" => {
            let (x, y, w, h) = (
                get_usize(&args, "x").unwrap_or(0),
                get_usize(&args, "y").unwrap_or(0),
                get_usize(&args, "w").unwrap_or(0),
                get_usize(&args, "h").unwrap_or(0),
            );
            match (parse_color(args.get("color"), 16), parse_color(args.get("color2"), 16)) {
                (Ok(Some(c)), Ok(None)) => {
                    halfblock::fill_rect(&mut state.canvas, x, y, w, h, c);
                    ok_text(format!("Filled {w}x{h} pixels at {x},{y}."))
                }
                (Ok(Some(c)), Ok(Some(c2))) => {
                    let mix = args.get("mix").and_then(|v| v.as_f64()).unwrap_or(0.5);
                    halfblock::dither_rect(&mut state.canvas, x, y, w, h, c, c2, mix.clamp(0.0, 1.0));
                    ok_text(format!("Dither-filled {w}x{h} at {x},{y} (mix {mix:.2})."))
                }
                (Ok(None), _) => err_text("color is required".into()),
                (Err(e), _) | (_, Err(e)) => err_text(e),
            }
        }
        "pixel_line" => {
            let g = |k: &str| args.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
            match parse_color(args.get("color"), 16) {
                Ok(Some(c)) => {
                    halfblock::line(&mut state.canvas, g("x0"), g("y0"), g("x1"), g("y1"), c);
                    ok_text("Line drawn.".into())
                }
                Ok(None) => err_text("color is required".into()),
                Err(e) => err_text(e),
            }
        }
        "pixel_ellipse" => {
            let g = |k: &str| args.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
            let fill = args.get("fill").and_then(|v| v.as_bool()).unwrap_or(true);
            let clip = match (
                args.get("clip_y_min").and_then(|v| v.as_i64()),
                args.get("clip_y_max").and_then(|v| v.as_i64()),
            ) {
                (None, None) => None,
                (lo, hi) => Some((lo.unwrap_or(0), hi.unwrap_or(i64::MAX))),
            };
            match parse_color(args.get("color"), 16) {
                Ok(Some(c)) => {
                    halfblock::ellipse(
                        &mut state.canvas,
                        g("cx"),
                        g("cy"),
                        g("rx"),
                        g("ry"),
                        c,
                        fill,
                        clip,
                    );
                    ok_text(format!(
                        "{} ellipse at {},{} r {}x{}.",
                        if fill { "Filled" } else { "Outlined" },
                        g("cx"),
                        g("cy"),
                        g("rx"),
                        g("ry")
                    ))
                }
                Ok(None) => err_text("color is required".into()),
                Err(e) => err_text(e),
            }
        }
        "import_image" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let width = get_usize(&args, "width").unwrap_or(WIDTH);
            let dither = args.get("dither").and_then(|v| v.as_bool()).unwrap_or(false);
            let ink = args.get("ink").and_then(|v| v.as_bool()).unwrap_or(false);
            let crop = match (
                get_usize(&args, "crop_x"),
                get_usize(&args, "crop_y"),
                get_usize(&args, "crop_w"),
                get_usize(&args, "crop_h"),
            ) {
                (None, None, None, None) => None,
                (x, y, w, h) => Some((
                    x.unwrap_or(0),
                    y.unwrap_or(0),
                    w.unwrap_or(usize::MAX),
                    h.unwrap_or(usize::MAX),
                )),
            };
            let colors256 = get_usize(&args, "colors").unwrap_or(16) == 256;
            match import_image::import_png(std::path::Path::new(path), width, dither, ink, crop, colors256) {
                Ok(c) => {
                    let rows = c.last_used_row() + 1;
                    state.canvas = c;
                    state.sauce = Sauce::default();
                    ok_text(format!(
                        "Imported {path} as {width}x{rows} cells. Use render_png to see it."
                    ))
                }
                Err(e) => err_text(format!("import failed: {e}")),
            }
        }
        "render_png" => match render_png_tool(state, &args) {
            Ok(v) => v,
            Err(e) => err_text(e),
        },
        "save" => match save(state, &args) {
            Ok(s) => ok_text(s),
            Err(e) => err_text(e),
        },
        "load" => match load(state, &args) {
            Ok(s) => ok_text(s),
            Err(e) => err_text(e),
        },
        _ => err_text(format!("unknown tool: {name}")),
    }
}

// ------------------------------------------------------------ tool bodies

fn put_text(state: &mut State, args: &Value) -> Result<String, String> {
    let x = get_usize(args, "x").ok_or("x is required")?;
    let y = get_usize(args, "y").ok_or("y is required")?;
    let text = args
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or("text is required")?;
    let fg = parse_color(args.get("fg"), 16)?.unwrap_or(7);
    let bg = parse_color(args.get("bg"), 8)?.unwrap_or(0);
    let mut cells = 0;
    let mut rows = 0;
    for (dy, line) in text.split('\n').enumerate() {
        let yy = y + dy;
        if yy >= MAX_ROWS {
            break;
        }
        rows += 1;
        for (dx, ch) in line.chars().enumerate() {
            let xx = x + dx;
            if xx >= WIDTH {
                break;
            }
            state.canvas.set(xx, yy, Cell { ch, fg, bg });
            cells += 1;
        }
    }
    Ok(format!("Wrote {cells} cells over {rows} row(s) at {x},{y}."))
}

fn fill_rect(state: &mut State, args: &Value) -> Result<String, String> {
    let x = get_usize(args, "x").ok_or("x is required")?;
    let y = get_usize(args, "y").ok_or("y is required")?;
    let w = get_usize(args, "w").ok_or("w is required")?;
    let h = get_usize(args, "h").ok_or("h is required")?;
    let ch = args
        .get("ch")
        .and_then(|v| v.as_str())
        .and_then(|s| s.chars().next())
        .unwrap_or(' ');
    let fg = parse_color(args.get("fg"), 16)?.unwrap_or(7);
    let bg = parse_color(args.get("bg"), 8)?.unwrap_or(0);
    for yy in y..(y + h).min(MAX_ROWS) {
        for xx in x..(x + w).min(WIDTH) {
            state.canvas.set(xx, yy, Cell { ch, fg, bg });
        }
    }
    Ok(format!("Filled {w}x{h} at {x},{y} with '{ch}'."))
}

fn draw_box(state: &mut State, args: &Value) -> Result<String, String> {
    let x = get_usize(args, "x").ok_or("x is required")?;
    let y = get_usize(args, "y").ok_or("y is required")?;
    let w = get_usize(args, "w").ok_or("w is required")?.max(2);
    let h = get_usize(args, "h").ok_or("h is required")?.max(2);
    let style = args.get("style").and_then(|v| v.as_str()).unwrap_or("single");
    let fg = parse_color(args.get("fg"), 16)?.unwrap_or(7);
    let bg = parse_color(args.get("bg"), 8)?.unwrap_or(0);
    // [tl, tr, bl, br, horiz, vert]
    let g: [char; 6] = match style {
        "double" => ['╔', '╗', '╚', '╝', '═', '║'],
        "block" => ['▄', '▄', '▀', '▀', ' ', '█'],
        _ => ['┌', '┐', '└', '┘', '─', '│'],
    };
    let (x1, y1) = (x + w - 1, y + h - 1);
    let mut put = |xx: usize, yy: usize, ch: char| {
        if xx < WIDTH && yy < MAX_ROWS {
            state.canvas.set(xx, yy, Cell { ch, fg, bg });
        }
    };
    for xx in x..=x1 {
        let top = if xx == x { g[0] } else if xx == x1 { g[1] } else { g[4] };
        let bot = if xx == x { g[2] } else if xx == x1 { g[3] } else { g[4] };
        // the block style runs ▄/▀ across the full top/bottom edges
        let (top, bot) = if style == "block" { ('▄', '▀') } else { (top, bot) };
        put(xx, y, top);
        put(xx, y1, bot);
    }
    for yy in (y + 1)..y1 {
        put(x, yy, g[5]);
        put(x1, yy, g[5]);
    }
    Ok(format!("Drew {style} box {w}x{h} at {x},{y}."))
}

fn render(canvas: &Canvas, colors: bool) -> String {
    let last = canvas.last_used_row();
    let mut out = String::new();
    for y in 0..=last {
        let mut line = String::new();
        for x in 0..WIDTH {
            line.push(canvas.get(x, y).ch);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out.push_str(&format!("-- {}x{} used --\n", WIDTH, last + 1));
    if colors {
        out.push_str("color runs (row: x0-x1 fg/bg …):\n");
        for y in 0..=last {
            let mut runs: Vec<String> = Vec::new();
            let mut start = 0;
            let mut cur = canvas.get(0, y);
            for x in 1..=WIDTH {
                let cell = if x < WIDTH { canvas.get(x, y) } else { Cell::default() };
                if x == WIDTH || cell.fg != cur.fg || cell.bg != cur.bg {
                    if !(cur.fg == 7 && cur.bg == 0) {
                        runs.push(format!("{}-{} {}/{}", start, x - 1, cur.fg, cur.bg));
                    }
                    start = x;
                    cur = cell;
                }
            }
            if !runs.is_empty() {
                out.push_str(&format!("{y}: {}\n", runs.join(" ")));
            }
        }
    }
    out
}

fn save(state: &mut State, args: &Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("path is required")?;
    let mut path = PathBuf::from(path);
    if path.extension().is_none() {
        path.set_extension("ans");
    }
    for (key, field, max) in [
        ("title", 0usize, 35usize),
        ("author", 1, 20),
        ("group", 2, 20),
    ] {
        if let Some(v) = args.get(key).and_then(|v| v.as_str()) {
            let v: String = v.chars().take(max).collect();
            match field {
                0 => state.sauce.title = v,
                1 => state.sauce.author = v,
                _ => state.sauce.group = v,
            }
        }
    }
    let has_sauce = state.sauce != Sauce::default();
    let meta = has_sauce.then_some(&state.sauce);
    let utf8 = args.get("utf8").and_then(|v| v.as_bool()).unwrap_or(false);
    ansi::save_with(&state.canvas, &path, meta, utf8).map_err(|e| format!("save failed: {e}"))?;
    Ok(format!(
        "Saved {} ({}x{} used{}). View it with: cat {}",
        path.display(),
        WIDTH,
        state.canvas.last_used_row() + 1,
        if has_sauce { ", SAUCE attached" } else { "" },
        path.display()
    ))
}

fn load(state: &mut State, args: &Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("path is required")?;
    let path = PathBuf::from(path);
    let canvas = ansi::load(&path).map_err(|e| format!("load failed: {e}"))?;
    let meta = sauce::read(&path).unwrap_or(None);
    let note = match &meta {
        Some(s) => format!(" SAUCE: \"{}\" by {} / {}.", s.title, s.author, s.group),
        None => String::new(),
    };
    state.sauce = meta.unwrap_or_default();
    let rows = canvas.last_used_row() + 1;
    state.canvas = canvas;
    Ok(format!("Loaded {} ({WIDTH}x{rows} used).{note}", path.display()))
}

fn render_png_tool(state: &mut State, args: &Value) -> Result<Value, String> {
    let scale = get_usize(args, "scale").unwrap_or(2).clamp(1, 8);
    let bytes =
        render_png::png_bytes(&state.canvas, scale).map_err(|e| format!("render failed: {e}"))?;
    let rows = state.canvas.last_used_row() + 1;
    let mut note = format!("{}x{} px (scale {scale}, {WIDTH}x{rows} cells)", WIDTH * 8 * scale, rows * 16 * scale);
    if let Some(p) = args.get("path").and_then(|v| v.as_str()) {
        let mut p = PathBuf::from(p);
        if p.extension().is_none() {
            p.set_extension("png");
        }
        std::fs::write(&p, &bytes).map_err(|e| format!("write failed: {e}"))?;
        note.push_str(&format!(", saved to {}", p.display()));
    }
    Ok(json!({ "content": [
        { "type": "image", "data": b64(&bytes), "mimeType": "image/png" },
        { "type": "text", "text": note }
    ]}))
}

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}

/// Resolve a font argument: a real path is used as-is; a bare name looks in
/// the fonts/ directory beside the project (exe lives in target/release/).
fn resolve_font(name: &str) -> PathBuf {
    let p = PathBuf::from(name);
    if p.exists() {
        return p;
    }
    let base = name.trim_end_matches(".tdf");
    if let Ok(exe) = std::env::current_exe() {
        if let Some(root) = exe.parent().and_then(|d| d.parent()).and_then(|d| d.parent()) {
            let candidate = root.join("fonts").join(format!("{base}.tdf"));
            if candidate.exists() {
                return candidate;
            }
        }
    }
    p
}

// ------------------------------------------------------------------ helpers

fn get_usize(args: &Value, key: &str) -> Option<usize> {
    args.get(key).and_then(|v| v.as_u64()).map(|v| v as usize)
}

/// Accepts an integer or a color name; `limit` is 16 for fg, 8 for bg.
fn parse_color(v: Option<&Value>, limit: u8) -> Result<Option<u8>, String> {
    let Some(v) = v else { return Ok(None) };
    if v.is_null() {
        return Ok(None);
    }
    let n = if let Some(n) = v.as_u64() {
        n as u8
    } else if let Some(s) = v.as_str() {
        match s.to_lowercase().replace([' ', '-', '_'], "").as_str() {
            "black" => 0,
            "blue" => 1,
            "green" => 2,
            "cyan" => 3,
            "red" => 4,
            "magenta" | "purple" => 5,
            "brown" | "darkyellow" => 6,
            "gray" | "grey" | "lightgray" | "lightgrey" | "white7" => 7,
            "darkgray" | "darkgrey" => 8,
            "lightblue" | "brightblue" => 9,
            "lightgreen" | "brightgreen" => 10,
            "lightcyan" | "brightcyan" => 11,
            "lightred" | "brightred" => 12,
            "lightmagenta" | "brightmagenta" | "pink" => 13,
            "yellow" => 14,
            "white" | "brightwhite" => 15,
            other => return Err(format!("unknown color: {other}")),
        }
    } else {
        return Err("color must be a number or name".into());
    };
    if n >= limit {
        return Err(format!("color {n} out of range (max {})", limit - 1));
    }
    Ok(Some(n))
}
