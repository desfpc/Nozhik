//! Rendered Markdown. The text editor remains the source of the document.
use crate::theme;
use egui::text::LayoutJob;
use egui::{Align, FontId, RichText, Sense, Stroke, TextFormat, Ui};
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

#[path = "table.rs"]
mod table;
use std::{iter::Peekable, path::Path};

const MAX_PREVIEW: usize = 1024 * 1024;

type Events<'a> = Peekable<Parser<'a>>;

#[derive(Clone, Default)]
struct Face {
    strong: bool,
    emphasis: bool,
    strike: bool,
    link: Option<String>,
    size: Option<f32>,
}

enum Inline {
    Text(String),
    Code(String),
    Break { hard: bool },
    Emphasis(Vec<Self>),
    Strong(Vec<Self>),
    Strike(Vec<Self>),
    Link { url: String, children: Vec<Self> },
    Image { children: Vec<Self> },
    Task(bool),
    Html(String),
}

struct Doc<'a> {
    gap: bool,
    space: f32,
    tables: usize,
    /// Multiplier for fonts and the spacing around them. `1.0` is the small size.
    scale: f32,
    opened: &'a mut Option<String>,
}

pub fn show(
    ui: &mut Ui,
    path: &Path,
    source: &str,
    enabled: bool,
    scale: f32,
    opened: &mut Option<String>,
) {
    let scale = finite_scale(scale);
    egui::ScrollArea::vertical()
        .id_salt(("markdown-gui", path.to_path_buf()))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add_enabled_ui(enabled, |ui| {
                ui.spacing_mut().item_spacing *= scale;
                ui.spacing_mut().indent *= scale;
                if source.trim().is_empty() {
                    ui.label(RichText::new("Empty Markdown document").color(theme::muted()));
                    return;
                }
                if source.len() > MAX_PREVIEW {
                    ui.label("Markdown preview supports documents up to 1 MiB. Use Text for the full source.");
                    return;
                }
                let mut events = Parser::new_ext(source, markdown_options()).peekable();
                let mut doc = Doc {
                    gap: false,
                    space: 8.0 * scale,
                    tables: 0,
                    scale,
                    opened,
                };
                render_until(ui, &mut events, None, &mut doc);
            });
        });
}

fn finite_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

fn markdown_options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
}

fn render_until(ui: &mut Ui, events: &mut Events<'_>, stop: Option<TagEnd>, doc: &mut Doc<'_>) {
    loop {
        // A tight list item has no Paragraph tag, so its text and code are loose events.
        if loose_inline(events.peek()) {
            let items = take_loose_inlines(events);
            if !items.is_empty() {
                block_gap(ui, doc);
                paint_inlines(ui, &items, &Face::default(), doc);
            }
            continue;
        }
        let Some(event) = events.next() else {
            break;
        };
        match event {
            Event::End(end) if stop == Some(end) => break,
            Event::Rule => {
                block_gap(ui, doc);
                paint_rule(ui, doc.scale);
            }
            Event::Start(tag) => render_start(ui, events, tag, doc),
            _ => {}
        }
    }
}

const fn loose_inline(event: Option<&Event<'_>>) -> bool {
    match event {
        Some(Event::Start(tag)) => is_inline_tag(tag),
        Some(Event::End(_) | Event::Rule) | None => false,
        Some(_) => true,
    }
}

const fn is_inline_tag(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Link { .. }
            | Tag::Image { .. }
            | Tag::Subscript
            | Tag::Superscript
    )
}

fn render_start(ui: &mut Ui, events: &mut Events<'_>, tag: Tag<'_>, doc: &mut Doc<'_>) {
    let end = tag.to_end();
    match tag {
        Tag::Paragraph => {
            block_gap(ui, doc);
            let items = take_inlines(events, end);
            paint_inlines(ui, &items, &Face::default(), doc);
        }
        Tag::Heading { level, .. } => {
            block_gap(ui, doc);
            let items = take_inlines(events, end);
            paint_heading(ui, level, &items, doc);
        }
        Tag::BlockQuote(_) => {
            block_gap(ui, doc);
            paint_quote(ui, events, end, doc);
        }
        Tag::CodeBlock(kind) => {
            block_gap(ui, doc);
            paint_code_block(ui, events, &kind, doc.scale);
        }
        Tag::HtmlBlock => {
            block_gap(ui, doc);
            let mut text = String::new();
            for event in events.by_ref() {
                match event {
                    Event::Html(part) | Event::Text(part) => text.push_str(&part),
                    Event::End(TagEnd::HtmlBlock) => break,
                    _ => {}
                }
            }
            paint_code_frame(ui, None, &text, doc.scale);
        }
        Tag::List(start) => render_list(ui, events, start, doc),
        Tag::Table(align) => {
            block_gap(ui, doc);
            paint_table(ui, events, doc, &align);
        }
        Tag::Item
        | Tag::TableHead
        | Tag::TableRow
        | Tag::TableCell
        | Tag::Emphasis
        | Tag::Strong
        | Tag::Strikethrough
        | Tag::Link { .. }
        | Tag::Image { .. }
        | Tag::FootnoteDefinition(_)
        | Tag::DefinitionList
        | Tag::DefinitionListTitle
        | Tag::DefinitionListDefinition
        | Tag::MetadataBlock(_)
        | Tag::Subscript
        | Tag::Superscript => skip_balanced(events, end),
    }
}

fn render_list(ui: &mut Ui, events: &mut Events<'_>, start: Option<u64>, doc: &mut Doc<'_>) {
    block_gap(ui, doc);
    let mut number = start.unwrap_or(1);
    let scale = doc.scale;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0 * scale;
        loop {
            match events.peek() {
                Some(Event::Start(Tag::Item)) => {
                    events.next();
                    let task = match events.peek() {
                        Some(Event::TaskListMarker(checked)) => {
                            let checked = *checked;
                            events.next();
                            Some(checked)
                        }
                        _ => None,
                    };
                    let marker = match task {
                        Some(true) => ListMarker::Text("☑".to_owned()),
                        Some(false) => ListMarker::Text("☐".to_owned()),
                        None if start.is_some() => {
                            let label = format!("{number}.");
                            number += 1;
                            ListMarker::Text(label)
                        }
                        None => ListMarker::Bullet,
                    };
                    let line = line_height(ui, scale);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0 * scale;
                        paint_list_marker(ui, &marker, line, scale);
                        ui.vertical(|ui| {
                            let saved_space = doc.space;
                            doc.space = 2.0 * scale;
                            doc.gap = false;
                            render_until(ui, events, Some(TagEnd::Item), doc);
                            doc.space = saved_space;
                            doc.gap = true;
                        });
                    });
                }
                Some(Event::End(TagEnd::List(_))) => {
                    events.next();
                    break;
                }
                _ => break,
            }
        }
    });
}

enum ListMarker {
    Bullet,
    Text(String),
}

/// Disc or label centered on the first line of the item.
fn paint_list_marker(ui: &mut Ui, marker: &ListMarker, line: f32, scale: f32) {
    match marker {
        ListMarker::Bullet => {
            let radius = (line * 0.28).max(3.5 * scale);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(radius * 2.0, line), Sense::hover());
            ui.painter()
                .circle_filled(rect.center(), radius, theme::text());
        }
        ListMarker::Text(label) => {
            let color = theme::text();
            let galley = ui.painter().layout(
                label.clone(),
                FontId::proportional(body_size(ui) * scale),
                color,
                f32::INFINITY,
            );
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(galley.size().x, line), Sense::hover());
            let y = (line - galley.size().y).mul_add(0.5, rect.top());
            ui.painter()
                .galley(egui::pos2(rect.left(), y), galley, color);
        }
    }
}

fn paint_quote(ui: &mut Ui, events: &mut Events<'_>, end: TagEnd, doc: &mut Doc<'_>) {
    let scale = doc.scale;
    ui.horizontal_top(|ui| {
        ui.add_space(12.0 * scale);
        let saved = doc.space;
        doc.space = 4.0 * scale;
        let inner = ui.vertical(|ui| render_until(ui, events, Some(end), doc));
        doc.space = saved;
        let bar = egui::Rect::from_min_size(
            inner.response.rect.left_top() - egui::vec2(10.0 * scale, 0.0),
            egui::vec2(3.0, inner.response.rect.height()),
        );
        ui.painter()
            .rect_filled(bar, 1.0_f32, theme::border_strong());
    });
}

fn paint_code_block(ui: &mut Ui, events: &mut Events<'_>, kind: &CodeBlockKind<'_>, scale: f32) {
    let language = match kind {
        CodeBlockKind::Fenced(label) if !label.is_empty() => Some(label.to_string()),
        _ => None,
    };
    let mut text = String::new();
    for event in events.by_ref() {
        match event {
            Event::Text(part) => text.push_str(&part),
            Event::End(TagEnd::CodeBlock) => break,
            _ => {}
        }
    }
    paint_code_frame(ui, language.as_deref(), &text, scale);
}

fn paint_code_frame(ui: &mut Ui, language: Option<&str>, text: &str, scale: f32) {
    let scale = finite_scale(scale);
    egui::Frame::new()
        .fill(theme::surface())
        .corner_radius(theme::RADIUS)
        .inner_margin(10.0 * scale)
        .show(ui, |ui| {
            if let Some(language) = language {
                let small = egui::TextStyle::Small.resolve(ui.style()).size * scale;
                ui.label(RichText::new(language).size(small).color(theme::muted()));
            }
            let mono = egui::TextStyle::Monospace.resolve(ui.style()).size * scale;
            ui.add(
                egui::Label::new(
                    RichText::new(text.trim_end_matches('\n'))
                        .monospace()
                        .size(mono)
                        .color(theme::text()),
                )
                .wrap(),
            );
        });
}

fn paint_table(ui: &mut Ui, events: &mut Events<'_>, doc: &mut Doc<'_>, align: &[Alignment]) {
    let mut header = Vec::new();
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut in_head = false;
    while let Some(event) = events.next() {
        match event {
            Event::Start(Tag::TableHead) => in_head = true,
            Event::End(TagEnd::TableHead) => {
                in_head = false;
                if !row.is_empty() {
                    header = std::mem::take(&mut row);
                }
            }
            Event::Start(Tag::TableRow) => row.clear(),
            Event::End(TagEnd::TableRow) => {
                let finished = std::mem::take(&mut row);
                if in_head {
                    header = finished;
                } else if !finished.is_empty() {
                    rows.push(finished);
                }
            }
            Event::Start(Tag::TableCell) => row.push(take_inlines(events, TagEnd::TableCell)),
            Event::End(TagEnd::Table) => break,
            _ => {}
        }
    }
    if header.is_empty() && rows.is_empty() {
        return;
    }
    doc.tables += 1;
    table::paint(ui, doc.tables, &header, &rows, align, doc.scale, doc.opened);
}

fn paint_heading(ui: &mut Ui, level: HeadingLevel, items: &[Inline], doc: &mut Doc<'_>) {
    let size = match level {
        HeadingLevel::H1 => 26.0,
        HeadingLevel::H2 => 22.0,
        HeadingLevel::H3 => 18.0,
        HeadingLevel::H4 => 16.0,
        HeadingLevel::H5 | HeadingLevel::H6 => 15.0,
    } * doc.scale;
    let face = Face {
        strong: true,
        size: Some(size),
        ..Face::default()
    };
    paint_inlines(ui, items, &face, doc);
    ui.add_space(2.0 * doc.scale);
}

fn paint_rule(ui: &mut Ui, scale: f32) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 8.0 * scale),
        egui::Sense::hover(),
    );
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0_f32, theme::border()),
    );
}

fn block_gap(ui: &mut Ui, doc: &mut Doc<'_>) {
    if doc.gap {
        ui.add_space(doc.space);
    }
    doc.gap = true;
}

fn take_inlines(events: &mut Events<'_>, end: TagEnd) -> Vec<Inline> {
    let mut items = Vec::new();
    while let Some(event) = events.next() {
        if let Event::End(found) = &event
            && *found == end
        {
            break;
        }
        if !push_inline(events, event, &mut items) {
            break;
        }
    }
    items
}

/// Inline events that belong to one sentence, up to the next block.
fn take_loose_inlines(events: &mut Events<'_>) -> Vec<Inline> {
    let mut items = Vec::new();
    while loose_inline(events.peek()) {
        let Some(event) = events.next() else {
            break;
        };
        if !push_inline(events, event, &mut items) {
            break;
        }
    }
    items
}

fn push_inline(events: &mut Events<'_>, event: Event<'_>, items: &mut Vec<Inline>) -> bool {
    match event {
        Event::Text(text) => items.push(Inline::Text(text.to_string())),
        Event::Code(text) | Event::InlineMath(text) | Event::DisplayMath(text) => {
            items.push(Inline::Code(text.to_string()));
        }
        Event::SoftBreak => items.push(Inline::Break { hard: false }),
        Event::HardBreak => items.push(Inline::Break { hard: true }),
        Event::Start(Tag::Emphasis) => {
            items.push(Inline::Emphasis(take_inlines(events, TagEnd::Emphasis)));
        }
        Event::Start(Tag::Strong) => {
            items.push(Inline::Strong(take_inlines(events, TagEnd::Strong)));
        }
        Event::Start(Tag::Strikethrough) => {
            items.push(Inline::Strike(take_inlines(events, TagEnd::Strikethrough)));
        }
        Event::Start(Tag::Link { dest_url, .. }) => items.push(Inline::Link {
            url: dest_url.to_string(),
            children: take_inlines(events, TagEnd::Link),
        }),
        Event::Start(Tag::Image { .. }) => items.push(Inline::Image {
            children: take_inlines(events, TagEnd::Image),
        }),
        Event::Start(tag) => skip_balanced(events, tag.to_end()),
        Event::TaskListMarker(checked) => items.push(Inline::Task(checked)),
        Event::Html(html) | Event::InlineHtml(html) => {
            items.push(Inline::Html(html.to_string()));
        }
        Event::FootnoteReference(name) => items.push(Inline::Text(format!("[{name}]"))),
        Event::End(_) | Event::Rule => return false,
    }
    true
}

fn skip_balanced(events: &mut Events<'_>, end: TagEnd) {
    let mut depth = 1_u32;
    for event in events.by_ref() {
        match event {
            Event::Start(tag) if tag.to_end() == end => depth += 1,
            Event::End(found) if found == end => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
    }
}

struct LinkHit {
    start: usize,
    end: usize,
    url: String,
}

struct Paragraph {
    job: LayoutJob,
    chars: usize,
    links: Vec<LinkHit>,
}

/// One paragraph is one galley. Inline code is a run in that galley, so a
/// backtick does not break the line before or after the span.
fn paint_inlines(ui: &mut Ui, items: &[Inline], face: &Face, doc: &mut Doc<'_>) {
    let mut paragraph = Paragraph {
        job: LayoutJob::default(),
        chars: 0,
        links: Vec::new(),
    };
    let width = ui.available_width();
    paragraph.job.wrap.max_width = if width.is_finite() {
        width.max(0.0)
    } else {
        f32::INFINITY
    };
    paragraph.job.break_on_newline = true;
    paragraph.job.halign = Align::LEFT;
    paragraph.append(ui, items, face, doc.scale);
    if paragraph.job.text.is_empty() {
        return;
    }
    let galley = ui.fonts(|fonts| fonts.layout_job(paragraph.job));
    let (rect, response) = ui.allocate_exact_size(galley.size(), Sense::click_and_drag());
    egui::text_selection::LabelSelectionState::label_text_selection(
        ui,
        &response,
        rect.min,
        galley.clone(),
        theme::text(),
        Stroke::NONE,
    );
    if let Some(pointer) = response.hover_pos()
        && let Some(url) = link_at(&galley, &paragraph.links, pointer - rect.min)
        && link_is_active(url)
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        if response.clicked() {
            *doc.opened = Some(url.to_owned());
        }
    }
}

impl Paragraph {
    fn append(&mut self, ui: &Ui, items: &[Inline], face: &Face, scale: f32) {
        for item in items {
            match item {
                Inline::Text(text) => self.push(ui, text, face, false, scale),
                Inline::Code(text) | Inline::Html(text) => self.push(ui, text, face, true, scale),
                Inline::Break { hard: true } => self.push(ui, "\n", face, false, scale),
                Inline::Break { hard: false } => self.push(ui, " ", face, false, scale),
                Inline::Task(checked) => {
                    self.push(ui, if *checked { "☑ " } else { "☐ " }, face, false, scale);
                }
                Inline::Emphasis(children) => {
                    let mut next = face.clone();
                    next.emphasis = true;
                    self.append(ui, children, &next, scale);
                }
                Inline::Strong(children) => {
                    let mut next = face.clone();
                    next.strong = true;
                    self.append(ui, children, &next, scale);
                }
                Inline::Strike(children) => {
                    let mut next = face.clone();
                    next.strike = true;
                    self.append(ui, children, &next, scale);
                }
                Inline::Link { url, children } => {
                    let start = self.chars;
                    let mut next = face.clone();
                    next.link = Some(url.clone());
                    self.append(ui, children, &next, scale);
                    if self.chars > start {
                        self.links.push(LinkHit {
                            start,
                            end: self.chars,
                            url: url.clone(),
                        });
                    }
                }
                Inline::Image { children } => {
                    let mut next = face.clone();
                    next.emphasis = true;
                    if children.is_empty() {
                        self.push(ui, "image", &next, false, scale);
                    } else {
                        self.append(ui, children, &next, scale);
                    }
                }
            }
        }
    }

    fn push(&mut self, ui: &Ui, text: &str, face: &Face, code: bool, scale: f32) {
        if text.is_empty() {
            return;
        }
        self.chars += text.chars().count();
        self.job
            .append(text, 0.0, text_format(ui, face, code, scale));
    }
}

fn text_format(ui: &Ui, face: &Face, code: bool, scale: f32) -> TextFormat {
    let size = face.size.unwrap_or_else(|| body_size(ui) * scale);
    let color = if code {
        theme::syntax_color("markup.raw")
    } else if face.link.is_some() {
        theme::link()
    } else if face.strong {
        theme::text_strong()
    } else {
        theme::text()
    };
    let font_id = if code {
        FontId::monospace((size - scale).max(11.0 * scale))
    } else if face.strong {
        FontId::new(size, theme::bold_family())
    } else {
        FontId::proportional(size)
    };
    TextFormat {
        font_id,
        color,
        italics: face.emphasis,
        background: if code {
            theme::chip()
        } else {
            egui::Color32::TRANSPARENT
        },
        underline: if code || face.link.is_none() {
            Stroke::NONE
        } else {
            Stroke::new(1.0_f32, theme::link())
        },
        strikethrough: if face.strike {
            Stroke::new(1.0_f32, color)
        } else {
            Stroke::NONE
        },
        valign: Align::Center,
        ..TextFormat::default()
    }
}

fn link_at<'a>(galley: &egui::Galley, links: &'a [LinkHit], pos: egui::Vec2) -> Option<&'a str> {
    let index = galley.cursor_from_pos(pos).index;
    links
        .iter()
        .find(|link| (link.start..link.end).contains(&index))
        .map(|link| link.url.as_str())
}

fn line_height(ui: &Ui, scale: f32) -> f32 {
    let font = FontId::proportional(body_size(ui) * scale);
    ui.fonts(|fonts| fonts.row_height(&font))
}

fn body_size(ui: &Ui) -> f32 {
    egui::TextStyle::Body.resolve(ui.style()).size
}

/// A link the preview can follow: a web address, or a path to a file.
fn link_is_active(url: &str) -> bool {
    let url = url.trim();
    if url.is_empty() || url.starts_with('#') {
        return false;
    }
    if let Some((scheme, rest)) = split_scheme(url) {
        return scheme_is(scheme, "http")
            || scheme_is(scheme, "https")
            || scheme_is(scheme, "mailto")
            || (scheme_is(scheme, "file") && file_url_path(rest).is_some());
    }
    !percent_decode(strip_fragment(url)).is_empty()
}

/// Where a Markdown link goes. `None` means the preview leaves it alone.
#[must_use]
pub(super) fn link_target(document: &Path, url: &str) -> Option<LinkTarget> {
    let url = url.trim();
    if url.is_empty() || url.starts_with('#') {
        return None;
    }
    if let Some((scheme, rest)) = split_scheme(url) {
        if scheme_is(scheme, "http") || scheme_is(scheme, "https") || scheme_is(scheme, "mailto") {
            return Some(LinkTarget::Browser(url.to_owned()));
        }
        if scheme_is(scheme, "file") {
            let path = file_url_path(rest)?;
            return Some(LinkTarget::File(normalize_path(Path::new(&path))));
        }
        return None;
    }
    resolve_file(document, url).map(LinkTarget::File)
}

/// Browser URL or a path opened in the editor.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum LinkTarget {
    Browser(String),
    File(std::path::PathBuf),
}

const fn scheme_is(scheme: &str, expected: &str) -> bool {
    scheme.eq_ignore_ascii_case(expected)
}

/// `scheme:rest` when `scheme` is not a single Windows drive letter.
fn split_scheme(url: &str) -> Option<(&str, &str)> {
    let colon = url.find(':')?;
    let scheme = &url[..colon];
    if scheme.is_empty()
        || scheme.len() == 1
        || !scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'.' | b'-'))
    {
        return None;
    }
    Some((scheme, &url[colon + 1..]))
}

fn file_url_path(rest: &str) -> Option<String> {
    let text = if let Some(after_slashes) = rest.strip_prefix("//") {
        if let Some(absolute) = after_slashes.strip_prefix('/') {
            format!("/{absolute}")
        } else {
            let (_, path) = after_slashes.split_once('/')?;
            format!("/{path}")
        }
    } else if rest.starts_with('/') {
        rest.to_owned()
    } else {
        return None;
    };
    let path = percent_decode(strip_fragment(&text));
    if path.is_empty() { None } else { Some(path) }
}

fn resolve_file(document: &Path, raw: &str) -> Option<std::path::PathBuf> {
    let text = percent_decode(strip_fragment(raw));
    if text.is_empty() {
        return None;
    }
    let relative = std::path::PathBuf::from(&text);
    let joined = if relative.is_absolute() {
        relative
    } else {
        document.parent()?.join(relative)
    };
    Some(normalize_path(&joined))
}

fn strip_fragment(url: &str) -> &str {
    url.split_once('#').map_or(url, |(path, _)| path)
}

fn normalize_path(path: &Path) -> std::path::PathBuf {
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(high) = bytes.get(index + 1).copied().and_then(hex_value)
            && let Some(low) = bytes.get(index + 2).copied().and_then(hex_value)
        {
            out.push(high * 16 + low);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{LinkTarget, link_target};
    use std::path::{Path, PathBuf};

    fn file(document: &str, url: &str) -> Result<PathBuf, String> {
        match link_target(Path::new(document), url) {
            Some(LinkTarget::File(path)) => Ok(path),
            Some(LinkTarget::Browser(url)) => Err(format!("browser {url}")),
            None => Err(format!("ignored {url}")),
        }
    }

    #[test]
    fn relative_links_resolve_next_to_the_markdown_file() -> Result<(), String> {
        let document = "/repo/README.md";
        assert_eq!(
            file(document, "./documentation/howto/prod-deploy.md")?,
            PathBuf::from("/repo/documentation/howto/prod-deploy.md")
        );
        assert_eq!(
            file(document, "./documentation/howto/mobile-setup.md")?,
            PathBuf::from("/repo/documentation/howto/mobile-setup.md")
        );
        assert_eq!(
            file("/repo/docs/guide.md", "../README.md")?,
            PathBuf::from("/repo/README.md")
        );
        assert_eq!(
            file(document, "/var/notes/absolute.md")?,
            PathBuf::from("/var/notes/absolute.md")
        );
        assert_eq!(
            file(document, "./my%20file.md#section")?,
            PathBuf::from("/repo/my file.md")
        );
        Ok(())
    }

    #[test]
    fn web_links_stay_in_the_browser_and_fragments_do_not_open() -> Result<(), String> {
        let document = Path::new("/repo/README.md");
        assert_eq!(
            link_target(document, "https://example.com/docs#guide"),
            Some(LinkTarget::Browser(
                "https://example.com/docs#guide".to_owned()
            ))
        );
        assert_eq!(
            link_target(document, "HTTP://example.com"),
            Some(LinkTarget::Browser("HTTP://example.com".to_owned()))
        );
        assert_eq!(
            link_target(document, "mailto:dev@example.com"),
            Some(LinkTarget::Browser("mailto:dev@example.com".to_owned()))
        );
        assert_eq!(
            file("/repo/README.md", "file:///var/tmp/a%20b.md#x")?,
            PathBuf::from("/var/tmp/a b.md")
        );
        assert_eq!(
            file("/repo/README.md", "file://localhost/var/tmp/note.md")?,
            PathBuf::from("/var/tmp/note.md")
        );
        assert_eq!(link_target(document, "#heading"), None);
        assert_eq!(link_target(document, "javascript:alert(1)"), None);
        assert_eq!(link_target(document, "   "), None);
        Ok(())
    }
}
