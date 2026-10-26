//! VT100/vte state adapter and immutable, UI-independent cell snapshots.

/// Terminal color: default theme, ANSI 16/256 index, or 24-bit RGB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalColor {
    /// Consumer's default foreground/background.
    Default,
    /// ANSI palette index, including the 6×6×6 cube and grayscale.
    Indexed(u8),
    /// 24-bit RGB color.
    Rgb(u8, u8, u8),
}
impl From<vt100::Color> for TerminalColor {
    fn from(color: vt100::Color) -> Self {
        match color {
            vt100::Color::Default => Self::Default,
            vt100::Color::Idx(i) => Self::Indexed(i),
            vt100::Color::Rgb(r, g, b) => Self::Rgb(r, g, b),
        }
    }
}
/// Formatting flags; independent SGR attributes may be enabled together.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Independent terminal SGR attributes"
)]
pub struct CellStyle {
    /// Bold/intense text.
    pub bold: bool,
    /// Reduced intensity.
    pub dim: bool,
    /// Italic glyphs.
    pub italic: bool,
    /// Underline.
    pub underline: bool,
    /// Swap foreground and background.
    pub inverse: bool,
}
/// One display cell. Wide glyphs occupy two columns; combining marks remain in text.
#[derive(Clone, Debug)]
pub struct TerminalCell {
    /// Full Unicode cell contents, including combining scalars.
    pub text: String,
    /// First scalar, or space in an empty/continuation cell.
    pub character: char,
    /// Foreground color.
    pub foreground: TerminalColor,
    /// Background color.
    pub background: TerminalColor,
    /// SGR flags.
    pub style: CellStyle,
    /// Wide glyph starts here.
    pub wide: bool,
    /// Second display column of a wide glyph; do not paint its text twice.
    pub continuation: bool,
}
/// Immutable visible screen snapshot, flattened in row-major order.
#[derive(Clone, Debug)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Independent negotiated terminal modes"
)]
pub struct TerminalGrid {
    /// Visible row count.
    pub rows: u16,
    /// Visible column count.
    pub cols: u16,
    /// Row-major cells; exactly rows × cols entries.
    pub cells: Vec<TerminalCell>,
    /// Zero-based live cursor row and column.
    pub cursor: (u16, u16),
    /// Cursor is painted only in the live viewport.
    pub cursor_visible: bool,
    /// Full-screen applications use a separate grid without scrollback.
    pub alternate: bool,
    /// SS3 cursor keys instead of CSI cursor keys.
    pub application_cursor: bool,
    /// Shell/application requests bracketed paste delimiters.
    pub bracketed_paste: bool,
    /// Actual scrollback offset, clamped to available history (zero is live).
    pub scrollback: usize,
    /// OSC window title, capped at 256 Unicode scalars.
    pub title: String,
}
impl TerminalGrid {
    /// Gets a visible cell; invalid coordinates return None.
    #[must_use]
    pub fn cell(&self, row: u16, col: u16) -> Option<&TerminalCell> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        self.cells
            .get(usize::from(row) * usize::from(self.cols) + usize::from(col))
    }
}
/// Parser and terminal state; only background workers mutate this object.
pub struct Emulator {
    parser: vt100::Parser<Callbacks>,
}
impl Emulator {
    /// Creates a bounded screen and 3000-line scrollback.
    #[must_use]
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new_with_callbacks(
                rows.clamp(1, 200),
                cols.clamp(1, 500),
                3000,
                Callbacks::default(),
            ),
        }
    }
    /// Processes fragmented UTF-8/ANSI bytes. Returns bounded terminal query replies.
    pub fn process(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.parser.process(bytes);
        std::mem::take(&mut self.parser.callbacks_mut().replies)
    }
    /// Changes geometry, retaining the parser's screen/history semantics.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser
            .screen_mut()
            .set_size(rows.clamp(1, 200), cols.clamp(1, 500));
    }
    /// Positive deltas move into history; negative deltas approach live output.
    pub fn scroll(&mut self, delta: i32) {
        if self.parser.screen().alternate_screen() {
            return;
        }
        let next = self
            .parser
            .screen()
            .scrollback()
            .saturating_add_signed(delta as isize);
        self.parser.screen_mut().set_scrollback(next);
    }
    /// Returns to the live bottom of the terminal before user input.
    pub fn live(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }
    /// Materializes the screen outside the UI thread.
    #[must_use]
    pub fn snapshot(&self) -> TerminalGrid {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        let mut cells = Vec::with_capacity(usize::from(rows) * usize::from(cols));
        for row in 0..rows {
            for col in 0..cols {
                if let Some(cell) = screen.cell(row, col) {
                    cells.push(TerminalCell {
                        text: cell.contents().into(),
                        character: cell.contents().chars().next().unwrap_or(' '),
                        foreground: cell.fgcolor().into(),
                        background: cell.bgcolor().into(),
                        style: CellStyle {
                            bold: cell.bold(),
                            dim: cell.dim(),
                            italic: cell.italic(),
                            underline: cell.underline(),
                            inverse: cell.inverse(),
                        },
                        wide: cell.is_wide(),
                        continuation: cell.is_wide_continuation(),
                    });
                }
            }
        }
        TerminalGrid {
            rows,
            cols,
            cells,
            cursor: (
                screen.cursor_position().0.min(rows - 1),
                screen.cursor_position().1.min(cols - 1),
            ),
            cursor_visible: !screen.hide_cursor() && screen.scrollback() == 0,
            alternate: screen.alternate_screen(),
            application_cursor: screen.application_cursor(),
            bracketed_paste: screen.bracketed_paste(),
            scrollback: screen.scrollback(),
            title: self.parser.callbacks().title.clone(),
        }
    }
}
#[derive(Default)]
struct Callbacks {
    title: String,
    replies: Vec<u8>,
}
impl vt100::Callbacks for Callbacks {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = String::from_utf8_lossy(title)
            .chars()
            .filter(|c| !c.is_control())
            .take(256)
            .collect();
    }
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        first: Option<u8>,
        _: Option<u8>,
        params: &[&[u16]],
        command: char,
    ) {
        if self.replies.len() > 4096 {
            return;
        }
        let parameter = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (first, command, parameter) {
            (None, 'n', 5) => self.replies.extend_from_slice(b"\x1b[0n"),
            (None, 'n', 6) => {
                let (r, c) = screen.cursor_position();
                let (rows, cols) = screen.size();
                let (r, c) = (r.min(rows - 1), c.min(cols - 1));
                self.replies
                    .extend_from_slice(format!("\x1b[{};{}R", r + 1, c + 1).as_bytes());
            }
            (None, 'c', 0) => self.replies.extend_from_slice(b"\x1b[?1;2c"),
            (Some(b'>'), 'c', 0) => self.replies.extend_from_slice(b"\x1b[>0;1;0c"),
            _ => {}
        }
    }
}
