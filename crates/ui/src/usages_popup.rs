//! Keyboard-navigable usages browser with file, line and highlighted source columns.
use nozhik_language_support::symbol_index::{ReferenceLocation, SymbolDeclaration, SymbolIndex};

#[allow(
    clippy::struct_excessive_bools,
    reason = "Open/detach and two independent filter toggles have valid combinations"
)]
pub struct UsagesPopup {
    pub open: bool,
    title: String,
    name: String,
    rows: Vec<ReferenceLocation>,
    selected: usize,
    detached: bool,
    grouped: bool,
    incoming_only: bool,
    filter: String,
}
impl UsagesPopup {
    pub fn new(index: &SymbolIndex, symbol: &SymbolDeclaration) -> Self {
        let mut rows: Vec<_> = index
            .references_by_symbol
            .get(&symbol.fqn)
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        rows.sort_by(|a, b| (&a.path, a.byte).cmp(&(&b.path, b.byte)));
        Self {
            open: true,
            title: format!("{:?} {}", symbol.kind, symbol.fqn),
            name: symbol.name.trim_start_matches('$').into(),
            rows,
            selected: 0,
            detached: false,
            grouped: true,
            incoming_only: false,
            filter: String::new(),
        }
    }
    pub fn show(&mut self, ctx: &egui::Context) -> Option<ReferenceLocation> {
        if self.detached {
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("usages_window"),
                egui::ViewportBuilder::default()
                    .with_title(&self.title)
                    .with_inner_size([920.0, 560.0]),
                |ctx, _| {
                    if ctx.input(|i| i.viewport().close_requested()) {
                        self.open = false;
                    }
                    self.show_window(ctx)
                },
            )
        } else {
            self.show_window(ctx)
        }
    }
    #[allow(
        clippy::too_many_lines,
        reason = "One popup pass coordinates keyboard routing and four-column row geometry"
    )]
    fn show_window(&mut self, ctx: &egui::Context) -> Option<ReferenceLocation> {
        let mut target = None;
        let mut open = self.open;
        egui::Window::new(&self.title)
            .id(egui::Id::new("usages_popup"))
            .open(&mut open)
            .title_bar(false)
            .collapsible(false)
            .resizable(true)
            .default_width(880.0)
            .max_width((ctx.screen_rect().width() - 20.0).max(100.0))
            .default_height(if self.detached { 500.0 } else { 290.0 })
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(egui::Color32::from_rgb(43, 45, 48))
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        egui::Color32::from_rgb(57, 59, 64),
                    ))
                    .corner_radius(7),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(&self.title);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button("↗")
                            .on_hover_text("Open usages in a separate window")
                            .clicked()
                        {
                            self.detached = !self.detached;
                        }
                        if ui.small_button("×").clicked() {
                            self.open = false;
                        }
                        ui.weak(format!("{} usages", self.rows.len()));
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.incoming_only, "↙ Incoming calls");
                    ui.label("All Places");
                    ui.checkbox(&mut self.grouped, "Group by file");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.filter)
                            .hint_text("Filter file / code")
                            .desired_width(180.0),
                    );
                });
                ui.separator();
                let mut rows: Vec<_> = self
                    .rows
                    .iter()
                    .filter(|r| {
                        !self.incoming_only
                            || r.preview
                                .get((r.column as usize).saturating_add(r.length)..)
                                .is_some_and(|tail| tail.trim_start().starts_with('('))
                    })
                    .filter(|r| {
                        self.filter.is_empty()
                            || format!("{} {}", r.path.display(), r.preview)
                                .to_lowercase()
                                .contains(&self.filter.to_lowercase())
                    })
                    .collect();
                if !self.grouped {
                    rows.sort_by_key(|r| (r.line, r.byte));
                }
                self.selected = self.selected.min(rows.len().saturating_sub(1));
                let mut moved = false;
                ui.input_mut(|i| {
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                        self.open = false;
                    }
                    if !rows.is_empty() {
                        if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                            self.selected = (self.selected + 1) % rows.len();
                            moved = true;
                        }
                        if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                            self.selected = self.selected.checked_sub(1).unwrap_or(rows.len() - 1);
                            moved = true;
                        }
                        if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                            target = rows.get(self.selected).map(|r| (*r).clone());
                        }
                    }
                });
                if rows.is_empty() {
                    ui.weak("No usages found");
                }
                egui::ScrollArea::both()
                    .max_height(if self.detached { 460.0 } else { 250.0 })
                    .show(ui, |ui| {
                        for (i, location) in rows.iter().enumerate() {
                            let frame = egui::Frame::new()
                                .fill(if self.selected == i {
                                    egui::Color32::from_rgb(46, 67, 110)
                                } else {
                                    egui::Color32::TRANSPARENT
                                })
                                .inner_margin(4);
                            let response = frame
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.colored_label(
                                            egui::Color32::from_rgb(87, 147, 255),
                                            "(C)",
                                        );
                                        ui.add_sized(
                                            [230.0, 20.0],
                                            egui::Label::new(
                                                location
                                                    .path
                                                    .file_name()
                                                    .unwrap_or_default()
                                                    .to_string_lossy(),
                                            ),
                                        )
                                        .on_hover_text(location.path.display().to_string());
                                        ui.add_sized(
                                            [42.0, 20.0],
                                            egui::Label::new(
                                                egui::RichText::new(
                                                    (location.line + 1).to_string(),
                                                )
                                                .color(egui::Color32::GRAY),
                                            ),
                                        );
                                        let mut job = egui::text::LayoutJob::default();
                                        let start = usize::try_from(location.column)
                                            .unwrap_or(0)
                                            .min(location.preview.len());
                                        let end =
                                            (start + location.length).min(location.preview.len());
                                        if let (Some(before), Some(name), Some(after)) = (
                                            location.preview.get(..start),
                                            location.preview.get(start..end),
                                            location.preview.get(end..),
                                        ) {
                                            for (text, accent) in
                                                [(before, false), (name, true), (after, false)]
                                            {
                                                job.append(
                                                    text,
                                                    0.0,
                                                    egui::TextFormat {
                                                        font_id: egui::FontId::monospace(13.0),
                                                        color: if accent {
                                                            egui::Color32::from_rgb(255, 211, 115)
                                                        } else {
                                                            egui::Color32::LIGHT_GRAY
                                                        },
                                                        ..Default::default()
                                                    },
                                                );
                                            }
                                        }
                                        ui.label(job).on_hover_text(&self.name);
                                    });
                                })
                                .response;
                            let click =
                                ui.interact(response.rect, ui.id().with(i), egui::Sense::click());
                            if moved && i == self.selected {
                                click.scroll_to_me(Some(egui::Align::Center));
                            }
                            if click.clicked() {
                                target = Some((*location).clone());
                            }
                        }
                    });
            });
        self.open &= open && target.is_none();
        target
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn usages_rows_keyboard_filter_and_escape() -> Result<(), Box<dyn std::error::Error>> {
        let mut popup = UsagesPopup {
            open: true,
            title: "Method \\App\\Car.run".into(),
            name: "run".into(),
            rows: (0..2)
                .map(|i| ReferenceLocation {
                    path: format!("caller{i}.php").into(),
                    byte: i * 10,
                    length: 3,
                    line: u32::try_from(i).unwrap_or(0),
                    column: 7,
                    preview: "$car-> run();".into(),
                })
                .collect(),
            selected: 0,
            detached: false,
            grouped: true,
            incoming_only: false,
            filter: String::new(),
        };
        let ctx = egui::Context::default();
        let mut target = None;
        for key in [
            None,
            None,
            Some(egui::Key::ArrowDown),
            Some(egui::Key::Enter),
        ] {
            let _frame = ctx.run(
                egui::RawInput {
                    events: key
                        .into_iter()
                        .map(|key| egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        })
                        .collect(),
                    ..Default::default()
                },
                |ctx| {
                    target = popup.show(ctx);
                },
            );
        }
        assert_eq!(
            target.ok_or("no selected usage")?.path,
            std::path::PathBuf::from("caller1.php")
        );
        assert!(!popup.open);
        popup.open = true;
        let _frame = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                popup.show(ctx);
            },
        );
        assert!(!popup.open);
        Ok(())
    }
}
