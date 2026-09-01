use ansidraw::{ansi, canvas, charset, cp437, render_png, sauce};

use std::io::{self, Write};
use std::path::PathBuf;

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{read, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent,
        KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
    execute, queue,
    style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{self, disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen},
};

use canvas::{Canvas, Cell, MAX_ROWS, WIDTH};

const UNDO_CAP: usize = 300;

enum Mode {
    Edit,
    /// Block selection anchored at (ax, ay); cursor is the opposite corner.
    Select { ax: usize, ay: usize },
    /// Carrying a block; Enter stamps it at the cursor.
    Stamp { buf: Vec<Vec<Cell>> },
}

/// Every editor action, so keybindings, the command palette, and the
/// sidebar all dispatch through one table.
#[derive(Clone, Copy, PartialEq)]
enum Cmd {
    Save,
    Load,
    Exit,
    Help,
    QuickPalette,
    CharPicker,
    CommandPalette,
    Block,
    Undo,
    Redo,
    InsertLine,
    DeleteLine,
    ClearPage,
    NextSet,
    PrevSet,
    SetCharset(usize),
    PickupColor,
    ToggleSidebar,
    SauceInfo,
    ExportPng,
}

/// (palette label, shortcut hint, command)
fn command_table() -> Vec<(String, &'static str, Cmd)> {
    let mut v: Vec<(String, &'static str, Cmd)> = vec![
        ("Save file".into(), "^S", Cmd::Save),
        ("Open file".into(), "^O", Cmd::Load),
        ("Character picker".into(), "^E", Cmd::CharPicker),
        ("Color palette".into(), "Esc", Cmd::QuickPalette),
        ("Block: select".into(), "^B", Cmd::Block),
        ("Undo".into(), "^Z", Cmd::Undo),
        ("Redo".into(), "^R", Cmd::Redo),
        ("Insert line".into(), "^T", Cmd::InsertLine),
        ("Delete line".into(), "^Y", Cmd::DeleteLine),
        ("Next character set".into(), "^N", Cmd::NextSet),
        ("Previous character set".into(), "^P", Cmd::PrevSet),
        ("Pick up color under cursor".into(), "Alt-U", Cmd::PickupColor),
        ("SAUCE info (title/author/group)".into(), "^D", Cmd::SauceInfo),
        ("Export PNG image (high-res)".into(), "", Cmd::ExportPng),
        ("Toggle sidebar".into(), "^W", Cmd::ToggleSidebar),
        ("Clear page".into(), "Alt-C", Cmd::ClearPage),
        ("Help".into(), "^G", Cmd::Help),
        ("Exit".into(), "^Q", Cmd::Exit),
    ];
    for (i, (name, glyphs)) in charset::SETS.iter().enumerate() {
        let g: String = glyphs.iter().collect();
        v.push((
            format!("Charset {}: {} {}", i + 1, name, g),
            "",
            Cmd::SetCharset(i),
        ));
    }
    v
}

struct App {
    canvas: Canvas,
    cur_x: usize,
    cur_y: usize,
    top: usize, // first canvas row shown
    view_h: usize,
    fg: u8,
    bg: u8,
    set_idx: usize,
    filename: Option<PathBuf>,
    dirty: bool,
    undo: Vec<(Canvas, usize, usize)>,
    redo: Vec<(Canvas, usize, usize)>,
    mode: Mode,
    msg: Option<String>,
    sidebar: bool,
    /// Canvas cell where a left-button press started (drag-select origin).
    drag_from: Option<(usize, usize)>,
    /// Click map for the sidebar, rebuilt each frame it is drawn.
    sb_map: Vec<(u16, SbHit)>,
    sauce: sauce::Sauce,
    /// Attach a SAUCE record when saving.
    sauce_on: bool,
}

fn main() {
    let arg = std::env::args().nth(1).map(PathBuf::from);

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let mut out = io::stdout();
        let _ = execute!(out, LeaveAlternateScreen, Show);
        let _ = disable_raw_mode();
        default_hook(info);
    }));

    if let Err(e) = run(arg) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(arg: Option<PathBuf>) -> io::Result<()> {
    let mut app = App::new(arg)?;
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    let res = app.event_loop(&mut out);
    execute!(out, DisableMouseCapture, LeaveAlternateScreen, Show)?;
    disable_raw_mode()?;
    res
}

fn read_key() -> io::Result<KeyEvent> {
    loop {
        match read()? {
            Event::Key(k) if k.kind != KeyEventKind::Release => return Ok(k),
            _ => {}
        }
    }
}

/// Next key press or mouse event (for modals that support the mouse).
fn read_input() -> io::Result<Event> {
    loop {
        match read()? {
            Event::Key(k) if k.kind == KeyEventKind::Release => continue,
            e @ (Event::Key(_) | Event::Mouse(_)) => return Ok(e),
            _ => {}
        }
    }
}

/// What a click on a given sidebar row means; rebuilt every frame.
#[derive(Clone, Copy)]
enum SbHit {
    FgStrip,
    BgStrip,
    SetName,
    /// Row showing glyphs i and i+5 of the active set.
    GlyphRow(usize),
    Run(Cmd),
}

impl App {
    fn new(arg: Option<PathBuf>) -> io::Result<Self> {
        let (canvas, filename, msg, meta) = match arg {
            Some(p) if p.exists() => {
                let c = ansi::load(&p)?;
                let s = sauce::read(&p)?;
                let m = format!("Loaded {}", p.display());
                (c, Some(p), Some(m), s)
            }
            Some(p) => (Canvas::new(25), Some(p), None, None),
            None => (Canvas::new(25), None, None, None),
        };
        let sauce_on = meta.is_some();
        Ok(App {
            canvas,
            cur_x: 0,
            cur_y: 0,
            top: 0,
            view_h: 24,
            fg: 7,
            bg: 0,
            set_idx: 0,
            filename,
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            mode: Mode::Edit,
            msg,
            sidebar: true,
            drag_from: None,
            sb_map: Vec::new(),
            sauce: meta.unwrap_or_default(),
            sauce_on,
        })
    }

    fn event_loop(&mut self, out: &mut io::Stdout) -> io::Result<()> {
        loop {
            self.draw(out)?;
            match read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release => {
                    if self.handle_key(k, out)? {
                        return Ok(());
                    }
                }
                Event::Mouse(m) => {
                    if self.handle_mouse(m, out)? {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
    }

    // ---------------------------------------------------------------- drawing

    fn select_rect(&self) -> Option<(usize, usize, usize, usize)> {
        if let Mode::Select { ax, ay } = self.mode {
            Some((
                ax.min(self.cur_x),
                ay.min(self.cur_y),
                ax.max(self.cur_x),
                ay.max(self.cur_y),
            ))
        } else {
            None
        }
    }

    fn draw(&mut self, out: &mut io::Stdout) -> io::Result<()> {
        let (tw, th) = terminal::size()?;
        let tw = tw as usize;
        self.view_h = (th as usize).saturating_sub(1).max(1);

        // keep cursor in view
        if self.cur_y < self.top {
            self.top = self.cur_y;
        }
        if self.cur_y >= self.top + self.view_h {
            self.top = self.cur_y + 1 - self.view_h;
        }

        queue!(out, Hide)?;
        self.draw_status(out, tw)?;

        let sel = self.select_rect();
        let stamp: Option<&Vec<Vec<Cell>>> = match &self.mode {
            Mode::Stamp { buf } => Some(buf),
            _ => None,
        };

        for vy in 0..self.view_h {
            let y = self.top + vy;
            queue!(out, MoveTo(0, (vy + 1) as u16))?;
            let mut cur: Option<(u8, u8)> = None;
            let cols = WIDTH.min(tw);
            for x in 0..cols {
                let mut cell = self.canvas.get(x, y);
                // stamp preview overlays the canvas at the cursor
                if let Some(buf) = stamp {
                    if y >= self.cur_y && x >= self.cur_x {
                        let (by, bx) = (y - self.cur_y, x - self.cur_x);
                        if let Some(c) = buf.get(by).and_then(|r| r.get(bx)) {
                            cell = *c;
                        }
                    }
                }
                let mut fg = cell.fg;
                let mut bg = cell.bg;
                // selection shown inverted
                if let Some((x0, y0, x1, y1)) = sel {
                    if x >= x0 && x <= x1 && y >= y0 && y <= y1 {
                        std::mem::swap(&mut fg, &mut bg);
                        if fg == bg {
                            fg = 0;
                            bg = 7;
                        }
                    }
                }
                if cur != Some((fg, bg)) {
                    queue!(
                        out,
                        SetForegroundColor(Color::AnsiValue(fg)),
                        SetBackgroundColor(Color::AnsiValue(bg))
                    )?;
                    cur = Some((fg, bg));
                }
                queue!(out, Print(cell.ch))?;
            }
            queue!(out, ResetColor, Clear(ClearType::UntilNewLine))?;
        }

        if self.sidebar_visible(tw) {
            self.draw_sidebar(out)?;
        } else {
            self.sb_map.clear();
        }

        let cy = (self.cur_y - self.top + 1) as u16;
        queue!(out, MoveTo(self.cur_x as u16, cy), Show)?;
        out.flush()
    }

    fn sidebar_visible(&self, tw: usize) -> bool {
        self.sidebar && tw >= WIDTH + 20
    }

    fn draw_sidebar(&mut self, out: &mut io::Stdout) -> io::Result<()> {
        self.sb_map.clear();
        let x0 = (WIDTH + 2) as u16; // col 80 is the separator, 81 a gutter
        let dim = Color::AnsiValue(8);
        let lit = Color::AnsiValue(7);
        let hi = Color::AnsiValue(15);

        let mut row: u16 = 1;
        let line = |out: &mut io::Stdout, row: &mut u16| -> io::Result<u16> {
            let r = *row;
            queue!(
                out,
                MoveTo((WIDTH) as u16, r),
                ResetColor,
                SetForegroundColor(dim),
                Print(" │ "),
                Clear(ClearType::UntilNewLine)
            )?;
            *row += 1;
            Ok(r)
        };

        // file + position
        let name = self
            .filename
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled.ans".into());
        let r = line(out, &mut row)?;
        queue!(out, MoveTo(x0, r), SetForegroundColor(hi),
            Print(format!("{}{}", name, if self.dirty { "*" } else { "" })))?;
        let r = line(out, &mut row)?;
        queue!(out, MoveTo(x0, r), SetForegroundColor(lit),
            Print(format!("{},{}", self.cur_x + 1, self.cur_y + 1)))?;
        let r = line(out, &mut row)?;
        if self.sauce_on {
            let by = if self.sauce.author.is_empty() {
                String::new()
            } else {
                format!(" · {}", self.sauce.author)
            };
            let s: String = format!("SAUCE ✓{}", by).chars().take(16).collect();
            queue!(out, MoveTo(x0, r), SetForegroundColor(lit), Print(s))?;
        } else {
            queue!(out, MoveTo(x0, r), SetForegroundColor(dim), Print("no SAUCE (^D)"))?;
        }
        self.sb_map.push((r, SbHit::Run(Cmd::SauceInfo)));
        line(out, &mut row)?;

        // colors
        let r = line(out, &mut row)?;
        queue!(out, MoveTo(x0, r), SetForegroundColor(dim), Print("FG"))?;
        let r = line(out, &mut row)?;
        queue!(out, MoveTo(x0, r))?;
        for i in 0..16u8 {
            queue!(out, SetForegroundColor(Color::AnsiValue(i)), Print('█'))?;
        }
        self.sb_map.push((r, SbHit::FgStrip));
        let r = line(out, &mut row)?;
        queue!(out, MoveTo(x0 + self.fg as u16, r), SetForegroundColor(hi), Print('▲'))?;
        self.sb_map.push((r, SbHit::FgStrip));
        let r = line(out, &mut row)?;
        queue!(out, MoveTo(x0, r), SetForegroundColor(dim), Print("BG"))?;
        let r = line(out, &mut row)?;
        queue!(out, MoveTo(x0, r))?;
        for i in 0..8u8 {
            queue!(out, SetForegroundColor(Color::AnsiValue(i)), Print('█'))?;
        }
        self.sb_map.push((r, SbHit::BgStrip));
        let r = line(out, &mut row)?;
        queue!(out, MoveTo(x0 + self.bg as u16, r), SetForegroundColor(hi), Print('▲'))?;
        self.sb_map.push((r, SbHit::BgStrip));
        line(out, &mut row)?;

        // character set
        let r = line(out, &mut row)?;
        queue!(out, MoveTo(x0, r), SetForegroundColor(dim),
            Print(format!("SET {:02} ", self.set_idx + 1)),
            SetForegroundColor(hi), Print(charset::name(self.set_idx)))?;
        self.sb_map.push((r, SbHit::SetName));
        for i in 0..5 {
            let r = line(out, &mut row)?;
            queue!(out, MoveTo(x0, r), SetForegroundColor(dim),
                Print(format!("F{} ", i + 1)),
                SetForegroundColor(hi), Print(charset::glyph(self.set_idx, i)),
                SetForegroundColor(dim), Print(format!("  F{:<2} ", i + 6)),
                SetForegroundColor(hi), Print(charset::glyph(self.set_idx, i + 5)))?;
            self.sb_map.push((r, SbHit::GlyphRow(i)));
        }
        line(out, &mut row)?;

        // cheatsheet (each row is clickable)
        for (keys, what, cmd) in [
            ("^K", "commands", Cmd::CommandPalette),
            ("^E", "char pick", Cmd::CharPicker),
            ("Esc", "colors", Cmd::QuickPalette),
            ("^B", "block", Cmd::Block),
            ("^S", "save", Cmd::Save),
            ("^Z", "undo", Cmd::Undo),
            ("^W", "hide this", Cmd::ToggleSidebar),
        ] {
            let r = line(out, &mut row)?;
            queue!(out, MoveTo(x0, r), SetForegroundColor(hi), Print(format!("{:<4}", keys)),
                SetForegroundColor(dim), Print(what))?;
            self.sb_map.push((r, SbHit::Run(cmd)));
        }

        // separator for any remaining rows
        while (row as usize) < self.view_h + 1 {
            line(out, &mut row)?;
        }
        queue!(out, ResetColor)?;
        Ok(())
    }

    fn draw_status(&self, out: &mut io::Stdout, tw: usize) -> io::Result<()> {
        let bar_fg = Color::AnsiValue(0);
        let bar_bg = Color::AnsiValue(7);
        queue!(out, MoveTo(0, 0), SetForegroundColor(bar_fg), SetBackgroundColor(bar_bg))?;

        let name = self
            .filename
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled.ans".into());
        let mode_hint = match self.mode {
            Mode::Edit => "^K Commands",
            Mode::Select { .. } => "C copy M move F fill E erase X/Y flip Esc",
            Mode::Stamp { .. } => "Enter stamp  Esc done",
        };
        let left = if let Some(m) = &self.msg {
            format!(" {} ", m)
        } else {
            format!(
                " {}{} {:>2},{:<4} {:02}:{} ",
                name,
                if self.dirty { "*" } else { " " },
                self.cur_x + 1,
                self.cur_y + 1,
                self.set_idx + 1,
                charset::name(self.set_idx),
            )
        };
        let glyphs: String = (0..10).map(|i| charset::glyph(self.set_idx, i)).collect();
        let mut used = 0usize;

        let put = |out: &mut io::Stdout, s: &str, used: &mut usize| -> io::Result<()> {
            for ch in s.chars() {
                if *used + 1 >= tw {
                    break;
                }
                queue!(out, Print(ch))?;
                *used += 1;
            }
            Ok(())
        };

        put(out, &left, &mut used)?;
        put(out, &glyphs, &mut used)?;
        put(out, " ", &mut used)?;

        // color swatches
        if used + 6 < tw {
            queue!(out, SetForegroundColor(Color::AnsiValue(self.fg)))?;
            queue!(out, Print("██"))?;
            queue!(out, SetForegroundColor(Color::AnsiValue(self.bg)))?;
            queue!(out, Print("██"))?;
            queue!(out, SetForegroundColor(bar_fg))?;
            used += 4;
            put(out, " ", &mut used)?;
        }
        put(out, mode_hint, &mut used)?;
        while used + 1 < tw {
            queue!(out, Print(' '))?;
            used += 1;
        }
        queue!(out, ResetColor)?;
        Ok(())
    }

    // ------------------------------------------------------------- undo/redo

    fn push_undo(&mut self) {
        self.redo.clear();
        self.undo.push((self.canvas.clone(), self.cur_x, self.cur_y));
        if self.undo.len() > UNDO_CAP {
            self.undo.remove(0);
        }
        self.dirty = true;
    }

    fn do_undo(&mut self) {
        if let Some((c, x, y)) = self.undo.pop() {
            self.redo.push((self.canvas.clone(), self.cur_x, self.cur_y));
            self.canvas = c;
            self.cur_x = x;
            self.cur_y = y;
            self.msg = Some("Undo".into());
        } else {
            self.msg = Some("Nothing to undo".into());
        }
    }

    fn do_redo(&mut self) {
        if let Some((c, x, y)) = self.redo.pop() {
            self.undo.push((self.canvas.clone(), self.cur_x, self.cur_y));
            self.canvas = c;
            self.cur_x = x;
            self.cur_y = y;
            self.msg = Some("Redo".into());
        } else {
            self.msg = Some("Nothing to redo".into());
        }
    }

    // ------------------------------------------------------------------ keys

    fn handle_key(&mut self, k: KeyEvent, out: &mut io::Stdout) -> io::Result<bool> {
        self.msg = None;
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);

        // keys specific to block modes first
        match &self.mode {
            Mode::Select { .. } => {
                if self.handle_select_key(k)? {
                    return Ok(false);
                }
            }
            Mode::Stamp { .. } => {
                if self.handle_stamp_key(k)? {
                    return Ok(false);
                }
            }
            Mode::Edit => {}
        }

        match k.code {
            // ------------------------------------------------------ commands
            KeyCode::Char('x') if alt => return self.exec(Cmd::Exit, out),
            KeyCode::Char('q') if ctrl => return self.exec(Cmd::Exit, out),
            KeyCode::Char('k') if ctrl => return self.exec(Cmd::CommandPalette, out),
            KeyCode::Char('s') if alt || ctrl => return self.exec(Cmd::Save, out),
            KeyCode::Char('l') if alt => return self.exec(Cmd::Load, out),
            KeyCode::Char('o') if ctrl => return self.exec(Cmd::Load, out),
            KeyCode::Char('h') if alt => return self.exec(Cmd::Help, out),
            KeyCode::Char('g') if ctrl => return self.exec(Cmd::Help, out),
            KeyCode::Char('c') if alt => return self.exec(Cmd::ClearPage, out),
            KeyCode::Char('r') if alt => return self.exec(Cmd::Undo, out),
            KeyCode::Char('z') if ctrl => return self.exec(Cmd::Undo, out),
            KeyCode::Char('r') if ctrl => return self.exec(Cmd::Redo, out),
            KeyCode::Char('b') if alt || ctrl => return self.exec(Cmd::Block, out),
            KeyCode::Char('e') if ctrl => return self.exec(Cmd::CharPicker, out),
            KeyCode::Char('w') if ctrl => return self.exec(Cmd::ToggleSidebar, out),
            KeyCode::Char('u') if alt => return self.exec(Cmd::PickupColor, out),
            KeyCode::Char('d') if alt || ctrl => return self.exec(Cmd::SauceInfo, out),
            KeyCode::Char('y') if ctrl => return self.exec(Cmd::DeleteLine, out),
            KeyCode::Char('t') if ctrl => return self.exec(Cmd::InsertLine, out),
            KeyCode::Char('i') if alt => return self.exec(Cmd::InsertLine, out),
            KeyCode::Char('n') if ctrl => return self.exec(Cmd::NextSet, out),
            KeyCode::Char('p') if ctrl => return self.exec(Cmd::PrevSet, out),
            KeyCode::Esc => return self.exec(Cmd::QuickPalette, out),
            KeyCode::Char('a') if ctrl => return self.exec(Cmd::QuickPalette, out),

            // ---------------------------------------------------------- color
            KeyCode::Up if ctrl => self.fg = (self.fg + 1) % 16,
            KeyCode::Down if ctrl => self.fg = (self.fg + 15) % 16,
            KeyCode::Right if ctrl => self.bg = (self.bg + 1) % 8,
            KeyCode::Left if ctrl => self.bg = (self.bg + 7) % 8,

            // ----------------------------------------------- character sets
            KeyCode::F(n) if alt && (1..=10).contains(&n) => {
                return self.exec(Cmd::SetCharset((n - 1) as usize), out);
            }
            KeyCode::F(n) if ctrl && (1..=5).contains(&n) => {
                return self.exec(Cmd::SetCharset(10 + (n - 1) as usize), out);
            }
            KeyCode::F(n) if (1..=10).contains(&n) => {
                self.put_char(charset::glyph(self.set_idx, (n - 1) as usize));
            }

            // ------------------------------------------------------- movement
            KeyCode::Up => self.cur_y = self.cur_y.saturating_sub(1),
            KeyCode::Down => self.cur_y = (self.cur_y + 1).min(MAX_ROWS - 1),
            KeyCode::Left => self.cur_x = self.cur_x.saturating_sub(1),
            KeyCode::Right => self.cur_x = (self.cur_x + 1).min(WIDTH - 1),
            KeyCode::PageUp => {
                self.cur_y = self.cur_y.saturating_sub(self.view_h);
                self.top = self.top.saturating_sub(self.view_h);
            }
            KeyCode::PageDown => {
                self.cur_y = (self.cur_y + self.view_h).min(MAX_ROWS - 1);
            }
            KeyCode::Home if ctrl => {
                self.cur_x = self.canvas.line_bounds(self.cur_y).map(|(f, _)| f).unwrap_or(0);
            }
            KeyCode::End if ctrl => {
                self.cur_x = self.canvas.line_bounds(self.cur_y).map(|(_, l)| l).unwrap_or(0);
            }
            KeyCode::Home => self.cur_x = 0,
            KeyCode::End => {
                self.cur_x = self
                    .canvas
                    .line_bounds(self.cur_y)
                    .map(|(_, l)| (l + 1).min(WIDTH - 1))
                    .unwrap_or(0);
            }
            KeyCode::Tab => self.cur_x = ((self.cur_x / 8 + 1) * 8).min(WIDTH - 1),
            KeyCode::BackTab => self.cur_x = (self.cur_x.saturating_sub(1) / 8) * 8,
            KeyCode::Enter => {
                self.cur_x = 0;
                self.cur_y = (self.cur_y + 1).min(MAX_ROWS - 1);
                self.canvas.ensure_row(self.cur_y);
            }

            // -------------------------------------------------------- editing
            KeyCode::Backspace => {
                if self.cur_x > 0 {
                    self.push_undo();
                    self.cur_x -= 1;
                    self.canvas.set(self.cur_x, self.cur_y, Cell::default());
                }
            }
            KeyCode::Delete => {
                self.push_undo();
                self.canvas.shift_left(self.cur_x, self.cur_y);
            }
            KeyCode::Insert => {
                self.push_undo();
                self.canvas.shift_right(self.cur_x, self.cur_y);
            }
            KeyCode::Char(c) if !ctrl && !alt => self.put_char(c),
            _ => {}
        }
        Ok(false)
    }

    fn handle_mouse(&mut self, m: MouseEvent, out: &mut io::Stdout) -> io::Result<bool> {
        let col = m.column as usize;
        let row = m.row;
        let canvas_pos = if col < WIDTH && row >= 1 && (row as usize) <= self.view_h {
            Some((col, self.top + row as usize - 1))
        } else {
            None
        };

        match m.kind {
            MouseEventKind::ScrollUp => self.cur_y = self.cur_y.saturating_sub(3),
            MouseEventKind::ScrollDown => {
                self.cur_y = (self.cur_y + 3).min(MAX_ROWS - 1);
                self.canvas.ensure_row(self.cur_y);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.msg = None;
                if let Some((x, y)) = canvas_pos {
                    match &self.mode {
                        // click stamps the carried block right there
                        Mode::Stamp { .. } => {
                            self.cur_x = x;
                            self.cur_y = y;
                            self.stamp_here();
                        }
                        // click restarts a selection at the click point
                        Mode::Select { .. } => {
                            self.cur_x = x;
                            self.cur_y = y;
                            self.mode = Mode::Select { ax: x, ay: y };
                            self.drag_from = Some((x, y));
                        }
                        Mode::Edit => {
                            self.cur_x = x;
                            self.cur_y = y;
                            self.canvas.ensure_row(y);
                            self.drag_from = Some((x, y));
                        }
                    }
                } else if let Some(hit) = self
                    .sb_map
                    .iter()
                    .find(|(r, _)| *r == row)
                    .map(|(_, h)| *h)
                {
                    let x0 = WIDTH + 2;
                    match hit {
                        SbHit::FgStrip => {
                            if (x0..x0 + 16).contains(&col) {
                                self.fg = (col - x0) as u8;
                            }
                        }
                        SbHit::BgStrip => {
                            if (x0..x0 + 8).contains(&col) {
                                self.bg = (col - x0) as u8;
                            }
                        }
                        SbHit::SetName => return self.exec(Cmd::NextSet, out),
                        SbHit::GlyphRow(i) => {
                            let idx = if col < x0 + 8 { i } else { i + 5 };
                            self.put_char(charset::glyph(self.set_idx, idx));
                        }
                        SbHit::Run(c) => return self.exec(c, out),
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let (Some((x, y)), Some((ax, ay))) = (canvas_pos, self.drag_from) {
                    if !matches!(self.mode, Mode::Stamp { .. }) {
                        self.mode = Mode::Select { ax, ay };
                        self.cur_x = x;
                        self.cur_y = y;
                        self.canvas.ensure_row(y);
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => self.drag_from = None,
            _ => {}
        }
        Ok(false)
    }

    /// Central command dispatch; returns true when the app should exit.
    fn exec(&mut self, cmd: Cmd, out: &mut io::Stdout) -> io::Result<bool> {
        match cmd {
            Cmd::Save => self.save_flow(out)?,
            Cmd::Load => self.load_flow(out)?,
            Cmd::Exit => return self.exit_flow(out),
            Cmd::Help => self.help(out)?,
            Cmd::QuickPalette => self.palette(out)?,
            Cmd::CharPicker => self.char_picker(out)?,
            Cmd::CommandPalette => {
                if let Some(c) = self.command_palette(out)? {
                    return self.exec(c, out);
                }
            }
            Cmd::Block => {
                self.mode = Mode::Select { ax: self.cur_x, ay: self.cur_y };
            }
            Cmd::Undo => self.do_undo(),
            Cmd::Redo => self.do_redo(),
            Cmd::InsertLine => {
                self.push_undo();
                self.canvas.insert_line(self.cur_y);
                self.msg = Some("Line inserted".into());
            }
            Cmd::DeleteLine => {
                self.push_undo();
                self.canvas.delete_line(self.cur_y);
                self.msg = Some("Line deleted".into());
            }
            Cmd::ClearPage => self.clear_flow(out)?,
            Cmd::NextSet => self.set_idx = (self.set_idx + 1) % charset::SETS.len(),
            Cmd::PrevSet => {
                self.set_idx = (self.set_idx + charset::SETS.len() - 1) % charset::SETS.len();
            }
            Cmd::SetCharset(i) => self.set_idx = i % charset::SETS.len(),
            Cmd::PickupColor => {
                let c = self.canvas.get(self.cur_x, self.cur_y);
                self.fg = c.fg;
                self.bg = c.bg;
                self.msg = Some("Picked up color under cursor".into());
            }
            Cmd::ToggleSidebar => self.sidebar = !self.sidebar,
            Cmd::SauceInfo => self.sauce_form(out)?,
            Cmd::ExportPng => {
                let guess = self
                    .filename
                    .as_ref()
                    .map(|p| p.with_extension("png").to_string_lossy().into_owned())
                    .unwrap_or_else(|| "untitled.png".into());
                if let Some(name) = self.prompt(out, "Export PNG as: ", &guess)? {
                    let mut p = PathBuf::from(name);
                    if p.extension().is_none() {
                        p.set_extension("png");
                    }
                    match render_png::save_png(&self.canvas, &p, 4) {
                        Ok((w, h)) => {
                            self.msg = Some(format!("Exported {} ({w}x{h})", p.display()));
                        }
                        Err(e) => self.msg = Some(format!("Export failed: {e}")),
                    }
                }
            }
        }
        Ok(false)
    }

    /// SAUCE metadata editor (Alt-D in the original).
    fn sauce_form(&mut self, out: &mut io::Stdout) -> io::Result<()> {
        let mut fields = [
            ("Title ", self.sauce.title.clone(), 35usize),
            ("Author", self.sauce.author.clone(), 20),
            ("Group ", self.sauce.group.clone(), 20),
        ];
        // opening the form on a sauceless file implies intent to add one
        let mut attach = self.sauce_on || self.sauce == sauce::Sauce::default();
        let mut sel: usize = 0; // 0-2 fields, 3 = attach toggle
        const W: usize = 50;
        let bx = 8u16;
        let by = 2u16;

        loop {
            let fg = Color::AnsiValue(15);
            let bg = Color::AnsiValue(4);
            let dim = Color::AnsiValue(11);
            queue!(out, Hide, SetForegroundColor(fg), SetBackgroundColor(bg))?;
            queue!(out, MoveTo(bx, by), Print(format!("╔{:═^w$}╗", " SAUCE ", w = W)))?;
            for (i, (label, val, max)) in fields.iter().enumerate() {
                let cursor = if i == sel { "_" } else { "" };
                let body = format!(" {} [{}{}]", label, val, cursor);
                let body: String = body.chars().take(W).collect();
                queue!(out, MoveTo(bx, by + 1 + i as u16), Print('║'))?;
                if i == sel {
                    queue!(out, SetForegroundColor(Color::AnsiValue(0)),
                        SetBackgroundColor(Color::AnsiValue(7)))?;
                }
                queue!(out, Print(format!("{:<w$}", body, w = W)))?;
                queue!(out, SetForegroundColor(fg), SetBackgroundColor(bg), Print('║'))?;
                let _ = max;
            }
            let toggle = format!(" Attach SAUCE record: {}", if attach { "Yes" } else { "No " });
            queue!(out, MoveTo(bx, by + 4), Print('║'))?;
            if sel == 3 {
                queue!(out, SetForegroundColor(Color::AnsiValue(0)),
                    SetBackgroundColor(Color::AnsiValue(7)))?;
            }
            queue!(out, Print(format!("{:<w$}", toggle, w = W)))?;
            queue!(out, SetForegroundColor(fg), SetBackgroundColor(bg), Print('║'))?;
            let info = format!(
                " {}x{} · date set on save",
                WIDTH,
                self.canvas.last_used_row() + 1
            );
            queue!(out, MoveTo(bx, by + 5), SetForegroundColor(dim),
                Print(format!("║{:<w$}║", info, w = W)), SetForegroundColor(fg))?;
            queue!(out, MoveTo(bx, by + 6),
                Print(format!("╚{:═^w$}╝", " ↑↓ field · Enter ok · Esc cancel ", w = W)),
                ResetColor)?;
            out.flush()?;

            match read_input()? {
                Event::Key(k) => match k.code {
                    KeyCode::Esc => return Ok(()),
                    KeyCode::Enter => {
                        self.sauce.title = fields[0].1.trim().to_string();
                        self.sauce.author = fields[1].1.trim().to_string();
                        self.sauce.group = fields[2].1.trim().to_string();
                        self.sauce_on = attach;
                        self.msg = Some(if attach {
                            "SAUCE will be attached on save".into()
                        } else {
                            "SAUCE will NOT be attached on save".into()
                        });
                        return Ok(());
                    }
                    KeyCode::Up | KeyCode::BackTab => sel = (sel + 3) % 4,
                    KeyCode::Down | KeyCode::Tab => sel = (sel + 1) % 4,
                    KeyCode::Backspace => {
                        if sel < 3 {
                            fields[sel].1.pop();
                        }
                    }
                    KeyCode::Left | KeyCode::Right => {
                        if sel == 3 {
                            attach = !attach;
                        }
                    }
                    KeyCode::Char(' ') if sel == 3 => attach = !attach,
                    KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                        if sel < 3 && fields[sel].1.chars().count() < fields[sel].2 {
                            fields[sel].1.push(c);
                        }
                    }
                    _ => {}
                },
                Event::Mouse(m) => {
                    if let MouseEventKind::Down(MouseButton::Left) = m.kind {
                        let r = m.row;
                        let in_box = (bx..bx + W as u16 + 2).contains(&m.column)
                            && (by..=by + 6).contains(&r);
                        if (by + 1..=by + 3).contains(&r) {
                            sel = (r - by - 1) as usize;
                        } else if r == by + 4 {
                            if sel == 3 {
                                attach = !attach;
                            }
                            sel = 3;
                        } else if !in_box {
                            return Ok(()); // click away cancels
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn put_char(&mut self, ch: char) {
        self.push_undo();
        self.canvas.set(
            self.cur_x,
            self.cur_y,
            Cell { ch, fg: self.fg, bg: self.bg },
        );
        self.cur_x = (self.cur_x + 1).min(WIDTH - 1);
    }

    // ------------------------------------------------------------- block mode

    /// Returns true if the key was consumed by select mode.
    fn handle_select_key(&mut self, k: KeyEvent) -> io::Result<bool> {
        let (x0, y0, x1, y1) = self.select_rect().unwrap();
        match k.code {
            KeyCode::Esc => {
                self.mode = Mode::Edit;
                Ok(true)
            }
            KeyCode::Char(c) => {
                match c.to_ascii_lowercase() {
                    'c' | 'm' => {
                        let buf = self.grab(x0, y0, x1, y1);
                        if c.to_ascii_lowercase() == 'm' {
                            self.push_undo();
                            self.erase(x0, y0, x1, y1);
                        }
                        self.cur_x = x0;
                        self.cur_y = y0;
                        self.mode = Mode::Stamp { buf };
                        self.msg = Some("Move cursor, Enter to stamp, Esc when done".into());
                    }
                    'e' => {
                        self.push_undo();
                        self.erase(x0, y0, x1, y1);
                        self.mode = Mode::Edit;
                    }
                    'f' => {
                        self.push_undo();
                        for y in y0..=y1 {
                            for x in x0..=x1 {
                                let mut cell = self.canvas.get(x, y);
                                cell.fg = self.fg;
                                cell.bg = self.bg;
                                self.canvas.set(x, y, cell);
                            }
                        }
                        self.mode = Mode::Edit;
                        self.msg = Some("Block filled with current attribute".into());
                    }
                    'x' => {
                        self.push_undo();
                        for y in y0..=y1 {
                            let mut row: Vec<Cell> =
                                (x0..=x1).map(|x| self.canvas.get(x, y)).collect();
                            row.reverse();
                            for (i, cell) in row.into_iter().enumerate() {
                                self.canvas.set(x0 + i, y, cell);
                            }
                        }
                        self.msg = Some("Block flipped horizontally".into());
                    }
                    'y' => {
                        self.push_undo();
                        let rows: Vec<Vec<Cell>> = (y0..=y1)
                            .rev()
                            .map(|y| (x0..=x1).map(|x| self.canvas.get(x, y)).collect())
                            .collect();
                        for (i, row) in rows.into_iter().enumerate() {
                            for (j, cell) in row.into_iter().enumerate() {
                                self.canvas.set(x0 + j, y0 + i, cell);
                            }
                        }
                        self.msg = Some("Block flipped vertically".into());
                    }
                    _ => {}
                }
                Ok(true)
            }
            _ => Ok(false), // movement keys fall through to extend the selection
        }
    }

    /// Returns true if the key was consumed by stamp mode.
    fn handle_stamp_key(&mut self, k: KeyEvent) -> io::Result<bool> {
        match k.code {
            KeyCode::Esc => {
                self.mode = Mode::Edit;
                Ok(true)
            }
            KeyCode::Enter => {
                self.stamp_here();
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn stamp_here(&mut self) {
        let buf = match &self.mode {
            Mode::Stamp { buf } => buf.clone(),
            _ => return,
        };
        self.push_undo();
        for (by, row) in buf.iter().enumerate() {
            for (bx, cell) in row.iter().enumerate() {
                self.canvas.set(self.cur_x + bx, self.cur_y + by, *cell);
            }
        }
        self.msg = Some("Stamped (Enter/click again to repeat, Esc when done)".into());
    }

    fn grab(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> Vec<Vec<Cell>> {
        (y0..=y1)
            .map(|y| (x0..=x1).map(|x| self.canvas.get(x, y)).collect())
            .collect()
    }

    fn erase(&mut self, x0: usize, y0: usize, x1: usize, y1: usize) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                self.canvas.set(x, y, Cell::default());
            }
        }
    }

    // --------------------------------------------------------------- modals

    /// The quick palette, opened with Esc like the original.
    fn palette(&mut self, out: &mut io::Stdout) -> io::Result<()> {
        loop {
            let bar_fg = Color::AnsiValue(0);
            let bar_bg = Color::AnsiValue(7);
            queue!(out, MoveTo(0, 1), SetForegroundColor(bar_fg), SetBackgroundColor(bar_bg))?;
            queue!(out, Print(" Quick palette   ↑↓ foreground   ←→ background   Esc/Enter close "))?;
            queue!(out, Clear(ClearType::UntilNewLine))?;

            queue!(out, MoveTo(0, 2), SetBackgroundColor(bar_bg), SetForegroundColor(bar_fg), Print(" FG "))?;
            for i in 0..16u8 {
                queue!(out, SetForegroundColor(Color::AnsiValue(i)), SetBackgroundColor(bar_bg))?;
                if i == self.fg {
                    queue!(out, SetBackgroundColor(Color::AnsiValue(0)), Print("▐█"))?;
                } else {
                    queue!(out, Print("██"))?;
                }
            }
            queue!(out, SetForegroundColor(bar_fg), SetBackgroundColor(bar_bg), Print("  "), Clear(ClearType::UntilNewLine))?;

            queue!(out, MoveTo(0, 3), SetBackgroundColor(bar_bg), SetForegroundColor(bar_fg), Print(" BG "))?;
            for i in 0..8u8 {
                queue!(out, SetForegroundColor(Color::AnsiValue(i)), SetBackgroundColor(bar_bg))?;
                if i == self.bg {
                    queue!(out, SetBackgroundColor(Color::AnsiValue(15)), Print("▐█"))?;
                } else {
                    queue!(out, Print("██"))?;
                }
            }
            queue!(out, SetForegroundColor(bar_fg), SetBackgroundColor(bar_bg), Print("  "), Clear(ClearType::UntilNewLine), ResetColor)?;
            out.flush()?;

            match read_input()? {
                Event::Key(k) => match k.code {
                    KeyCode::Up => self.fg = (self.fg + 1) % 16,
                    KeyCode::Down => self.fg = (self.fg + 15) % 16,
                    KeyCode::Right => self.bg = (self.bg + 1) % 8,
                    KeyCode::Left => self.bg = (self.bg + 7) % 8,
                    KeyCode::Esc | KeyCode::Enter => return Ok(()),
                    _ => {}
                },
                Event::Mouse(m) => {
                    if let MouseEventKind::Down(MouseButton::Left) = m.kind {
                        let c = m.column as usize;
                        // row 2: FG swatches (2 cells each), row 3: BG swatches
                        if m.row == 2 && (4..36).contains(&c) {
                            self.fg = ((c - 4) / 2) as u8;
                        } else if m.row == 3 && (4..20).contains(&c) {
                            self.bg = ((c - 4) / 2) as u8;
                        } else if !(1..=3).contains(&m.row) {
                            return Ok(()); // click away closes
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Fuzzy command palette (Ctrl-K): type to filter, Enter runs.
    fn command_palette(&mut self, out: &mut io::Stdout) -> io::Result<Option<Cmd>> {
        let all = command_table();
        let mut query = String::new();
        let mut sel: usize = 0;
        const ROWS: usize = 12;
        const W: usize = 48; // interior width

        loop {
            let q = query.to_lowercase();
            let mut filtered: Vec<&(String, &'static str, Cmd)> = all
                .iter()
                .filter(|(n, _, _)| n.to_lowercase().starts_with(&q))
                .collect();
            filtered.extend(all.iter().filter(|(n, _, _)| {
                let l = n.to_lowercase();
                !l.starts_with(&q) && l.contains(&q)
            }));
            if sel >= filtered.len() {
                sel = filtered.len().saturating_sub(1);
            }

            let bx = 10u16;
            let by = 2u16;
            let fg = Color::AnsiValue(15);
            let bg = Color::AnsiValue(4);
            queue!(out, Hide, SetForegroundColor(fg), SetBackgroundColor(bg))?;
            queue!(out, MoveTo(bx, by), Print(format!("╔{:═^w$}╗", " Commands ", w = W)))?;
            let qline: String = format!(" > {}_", query).chars().take(W).collect();
            queue!(out, MoveTo(bx, by + 1), Print(format!("║{:<w$}║", qline, w = W)))?;
            for i in 0..ROWS {
                let content = match filtered.get(i) {
                    Some((name, keys, _)) => {
                        let name: String = name.chars().take(W - 8).collect();
                        let pad = W - 2 - name.chars().count() - keys.chars().count();
                        format!(" {}{}{} ", name, " ".repeat(pad), keys)
                    }
                    None => " ".repeat(W),
                };
                queue!(out, MoveTo(bx, by + 2 + i as u16))?;
                if i == sel && !filtered.is_empty() {
                    queue!(
                        out,
                        Print('║'),
                        SetForegroundColor(Color::AnsiValue(0)),
                        SetBackgroundColor(Color::AnsiValue(7)),
                        Print(&content),
                        SetForegroundColor(fg),
                        SetBackgroundColor(bg),
                        Print('║')
                    )?;
                } else {
                    queue!(out, Print(format!("║{}║", content)))?;
                }
            }
            queue!(out, MoveTo(bx, by + 2 + ROWS as u16),
                Print(format!("╚{:═^w$}╝", " ↑↓ select · Enter run · Esc close ", w = W)))?;
            queue!(out, ResetColor)?;
            out.flush()?;

            match read_input()? {
                Event::Key(k) => match k.code {
                    KeyCode::Esc => return Ok(None),
                    KeyCode::Enter => return Ok(filtered.get(sel).map(|t| t.2)),
                    KeyCode::Up => sel = sel.saturating_sub(1),
                    KeyCode::Down => {
                        if sel + 1 < filtered.len().min(ROWS) {
                            sel += 1;
                        }
                    }
                    KeyCode::Backspace => {
                        query.pop();
                        sel = 0;
                    }
                    KeyCode::Char('k') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(None); // Ctrl-K again closes
                    }
                    KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                        query.push(c);
                        sel = 0;
                    }
                    _ => {}
                },
                Event::Mouse(m) => match m.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        let (c, r) = (m.column, m.row);
                        let list_top = by + 2;
                        let in_box = (bx..bx + W as u16 + 2).contains(&c)
                            && (by..=by + 2 + ROWS as u16).contains(&r);
                        if (list_top..list_top + ROWS as u16).contains(&r) && in_box {
                            let i = (r - list_top) as usize;
                            if i < filtered.len() {
                                return Ok(Some(filtered[i].2)); // click runs it
                            }
                        } else if !in_box {
                            return Ok(None); // click away closes
                        }
                    }
                    MouseEventKind::ScrollUp => sel = sel.saturating_sub(1),
                    MouseEventKind::ScrollDown => {
                        if sel + 1 < filtered.len().min(ROWS) {
                            sel += 1;
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    /// Full CP437 character grid (Ctrl-E). Enter inserts at the cursor.
    fn char_picker(&mut self, out: &mut io::Stdout) -> io::Result<()> {
        let mut sel: u8 = 0xB0; // start on ░
        loop {
            let bx = 6u16;
            let by = 2u16;
            let fg = Color::AnsiValue(15);
            let bg = Color::AnsiValue(4);
            queue!(out, Hide, SetForegroundColor(fg), SetBackgroundColor(bg))?;
            queue!(out, MoveTo(bx, by), Print(format!("╔{:═^32}╗", " Characters ")))?;
            for r in 0..8u16 {
                queue!(out, MoveTo(bx, by + 1 + r), Print('║'))?;
                for c in 0..32u16 {
                    let b = (r * 32 + c) as u8;
                    let ch = cp437::byte_to_char(b);
                    if b == sel {
                        queue!(
                            out,
                            SetForegroundColor(Color::AnsiValue(0)),
                            SetBackgroundColor(Color::AnsiValue(7)),
                            Print(ch),
                            SetForegroundColor(fg),
                            SetBackgroundColor(bg)
                        )?;
                    } else {
                        queue!(out, Print(ch))?;
                    }
                }
                queue!(out, Print('║'))?;
            }
            let info = format!(
                " {} {:#04X} ({:>3}) · Enter insert · Esc ",
                cp437::byte_to_char(sel),
                sel,
                sel
            );
            queue!(out, MoveTo(bx, by + 9), Print(format!("╚{:═^32}╝", info)), ResetColor)?;
            out.flush()?;

            match read_input()? {
                Event::Key(k) => match k.code {
                    KeyCode::Esc => return Ok(()),
                    KeyCode::Enter => {
                        self.put_char(cp437::byte_to_char(sel));
                        return Ok(());
                    }
                    KeyCode::Left => sel = sel.wrapping_sub(1),
                    KeyCode::Right => sel = sel.wrapping_add(1),
                    KeyCode::Up => sel = sel.wrapping_sub(32),
                    KeyCode::Down => sel = sel.wrapping_add(32),
                    _ => {}
                },
                Event::Mouse(m) => {
                    if let MouseEventKind::Down(MouseButton::Left) = m.kind {
                        let (c, r) = (m.column, m.row);
                        let in_grid = (bx + 1..bx + 33).contains(&c) && (by + 1..by + 9).contains(&r);
                        if in_grid {
                            let b = ((r - by - 1) * 32 + (c - bx - 1)) as u8;
                            if b == sel {
                                // second click on the same glyph inserts it
                                self.put_char(cp437::byte_to_char(sel));
                                return Ok(());
                            }
                            sel = b;
                        } else {
                            return Ok(()); // click away closes
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// One-line text prompt on the status bar. Esc cancels.
    fn prompt(&mut self, out: &mut io::Stdout, label: &str, initial: &str) -> io::Result<Option<String>> {
        let mut s = String::from(initial);
        loop {
            queue!(
                out,
                MoveTo(0, 0),
                SetForegroundColor(Color::AnsiValue(0)),
                SetBackgroundColor(Color::AnsiValue(7)),
                Print(format!(" {}{}", label, s)),
                Clear(ClearType::UntilNewLine),
                ResetColor,
                Show,
            )?;
            out.flush()?;
            let k = read_key()?;
            match k.code {
                KeyCode::Enter => {
                    let t = s.trim().to_string();
                    return Ok(if t.is_empty() { None } else { Some(t) });
                }
                KeyCode::Esc => return Ok(None),
                KeyCode::Backspace => {
                    s.pop();
                }
                KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => s.push(c),
                _ => {}
            }
        }
    }

    fn save_flow(&mut self, out: &mut io::Stdout) -> io::Result<()> {
        let guess = self
            .filename
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled.ans".into());
        let Some(name) = self.prompt(out, "Save as: ", &guess)? else {
            self.msg = Some("Save cancelled".into());
            return Ok(());
        };
        let mut path = PathBuf::from(name);
        if path.extension().is_none() {
            path.set_extension("ans");
        }
        let meta = if self.sauce_on { Some(&self.sauce) } else { None };
        match ansi::save(&self.canvas, &path, meta) {
            Ok(()) => {
                self.msg = Some(format!(
                    "Saved {}{}",
                    path.display(),
                    if self.sauce_on { " (with SAUCE)" } else { "" }
                ));
                self.filename = Some(path);
                self.dirty = false;
            }
            Err(e) => self.msg = Some(format!("Save failed: {e}")),
        }
        Ok(())
    }

    fn load_flow(&mut self, out: &mut io::Stdout) -> io::Result<()> {
        if self.dirty && !self.confirm(out, "Discard unsaved changes? (y/n) ")? {
            return Ok(());
        }
        let Some(name) = self.prompt(out, "Load file: ", "")? else {
            return Ok(());
        };
        let path = PathBuf::from(name);
        match ansi::load(&path) {
            Ok(c) => {
                let meta = sauce::read(&path).unwrap_or(None);
                self.sauce_on = meta.is_some();
                self.sauce = meta.unwrap_or_default();
                self.canvas = c;
                self.filename = Some(path);
                self.cur_x = 0;
                self.cur_y = 0;
                self.top = 0;
                self.dirty = false;
                self.undo.clear();
                self.redo.clear();
                self.mode = Mode::Edit;
                self.msg = Some("Loaded".into());
            }
            Err(e) => self.msg = Some(format!("Load failed: {e}")),
        }
        Ok(())
    }

    fn clear_flow(&mut self, out: &mut io::Stdout) -> io::Result<()> {
        if self.confirm(out, "Clear the whole page? (y/n) ")? {
            self.push_undo();
            self.canvas = Canvas::new(25);
            self.cur_x = 0;
            self.cur_y = 0;
            self.top = 0;
            self.msg = Some("Page cleared".into());
        }
        Ok(())
    }

    fn exit_flow(&mut self, out: &mut io::Stdout) -> io::Result<bool> {
        if !self.dirty {
            return Ok(true);
        }
        loop {
            queue!(
                out,
                MoveTo(0, 0),
                SetForegroundColor(Color::AnsiValue(0)),
                SetBackgroundColor(Color::AnsiValue(7)),
                Print(" Save before exiting? (y/n, Esc cancels) "),
                Clear(ClearType::UntilNewLine),
                ResetColor,
            )?;
            out.flush()?;
            match read_key()?.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.save_flow(out)?;
                    return Ok(!self.dirty);
                }
                KeyCode::Char('n') | KeyCode::Char('N') => return Ok(true),
                KeyCode::Esc => return Ok(false),
                _ => {}
            }
        }
    }

    fn confirm(&mut self, out: &mut io::Stdout, q: &str) -> io::Result<bool> {
        queue!(
            out,
            MoveTo(0, 0),
            SetForegroundColor(Color::AnsiValue(0)),
            SetBackgroundColor(Color::AnsiValue(7)),
            Print(format!(" {}", q)),
            Clear(ClearType::UntilNewLine),
            ResetColor,
        )?;
        out.flush()?;
        loop {
            match read_key()?.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => return Ok(true),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => return Ok(false),
                _ => {}
            }
        }
    }

    fn help(&mut self, out: &mut io::Stdout) -> io::Result<()> {
        const LINES: &[&str] = &[
            "╔══════════════════════ ansidraw ── keys ══════════════════════╗",
            "║ The two keys to remember:                                    ║",
            "║   Ctrl-K        command palette — every command, filterable  ║",
            "║   Ctrl-E        character picker — all 256 CP437 glyphs      ║",
            "║ Drawing                                                      ║",
            "║   F1-F10        draw glyph from the active character set     ║",
            "║   Alt-F1..F10   choose set 1-10    Ctrl-F1..F5  set 11-15    ║",
            "║   Ctrl-N/P      next / previous character set                ║",
            "║   type          any key draws with the current color         ║",
            "║ Color                                                        ║",
            "║   Esc / Ctrl-A  quick palette                                ║",
            "║   Ctrl-Up/Down  foreground     Ctrl-Left/Right  background   ║",
            "║   Alt-U         pick up the color under the cursor           ║",
            "║ Blocks (Alt-B or Ctrl-B)                                     ║",
            "║   move to size, then C copy  M move  F fill  E erase         ║",
            "║   X/Y flip horizontally/vertically, Esc leaves block mode    ║",
            "║   stamping: Enter stamps at cursor, repeat as desired        ║",
            "║ Lines                                                        ║",
            "║   Ctrl-T/Alt-I insert line   Ctrl-Y delete line              ║",
            "║   Insert/Delete shift the line right/left                    ║",
            "║ Files                                                        ║",
            "║   Alt-S/Ctrl-S save .ANS     Alt-L/Ctrl-O load               ║",
            "║   Alt-D/Ctrl-D SAUCE info (title/author/group)               ║",
            "║   Alt-C clear page           Alt-X/Ctrl-Q exit               ║",
            "║ Undo:  Alt-R/Ctrl-Z undo     Ctrl-R redo                     ║",
            "║ View:  Ctrl-W toggle the sidebar                             ║",
            "║ Mouse: click moves cursor · drag selects a block · wheel     ║",
            "║   scrolls · click sidebar colors/glyphs/commands · click     ║",
            "║   stamps a carried block · popups are clickable too          ║",
            "╚═══════════════════ press any key to close ═══════════════════╝",
        ];
        for (i, line) in LINES.iter().enumerate() {
            queue!(
                out,
                MoveTo(4, (i + 1) as u16),
                SetForegroundColor(Color::AnsiValue(15)),
                SetBackgroundColor(Color::AnsiValue(4)),
                Print(line),
            )?;
        }
        queue!(out, ResetColor)?;
        out.flush()?;
        read_key()?;
        Ok(())
    }
}
