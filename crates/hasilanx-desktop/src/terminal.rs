use gpui_kit::{Keystroke, Rgba, rgb};
use std::collections::VecDeque;
use unicode_width::UnicodeWidthChar;
use vte::{Params, Parser, Perform};

const MAX_SCROLLBACK: usize = 5000;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TermCell {
    pub ch: char,
    pub fg: Option<Rgba>,
    pub bg: Option<Rgba>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub hidden: bool,
    /// True if this cell holds the leading half of a 2-column CJK / Emoji glyph.
    pub wide: bool,
    /// True if this cell is the trailing spacer of a 2-column glyph.
    pub wide_spacer: bool,
}

impl Default for TermCell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: None,
            bg: None,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            strikethrough: false,
            hidden: false,
            wide: false,
            wide_spacer: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TermPoint {
    /// Absolute line index across `scrollback + grid` (or `0..rows` in alt screen).
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSelection {
    pub anchor: TermPoint,
    pub head: TermPoint,
}

impl TermSelection {
    pub fn normalized(self) -> (TermPoint, TermPoint) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.head
    }
}

#[derive(Debug, Clone, Copy)]
struct SavedCursor {
    row: usize,
    col: usize,
    fg: Option<Rgba>,
    bg: Option<Rgba>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    inverse: bool,
    hidden: bool,
    auto_wrap: bool,
}

struct SavedMainScreen {
    grid: Vec<Vec<TermCell>>,
    cursor_row: usize,
    cursor_col: usize,
    pending_wrap: bool,
    scroll_top: usize,
    scroll_bottom: usize,
}

pub struct TerminalBuffer {
    parser: Parser,
    screen: ScreenGrid,
    pub opened: bool,
    pub exited_code: Option<i32>,
    pub selection: Option<TermSelection>,
}

struct ScreenGrid {
    cols: usize,
    rows: usize,
    scrollback: VecDeque<Vec<TermCell>>,
    grid: Vec<Vec<TermCell>>,
    cursor_row: usize,
    cursor_col: usize,
    pending_wrap: bool,
    scroll_top: usize,
    scroll_bottom: usize,
    scroll_offset: usize,
    // Current SGR attributes
    fg: Option<Rgba>,
    bg: Option<Rgba>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    inverse: bool,
    hidden: bool,
    // Terminal modes
    auto_wrap: bool,
    cursor_visible: bool,
    app_cursor_mode: bool,
    bracketed_paste: bool,
    in_alt_screen: bool,
    saved_main: Option<SavedMainScreen>,
    saved_cursor: Option<SavedCursor>,
    // OSC metadata
    title: Option<String>,
    cwd: Option<String>,
    // Responses to terminal queries (e.g. CSI 6n cursor position report)
    pending_replies: Vec<Vec<u8>>,
}

impl Default for TerminalBuffer {
    fn default() -> Self {
        Self::with_size(100, 24)
    }
}

impl TerminalBuffer {
    pub fn with_size(cols: usize, rows: usize) -> Self {
        let cols = cols.max(20);
        let rows = rows.max(4);
        Self {
            parser: Parser::new(),
            screen: ScreenGrid {
                cols,
                rows,
                scrollback: VecDeque::new(),
                grid: vec![vec![TermCell::default(); cols]; rows],
                cursor_row: 0,
                cursor_col: 0,
                pending_wrap: false,
                scroll_top: 0,
                scroll_bottom: rows - 1,
                scroll_offset: 0,
                fg: None,
                bg: None,
                bold: false,
                dim: false,
                italic: false,
                underline: false,
                strikethrough: false,
                inverse: false,
                hidden: false,
                auto_wrap: true,
                cursor_visible: true,
                app_cursor_mode: false,
                bracketed_paste: false,
                in_alt_screen: false,
                saved_main: None,
                saved_cursor: None,
                title: None,
                cwd: None,
                pending_replies: Vec::new(),
            },
            opened: false,
            exited_code: None,
            selection: None,
        }
    }

    pub fn cols(&self) -> usize {
        self.screen.cols
    }

    pub fn rows(&self) -> usize {
        self.screen.rows
    }

    pub fn title(&self) -> Option<&str> {
        self.screen.title.as_deref()
    }

    pub fn cwd(&self) -> Option<&str> {
        self.screen.cwd.as_deref()
    }

    pub fn in_alt_screen(&self) -> bool {
        self.screen.in_alt_screen
    }

    #[cfg(test)]
    pub fn cursor_visible(&self) -> bool {
        self.screen.cursor_visible
    }

    pub fn app_cursor_mode(&self) -> bool {
        self.screen.app_cursor_mode
    }

    pub fn bracketed_paste(&self) -> bool {
        self.screen.bracketed_paste
    }

    pub fn take_replies(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.screen.pending_replies)
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.opened = true;
        self.exited_code = None;
        self.parser.advance(&mut self.screen, bytes);
    }

    pub fn resize(&mut self, cols: usize, rows: usize) -> bool {
        let cols = cols.max(20);
        let rows = rows.max(4);
        if cols == self.screen.cols && rows == self.screen.rows {
            return false;
        }
        self.screen.resize(cols, rows);
        self.selection = None;
        true
    }

    pub fn clear(&mut self) {
        self.screen.scrollback.clear();
        self.screen.grid = vec![vec![TermCell::default(); self.screen.cols]; self.screen.rows];
        self.screen.cursor_row = 0;
        self.screen.cursor_col = 0;
        self.screen.pending_wrap = false;
        self.screen.scroll_offset = 0;
        self.selection = None;
    }

    pub fn scroll_offset(&self) -> usize {
        if self.screen.in_alt_screen {
            0
        } else {
            self.screen.scroll_offset.min(self.max_scroll_offset())
        }
    }

    pub fn max_scroll_offset(&self) -> usize {
        if self.screen.in_alt_screen {
            0
        } else {
            self.screen.scrollback.len()
        }
    }

    pub fn total_lines(&self) -> usize {
        if self.screen.in_alt_screen {
            self.screen.rows
        } else {
            self.screen.scrollback.len() + self.screen.rows
        }
    }

    /// Positive `delta_lines` scrolls up into scrollback history; negative scrolls down toward live bottom.
    pub fn scroll_by(&mut self, delta_lines: isize) {
        if self.screen.in_alt_screen {
            self.screen.scroll_offset = 0;
            return;
        }
        let max_off = self.max_scroll_offset() as isize;
        let next = (self.screen.scroll_offset as isize + delta_lines).clamp(0, max_off);
        self.screen.scroll_offset = next as usize;
    }

    pub fn scroll_to_bottom(&mut self) {
        self.screen.scroll_offset = 0;
    }

    pub fn scroll_to_top(&mut self) {
        self.screen.scroll_offset = self.max_scroll_offset();
    }

    /// Returns the starting absolute line index of the current viewport.
    pub fn viewport_start_line(&self) -> usize {
        if self.screen.in_alt_screen {
            0
        } else {
            self.screen
                .scrollback
                .len()
                .saturating_sub(self.scroll_offset())
        }
    }

    /// Access row cells by absolute line index across scrollback + active grid.
    pub fn line_cells(&self, abs_line: usize) -> Option<&[TermCell]> {
        if self.screen.in_alt_screen {
            self.screen.grid.get(abs_line).map(Vec::as_slice)
        } else {
            let sb_len = self.screen.scrollback.len();
            if abs_line < sb_len {
                self.screen.scrollback.get(abs_line).map(Vec::as_slice)
            } else {
                self.screen.grid.get(abs_line - sb_len).map(Vec::as_slice)
            }
        }
    }

    /// Returns viewport cursor `(viewport_row, col)` if visible inside current viewport.
    pub fn viewport_cursor(&self) -> Option<(usize, usize)> {
        if !self.screen.cursor_visible {
            return None;
        }
        let abs_cursor_line = if self.screen.in_alt_screen {
            self.screen.cursor_row
        } else {
            self.screen.scrollback.len() + self.screen.cursor_row
        };
        let start = self.viewport_start_line();
        if abs_cursor_line >= start && abs_cursor_line < start + self.screen.rows {
            Some((
                abs_cursor_line - start,
                self.screen.cursor_col.min(self.screen.cols - 1),
            ))
        } else {
            None
        }
    }

    pub fn has_selection(&self) -> bool {
        self.selection.is_some_and(|s| !s.is_empty())
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    pub fn select_word_at(&mut self, abs_line: usize, col: usize) {
        let Some(cells) = self.line_cells(abs_line) else {
            return;
        };
        if cells.is_empty() {
            return;
        }
        let col = col.min(cells.len() - 1);
        let is_word = |ch: char| {
            !ch.is_whitespace()
                && !matches!(
                    ch,
                    '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ',' | ';'
                )
        };
        if !is_word(cells[col].ch) {
            self.selection = Some(TermSelection {
                anchor: TermPoint { line: abs_line, col },
                head: TermPoint { line: abs_line, col },
            });
            return;
        }
        let mut left = col;
        while left > 0 && (cells[left - 1].wide_spacer || is_word(cells[left - 1].ch)) {
            left -= 1;
        }
        let mut right = col;
        while right + 1 < cells.len()
            && (cells[right + 1].wide_spacer || is_word(cells[right + 1].ch))
        {
            right += 1;
        }
        self.selection = Some(TermSelection {
            anchor: TermPoint {
                line: abs_line,
                col: left,
            },
            head: TermPoint {
                line: abs_line,
                col: right,
            },
        });
    }

    pub fn select_line_at(&mut self, abs_line: usize) {
        let Some(cells) = self.line_cells(abs_line) else {
            return;
        };
        let end_col = cells
            .iter()
            .rposition(|c| !c.wide_spacer && c.ch != ' ')
            .unwrap_or(cells.len().saturating_sub(1));
        self.selection = Some(TermSelection {
            anchor: TermPoint {
                line: abs_line,
                col: 0,
            },
            head: TermPoint {
                line: abs_line,
                col: end_col,
            },
        });
    }

    pub fn selected_text(&self) -> Option<String> {
        let sel = self.selection?;
        if sel.is_empty() {
            return None;
        }
        let (start, end) = sel.normalized();
        let mut lines = Vec::new();
        for abs_line in start.line..=end.line {
            let Some(cells) = self.line_cells(abs_line) else {
                continue;
            };
            if cells.is_empty() {
                lines.push(String::new());
                continue;
            }
            let c0 = if abs_line == start.line {
                start.col.min(cells.len() - 1)
            } else {
                0
            };
            let c1 = if abs_line == end.line {
                end.col.min(cells.len() - 1)
            } else {
                cells.len() - 1
            };
            let mut s = String::new();
            for cell in &cells[c0..=c1] {
                if !cell.wide_spacer {
                    s.push(cell.ch);
                }
            }
            lines.push(s.trim_end().to_string());
        }
        let joined = lines.join("\n");
        if joined.is_empty() {
            None
        } else {
            Some(joined)
        }
    }

    /// Detect an `http://` or `https://` URL at `(abs_line, col)` for Ctrl+Click navigation.
    pub fn url_at(&self, abs_line: usize, col: usize) -> Option<(usize, usize, String)> {
        let cells = self.line_cells(abs_line)?;
        if col >= cells.len() {
            return None;
        }
        let mut text = String::with_capacity(cells.len());
        let mut col_to_byte = Vec::with_capacity(cells.len());
        let mut byte_to_col = Vec::new();
        for (idx, cell) in cells.iter().enumerate() {
            col_to_byte.push(text.len());
            if !cell.wide_spacer {
                let start_b = text.len();
                text.push(cell.ch);
                for _ in start_b..text.len() {
                    byte_to_col.push(idx);
                }
            }
        }
        let target_byte = *col_to_byte.get(col)?;
        for prefix in ["https://", "http://"] {
            let mut search_from = 0;
            while let Some(rel) = text[search_from..].find(prefix) {
                let start = search_from + rel;
                let rest = &text[start..];
                let mut end_rel = rest
                    .find(|c: char| {
                        c.is_whitespace()
                            || matches!(c, '"' | '\'' | '`' | '<' | '>' | '[' | ']' | '{' | '}')
                    })
                    .unwrap_or(rest.len());
                while end_rel > prefix.len() {
                    let tail = rest[..end_rel].chars().last().unwrap_or(' ');
                    if matches!(tail, '.' | ',' | ';' | ':' | ')' | '!' | '?') {
                        end_rel -= tail.len_utf8();
                    } else {
                        break;
                    }
                }
                let end = start + end_rel;
                if target_byte >= start && target_byte < end {
                    let c0 = *byte_to_col.get(start).unwrap_or(&0);
                    let c1 = *byte_to_col.get(end.saturating_sub(1)).unwrap_or(&c0);
                    return Some((c0, c1, rest[..end_rel].to_string()));
                }
                search_from = end.max(start + prefix.len());
            }
        }
        None
    }

    pub fn plain_text(&self) -> String {
        let mut out = Vec::new();
        if !self.screen.in_alt_screen {
            for row in &self.screen.scrollback {
                out.push(trim_row_text(row));
            }
        }
        let last_used = self.last_nonempty_grid_row();
        for row in &self.screen.grid[..=last_used] {
            out.push(trim_row_text(row));
        }
        out.join("\n")
    }

    fn last_nonempty_grid_row(&self) -> usize {
        for r in (0..self.screen.rows).rev() {
            if self.screen.grid[r]
                .iter()
                .any(|c| !c.wide_spacer && c.ch != ' ')
            {
                return r;
            }
        }
        0
    }
}

fn trim_row_text(row: &[TermCell]) -> String {
    let mut s = String::with_capacity(row.len());
    for c in row {
        if !c.wide_spacer {
            s.push(c.ch);
        }
    }
    s.trim_end().to_string()
}

impl ScreenGrid {
    fn resize(&mut self, new_cols: usize, new_rows: usize) {
        let old_rows = self.rows;
        if new_rows < old_rows && !self.in_alt_screen {
            let overflow = old_rows - new_rows;
            // Push top rows into scrollback only if cursor would fall past new_rows
            let push_count = self.cursor_row.saturating_sub(new_rows - 1).min(overflow);
            for _ in 0..push_count {
                if !self.grid.is_empty() {
                    let top = self.grid.remove(0);
                    self.push_scrollback_line(top);
                }
            }
            self.cursor_row = self.cursor_row.saturating_sub(push_count);
        }

        // Resize existing rows horizontally
        for row in &mut self.grid {
            resize_row(row, new_cols);
        }

        // Adjust row count vertically
        while self.grid.len() < new_rows {
            self.grid.push(vec![TermCell::default(); new_cols]);
        }
        self.grid.truncate(new_rows);

        self.cols = new_cols;
        self.rows = new_rows;
        self.cursor_row = self.cursor_row.min(new_rows - 1);
        self.cursor_col = self.cursor_col.min(new_cols - 1);
        self.pending_wrap = false;
        self.scroll_top = 0;
        self.scroll_bottom = new_rows - 1;
        self.scroll_offset = self.scroll_offset.min(self.scrollback.len());
    }

    fn push_scrollback_line(&mut self, line: Vec<TermCell>) {
        self.scrollback.push_back(line);
        if self.scrollback.len() > MAX_SCROLLBACK {
            self.scrollback.pop_front();
        } else if self.scroll_offset > 0 {
            // Keep viewport anchored to the same history lines while user is scrolled up
            self.scroll_offset = (self.scroll_offset + 1).min(self.scrollback.len());
        }
    }

    fn scroll_region_up(&mut self, count: usize) {
        let top = self.scroll_top.min(self.rows - 1);
        let bottom = self.scroll_bottom.min(self.rows - 1);
        if top > bottom {
            return;
        }
        let n = count.min(bottom - top + 1);
        let full_screen_scroll = top == 0 && bottom == self.rows - 1 && !self.in_alt_screen;
        for _ in 0..n {
            let removed = self.grid.remove(top);
            if full_screen_scroll {
                self.push_scrollback_line(removed);
            }
            self.grid
                .insert(bottom, vec![TermCell::default(); self.cols]);
        }
    }

    fn scroll_region_down(&mut self, count: usize) {
        let top = self.scroll_top.min(self.rows - 1);
        let bottom = self.scroll_bottom.min(self.rows - 1);
        if top > bottom {
            return;
        }
        let n = count.min(bottom - top + 1);
        for _ in 0..n {
            self.grid.remove(bottom);
            self.grid.insert(top, vec![TermCell::default(); self.cols]);
        }
    }

    fn newline(&mut self) {
        self.pending_wrap = false;
        if self.cursor_row == self.scroll_bottom {
            self.scroll_region_up(1);
        } else if self.cursor_row + 1 < self.rows {
            self.cursor_row += 1;
        }
    }

    fn reverse_index(&mut self) {
        self.pending_wrap = false;
        if self.cursor_row == self.scroll_top {
            self.scroll_region_down(1);
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
        }
    }

    fn current_cell(&self, ch: char, wide: bool) -> TermCell {
        let (fg, bg) = if self.inverse {
            (
                Some(self.bg.unwrap_or(rgb(0x0b0d12))),
                Some(self.fg.unwrap_or(rgb(0xe4e4e7))),
            )
        } else {
            (self.fg, self.bg)
        };
        TermCell {
            ch,
            fg,
            bg,
            bold: self.bold,
            dim: self.dim,
            italic: self.italic,
            underline: self.underline,
            strikethrough: self.strikethrough,
            hidden: self.hidden,
            wide,
            wide_spacer: false,
        }
    }

    fn blank_cell(&self) -> TermCell {
        TermCell {
            bg: self.bg,
            ..TermCell::default()
        }
    }

    fn clear_wide_conflict(&mut self, row: usize, col: usize) {
        if col >= self.cols {
            return;
        }
        if self.grid[row][col].wide_spacer && col > 0 {
            self.grid[row][col - 1] = TermCell::default();
            self.grid[row][col] = TermCell::default();
        } else if self.grid[row][col].wide && col + 1 < self.cols {
            self.grid[row][col] = TermCell::default();
            self.grid[row][col + 1] = TermCell::default();
        }
    }

    fn save_cursor(&mut self) {
        self.saved_cursor = Some(SavedCursor {
            row: self.cursor_row,
            col: self.cursor_col,
            fg: self.fg,
            bg: self.bg,
            bold: self.bold,
            dim: self.dim,
            italic: self.italic,
            underline: self.underline,
            strikethrough: self.strikethrough,
            inverse: self.inverse,
            hidden: self.hidden,
            auto_wrap: self.auto_wrap,
        });
    }

    fn restore_cursor(&mut self) {
        if let Some(sc) = self.saved_cursor {
            self.cursor_row = sc.row.min(self.rows - 1);
            self.cursor_col = sc.col.min(self.cols - 1);
            self.pending_wrap = false;
            self.fg = sc.fg;
            self.bg = sc.bg;
            self.bold = sc.bold;
            self.dim = sc.dim;
            self.italic = sc.italic;
            self.underline = sc.underline;
            self.strikethrough = sc.strikethrough;
            self.inverse = sc.inverse;
            self.hidden = sc.hidden;
            self.auto_wrap = sc.auto_wrap;
        }
    }

    fn enter_alt_screen(&mut self, save_cursor: bool) {
        if save_cursor {
            self.save_cursor();
        }
        if self.in_alt_screen {
            return;
        }
        self.saved_main = Some(SavedMainScreen {
            grid: std::mem::replace(&mut self.grid, vec![vec![TermCell::default(); self.cols]; self.rows]),
            cursor_row: self.cursor_row,
            cursor_col: self.cursor_col,
            pending_wrap: self.pending_wrap,
            scroll_top: self.scroll_top,
            scroll_bottom: self.scroll_bottom,
        });
        self.in_alt_screen = true;
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.pending_wrap = false;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
        self.scroll_offset = 0;
    }

    fn leave_alt_screen(&mut self, restore_cursor: bool) {
        if !self.in_alt_screen {
            return;
        }
        if let Some(mut saved) = self.saved_main.take() {
            for row in &mut saved.grid {
                resize_row(row, self.cols);
            }
            while saved.grid.len() < self.rows {
                saved.grid.push(vec![TermCell::default(); self.cols]);
            }
            saved.grid.truncate(self.rows);
            self.grid = saved.grid;
            self.cursor_row = saved.cursor_row.min(self.rows - 1);
            self.cursor_col = saved.cursor_col.min(self.cols - 1);
            self.pending_wrap = saved.pending_wrap;
            self.scroll_top = saved.scroll_top.min(self.rows - 1);
            self.scroll_bottom = saved.scroll_bottom.min(self.rows - 1);
        }
        self.in_alt_screen = false;
        if restore_cursor {
            self.restore_cursor();
        }
    }

    fn reset_all(&mut self) {
        self.in_alt_screen = false;
        self.saved_main = None;
        self.saved_cursor = None;
        self.grid = vec![vec![TermCell::default(); self.cols]; self.rows];
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.pending_wrap = false;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
        self.scroll_offset = 0;
        self.fg = None;
        self.bg = None;
        self.bold = false;
        self.dim = false;
        self.italic = false;
        self.underline = false;
        self.strikethrough = false;
        self.inverse = false;
        self.hidden = false;
        self.auto_wrap = true;
        self.cursor_visible = true;
        self.app_cursor_mode = false;
        self.bracketed_paste = false;
    }
}

fn resize_row(row: &mut Vec<TermCell>, new_cols: usize) {
    row.resize(new_cols, TermCell::default());
    // Never leave a dangling wide leading cell at the last column
    if let Some(last) = row.last_mut() {
        if last.wide {
            *last = TermCell::default();
        }
    }
}

impl Perform for ScreenGrid {
    fn print(&mut self, c: char) {
        let width = UnicodeWidthChar::width(c).unwrap_or(0);
        if width == 0 {
            return;
        }
        if self.pending_wrap {
            if self.auto_wrap {
                self.cursor_col = 0;
                self.newline();
            } else {
                self.cursor_col = self.cols - 1;
                self.pending_wrap = false;
            }
        }

        if width == 2 && self.cursor_col + 1 >= self.cols {
            if self.auto_wrap {
                // Fill last column with blank before wrapping wide char onto next line
                self.clear_wide_conflict(self.cursor_row, self.cursor_col);
                self.grid[self.cursor_row][self.cursor_col] = self.blank_cell();
                self.cursor_col = 0;
                self.newline();
            } else {
                return;
            }
        }

        let row = self.cursor_row;
        let col = self.cursor_col;
        self.clear_wide_conflict(row, col);
        if width == 2 {
            self.clear_wide_conflict(row, col + 1);
            let cell = self.current_cell(c, true);
            let mut spacer = cell;
            spacer.ch = ' ';
            spacer.wide = false;
            spacer.wide_spacer = true;
            self.grid[row][col] = cell;
            if col + 1 < self.cols {
                self.grid[row][col + 1] = spacer;
            }
            if col + 2 >= self.cols {
                self.cursor_col = self.cols - 1;
                self.pending_wrap = true;
            } else {
                self.cursor_col = col + 2;
            }
        } else {
            let cell = self.current_cell(c, false);
            self.grid[row][col] = cell;
            if col + 1 >= self.cols {
                self.pending_wrap = true;
            } else {
                self.cursor_col = col + 1;
            }
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b | 0x0c => self.newline(),
            b'\r' => {
                self.cursor_col = 0;
                self.pending_wrap = false;
            }
            0x08 => {
                self.pending_wrap = false;
                self.cursor_col = self.cursor_col.saturating_sub(1);
            }
            b'\t' => {
                self.pending_wrap = false;
                let next_tab = ((self.cursor_col / 8) + 1) * 8;
                self.cursor_col = next_tab.min(self.cols - 1);
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        if !intermediates.is_empty() {
            return;
        }
        match byte {
            b'7' => self.save_cursor(),
            b'8' => self.restore_cursor(),
            b'D' => self.newline(),
            b'E' => {
                self.cursor_col = 0;
                self.newline();
            }
            b'M' => self.reverse_index(),
            b'c' => self.reset_all(),
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.is_empty() {
            return;
        }
        let code = std::str::from_utf8(params[0]).unwrap_or_default();
        match code {
            "0" | "2" => {
                if let Some(raw_title) = params.get(1) {
                    let t = String::from_utf8_lossy(raw_title).trim().to_string();
                    self.title = (!t.is_empty()).then_some(t);
                }
            }
            "7" => {
                if let Some(raw_uri) = params.get(1) {
                    let uri = String::from_utf8_lossy(raw_uri);
                    if let Some(path) = parse_osc7_path(&uri) {
                        self.cwd = Some(path);
                    }
                }
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        let is_private = intermediates == b"?";
        let groups: Vec<Vec<u16>> = params.iter().map(|p| p.to_vec()).collect();
        let p = |idx: usize| -> usize {
            groups
                .get(idx)
                .and_then(|g| g.first().copied())
                .unwrap_or(0) as usize
        };

        if is_private {
            let enable = match action {
                'h' => true,
                'l' => false,
                _ => return,
            };
            for g in &groups {
                let mode = g.first().copied().unwrap_or(0);
                match mode {
                    1 => self.app_cursor_mode = enable,
                    7 => self.auto_wrap = enable,
                    25 => self.cursor_visible = enable,
                    47 | 1047 => {
                        if enable {
                            self.enter_alt_screen(false);
                        } else {
                            self.leave_alt_screen(false);
                        }
                    }
                    1049 => {
                        if enable {
                            self.enter_alt_screen(true);
                        } else {
                            self.leave_alt_screen(true);
                        }
                    }
                    2004 => self.bracketed_paste = enable,
                    _ => {}
                }
            }
            return;
        }

        match action {
            'H' | 'f' => {
                let r = p(0).saturating_sub(1).min(self.rows - 1);
                let c = p(1).saturating_sub(1).min(self.cols - 1);
                self.cursor_row = r;
                self.cursor_col = c;
                self.pending_wrap = false;
            }
            'A' => {
                self.pending_wrap = false;
                let n = p(0).max(1);
                self.cursor_row = self.cursor_row.saturating_sub(n);
            }
            'B' => {
                self.pending_wrap = false;
                let n = p(0).max(1);
                self.cursor_row = (self.cursor_row + n).min(self.rows - 1);
            }
            'C' => {
                self.pending_wrap = false;
                let n = p(0).max(1);
                self.cursor_col = (self.cursor_col + n).min(self.cols - 1);
            }
            'D' => {
                self.pending_wrap = false;
                let n = p(0).max(1);
                self.cursor_col = self.cursor_col.saturating_sub(n);
            }
            'E' => {
                self.pending_wrap = false;
                let n = p(0).max(1);
                self.cursor_row = (self.cursor_row + n).min(self.rows - 1);
                self.cursor_col = 0;
            }
            'F' => {
                self.pending_wrap = false;
                let n = p(0).max(1);
                self.cursor_row = self.cursor_row.saturating_sub(n);
                self.cursor_col = 0;
            }
            'G' | '`' => {
                self.pending_wrap = false;
                self.cursor_col = p(0).saturating_sub(1).min(self.cols - 1);
            }
            'd' => {
                self.pending_wrap = false;
                self.cursor_row = p(0).saturating_sub(1).min(self.rows - 1);
            }
            's' => self.save_cursor(),
            'u' => self.restore_cursor(),
            'r' => {
                let top = p(0).saturating_sub(1);
                let bottom = if p(1) == 0 {
                    self.rows - 1
                } else {
                    p(1).saturating_sub(1).min(self.rows - 1)
                };
                if top < bottom {
                    self.scroll_top = top;
                    self.scroll_bottom = bottom;
                } else {
                    self.scroll_top = 0;
                    self.scroll_bottom = self.rows - 1;
                }
                self.cursor_row = 0;
                self.cursor_col = 0;
                self.pending_wrap = false;
            }
            'S' => {
                self.scroll_region_up(p(0).max(1));
            }
            'T' => {
                self.scroll_region_down(p(0).max(1));
            }
            'L' => {
                let n = p(0).max(1);
                if self.cursor_row >= self.scroll_top && self.cursor_row <= self.scroll_bottom {
                    let count = n.min(self.scroll_bottom - self.cursor_row + 1);
                    for _ in 0..count {
                        self.grid.remove(self.scroll_bottom);
                        self.grid
                            .insert(self.cursor_row, vec![self.blank_cell(); self.cols]);
                    }
                }
            }
            'M' => {
                let n = p(0).max(1);
                if self.cursor_row >= self.scroll_top && self.cursor_row <= self.scroll_bottom {
                    let count = n.min(self.scroll_bottom - self.cursor_row + 1);
                    for _ in 0..count {
                        self.grid.remove(self.cursor_row);
                        self.grid
                            .insert(self.scroll_bottom, vec![self.blank_cell(); self.cols]);
                    }
                }
            }
            'K' => {
                let blank = self.blank_cell();
                let row = &mut self.grid[self.cursor_row];
                match p(0) {
                    0 => {
                        for cell in row.iter_mut().skip(self.cursor_col) {
                            *cell = blank;
                        }
                    }
                    1 => {
                        let end = self.cursor_col.min(self.cols - 1);
                        for cell in row.iter_mut().take(end + 1) {
                            *cell = blank;
                        }
                    }
                    2 => {
                        for cell in row.iter_mut() {
                            *cell = blank;
                        }
                    }
                    _ => {}
                }
            }
            'J' => {
                let blank = self.blank_cell();
                match p(0) {
                    0 => {
                        for cell in self.grid[self.cursor_row].iter_mut().skip(self.cursor_col) {
                            *cell = blank;
                        }
                        for r in (self.cursor_row + 1)..self.rows {
                            self.grid[r] = vec![blank; self.cols];
                        }
                    }
                    1 => {
                        for r in 0..self.cursor_row {
                            self.grid[r] = vec![blank; self.cols];
                        }
                        let end = self.cursor_col.min(self.cols - 1);
                        for cell in self.grid[self.cursor_row].iter_mut().take(end + 1) {
                            *cell = blank;
                        }
                    }
                    2 => {
                        for r in 0..self.rows {
                            self.grid[r] = vec![blank; self.cols];
                        }
                    }
                    3 => {
                        self.scrollback.clear();
                        self.scroll_offset = 0;
                        for r in 0..self.rows {
                            self.grid[r] = vec![blank; self.cols];
                        }
                    }
                    _ => {}
                }
            }
            'X' => {
                let n = p(0).max(1);
                let blank = self.blank_cell();
                let end = (self.cursor_col + n).min(self.cols);
                for c in self.cursor_col..end {
                    self.grid[self.cursor_row][c] = blank;
                }
            }
            'P' => {
                let n = p(0).max(1);
                let blank = self.blank_cell();
                let row = &mut self.grid[self.cursor_row];
                for c in self.cursor_col..self.cols {
                    row[c] = if c + n < self.cols {
                        row[c + n]
                    } else {
                        blank
                    };
                }
            }
            '@' => {
                let n = p(0).max(1);
                let blank = self.blank_cell();
                let row = &mut self.grid[self.cursor_row];
                for c in (self.cursor_col..self.cols).rev() {
                    row[c] = if c >= self.cursor_col + n {
                        row[c - n]
                    } else {
                        blank
                    };
                }
            }
            'n' => {
                // Device Status Report: respond to Cursor Position Report (CSI 6n)
                if p(0) == 6 {
                    let reply = format!("\x1b[{};{}R", self.cursor_row + 1, self.cursor_col + 1);
                    self.pending_replies.push(reply.into_bytes());
                } else if p(0) == 5 {
                    self.pending_replies.push(b"\x1b[0n".to_vec());
                }
            }
            'm' => {
                if groups.is_empty() {
                    self.reset_sgr();
                    return;
                }
                let mut i = 0;
                while i < groups.len() {
                    let g = &groups[i];
                    let code = g.first().copied().unwrap_or(0);
                    match code {
                        0 => self.reset_sgr(),
                        1 => self.bold = true,
                        2 => self.dim = true,
                        3 => self.italic = true,
                        4 => self.underline = true,
                        7 => self.inverse = true,
                        8 => self.hidden = true,
                        9 => self.strikethrough = true,
                        22 => {
                            self.bold = false;
                            self.dim = false;
                        }
                        23 => self.italic = false,
                        24 => self.underline = false,
                        27 => self.inverse = false,
                        28 => self.hidden = false,
                        29 => self.strikethrough = false,
                        30..=37 => self.fg = Some(ansi_16(code - 30, self.bold)),
                        39 => self.fg = None,
                        40..=47 => self.bg = Some(ansi_16(code - 40, false)),
                        49 => self.bg = None,
                        90..=97 => self.fg = Some(ansi_16(code - 90, true)),
                        100..=107 => self.bg = Some(ansi_16(code - 100, true)),
                        38 | 48 => {
                            let is_fg = code == 38;
                            // Handle colon-separated subparams (e.g. 38:5:196 or 38:2::r:g:b)
                            if g.len() >= 3 && g[1] == 5 {
                                let col = ansi_256(g[2] as u8);
                                if is_fg {
                                    self.fg = Some(col);
                                } else {
                                    self.bg = Some(col);
                                }
                            } else if g.len() >= 5 && g[1] == 2 {
                                let offset = if g.len() >= 6 { 3 } else { 2 };
                                let col = rgb_from_u16(g[offset], g[offset + 1], g[offset + 2]);
                                if is_fg {
                                    self.fg = Some(col);
                                } else {
                                    self.bg = Some(col);
                                }
                            } else if i + 2 < groups.len()
                                && groups[i + 1].first().copied() == Some(5)
                            {
                                let idx = groups[i + 2].first().copied().unwrap_or(0) as u8;
                                let col = ansi_256(idx);
                                if is_fg {
                                    self.fg = Some(col);
                                } else {
                                    self.bg = Some(col);
                                }
                                i += 2;
                            } else if i + 4 < groups.len()
                                && groups[i + 1].first().copied() == Some(2)
                            {
                                let r = groups[i + 2].first().copied().unwrap_or(0);
                                let gr = groups[i + 3].first().copied().unwrap_or(0);
                                let b = groups[i + 4].first().copied().unwrap_or(0);
                                let col = rgb_from_u16(r, gr, b);
                                if is_fg {
                                    self.fg = Some(col);
                                } else {
                                    self.bg = Some(col);
                                }
                                i += 4;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
            }
            _ => {}
        }
    }
}

impl ScreenGrid {
    fn reset_sgr(&mut self) {
        self.fg = None;
        self.bg = None;
        self.bold = false;
        self.dim = false;
        self.italic = false;
        self.underline = false;
        self.strikethrough = false;
        self.inverse = false;
        self.hidden = false;
    }
}

fn rgb_from_u16(r: u16, g: u16, b: u16) -> Rgba {
    let r = (r.min(255)) as u32;
    let g = (g.min(255)) as u32;
    let b = (b.min(255)) as u32;
    rgb((r << 16) | (g << 8) | b)
}

fn parse_osc7_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let slash = rest.find('/')?;
    let raw_path = &rest[slash..];
    Some(percent_decode(raw_path))
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex_str) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(val) = u8::from_str_radix(hex_str, 16) {
                    out.push(val);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn ansi_16(idx: u16, bright: bool) -> Rgba {
    match (idx, bright) {
        (0, false) => rgb(0x3f3f46),
        (1, false) => rgb(0xef4444),
        (2, false) => rgb(0x22c55e),
        (3, false) => rgb(0xeab308),
        (4, false) => rgb(0x3b82f6),
        (5, false) => rgb(0xa855f7),
        (6, false) => rgb(0x06b6d4),
        (7, false) => rgb(0xe4e4e7),
        (0, true) => rgb(0x71717a),
        (1, true) => rgb(0xf87171),
        (2, true) => rgb(0x4ade80),
        (3, true) => rgb(0xfacc15),
        (4, true) => rgb(0x60a5fa),
        (5, true) => rgb(0xc084fc),
        (6, true) => rgb(0x22d3ee),
        _ => rgb(0xffffff),
    }
}

fn ansi_256(code: u8) -> Rgba {
    match code {
        0..=7 => ansi_16(code as u16, false),
        8..=15 => ansi_16((code - 8) as u16, true),
        16..=231 => {
            let v = code - 16;
            let r = (v / 36) * 51;
            let g = ((v % 36) / 6) * 51;
            let b = (v % 6) * 51;
            rgb(((r as u32) << 16) | ((g as u32) << 8) | (b as u32))
        }
        232..=255 => {
            let gray = ((code - 232) as u32) * 10 + 8;
            rgb((gray << 16) | (gray << 8) | gray)
        }
    }
}

/// Format clipboard text for PTY paste, wrapping in bracketed paste markers when active.
pub fn format_paste_bytes(text: &str, bracketed_paste: bool) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed_paste {
        let mut out = Vec::with_capacity(normalized.len() + 12);
        out.extend_from_slice(b"\x1b[200~");
        out.extend_from_slice(normalized.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        normalized.into_bytes()
    }
}

fn xterm_modifier_param(ks: &Keystroke) -> u8 {
    let mut m = 1u8;
    if ks.modifiers.shift {
        m += 1;
    }
    if ks.modifiers.alt {
        m += 2;
    }
    if ks.modifiers.control {
        m += 4;
    }
    m
}

/// Convert a GPUI Keystroke into raw PTY bytes for direct interactive terminal input.
/// Returns `None` for app-level terminal shortcuts (dock toggle, copy/paste, zoom) or
/// plain printable text when `defer_printable_to_ime` is true.
pub fn keystroke_to_pty_bytes(
    ks: &Keystroke,
    app_cursor_mode: bool,
    defer_printable_to_ime: bool,
) -> Option<Vec<u8>> {
    let key = ks.key.as_str();
    let mods = &ks.modifiers;

    // Leave global dock/app shortcuts untouched
    if mods.control && !mods.alt {
        match key {
            "`" => return None,
            "j" | "d" | "t" | "o" | "c" | "v" if mods.shift => return None,
            "=" | "+" | "-" | "0" => return None,
            _ => {}
        }
    }
    if mods.platform {
        return None;
    }

    // Navigation & editing keys with xterm modifier support
    let mod_param = xterm_modifier_param(ks);
    match key {
        "enter" => return Some(if mods.alt { b"\x1b\r".to_vec() } else { b"\r".to_vec() }),
        "backspace" => {
            return Some(if mods.alt {
                vec![0x1b, 0x7f]
            } else if mods.control {
                vec![0x08]
            } else {
                vec![0x7f]
            });
        }
        "tab" => {
            return Some(if mods.shift {
                b"\x1b[Z".to_vec()
            } else {
                b"\t".to_vec()
            });
        }
        "escape" => return Some(vec![0x1b]),
        "up" | "down" | "right" | "left" => {
            let code = match key {
                "up" => 'A',
                "down" => 'B',
                "right" => 'C',
                _ => 'D',
            };
            return Some(if mod_param > 1 {
                format!("\x1b[1;{mod_param}{code}").into_bytes()
            } else if app_cursor_mode {
                format!("\x1bO{code}").into_bytes()
            } else {
                format!("\x1b[{code}").into_bytes()
            });
        }
        "home" | "end" => {
            let code = if key == "home" { 'H' } else { 'F' };
            return Some(if mod_param > 1 {
                format!("\x1b[1;{mod_param}{code}").into_bytes()
            } else if app_cursor_mode {
                format!("\x1bO{code}").into_bytes()
            } else {
                format!("\x1b[{code}").into_bytes()
            });
        }
        "insert" | "delete" | "pageup" | "pagedown" => {
            let num = match key {
                "insert" => 2,
                "delete" => 3,
                "pageup" => 5,
                _ => 6,
            };
            return Some(if mod_param > 1 {
                format!("\x1b[{num};{mod_param}~").into_bytes()
            } else {
                format!("\x1b[{num}~").into_bytes()
            });
        }
        "f1" | "f2" | "f3" | "f4" => {
            let code = match key {
                "f1" => 'P',
                "f2" => 'Q',
                "f3" => 'R',
                _ => 'S',
            };
            return Some(if mod_param > 1 {
                format!("\x1b[1;{mod_param}{code}").into_bytes()
            } else {
                format!("\x1bO{code}").into_bytes()
            });
        }
        "f5" | "f6" | "f7" | "f8" | "f9" | "f10" | "f11" | "f12" => {
            let num = match key {
                "f5" => 15,
                "f6" => 17,
                "f7" => 18,
                "f8" => 19,
                "f9" => 20,
                "f10" => 21,
                "f11" => 23,
                _ => 24,
            };
            return Some(if mod_param > 1 {
                format!("\x1b[{num};{mod_param}~").into_bytes()
            } else {
                format!("\x1b[{num}~").into_bytes()
            });
        }
        _ => {}
    }

    // Ctrl combinations
    if mods.control {
        let ctrl_byte = match key {
            "space" | "@" | "2" => Some(0x00),
            "[" | "3" => Some(0x1b),
            "\\" | "4" => Some(0x1c),
            "]" | "5" => Some(0x1d),
            "^" | "6" => Some(0x1e),
            "_" | "7" => Some(0x1f),
            k if k.len() == 1 => {
                let b = k.as_bytes()[0].to_ascii_lowercase();
                if (b'a'..=b'z').contains(&b) {
                    Some(b - b'a' + 1)
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(b) = ctrl_byte {
            return Some(if mods.alt { vec![0x1b, b] } else { vec![b] });
        }
        return None;
    }

    // Alt + char (e.g. Alt+f, Alt+b, Alt+d in readline/zsh)
    if mods.alt {
        if let Some(ref ch) = ks.key_char {
            if !ch.is_empty() {
                let mut out = vec![0x1b];
                out.extend_from_slice(ch.as_bytes());
                return Some(out);
            }
        }
        if key == "space" {
            return Some(b"\x1b ".to_vec());
        }
        if key.len() == 1 {
            let mut out = vec![0x1b];
            out.extend_from_slice(key.as_bytes());
            return Some(out);
        }
        return None;
    }

    // Plain printable text: when IME handler is active and `key_char` is present,
    // let Wayland/OS InputHandler commit via `replace_text_in_range` to prevent double-typing.
    if defer_printable_to_ime && ks.key_char.as_ref().is_some_and(|s| !s.is_empty()) {
        return None;
    }

    if key == "space" {
        return Some(b" ".to_vec());
    }
    if let Some(ref ch) = ks.key_char {
        if !ch.is_empty() {
            return Some(ch.as_bytes().to_vec());
        }
    }
    if key.len() == 1 {
        return Some(key.as_bytes().to_vec());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::Modifiers;

    #[test]
    fn ansi_colors_wide_cjk_and_plain_text() {
        let mut buf = TerminalBuffer::with_size(40, 6);
        buf.feed(b"\x1b[1;3;32mOK\x1b[0m \xe6\x97\xa5\xe6\x9c\xac\xe8\xaa\x9e\r\n\x1b[38;2;255;128;0morange\x1b[0m");

        assert_eq!(buf.plain_text(), "OK 日本語\norange");
        let row0 = buf.line_cells(0).unwrap();
        assert_eq!(row0[0].ch, 'O');
        assert!(row0[0].bold && row0[0].italic);
        assert_eq!(row0[0].fg, Some(rgb(0x4ade80)));
        // '日' at col 3 occupies cols 3 (wide) and 4 (wide_spacer)
        assert_eq!(row0[3].ch, '日');
        assert!(row0[3].wide);
        assert!(row0[4].wide_spacer);
        assert_eq!(row0[5].ch, '本');
        let row1 = buf.line_cells(1).unwrap();
        assert_eq!(row1[0].fg, Some(rgb(0xff8000)));
    }

    #[test]
    fn alt_screen_scroll_region_and_osc_metadata() {
        let mut buf = TerminalBuffer::with_size(40, 5);
        buf.feed(b"\x1b]0;cargo test\x07\x1b]7;file://host/home/user/My%20Proj\x07");
        assert_eq!(buf.title(), Some("cargo test"));
        assert_eq!(buf.cwd(), Some("/home/user/My Proj"));

        buf.feed(b"main-line\r\n");
        buf.feed(b"\x1b[?1049h\x1b[?25lALT SCREEN");
        assert!(buf.in_alt_screen());
        assert!(!buf.cursor_visible());
        assert_eq!(buf.plain_text(), "ALT SCREEN");

        buf.feed(b"\x1b[?25h\x1b[?1049l");
        assert!(!buf.in_alt_screen());
        assert!(buf.cursor_visible());
        assert_eq!(buf.plain_text(), "main-line");

        // CSI 6n replies with cursor position
        buf.feed(b"\x1b[6n");
        let replies = buf.take_replies();
        assert_eq!(replies, vec![b"\x1b[2;1R".to_vec()]);
    }

    #[test]
    fn selection_url_and_resize() {
        let mut buf = TerminalBuffer::with_size(60, 6);
        buf.feed(b"visit https://example.com/docs/api. for info\r\nsecond line");

        let url = buf.url_at(0, 10).unwrap();
        assert_eq!(url.2, "https://example.com/docs/api");

        buf.select_word_at(0, 10);
        assert_eq!(buf.selected_text().as_deref(), Some("https://example.com/docs/api."));

        buf.select_line_at(1);
        assert_eq!(buf.selected_text().as_deref(), Some("second line"));

        assert!(buf.resize(80, 10));
        assert_eq!(buf.cols(), 80);
        assert_eq!(buf.rows(), 10);
    }

    #[test]
    fn keystroke_maps_control_modifiers_and_paste() {
        let ctrl_c = Keystroke {
            modifiers: Modifiers {
                control: true,
                ..Default::default()
            },
            key: "c".into(),
            key_char: None,
        };
        assert_eq!(keystroke_to_pty_bytes(&ctrl_c, false, true), Some(vec![0x03]));

        let up_normal = Keystroke {
            modifiers: Modifiers::default(),
            key: "up".into(),
            key_char: None,
        };
        assert_eq!(
            keystroke_to_pty_bytes(&up_normal, false, true),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            keystroke_to_pty_bytes(&up_normal, true, true),
            Some(b"\x1bOA".to_vec())
        );

        let ctrl_right = Keystroke {
            modifiers: Modifiers {
                control: true,
                ..Default::default()
            },
            key: "right".into(),
            key_char: None,
        };
        assert_eq!(
            keystroke_to_pty_bytes(&ctrl_right, false, true),
            Some(b"\x1b[1;5C".to_vec())
        );

        let shift_tab = Keystroke {
            modifiers: Modifiers {
                shift: true,
                ..Default::default()
            },
            key: "tab".into(),
            key_char: None,
        };
        assert_eq!(
            keystroke_to_pty_bytes(&shift_tab, false, true),
            Some(b"\x1b[Z".to_vec())
        );

        assert_eq!(
            format_paste_bytes("a\nb", true),
            b"\x1b[200~a\rb\x1b[201~".to_vec()
        );
    }
}
