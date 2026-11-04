//! VT100/vte state adapter and immutable, UI-independent cell snapshots.

/// Negotiated xterm mouse tracking and coordinate encoding.
pub use vt100::{MouseProtocolEncoding, MouseProtocolMode};

/// Position in the bounded terminal text buffer (history followed by screen).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPosition {
    /// Zero-based physical row, including scrollback.
    pub row: usize,
    /// Zero-based cell column; end positions may equal the column count.
    pub col: u16,
}

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
    /// Rows joined to their successor by an automatic terminal wrap.
    pub wrapped_rows: Vec<bool>,
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
    /// Mouse events requested by the running program.
    pub mouse_mode: MouseProtocolMode,
    /// Encoding requested for mouse coordinates.
    pub mouse_encoding: MouseProtocolEncoding,
    /// Frozen text review; live output continues independently.
    pub reviewing: bool,
    /// Identifies selections invalidated by resize or returning to live output.
    pub selection_epoch: u64,
    /// Number of available history rows (at most 3000).
    pub history_len: usize,
    /// First visible row in the history + screen buffer.
    pub viewport_start: usize,
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
    review: Option<vt100::Screen>,
    history_len: usize,
    selection_epoch: u64,
}
impl Emulator {
    /// Creates a bounded screen and 3000-line scrollback.
    #[must_use]
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            review: None,
            history_len: 0,
            selection_epoch: 0,
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
        if self.review.is_none() {
            self.refresh_history_len();
        }
        std::mem::take(&mut self.parser.callbacks_mut().replies)
    }
    /// Takes one bounded OSC 52 clipboard write; clipboard-read queries are unsupported.
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.parser.callbacks_mut().clipboard.take()
    }
    /// Changes geometry, retaining the parser's screen/history semantics.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.live();
        self.parser
            .screen_mut()
            .set_size(rows.clamp(1, 200), cols.clamp(1, 500));
        self.refresh_history_len();
    }
    fn refresh_history_len(&mut self) {
        let screen = self.parser.screen_mut();
        screen.set_scrollback(usize::MAX);
        self.history_len = screen.scrollback();
        screen.set_scrollback(0);
    }
    /// Freezes the current terminal buffer for scrolling/selection without pausing output.
    pub fn review(&mut self) {
        if self.review.is_none() {
            self.refresh_history_len();
            self.review = Some(self.parser.screen().clone());
        }
    }
    /// Positive deltas move into a frozen normal-screen history; negative approach bottom.
    pub fn scroll(&mut self, delta: i32) {
        if self.parser.screen().alternate_screen() && self.review.is_none() {
            return;
        }
        self.review();
        if let Some(screen) = &mut self.review {
            let next = screen.scrollback().saturating_add_signed(delta as isize);
            screen.set_scrollback(next);
        }
    }
    /// Sets an absolute history offset in a frozen text view.
    pub fn scroll_to(&mut self, offset: usize) {
        self.review();
        if let Some(screen) = &mut self.review {
            screen.set_scrollback(offset);
        }
    }
    /// Returns to live output and invalidates selections from the frozen buffer.
    pub fn live(&mut self) {
        if self.review.take().is_some() {
            self.selection_epoch = self.selection_epoch.wrapping_add(1);
        }
        self.parser.screen_mut().set_scrollback(0);
        self.refresh_history_len();
    }
    /// Copies a half-open cell range, or all available terminal text. No model input.
    /// # Errors
    /// Rejects stale selections, invalid ranges and clipboard results above 4 MiB.
    pub fn copy_text(
        &mut self,
        epoch: u64,
        range: Option<(TextPosition, TextPosition)>,
    ) -> crate::Result<String> {
        if epoch != self.selection_epoch {
            return Err(crate::TerminalError::Invalid(
                "Terminal selection expired; select the text again".into(),
            ));
        }
        self.review();
        let screen = self
            .review
            .as_mut()
            .ok_or_else(|| crate::TerminalError::Invalid("Terminal text unavailable".into()))?;
        let (rows, cols) = screen.size();
        let total = self.history_len + usize::from(rows);
        let (start, end) = range.map_or(
            (
                TextPosition { row: 0, col: 0 },
                TextPosition {
                    row: total - 1,
                    col: cols,
                },
            ),
            |(a, b)| (a.min(b), a.max(b)),
        );
        if end.row >= total || start.col > cols || end.col > cols {
            return Err(crate::TerminalError::Invalid(
                "Terminal selection outside text buffer".into(),
            ));
        }
        let offset = screen.scrollback();
        let result = copy_rows(screen, self.history_len, start, end);
        screen.set_scrollback(offset);
        result
    }
    /// Materializes the screen outside the UI thread.
    #[must_use]
    pub fn snapshot(&self) -> TerminalGrid {
        let screen = self.review.as_ref().unwrap_or_else(|| self.parser.screen());
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
            wrapped_rows: (0..rows).map(|row| screen.row_wrapped(row)).collect(),
            cursor: (
                screen.cursor_position().0.min(rows - 1),
                screen.cursor_position().1.min(cols - 1),
            ),
            cursor_visible: !screen.hide_cursor()
                && screen.scrollback() == 0
                && self.review.is_none(),
            alternate: screen.alternate_screen(),
            application_cursor: screen.application_cursor(),
            bracketed_paste: screen.bracketed_paste(),
            mouse_mode: if self.review.is_some() {
                MouseProtocolMode::None
            } else {
                screen.mouse_protocol_mode()
            },
            mouse_encoding: screen.mouse_protocol_encoding(),
            reviewing: self.review.is_some(),
            selection_epoch: self.selection_epoch,
            history_len: self.history_len,
            viewport_start: self.history_len.saturating_sub(screen.scrollback()),
            scrollback: screen.scrollback(),
            title: self.parser.callbacks().title.clone(),
        }
    }
}
fn copy_rows(
    screen: &mut vt100::Screen,
    history: usize,
    start: TextPosition,
    end: TextPosition,
) -> crate::Result<String> {
    let (_, cols) = screen.size();
    let mut text = String::new();
    for row in start.row..=end.row {
        screen.set_scrollback(history.saturating_sub(row));
        let visible = u16::try_from(row.saturating_sub(history))
            .map_err(|_| crate::TerminalError::Invalid("Invalid terminal row".into()))?;
        let from = if row == start.row { start.col } else { 0 };
        let to = if row == end.row { end.col } else { cols };
        let mut line = String::new();
        for col in from..to {
            if let Some(cell) = screen.cell(visible, col)
                && !cell.is_wide_continuation()
            {
                if cell.contents().is_empty() {
                    line.push(' ');
                } else {
                    line.push_str(cell.contents());
                }
            }
        }
        text.push_str(line.trim_end());
        if row < end.row && !screen.row_wrapped(visible) {
            text.push('\n');
        }
        if text.len() > 4 * 1024 * 1024 {
            return Err(crate::TerminalError::Invalid(
                "Terminal copy exceeds 4 MiB".into(),
            ));
        }
    }
    Ok(text)
}

#[derive(Default)]
struct Callbacks {
    title: String,
    replies: Vec<u8>,
    clipboard: Option<String>,
}
impl vt100::Callbacks for Callbacks {
    fn copy_to_clipboard(&mut self, _: &mut vt100::Screen, target: &[u8], data: &[u8]) {
        use base64::Engine as _;
        if (!target.is_empty() && !target.contains(&b'c')) || data.len() > 6 * 1024 * 1024 {
            return;
        }
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data)
            && bytes.len() <= 4 * 1024 * 1024
            && let Ok(text) = String::from_utf8(bytes)
        {
            self.clipboard = Some(text);
        }
    }

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
