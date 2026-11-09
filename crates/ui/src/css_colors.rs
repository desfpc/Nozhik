//! CSS gutter swatches and revision-bound color editing.
use egui::{Color32, Pos2, Rect, Sense, Ui};
use nozhik_buffer::{BufferError, EditTransaction, TextBuffer, TextEdit};
use nozhik_language_support::css::ColorOccurrence;

pub struct Picker {
    buffer: nozhik_core::BufferId,
    revision: u64,
    range: std::ops::Range<usize>,
    color: Color32,
    rgba: [u8; 4],
    hex: String,
    origin: Pos2,
}
impl Picker {
    pub fn new(buffer: &TextBuffer, occurrence: &ColorOccurrence, origin: Pos2) -> Self {
        let [r, g, b, a] = occurrence.rgba;
        Self {
            buffer: buffer.snapshot().buffer_id,
            revision: buffer.snapshot().revision,
            range: occurrence.edit_range.clone(),
            color: Color32::from_rgba_unmultiplied(r, g, b, a),
            rgba: occurrence.rgba,
            hex: literal(occurrence.rgba),
            origin,
        }
    }
    pub fn valid(&self, buffer: &TextBuffer) -> bool {
        self.buffer == buffer.snapshot().buffer_id && self.revision == buffer.snapshot().revision
    }
    pub fn show(&mut self, ui: &Ui, buffer: &mut TextBuffer) -> Result<bool, BufferError> {
        let mut close = false;
        let mut changed = false;
        let popup = egui::Area::new(ui.id().with(("css_color_picker", self.buffer)))
            .order(egui::Order::Foreground)
            .fixed_pos(self.origin)
            .constrain(true)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(280.0);
                    ui.horizontal(|ui| {
                        ui.strong("CSS color");
                        if ui.button("×").clicked() {
                            close = true;
                        }
                    });
                    changed |= egui::color_picker::color_picker_color32(
                        ui,
                        &mut self.color,
                        egui::color_picker::Alpha::OnlyBlend,
                    );
                    if changed {
                        self.rgba = self.color.to_srgba_unmultiplied();
                    }
                    let mut rgba = self.rgba;
                    ui.horizontal(|ui| {
                        for (label, value) in ["R", "G", "B", "A"].into_iter().zip(&mut rgba) {
                            changed |= ui
                                .add(
                                    egui::DragValue::new(value)
                                        .range(0..=255)
                                        .prefix(format!("{label} ")),
                                )
                                .changed();
                        }
                    });
                    if changed {
                        self.color =
                            Color32::from_rgba_unmultiplied(rgba[0], rgba[1], rgba[2], rgba[3]);
                        self.rgba = rgba;
                        self.hex = literal(rgba);
                    }
                    ui.horizontal(|ui| {
                        ui.label("Hex");
                        if ui.add(crate::form::singleline(&mut self.hex)).changed()
                            && let Some(rgba) = parse_hex(&self.hex)
                        {
                            self.color =
                                Color32::from_rgba_unmultiplied(rgba[0], rgba[1], rgba[2], rgba[3]);
                            self.rgba = rgba;
                            changed = true;
                        }
                    });
                });
            });
        if changed {
            self.replace(buffer, &literal(self.rgba))?;
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape))
            || (ui.input(|i| i.pointer.any_pressed())
                && ui
                    .input(|i| i.pointer.interact_pos())
                    .is_some_and(|p| !popup.response.rect.contains(p)))
        {
            close = true;
        }
        Ok(close)
    }
    fn replace(&mut self, buffer: &mut TextBuffer, text: &str) -> Result<(), BufferError> {
        if !self.valid(buffer) {
            return Ok(());
        }
        buffer.apply_transaction(EditTransaction {
            edits: vec![TextEdit {
                range: self.range.clone(),
                text: text.into(),
            }],
            group: None,
        })?;
        self.range.end = self.range.start + text.len();
        self.revision = buffer.snapshot().revision;
        Ok(())
    }
}
fn literal([r, g, b, a]: [u8; 4]) -> String {
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}
fn parse_hex(raw: &str) -> Option<[u8; 4]> {
    let raw = raw.trim().trim_start_matches('#');
    if !matches!(raw.len(), 3 | 4 | 6 | 8) || !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let expanded = if raw.len() <= 4 {
        raw.chars().flat_map(|c| [c, c]).collect::<String>()
    } else {
        raw.into()
    };
    let mut rgba = [255; 4];
    for (channel, bytes) in rgba.iter_mut().zip(expanded.as_bytes().as_chunks::<2>().0) {
        *channel = u8::from_str_radix(std::str::from_utf8(bytes).ok()?, 16).ok()?;
    }
    Some(rgba)
}
pub fn swatch(ui: &Ui, id: egui::Id, rect: Rect, rgba: [u8; 4], enabled: bool) -> egui::Response {
    let size = rect.width() / 2.0;
    for y in 0u8..2 {
        for x in 0u8..2 {
            ui.painter().rect_filled(
                Rect::from_min_size(
                    rect.min + egui::vec2(f32::from(x) * size, f32::from(y) * size),
                    egui::vec2(size, size),
                ),
                0.0,
                if x == y {
                    Color32::from_gray(180)
                } else {
                    Color32::from_gray(100)
                },
            );
        }
    }
    ui.painter().rect_filled(
        rect,
        0.0,
        Color32::from_rgba_unmultiplied(rgba[0], rgba[1], rgba[2], rgba[3]),
    );
    ui.painter().rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0_f32, Color32::from_gray(85)),
        egui::StrokeKind::Inside,
    );
    ui.interact(
        rect,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    )
    .on_hover_text(format!("{} · Click to edit color", literal(rgba)))
}
