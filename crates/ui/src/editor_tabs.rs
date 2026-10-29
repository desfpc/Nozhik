//! Single-row editor tabs with viewport-derived overflow and guarded file actions.
use crate::{NozhikUi, integration::CloseTarget, theme};
use egui::{Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};
use nozhik_core::BufferId;

const HEIGHT: f32 = 34.0;
const CONTROL: f32 = 28.0;

#[derive(Default)]
pub struct TabStrip {
    active: Option<BufferId>,
    #[cfg(test)]
    observation: Observation,
}

#[derive(Clone, Copy)]
enum TabId {
    File(usize),
    Sql(usize),
}

struct Tab {
    id: TabId,
    buffer: BufferId,
    name: String,
    path: String,
    dirty: bool,
    selected: bool,
}

fn fully_visible(tab: Rect, viewport: Rect) -> bool {
    // Subpixel rounding at the scroll boundary must not create phantom overflow.
    tab.left() >= viewport.left() - 0.5 && tab.right() <= viewport.right() + 0.5
}

fn menu_frame() -> egui::Frame {
    egui::Frame::popup(&egui::Style::default())
        .fill(theme::CHROME)
        .stroke(Stroke::new(1.0_f32, theme::MUTED))
        .corner_radius(8)
        .inner_margin(6)
}

impl NozhikUi {
    fn tab_snapshot(&self) -> Vec<Tab> {
        let sql = self
            .database
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| t.open)
            .map(|(i, t)| Tab {
                id: TabId::Sql(i),
                buffer: t.buffer.snapshot().buffer_id,
                name: format!("{} · {}", t.title, t.connection),
                path: t
                    .path
                    .as_ref()
                    .map_or_else(|| t.connection.clone(), |p| p.display().to_string()),
                dirty: t.buffer.is_dirty(),
                selected: self.database.active == Some(i),
            });
        let files = self
            .documents
            .iter()
            .enumerate()
            .filter(|(_, d)| d.open)
            .map(|(i, d)| Tab {
                id: TabId::File(i),
                buffer: d.buffer.snapshot().buffer_id,
                name: d.name.clone(),
                path: d.path.display().to_string(),
                dirty: d.buffer.is_dirty(),
                selected: self.database.active.is_none() && self.active == Some(i),
            });
        sql.chain(files).collect()
    }

    fn activate_tab(&mut self, id: TabId) {
        match id {
            TabId::File(i) => self.open_document(i),
            TabId::Sql(i) => self.database.active = Some(i),
        }
    }

    pub(crate) fn close_unmodified_tabs(&mut self, ctx: &egui::Context) {
        // Reverse removal retains indices, active selection and the existing buffer lifecycle.
        for i in (0..self.documents.len()).rev() {
            if self.documents[i].open && !self.documents[i].buffer.is_dirty() {
                self.request_close(CloseTarget::File(i), ctx);
            }
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "One row shares geometry with its anchored overflow and actions menus"
    )]
    pub(crate) fn editor_tabs(&mut self, ui: &mut Ui) {
        let popup_limit = ui.max_rect().size() * 0.5;
        let tabs = self.tab_snapshot();
        let font = FontId::proportional(13.0);
        let widths: Vec<_> = tabs
            .iter()
            .map(|t| {
                ui.painter()
                    .layout_no_wrap(t.name.clone(), font.clone(), theme::TEXT)
                    .size()
                    .x
                    + 48.0
            })
            .collect();
        let total: f32 = widths.iter().sum();
        let overflow = total > (ui.available_width() - CONTROL - 4.0).max(0.0) + 0.5;
        let reserved = if overflow {
            CONTROL.mul_add(2.0, 8.0)
        } else {
            CONTROL + 4.0
        };
        let active = tabs.iter().find(|t| t.selected).map(|t| t.buffer);
        let reveal = active != self.tabs.active;
        self.tabs.active = active;
        let mut activate = None;
        let mut close = None;
        let mut hidden = Vec::new();
        #[cfg(test)]
        let mut observation = Observation::default();
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            ui.visuals_mut().clip_rect_margin = 0.0;
            ui.horizontal(|ui| {
                let width = (ui.available_width() - reserved).max(1.0);
                ui.allocate_ui_with_layout(
                    vec2(width, HEIGHT),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_max_width(width);
                        let mut rects = Vec::new();
                        let scroll = egui::ScrollArea::horizontal()
                            .id_salt("editor_tab_scroll")
                            .scroll_bar_visibility(
                                egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                            )
                            .auto_shrink([false, false])
                            .max_width(width)
                            .max_height(HEIGHT)
                            .show(ui, |ui| {
                                for (tab, width) in tabs.iter().zip(&widths) {
                                    let (rect, response) = ui
                                        .allocate_exact_size(vec2(*width, HEIGHT), Sense::click());
                                    rects.push(rect);
                                    if tab.selected && reveal {
                                        ui.scroll_to_rect_animation(
                                            rect,
                                            None,
                                            egui::style::ScrollAnimation::none(),
                                        );
                                    }
                                    let close_rect = Rect::from_center_size(
                                        pos2(rect.right() - 15.0, rect.center().y),
                                        vec2(20.0, 22.0),
                                    );
                                    let close_response = ui.interact(
                                        close_rect,
                                        response.id.with("close"),
                                        Sense::click(),
                                    );
                                    if close_response.clicked() {
                                        close = Some(tab.id);
                                    } else if response.clicked() {
                                        activate = Some(tab.id);
                                    }
                                    paint_tab(
                                        ui,
                                        tab,
                                        rect,
                                        response.hovered(),
                                        close_response.hovered(),
                                        &font,
                                    );
                                    response.on_hover_text(&tab.path);
                                }
                            });
                        #[cfg(test)]
                        {
                            observation.rects = rects.clone();
                            observation.viewport = Some(scroll.inner_rect);
                            observation.scroll_id = Some(scroll.id);
                        }
                        hidden.extend(
                            tabs.iter()
                                .zip(rects)
                                .filter(|(_, rect)| !fully_visible(*rect, scroll.inner_rect))
                                .map(|(t, _)| t),
                        );
                    },
                );
                ui.add_space(4.0);
                if overflow {
                    let button = control(ui, "Show hidden tabs", false);
                    #[cfg(test)]
                    {
                        observation.overflow = Some(button.rect);
                    }
                    if let Some(menu) = overflow_menu(&button, &hidden, popup_limit) {
                        activate = menu.inner.or(activate);
                    }
                    ui.add_space(4.0);
                }
                let button = control(ui, "Tab actions", true);
                #[cfg(test)]
                {
                    observation.actions = Some(button.rect);
                }
                egui::Popup::menu(&button)
                    .frame(menu_frame())
                    .width(220.0)
                    .show(|ui| {
                        if ui.button("Go to File…").clicked() {
                            self.search.open_filter(2);
                            ui.close();
                        }
                        ui.separator();
                        let any = self.documents.iter().any(|d| d.open);
                        if ui
                            .add_enabled(any, egui::Button::new("Close All Tabs"))
                            .clicked()
                        {
                            self.request_close(CloseTarget::Files, ui.ctx());
                            ui.close();
                        }
                        let clean = self
                            .documents
                            .iter()
                            .any(|d| d.open && !d.buffer.is_dirty());
                        if ui
                            .add_enabled(clean, egui::Button::new("Close Unmodified Tabs"))
                            .clicked()
                        {
                            self.close_unmodified_tabs(ui.ctx());
                            ui.close();
                        }
                    });
            });
        });
        #[cfg(test)]
        {
            observation.hidden = hidden.iter().map(|t| t.buffer).collect();
            self.tabs.observation = observation;
        }
        ui.painter().hline(
            ui.min_rect().x_range(),
            ui.cursor().top(),
            Stroke::new(1.0_f32, theme::BORDER),
        );
        ui.add_space(4.0);
        if let Some(id) = activate {
            self.activate_tab(id);
        }
        if let Some(id) = close {
            self.request_close(
                match id {
                    TabId::File(i) => CloseTarget::File(i),
                    TabId::Sql(i) => CloseTarget::Sql(i),
                },
                ui.ctx(),
            );
        }
    }
}

fn overflow_menu(
    button: &egui::Response,
    hidden: &[&Tab],
    limit: Vec2,
) -> Option<egui::InnerResponse<Option<TabId>>> {
    let frame = menu_frame();
    let inner_limit = (limit - frame.total_margin().sum()).max(Vec2::splat(1.0));
    egui::Popup::menu(button)
        .frame(frame)
        .width(limit.x)
        .show(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.spacing_mut().button_padding = vec2(8.0, 3.0);
            let font = egui::TextStyle::Button.resolve(ui.style());
            let labels: Vec<_> = hidden
                .iter()
                .map(|tab| format!("{}{}", tab.name, if tab.dirty { " ●" } else { "" }))
                .collect();
            let text_size = labels.iter().fold(Vec2::ZERO, |size, label| {
                size.max(
                    ui.painter()
                        .layout_no_wrap(label.clone(), font.clone(), theme::TEXT)
                        .size(),
                )
            });
            let row_height = (text_size.y + 6.0).max(ui.spacing().interact_size.y);
            let content_height = labels
                .iter()
                .fold(0.0_f32, |height, _| height + row_height + 2.0)
                - 2.0;
            let scrollbar = if content_height > inner_limit.y {
                ui.spacing().scroll.allocated_width()
            } else {
                0.0
            };
            let size = vec2(text_size.x + 16.0 + scrollbar, content_height)
                .min(inner_limit)
                .max(Vec2::splat(1.0));
            // Area remembers the previous popup size. Set both bounds so short lists
            // shrink again and long lists aren't constrained by the old small area.
            ui.set_min_size(size);
            ui.set_max_size(size);
            let mut activated = None;
            egui::ScrollArea::vertical()
                .id_salt("hidden_editor_tabs")
                .min_scrolled_height(1.0)
                .max_height(size.y)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (tab, label) in hidden.iter().zip(labels) {
                        if ui
                            .add_sized(
                                [ui.available_width(), row_height],
                                egui::Button::selectable(tab.selected, label).truncate(),
                            )
                            .on_hover_text(&tab.path)
                            .clicked()
                        {
                            activated = Some(tab.id);
                            ui.close();
                        }
                    }
                });
            activated
        })
}

fn paint_tab(ui: &Ui, tab: &Tab, rect: Rect, hovered: bool, close_hovered: bool, font: &FontId) {
    let painter = ui.painter();
    if tab.selected || hovered {
        painter.rect_filled(
            rect.shrink2(vec2(0.0, 2.0)),
            CornerRadius {
                nw: 8,
                ne: 8,
                sw: 0,
                se: 0,
            },
            if tab.selected {
                theme::HOVER
            } else {
                theme::PANEL
            },
        );
    }
    painter.text(
        pos2(rect.left() + 12.0, rect.center().y),
        Align2::LEFT_CENTER,
        &tab.name,
        font.clone(),
        theme::TEXT,
    );
    let center = pos2(rect.right() - 15.0, rect.center().y);
    if tab.dirty && !close_hovered {
        painter.circle_filled(center, 3.0, theme::TEXT);
    } else if hovered || tab.selected || close_hovered {
        let stroke = Stroke::new(
            1.2_f32,
            if close_hovered {
                theme::TEXT
            } else {
                theme::MUTED
            },
        );
        painter.line_segment([center - vec2(3.0, 3.0), center + vec2(3.0, 3.0)], stroke);
        painter.line_segment([center + vec2(-3.0, 3.0), center + vec2(3.0, -3.0)], stroke);
    }
}

fn control(ui: &mut Ui, label: &str, dots: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(CONTROL, CONTROL), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    ui.painter().rect_filled(
        rect,
        5.0,
        if response.hovered() || !dots {
            theme::HOVER
        } else {
            Color32::TRANSPARENT
        },
    );
    let center = rect.center();
    if dots {
        for x in [-5.0, 0.0, 5.0] {
            ui.painter()
                .circle_filled(center + vec2(x, 0.0), 1.3, theme::TEXT);
        }
    } else {
        ui.painter().line_segment(
            [center + vec2(-4.0, -2.0), center + vec2(0.0, 2.0)],
            Stroke::new(1.4_f32, theme::TEXT),
        );
        ui.painter().line_segment(
            [center + vec2(0.0, 2.0), center + vec2(4.0, -2.0)],
            Stroke::new(1.4_f32, theme::TEXT),
        );
    }
    response.on_hover_text(label)
}

#[cfg(test)]
#[derive(Default)]
struct Observation {
    rects: Vec<Rect>,
    viewport: Option<Rect>,
    scroll_id: Option<egui::Id>,
    hidden: Vec<BufferId>,
    overflow: Option<Rect>,
    actions: Option<Rect>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Document;
    use egui::{Event, PointerButton, Pos2};

    fn frame(
        shell: &mut NozhikUi,
        ctx: &egui::Context,
        width: f32,
        events: Vec<Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 400.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| shell.editor_tabs(ui));
            },
        )
    }

    fn click(shell: &mut NozhikUi, ctx: &egui::Context, point: Pos2) -> egui::FullOutput {
        frame(
            shell,
            ctx,
            360.0,
            vec![
                Event::PointerMoved(point),
                Event::PointerButton {
                    pos: point,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                Event::PointerButton {
                    pos: point,
                    button: PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        )
    }

    fn text_rect(output: &egui::FullOutput, label: &str) -> Option<Rect> {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text)
                if text.galley.text() == label && shape.clip_rect.contains(text.pos) =>
            {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        })
    }

    #[test]
    fn overflow_popup_grows_shrinks_and_caps_both_dimensions()
    -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let tabs: Vec<_> = (0..30)
            .map(|i| Tab {
                id: TabId::File(i),
                buffer: BufferId(1),
                name: format!(
                    "very_long_filename_that_should_use_the_available_popup_width_and_exceed_half_the_editor_window_width_{i}.php"
                ),
                path: String::new(),
                dirty: false,
                selected: false,
            })
            .collect();
        let short = Tab {
            name: "file.php".into(),
            id: TabId::File(0),
            buffer: BufferId(1),
            path: String::new(),
            dirty: false,
            selected: false,
        };
        let render = |hidden: &[&Tab], screen: Vec2| -> Result<Vec2, Box<dyn std::error::Error>> {
            let mut size = None;
            for _ in 0..3 {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, screen)),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let limit = ui.max_rect().size() * 0.5;
                            let button = ui.button("overflow");
                            egui::Popup::open_id(ctx, egui::Popup::default_response_id(&button));
                            if let Some(menu) = overflow_menu(&button, hidden, limit) {
                                size = Some(menu.response.rect.size());
                                assert!(menu.response.rect.width() <= limit.x + 1.0);
                                assert!(menu.response.rect.height() <= limit.y + 1.0);
                            }
                        });
                    },
                );
                assert_ne!(output.shapes.len(), 0);
            }
            size.ok_or_else(|| "missing popup".into())
        };
        let compact = render(&[&short], vec2(1000.0, 700.0))?;
        assert!(compact.y < 50.0);
        let large = render(&tabs.iter().collect::<Vec<_>>(), vec2(1000.0, 700.0))?;
        assert!(large.x > compact.x * 2.0);
        assert!(large.y > compact.y * 3.0);
        assert!(large.x > 480.0 && large.y > 330.0, "large={large:?}");
        let resized = render(&tabs.iter().collect::<Vec<_>>(), vec2(600.0, 400.0))?;
        assert!(resized.x < large.x && resized.y < large.y);
        let shrunk = render(&[&short], vec2(1000.0, 700.0))?;
        assert!((shrunk.x - compact.x).abs() < 1.0);
        assert!((shrunk.y - compact.y).abs() < 1.0);
        Ok(())
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "End-to-end pointer/wheel sequence verifies geometry and buffer lifecycle together"
    )]
    fn scroll_overflow_menu_and_guarded_bulk_actions() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let bus = nozhik_event_bus::EventBus::new(128)?;
        let settings =
            nozhik_config::ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, nozhik_config::Config::default());
        for i in 0..8 {
            shell.documents.push(Document {
                name: format!("document_{i}.php"),
                path: dir.path().join(format!("document_{i}.php")),
                buffer: nozhik_buffer::TextBuffer::load_from_str(BufferId(i + 1), "original"),
                open: true,
                binary: false,
            });
        }
        shell.active = Some(0);
        let ctx = egui::Context::default();
        frame(&mut shell, &ctx, 360.0, vec![]);
        frame(&mut shell, &ctx, 360.0, vec![]);
        let obs = &shell.tabs.observation;
        let viewport = obs.viewport.ok_or("viewport")?;
        assert!(
            obs.rects
                .windows(2)
                .all(|r| (r[0].top() - r[1].top()).abs() < 0.1)
        );
        assert!(obs.actions.ok_or("actions")?.right() <= 360.0);
        assert!(obs.overflow.is_some());
        assert!(!obs.hidden.contains(&BufferId(1)));
        assert!(obs.hidden.contains(&BufferId(8)));
        let id = obs.scroll_id.ok_or("scroll")?;
        frame(
            &mut shell,
            &ctx,
            360.0,
            vec![
                Event::PointerMoved(viewport.center()),
                Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(-200.0, 0.0),
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        frame(&mut shell, &ctx, 360.0, vec![]);
        assert!(
            egui::scroll_area::State::load(&ctx, id)
                .ok_or("scroll state")?
                .offset
                .x
                > 0.0
        );
        let obs = &shell.tabs.observation;
        assert!(obs.hidden.contains(&BufferId(1)));
        assert!(obs.hidden.contains(&BufferId(8)));
        assert!(
            obs.rects
                .iter()
                .any(|r| r.intersects(viewport) && !fully_visible(*r, viewport))
        );
        let point = obs.overflow.ok_or("overflow")?.center();
        click(&mut shell, &ctx, point);
        let output = frame(&mut shell, &ctx, 360.0, vec![]);
        let target = text_rect(&output, "document_7.php").ok_or("hidden menu item")?;
        click(&mut shell, &ctx, target.center());
        assert_eq!(shell.active, Some(7));
        for _ in 0..10 {
            frame(&mut shell, &ctx, 360.0, vec![]);
        }
        assert!(!shell.tabs.observation.hidden.contains(&BufferId(8)));
        frame(&mut shell, &ctx, 1800.0, vec![]);
        frame(&mut shell, &ctx, 1800.0, vec![]);
        assert!(shell.tabs.observation.overflow.is_none());
        assert_eq!(shell.tabs.observation.hidden.len(), 0);
        frame(&mut shell, &ctx, 360.0, vec![]);
        frame(&mut shell, &ctx, 360.0, vec![]);

        let point = shell.tabs.observation.actions.ok_or("actions")?.center();
        click(&mut shell, &ctx, point);
        let output = frame(&mut shell, &ctx, 360.0, vec![]);
        let target = text_rect(&output, "Go to File…").ok_or("go to file")?;
        click(&mut shell, &ctx, target.center());
        assert!(shell.search.open);
        assert!(shell.search.files_filter_selected());
        shell.search.open = false;
        frame(&mut shell, &ctx, 360.0, vec![]);
        shell.documents[7].buffer.insert(0, "edited")?;
        let point = shell.tabs.observation.actions.ok_or("actions")?.center();
        click(&mut shell, &ctx, point);
        let output = frame(&mut shell, &ctx, 360.0, vec![]);
        let target = text_rect(&output, "Close Unmodified Tabs").ok_or("close clean")?;
        click(&mut shell, &ctx, target.center());
        assert_eq!(shell.documents.len(), 1);
        assert!(shell.documents[0].buffer.is_dirty());
        assert_eq!(shell.active, Some(0));
        frame(&mut shell, &ctx, 360.0, vec![]);
        assert!(shell.tabs.observation.overflow.is_none());
        let point = shell.tabs.observation.actions.ok_or("actions")?.center();
        click(&mut shell, &ctx, point);
        let output = frame(&mut shell, &ctx, 360.0, vec![]);
        let target = text_rect(&output, "Close All Tabs").ok_or("close all")?;
        click(&mut shell, &ctx, target.center());
        assert!(matches!(shell.integration.close, Some(CloseTarget::Files)));
        assert_eq!(shell.documents.len(), 1);
        // Cancel keeps every modified buffer; a clean bulk close releases all files.
        shell.integration.close = None;
        shell.documents[0].buffer.mark_saved();
        shell.request_close(CloseTarget::Files, &ctx);
        assert!(shell.documents.is_empty());
        assert!(shell.active.is_none());
        Ok(())
    }
}
