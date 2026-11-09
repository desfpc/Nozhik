//! Rendered Markdown. The text editor remains the source of the document.
use crate::theme;
use egui::{RichText, Stroke, Ui};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
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

struct Doc {
    gap: bool,
    space: f32,
    tables: usize,
}

pub fn show(ui: &mut Ui, path: &Path, source: &str, enabled: bool) {
    egui::ScrollArea::vertical()
        .id_salt(("markdown-gui", path.to_path_buf()))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add_enabled_ui(enabled, |ui| {
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
                    space: 8.0,
                    tables: 0,
                };
                render_until(ui, &mut events, None, &mut doc);
            });
        });
}

fn markdown_options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
}

fn render_until(ui: &mut Ui, events: &mut Events<'_>, stop: Option<TagEnd>, doc: &mut Doc) {
    while let Some(event) = events.next() {
        match event {
            Event::End(end) if stop == Some(end) => break,
            Event::End(_) | Event::SoftBreak | Event::HardBreak | Event::TaskListMarker(_) => {}
            Event::Rule => {
                block_gap(ui, doc);
                paint_rule(ui);
            }
            Event::Start(tag) => render_start(ui, events, tag, doc),
            Event::Text(text) => {
                block_gap(ui, doc);
                paint_inlines(ui, &[Inline::Text(text.to_string())], &Face::default());
            }
            Event::Code(text)
            | Event::InlineMath(text)
            | Event::DisplayMath(text)
            | Event::Html(text)
            | Event::InlineHtml(text) => {
                block_gap(ui, doc);
                paint_inlines(ui, &[Inline::Code(text.to_string())], &Face::default());
            }
            Event::FootnoteReference(name) => {
                block_gap(ui, doc);
                paint_inlines(ui, &[Inline::Text(format!("[{name}]"))], &Face::default());
            }
        }
    }
}

fn render_start(ui: &mut Ui, events: &mut Events<'_>, tag: Tag<'_>, doc: &mut Doc) {
    let end = tag.to_end();
    match tag {
        Tag::Paragraph => {
            block_gap(ui, doc);
            let items = take_inlines(events, end);
            paint_inlines(ui, &items, &Face::default());
        }
        Tag::Heading { level, .. } => {
            block_gap(ui, doc);
            let items = take_inlines(events, end);
            paint_heading(ui, level, &items);
        }
        Tag::BlockQuote(_) => {
            block_gap(ui, doc);
            paint_quote(ui, events, end, doc);
        }
        Tag::CodeBlock(kind) => {
            block_gap(ui, doc);
            paint_code_block(ui, events, &kind);
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
            paint_code_frame(ui, None, &text);
        }
        Tag::List(start) => render_list(ui, events, start, doc),
        Tag::Table(_) => {
            block_gap(ui, doc);
            paint_table(ui, events, doc);
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

fn render_list(ui: &mut Ui, events: &mut Events<'_>, start: Option<u64>, doc: &mut Doc) {
    let mut number = start.unwrap_or(1);
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
                    Some(true) => "☑".to_owned(),
                    Some(false) => "☐".to_owned(),
                    None if start.is_some() => {
                        let label = format!("{number}.");
                        number += 1;
                        label
                    }
                    None => "•".to_owned(),
                };
                ui.horizontal_top(|ui| {
                    ui.label(RichText::new(marker).color(theme::muted()));
                    ui.vertical(|ui| {
                        let saved = doc.space;
                        doc.space = 2.0;
                        render_until(ui, events, Some(TagEnd::Item), doc);
                        doc.space = saved;
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
}

fn paint_quote(ui: &mut Ui, events: &mut Events<'_>, end: TagEnd, doc: &mut Doc) {
    ui.horizontal_top(|ui| {
        ui.add_space(12.0);
        let saved = doc.space;
        doc.space = 4.0;
        let inner = ui.vertical(|ui| render_until(ui, events, Some(end), doc));
        doc.space = saved;
        let bar = egui::Rect::from_min_size(
            inner.response.rect.left_top() - egui::vec2(10.0, 0.0),
            egui::vec2(3.0, inner.response.rect.height()),
        );
        ui.painter()
            .rect_filled(bar, 1.0_f32, theme::border_strong());
    });
}

fn paint_code_block(ui: &mut Ui, events: &mut Events<'_>, kind: &CodeBlockKind<'_>) {
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
    paint_code_frame(ui, language.as_deref(), &text);
}

fn paint_code_frame(ui: &mut Ui, language: Option<&str>, text: &str) {
    egui::Frame::new()
        .fill(theme::surface())
        .corner_radius(theme::RADIUS)
        .inner_margin(10)
        .show(ui, |ui| {
            if let Some(language) = language {
                ui.label(RichText::new(language).small().color(theme::muted()));
            }
            ui.add(
                egui::Label::new(
                    RichText::new(text.trim_end_matches('\n'))
                        .monospace()
                        .color(theme::text()),
                )
                .wrap(),
            );
        });
}

fn paint_table(ui: &mut Ui, events: &mut Events<'_>, doc: &mut Doc) {
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
    let id = doc.tables;
    egui::ScrollArea::horizontal()
        .id_salt(("markdown-table", id))
        .show(ui, |ui| {
            egui::Grid::new(("markdown-grid", id))
                .spacing(egui::vec2(0.0, 0.0))
                .show(ui, |ui| {
                    if !header.is_empty() {
                        for cell in &header {
                            paint_cell(ui, cell, true);
                        }
                        ui.end_row();
                    }
                    for cells in &rows {
                        for cell in cells {
                            paint_cell(ui, cell, false);
                        }
                        ui.end_row();
                    }
                });
        });
}

fn paint_cell(ui: &mut Ui, items: &[Inline], header: bool) {
    let fill = if header {
        theme::surface()
    } else {
        theme::background()
    };
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0_f32, theme::border()))
        .inner_margin(8)
        .show(ui, |ui| {
            ui.set_min_width(72.0);
            let face = Face {
                strong: header,
                ..Face::default()
            };
            paint_inlines(ui, items, &face);
        });
}

fn paint_heading(ui: &mut Ui, level: HeadingLevel, items: &[Inline]) {
    let size = match level {
        HeadingLevel::H1 => 26.0,
        HeadingLevel::H2 => 22.0,
        HeadingLevel::H3 => 18.0,
        HeadingLevel::H4 => 16.0,
        HeadingLevel::H5 | HeadingLevel::H6 => 15.0,
    };
    let face = Face {
        strong: true,
        size: Some(size),
        ..Face::default()
    };
    paint_inlines(ui, items, &face);
    ui.add_space(2.0);
}

fn paint_rule(ui: &mut Ui) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 8.0), egui::Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0_f32, theme::border()),
    );
}

fn block_gap(ui: &mut Ui, doc: &mut Doc) {
    if doc.gap {
        ui.add_space(doc.space);
    }
    doc.gap = true;
}

fn take_inlines(events: &mut Events<'_>, end: TagEnd) -> Vec<Inline> {
    let mut items = Vec::new();
    while let Some(event) = events.next() {
        match event {
            Event::End(found) if found == end => break,
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
            Event::End(_) | Event::Rule => break,
        }
    }
    items
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

fn paint_inlines(ui: &mut Ui, items: &[Inline], face: &Face) {
    let mut line = Vec::new();
    for item in items {
        if matches!(item, Inline::Break { hard: true }) {
            paint_line(ui, &line, face);
            line.clear();
        } else {
            line.push(item);
        }
    }
    if !line.is_empty() {
        paint_line(ui, &line, face);
    }
}

fn paint_line(ui: &mut Ui, items: &[&Inline], face: &Face) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.spacing_mut().item_spacing.y = 2.0;
        for item in items {
            paint_item(ui, item, face);
        }
    });
}

fn paint_item(ui: &mut Ui, item: &Inline, face: &Face) {
    match item {
        Inline::Text(text) => paint_text(ui, text, face),
        Inline::Code(text) | Inline::Html(text) => paint_code_span(ui, text, face),
        Inline::Break { .. } => paint_text(ui, " ", face),
        Inline::Task(checked) => paint_text(ui, if *checked { "☑ " } else { "☐ " }, face),
        Inline::Emphasis(children) => {
            let mut next = face.clone();
            next.emphasis = true;
            paint_inlines(ui, children, &next);
        }
        Inline::Strong(children) => {
            let mut next = face.clone();
            next.strong = true;
            paint_inlines(ui, children, &next);
        }
        Inline::Strike(children) => {
            let mut next = face.clone();
            next.strike = true;
            paint_inlines(ui, children, &next);
        }
        Inline::Link { url, children } => {
            let mut next = face.clone();
            next.link = Some(url.clone());
            paint_inlines(ui, children, &next);
        }
        Inline::Image { children } => {
            let mut next = face.clone();
            next.emphasis = true;
            if children.is_empty() {
                paint_text(ui, "image", &next);
            } else {
                paint_inlines(ui, children, &next);
            }
        }
    }
}

fn paint_text(ui: &mut Ui, value: &str, face: &Face) {
    if value.is_empty() {
        return;
    }
    let mut text = RichText::new(value).size(face.size.unwrap_or_else(|| body_size(ui)));
    text = if face.strong {
        text.family(theme::bold_family())
            .color(theme::text_strong())
    } else {
        text.color(theme::text())
    };
    if face.emphasis {
        text = text.italics();
    }
    if face.strike {
        text = text.strikethrough();
    }
    if let Some(url) = &face.link {
        text = text.color(theme::link()).underline();
        let response = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if response.clicked() && safe_url(url) {
            crate::editor_view::open_documentation(url.clone());
        }
        return;
    }
    ui.label(text);
}

fn paint_code_span(ui: &mut Ui, value: &str, face: &Face) {
    let mut text = RichText::new(value)
        .monospace()
        .size((face.size.unwrap_or_else(|| body_size(ui)) - 1.0).max(11.0))
        .color(theme::syntax_color("markup.raw"));
    if face.strike {
        text = text.strikethrough();
    }
    egui::Frame::new()
        .fill(theme::chip())
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(4, 1))
        .show(ui, |ui| {
            ui.label(text);
        });
}

fn body_size(ui: &Ui) -> f32 {
    egui::TextStyle::Body.resolve(ui.style()).size
}

fn safe_url(url: &str) -> bool {
    let url = url.trim();
    url.starts_with("https://") || url.starts_with("http://") || url.starts_with("mailto:")
}
