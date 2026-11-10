//! Markdown table grid.
//!
//! A column is as wide as the widest cell in it. The floor is the widest word
//! or code chip, so inline code stays on one line. When the table is wider
//! than the pane, columns that can wrap shrink toward that floor and text
//! breaks on spaces. Cells in one row share the tallest cell's height.
use super::{Face, Inline, body_size, link_is_active};
use crate::theme;
use egui::text::LayoutJob;
use egui::{Color32, CursorIcon, FontId, Pos2, Rect, Sense, Stroke, TextFormat, Ui, pos2, vec2};
use pulldown_cmark::Alignment;
use std::sync::Arc;

const PAD_X: f32 = 8.0;
const PAD_Y: f32 = 6.0;
const CODE_PAD_X: f32 = 4.0;
const CODE_PAD_Y: f32 = 1.0;
const LINE_GAP: f32 = 2.0;
/// Spare pixels so a measured line is not wrapped by rounding.
const WIDTH_SLACK: f32 = 1.0;
const FIT_SLACK: f32 = 0.5;
/// Cap for one unbreakable token. Longer tokens wrap inside their own chip.
const MAX_ATOM: f32 = 360.0;
const MIN_COL: f32 = 36.0;

#[derive(Clone, Copy)]
struct Metrics {
    scale: f32,
}

impl Metrics {
    fn px(self, base: f32) -> f32 {
        base * self.scale
    }
}

struct Draw<'a> {
    metrics: Metrics,
    opened: &'a mut Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Word,
    Code,
    Space,
    Break,
}

struct Piece {
    kind: Kind,
    width: f32,
    height: f32,
    sticky: bool,
    galley: Option<Arc<egui::Galley>>,
    link: Option<String>,
}

#[derive(Default)]
struct Line {
    indices: Vec<usize>,
    width: f32,
    height: f32,
}

struct CellPlan {
    pieces: Vec<Piece>,
    min_content: f32,
    pref_content: f32,
    line_height: f32,
}

struct RowPlan {
    cells: Vec<CellPlan>,
    header: bool,
}

struct PaintedCell {
    lines: Vec<Line>,
    pieces: Vec<Piece>,
    line_height: f32,
}

struct PaintedRow {
    height: f32,
    header: bool,
    cells: Vec<PaintedCell>,
}

#[derive(Default)]
struct TableLayout {
    widths: Vec<f32>,
    rows: Vec<PaintedRow>,
    aligns: Vec<Alignment>,
}

#[derive(Clone, Copy)]
struct CellId {
    table: usize,
    row: usize,
    column: usize,
}

/// Draws one Markdown table and keeps its horizontal scroll separate.
///
/// Column width follows the cells in that column. Cells in one row share a height.
pub(super) fn paint(
    ui: &mut Ui,
    id: usize,
    header: &[Vec<Inline>],
    rows: &[Vec<Vec<Inline>>],
    align: &[Alignment],
    scale: f32,
    opened: &mut Option<String>,
) {
    let metrics = Metrics {
        scale: if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        },
    };
    // Read the pane width before the horizontal scroller, which reports the
    // content width and would otherwise feed the table its own size.
    let available = ui.available_width();
    let layout = layout_table(ui, header, rows, align, available, metrics);
    if layout.widths.is_empty() || layout.rows.is_empty() {
        return;
    }
    egui::ScrollArea::horizontal()
        .id_salt(("markdown-table", id))
        .show(ui, |ui| {
            draw_table(ui, id, &layout, &mut Draw { metrics, opened });
        });
}

fn layout_table(
    ui: &Ui,
    header: &[Vec<Inline>],
    rows: &[Vec<Vec<Inline>>],
    align: &[Alignment],
    available: f32,
    metrics: Metrics,
) -> TableLayout {
    let columns = column_count(header, rows);
    if columns == 0 {
        return TableLayout::default();
    }
    let mut plans = Vec::new();
    if !header.is_empty() {
        plans.push(plan_row(ui, header, columns, true, metrics));
    }
    for row in rows {
        plans.push(plan_row(ui, row, columns, false, metrics));
    }
    let floor = metrics.px(MIN_COL);
    let mut mins = vec![floor; columns];
    let mut prefs = vec![floor; columns];
    for plan in &plans {
        for (index, cell) in plan.cells.iter().enumerate() {
            let min = outer(cell.min_content, metrics);
            let pref = outer(cell.pref_content, metrics).max(min);
            mins[index] = mins[index].max(min);
            prefs[index] = prefs[index].max(pref);
        }
    }
    let widths = column_widths(&mins, &prefs, available);
    let aligns = (0..columns)
        .map(|index| align.get(index).copied().unwrap_or(Alignment::None))
        .collect();
    let painted = plans
        .into_iter()
        .map(|plan| layout_row(plan, &widths, metrics))
        .collect();
    TableLayout {
        widths,
        rows: painted,
        aligns,
    }
}

fn column_count(header: &[Vec<Inline>], rows: &[Vec<Vec<Inline>>]) -> usize {
    let mut count = header.len();
    for row in rows {
        count = count.max(row.len());
    }
    count
}

fn plan_row(
    ui: &Ui,
    cells: &[Vec<Inline>],
    columns: usize,
    header: bool,
    metrics: Metrics,
) -> RowPlan {
    let mut planned = Vec::with_capacity(columns);
    for index in 0..columns {
        let items = cells.get(index).map_or(&[][..], Vec::as_slice);
        planned.push(plan_cell(ui, items, header, metrics));
    }
    RowPlan {
        cells: planned,
        header,
    }
}

fn plan_cell(ui: &Ui, items: &[Inline], header: bool, metrics: Metrics) -> CellPlan {
    let face = Face {
        strong: header,
        ..Face::default()
    };
    let font = font_id(ui, &face, false, metrics);
    let line_height = ui.fonts(|fonts| fonts.row_height(&font));
    let mut pieces = Vec::new();
    push_inlines(ui, &mut pieces, items, &face, line_height, metrics);
    CellPlan {
        min_content: minimum_content(&pieces),
        pref_content: preferred_content(&pieces),
        line_height,
        pieces,
    }
}

fn layout_row(plan: RowPlan, widths: &[f32], metrics: Metrics) -> PaintedRow {
    let header = plan.header;
    let fallback = plan
        .cells
        .iter()
        .map(|cell| cell.line_height)
        .fold(0.0, f32::max);
    let mut cells = Vec::with_capacity(plan.cells.len());
    let mut content_height_max = fallback;
    for (index, cell) in plan.cells.into_iter().enumerate() {
        let width = widths
            .get(index)
            .copied()
            .unwrap_or_else(|| metrics.px(MIN_COL));
        let max_width = metrics.px(PAD_X).mul_add(-2.0, width).max(0.0);
        let lines = wrap_pieces(&cell.pieces, max_width);
        content_height_max = content_height_max.max(content_height(
            &lines,
            cell.line_height,
            metrics.px(LINE_GAP),
        ));
        cells.push(PaintedCell {
            lines,
            pieces: cell.pieces,
            line_height: cell.line_height,
        });
    }
    PaintedRow {
        height: metrics.px(PAD_Y).mul_add(2.0, content_height_max),
        header,
        cells,
    }
}

fn draw_table(ui: &mut Ui, id: usize, layout: &TableLayout, draw: &mut Draw<'_>) {
    let total_w: f32 = layout.widths.iter().sum();
    let total_h: f32 = layout.rows.iter().map(|row| row.height).sum();
    if total_w <= 0.0 || total_h <= 0.0 {
        return;
    }
    let (rect, _) = ui.allocate_exact_size(vec2(total_w, total_h), Sense::hover());
    let painter = ui.painter().clone();
    let mut y = rect.top();
    let mut heights = Vec::with_capacity(layout.rows.len());
    for (row_index, row) in layout.rows.iter().enumerate() {
        let mut x = rect.left();
        for (column, cell) in row.cells.iter().enumerate() {
            let Some(width) = layout.widths.get(column).copied() else {
                continue;
            };
            let cell_rect = Rect::from_min_size(pos2(x, y), vec2(width, row.height));
            let fill = if row.header {
                theme::surface()
            } else {
                theme::background()
            };
            painter.rect_filled(cell_rect, 0.0, fill);
            let align = layout
                .aligns
                .get(column)
                .copied()
                .unwrap_or(Alignment::None);
            draw_cell(
                ui,
                &painter,
                CellId {
                    table: id,
                    row: row_index,
                    column,
                },
                cell,
                cell_rect,
                align,
                draw,
            );
            x += width;
        }
        heights.push(row.height);
        y += row.height;
    }
    draw_grid(&painter, rect, &layout.widths, &heights);
}

fn draw_cell(
    ui: &Ui,
    painter: &egui::Painter,
    id: CellId,
    cell: &PaintedCell,
    rect: Rect,
    align: Alignment,
    draw: &mut Draw<'_>,
) {
    let pad_x = draw.metrics.px(PAD_X);
    let content_width = pad_x.mul_add(-2.0, rect.width()).max(0.0);
    let mut y = rect.top() + draw.metrics.px(PAD_Y);
    for line in &cell.lines {
        let height = line.height.max(cell.line_height);
        let mut x = align_x(align, rect.left() + pad_x, content_width, line.width);
        for &index in &line.indices {
            let Some(piece) = cell.pieces.get(index) else {
                continue;
            };
            let piece_y = (height - piece.height).max(0.0).mul_add(0.5, y);
            draw_piece(ui, painter, id, index, piece, pos2(x, piece_y), draw);
            x += piece.width;
        }
        y += height + draw.metrics.px(LINE_GAP);
    }
}

fn draw_piece(
    ui: &Ui,
    painter: &egui::Painter,
    id: CellId,
    index: usize,
    piece: &Piece,
    pos: Pos2,
    draw: &mut Draw<'_>,
) {
    match piece.kind {
        Kind::Code => {
            painter.rect_filled(
                Rect::from_min_size(pos, vec2(piece.width, piece.height)),
                4.0,
                theme::chip(),
            );
            if let Some(galley) = &piece.galley {
                painter.galley(
                    pos + vec2(draw.metrics.px(CODE_PAD_X), draw.metrics.px(CODE_PAD_Y)),
                    Arc::clone(galley),
                    Color32::PLACEHOLDER,
                );
            }
        }
        Kind::Word => {
            if let Some(galley) = &piece.galley {
                painter.galley(pos, Arc::clone(galley), Color32::PLACEHOLDER);
            }
        }
        Kind::Space | Kind::Break => {}
    }
    let Some(url) = &piece.link else {
        return;
    };
    let rect = Rect::from_min_size(pos, vec2(piece.width, piece.height));
    if rect.width() < 1.0 || rect.height() < 1.0 {
        return;
    }
    let response = ui.interact(
        rect,
        ui.id()
            .with(("markdown-cell-link", id.table, id.row, id.column, index)),
        Sense::click(),
    );
    if !link_is_active(url) {
        return;
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    if response.clicked() {
        *draw.opened = Some(url.clone());
    }
}

fn draw_grid(painter: &egui::Painter, rect: Rect, widths: &[f32], heights: &[f32]) {
    let stroke = Stroke::new(1.0_f32, theme::border());
    let mut x = rect.left();
    painter.vline(x, rect.y_range(), stroke);
    for (index, width) in widths.iter().enumerate() {
        x += *width;
        let x = if index + 1 == widths.len() {
            rect.right()
        } else {
            x
        };
        painter.vline(x, rect.y_range(), stroke);
    }
    let mut y = rect.top();
    painter.hline(rect.x_range(), y, stroke);
    for (index, height) in heights.iter().enumerate() {
        y += *height;
        let y = if index + 1 == heights.len() {
            rect.bottom()
        } else {
            y
        };
        painter.hline(rect.x_range(), y, stroke);
    }
}

fn outer(content: f32, metrics: Metrics) -> f32 {
    metrics
        .px(PAD_X)
        .mul_add(2.0, content + WIDTH_SLACK)
        .max(metrics.px(MIN_COL))
}

fn column_widths(mins: &[f32], prefs: &[f32], available: f32) -> Vec<f32> {
    if mins.is_empty() {
        return Vec::new();
    }
    let prefs: Vec<f32> = mins
        .iter()
        .zip(prefs)
        .map(|(min, pref)| pref.max(*min))
        .collect();
    if !available.is_finite() {
        return prefs;
    }
    let pref_sum: f32 = prefs.iter().sum();
    if pref_sum <= available {
        return prefs;
    }
    let min_sum: f32 = mins.iter().copied().sum();
    if min_sum >= available {
        return mins.to_vec();
    }
    let slack_total = pref_sum - min_sum;
    if slack_total <= f32::EPSILON {
        return mins.to_vec();
    }
    let shrink_total = pref_sum - available;
    let mut widths = Vec::with_capacity(mins.len());
    let mut assigned = 0.0;
    for (index, (min, pref)) in mins.iter().zip(&prefs).enumerate() {
        if index + 1 == mins.len() {
            widths.push((available - assigned).clamp(*min, *pref));
        } else {
            let slack = (pref - min).max(0.0);
            let width = (pref - shrink_total * slack / slack_total).clamp(*min, *pref);
            assigned += width;
            widths.push(width);
        }
    }
    widths
}

fn wrap_pieces(pieces: &[Piece], max_width: f32) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut current = Line::default();
    let mut index = 0;
    while index < pieces.len() {
        match pieces[index].kind {
            Kind::Break => {
                push_line(&mut lines, &mut current, pieces[index].height);
                index += 1;
            }
            Kind::Space => {
                index = take_space(pieces, index, max_width, &mut lines, &mut current);
            }
            Kind::Word | Kind::Code => {
                index = take_box(pieces, index, max_width, &mut lines, &mut current);
            }
        }
    }
    if !current.indices.is_empty() {
        lines.push(current);
    }
    lines
}

fn push_line(lines: &mut Vec<Line>, current: &mut Line, blank_height: f32) {
    if current.indices.is_empty() {
        lines.push(Line {
            indices: Vec::new(),
            width: 0.0,
            height: blank_height,
        });
        return;
    }
    lines.push(std::mem::take(current));
}

fn take_space(
    pieces: &[Piece],
    index: usize,
    max_width: f32,
    lines: &mut Vec<Line>,
    current: &mut Line,
) -> usize {
    let Some(next) = next_content(pieces, index + 1) else {
        return index + 1;
    };
    if current.indices.is_empty() {
        return index + 1;
    }
    let needed = current.width + pieces[index].width + pieces[next].width;
    if needed > max_width + FIT_SLACK {
        lines.push(std::mem::take(current));
        return next;
    }
    current.indices.push(index);
    current.width += pieces[index].width;
    current.height = current.height.max(pieces[index].height);
    index + 1
}

fn take_box(
    pieces: &[Piece],
    index: usize,
    max_width: f32,
    lines: &mut Vec<Line>,
    current: &mut Line,
) -> usize {
    let width = pieces[index].width;
    let height = pieces[index].height;
    let sticky = pieces[index].sticky && !current.indices.is_empty();
    let fits = current.width + width <= max_width + FIT_SLACK;
    if !current.indices.is_empty() && !sticky && !fits {
        lines.push(std::mem::take(current));
        return index;
    }
    current.indices.push(index);
    current.width += width;
    current.height = current.height.max(height);
    index + 1
}

const fn next_content(pieces: &[Piece], mut index: usize) -> Option<usize> {
    while index < pieces.len() {
        match pieces[index].kind {
            Kind::Space => index += 1,
            Kind::Break => return None,
            Kind::Word | Kind::Code => return Some(index),
        }
    }
    None
}

fn content_height(lines: &[Line], fallback: f32, line_gap: f32) -> f32 {
    if lines.is_empty() {
        return fallback;
    }
    let mut height = 0.0;
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            height += line_gap;
        }
        height += line.height.max(fallback);
    }
    height
}

fn preferred_content(pieces: &[Piece]) -> f32 {
    let mut best = 0.0_f32;
    let mut current = 0.0;
    for piece in pieces {
        match piece.kind {
            Kind::Break => {
                best = best.max(current);
                current = 0.0;
            }
            Kind::Space if current == 0.0 => {}
            _ => current += piece.width,
        }
    }
    best.max(current)
}

fn minimum_content(pieces: &[Piece]) -> f32 {
    pieces
        .iter()
        .filter(|piece| matches!(piece.kind, Kind::Word | Kind::Code))
        .map(|piece| piece.width)
        .fold(0.0, f32::max)
}

fn align_x(align: Alignment, left: f32, width: f32, line: f32) -> f32 {
    let extra = (width - line).max(0.0);
    match align {
        Alignment::Center => extra.mul_add(0.5, left),
        Alignment::Right => left + extra,
        Alignment::Left | Alignment::None => left,
    }
}

fn push_inlines(
    ui: &Ui,
    pieces: &mut Vec<Piece>,
    items: &[Inline],
    face: &Face,
    line_height: f32,
    metrics: Metrics,
) {
    for item in items {
        match item {
            Inline::Text(text) => push_text(ui, pieces, text, face, line_height, metrics),
            Inline::Code(text) | Inline::Html(text) => {
                push_code(ui, pieces, text, face, metrics);
            }
            Inline::Break { hard: true } => pieces.push(Piece::break_line(line_height)),
            Inline::Break { hard: false } => {
                pieces.push(Piece::space(space_width(ui, face, metrics), line_height));
            }
            Inline::Task(checked) => {
                let mark = if *checked { "☑ " } else { "☐ " };
                push_text(ui, pieces, mark, face, line_height, metrics);
            }
            Inline::Emphasis(children) => {
                let mut next = face.clone();
                next.emphasis = true;
                push_inlines(ui, pieces, children, &next, line_height, metrics);
            }
            Inline::Strong(children) => {
                let mut next = face.clone();
                next.strong = true;
                push_inlines(ui, pieces, children, &next, line_height, metrics);
            }
            Inline::Strike(children) => {
                let mut next = face.clone();
                next.strike = true;
                push_inlines(ui, pieces, children, &next, line_height, metrics);
            }
            Inline::Link { url, children } => {
                let mut next = face.clone();
                next.link = Some(url.clone());
                push_inlines(ui, pieces, children, &next, line_height, metrics);
            }
            Inline::Image { children } => {
                let mut next = face.clone();
                next.emphasis = true;
                if children.is_empty() {
                    push_text(ui, pieces, "image", &next, line_height, metrics);
                } else {
                    push_inlines(ui, pieces, children, &next, line_height, metrics);
                }
            }
        }
    }
}

fn push_text(
    ui: &Ui,
    pieces: &mut Vec<Piece>,
    text: &str,
    face: &Face,
    line_height: f32,
    metrics: Metrics,
) {
    let mut start = 0;
    for (index, ch) in text.char_indices() {
        if !ch.is_whitespace() {
            continue;
        }
        if start < index {
            push_word(ui, pieces, &text[start..index], face, metrics);
        }
        if ch == '\n' {
            pieces.push(Piece::break_line(line_height));
        } else if !ch.is_control() {
            pieces.push(Piece::space(space_width(ui, face, metrics), line_height));
        }
        start = index + ch.len_utf8();
    }
    if start < text.len() {
        push_word(ui, pieces, &text[start..], face, metrics);
    }
}

fn push_word(ui: &Ui, pieces: &mut Vec<Piece>, text: &str, face: &Face, metrics: Metrics) {
    if text.is_empty() {
        return;
    }
    let galley = layout_galley(ui, text, face, false, metrics);
    pieces.push(Piece::word(
        galley.size().x,
        galley.size().y,
        is_sticky(text),
        galley,
        face.link.clone(),
    ));
}

fn push_code(ui: &Ui, pieces: &mut Vec<Piece>, text: &str, face: &Face, metrics: Metrics) {
    if text.is_empty() {
        return;
    }
    let galley = layout_galley(ui, text, face, true, metrics);
    pieces.push(Piece::code(
        metrics.px(CODE_PAD_X).mul_add(2.0, galley.size().x),
        metrics.px(CODE_PAD_Y).mul_add(2.0, galley.size().y),
        galley,
        face.link.clone(),
    ));
}

fn layout_galley(
    ui: &Ui,
    text: &str,
    face: &Face,
    code: bool,
    metrics: Metrics,
) -> Arc<egui::Galley> {
    let cap = metrics.px(MAX_ATOM);
    let galley = layout_galley_at(ui, text, face, code, f32::INFINITY, false, metrics);
    if galley.size().x <= cap {
        return galley;
    }
    let break_anywhere = !text.chars().any(char::is_whitespace);
    layout_galley_at(ui, text, face, code, cap, break_anywhere, metrics)
}

fn layout_galley_at(
    ui: &Ui,
    text: &str,
    face: &Face,
    code: bool,
    max_width: f32,
    break_anywhere: bool,
    metrics: Metrics,
) -> Arc<egui::Galley> {
    let color = text_color(face, code);
    let format = TextFormat {
        font_id: font_id(ui, face, code, metrics),
        color,
        italics: face.emphasis && !code,
        underline: if face.link.is_some() && !code {
            Stroke::new(1.0_f32, theme::link())
        } else {
            Stroke::NONE
        },
        strikethrough: if face.strike {
            Stroke::new(1.0_f32, color)
        } else {
            Stroke::NONE
        },
        background: Color32::TRANSPARENT,
        expand_bg: 0.0,
        ..Default::default()
    };
    let mut job = LayoutJob::single_section(text.to_owned(), format);
    job.wrap.max_width = max_width;
    job.wrap.break_anywhere = break_anywhere;
    job.break_on_newline = true;
    ui.painter().layout_job(job)
}

fn text_color(face: &Face, code: bool) -> Color32 {
    if code {
        theme::syntax_color("markup.raw")
    } else if face.link.is_some() {
        theme::link()
    } else if face.strong {
        theme::text_strong()
    } else {
        theme::text()
    }
}

fn font_id(ui: &Ui, face: &Face, code: bool, metrics: Metrics) -> FontId {
    let size = face.size.unwrap_or_else(|| body_size(ui) * metrics.scale);
    if code {
        FontId::monospace((size - metrics.scale).max(11.0 * metrics.scale))
    } else if face.strong {
        FontId::new(size, theme::bold_family())
    } else {
        FontId::proportional(size)
    }
}

fn space_width(ui: &Ui, face: &Face, metrics: Metrics) -> f32 {
    let font = font_id(ui, face, false, metrics);
    ui.fonts(|fonts| fonts.glyph_width(&font, ' '))
}

fn is_sticky(text: &str) -> bool {
    let mut count = 0_usize;
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch.is_whitespace() {
            return false;
        }
        count += 1;
        if count > 3 {
            return false;
        }
    }
    count > 0
}

impl Piece {
    const fn word(
        width: f32,
        height: f32,
        sticky: bool,
        galley: Arc<egui::Galley>,
        link: Option<String>,
    ) -> Self {
        Self {
            kind: Kind::Word,
            width,
            height,
            sticky,
            galley: Some(galley),
            link,
        }
    }

    const fn code(
        width: f32,
        height: f32,
        galley: Arc<egui::Galley>,
        link: Option<String>,
    ) -> Self {
        Self {
            kind: Kind::Code,
            width,
            height,
            sticky: false,
            galley: Some(galley),
            link,
        }
    }

    const fn space(width: f32, height: f32) -> Self {
        Self {
            kind: Kind::Space,
            width,
            height,
            sticky: false,
            galley: None,
            link: None,
        }
    }

    const fn break_line(height: f32) -> Self {
        Self {
            kind: Kind::Break,
            width: 0.0,
            height,
            sticky: false,
            galley: None,
            link: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Alignment, Kind, LINE_GAP, Piece, align_x, column_widths, content_height, minimum_content,
        preferred_content, wrap_pieces,
    };

    fn close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 0.02, "{actual} != {expected}");
    }

    fn word(width: f32) -> Piece {
        Piece {
            kind: Kind::Word,
            width,
            height: 10.0,
            sticky: false,
            galley: None,
            link: None,
        }
    }

    fn sticky(width: f32) -> Piece {
        Piece {
            kind: Kind::Word,
            width,
            height: 10.0,
            sticky: true,
            galley: None,
            link: None,
        }
    }

    fn code(width: f32) -> Piece {
        Piece {
            kind: Kind::Code,
            width,
            height: 12.0,
            sticky: false,
            galley: None,
            link: None,
        }
    }

    fn space() -> Piece {
        Piece {
            kind: Kind::Space,
            width: 4.0,
            height: 10.0,
            sticky: false,
            galley: None,
            link: None,
        }
    }

    fn hard() -> Piece {
        Piece {
            kind: Kind::Break,
            width: 0.0,
            height: 10.0,
            sticky: false,
            galley: None,
            link: None,
        }
    }

    #[test]
    fn wrap_keeps_a_code_chip_whole_and_breaks_on_spaces() {
        let pieces = vec![word(30.0), space(), code(50.0), space(), word(20.0)];
        let wrapped = wrap_pieces(&pieces, 60.0);
        assert_eq!(wrapped.len(), 3);
        assert_eq!(wrapped[0].indices, vec![0]);
        assert_eq!(wrapped[1].indices, vec![2]);
        close(wrapped[1].width, 50.0);
        assert_eq!(wrapped[2].indices, vec![4]);

        let fitted = wrap_pieces(&pieces, 110.0);
        assert_eq!(fitted.len(), 1);
        close(fitted[0].width, 30.0 + 4.0 + 50.0 + 4.0 + 20.0);
    }

    #[test]
    fn wrap_honors_hard_breaks_and_glues_punctuation() {
        let broken = wrap_pieces(&[word(20.0), hard(), word(15.0)], 100.0);
        assert_eq!(broken.len(), 2);
        assert_eq!(broken[0].indices, vec![0]);
        assert_eq!(broken[1].indices, vec![2]);

        let comma = wrap_pieces(&[code(50.0), sticky(6.0)], 52.0);
        assert_eq!(comma.len(), 1);
        assert_eq!(comma[0].indices, vec![0, 1]);
        close(comma[0].width, 56.0);
    }

    #[test]
    fn content_width_uses_the_longest_line_and_the_widest_token() {
        let pieces = vec![word(20.0), space(), word(30.0), hard(), code(40.0)];
        close(preferred_content(&pieces), 54.0);
        close(minimum_content(&pieces), 40.0);
        let lines = wrap_pieces(&pieces, 100.0);
        close(
            content_height(&lines, 10.0, LINE_GAP),
            10.0 + LINE_GAP + 12.0,
        );
        close(content_height(&[], 16.0, LINE_GAP), 16.0);
    }

    #[test]
    fn column_widths_follow_content_then_shrink_to_the_token_floor() {
        let fitted = column_widths(&[10.0, 20.0], &[30.0, 40.0], 200.0);
        close(fitted[0], 30.0);
        close(fitted[1], 40.0);

        let open = column_widths(&[10.0], &[80.0], f32::INFINITY);
        close(open[0], 80.0);
        assert_eq!(column_widths(&[], &[], 10.0), [] as [f32; 0]);

        let too_narrow = column_widths(&[40.0, 50.0], &[40.0, 80.0], 30.0);
        close(too_narrow[0], 40.0);
        close(too_narrow[1], 50.0);

        let lifted = column_widths(&[50.0], &[10.0], 1000.0);
        close(lifted[0], 50.0);

        let widths = column_widths(&[10.0, 10.0, 10.0], &[10.0, 30.0, 60.0], 70.0);
        close(widths[0], 10.0);
        close(widths.iter().copied().sum(), 70.0);
        let shrink_port = 30.0 - widths[1];
        let shrink_note = 60.0 - widths[2];
        close(shrink_port / 20.0, shrink_note / 50.0);

        let floored = column_widths(&[80.0, 120.0], &[80.0, 400.0], 250.0);
        close(floored[0], 80.0);
        close(floored[1], 170.0);
        assert!(floored[1] >= 120.0);
    }

    #[test]
    fn align_x_places_a_short_line_inside_the_cell() {
        close(align_x(Alignment::Left, 10.0, 40.0, 10.0), 10.0);
        close(align_x(Alignment::None, 10.0, 40.0, 10.0), 10.0);
        close(align_x(Alignment::Center, 10.0, 40.0, 10.0), 25.0);
        close(align_x(Alignment::Right, 10.0, 40.0, 10.0), 40.0);
        close(align_x(Alignment::Right, 10.0, 40.0, 80.0), 10.0);
    }

    #[test]
    fn blank_line_keeps_a_height() {
        let lines = wrap_pieces(&[hard()], 40.0);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].indices, [] as [usize; 0]);
        close(lines[0].height, 10.0);
    }
}
