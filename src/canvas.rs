pub const WIDTH: usize = 80;
pub const MAX_ROWS: usize = 1000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    pub ch: char,
    pub fg: u8, // 0-15
    pub bg: u8, // 0-7
}

impl Default for Cell {
    fn default() -> Self {
        Cell { ch: ' ', fg: 7, bg: 0 }
    }
}

impl Cell {
    pub fn is_blank(&self) -> bool {
        self.ch == ' ' && self.bg == 0
    }
}

#[derive(Clone)]
pub struct Canvas {
    pub rows: Vec<Vec<Cell>>,
}

impl Canvas {
    pub fn new(rows: usize) -> Self {
        Canvas {
            rows: vec![vec![Cell::default(); WIDTH]; rows.max(1)],
        }
    }

    pub fn ensure_row(&mut self, y: usize) {
        while self.rows.len() <= y && self.rows.len() < MAX_ROWS {
            self.rows.push(vec![Cell::default(); WIDTH]);
        }
    }

    pub fn get(&self, x: usize, y: usize) -> Cell {
        self.rows
            .get(y)
            .and_then(|r| r.get(x))
            .copied()
            .unwrap_or_default()
    }

    pub fn set(&mut self, x: usize, y: usize, c: Cell) {
        if x >= WIDTH || y >= MAX_ROWS {
            return;
        }
        self.ensure_row(y);
        if y < self.rows.len() {
            self.rows[y][x] = c;
        }
    }

    pub fn insert_line(&mut self, y: usize) {
        if y <= self.rows.len() {
            self.rows.insert(y, vec![Cell::default(); WIDTH]);
            if self.rows.len() > MAX_ROWS {
                self.rows.pop();
            }
        }
    }

    pub fn delete_line(&mut self, y: usize) {
        if y < self.rows.len() {
            self.rows.remove(y);
        }
        if self.rows.is_empty() {
            self.rows.push(vec![Cell::default(); WIDTH]);
        }
    }

    /// Shift the row right of x (inclusive) one cell to the left; last cell blanked.
    pub fn shift_left(&mut self, x: usize, y: usize) {
        if y >= self.rows.len() || x >= WIDTH {
            return;
        }
        self.rows[y].remove(x);
        self.rows[y].push(Cell::default());
    }

    /// Shift the row right of x (inclusive) one cell to the right; last cell lost.
    pub fn shift_right(&mut self, x: usize, y: usize) {
        if y >= self.rows.len() || x >= WIDTH {
            return;
        }
        self.rows[y].pop();
        self.rows[y].insert(x, Cell::default());
    }

    /// Index of the last row containing any non-blank cell.
    pub fn last_used_row(&self) -> usize {
        self.rows
            .iter()
            .rposition(|r| r.iter().any(|c| !c.is_blank()))
            .unwrap_or(0)
    }

    /// First and last non-blank columns of a row, if any.
    pub fn line_bounds(&self, y: usize) -> Option<(usize, usize)> {
        let row = self.rows.get(y)?;
        let first = row.iter().position(|c| !c.is_blank())?;
        let last = row.iter().rposition(|c| !c.is_blank())?;
        Some((first, last))
    }
}
