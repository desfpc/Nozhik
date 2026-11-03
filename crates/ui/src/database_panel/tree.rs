//! Compact database object tree. All measurements are logical pixels (2× on Retina).
use super::DatabaseUi;
use egui::{
    Color32, FontId, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2,
};
use nozhik_database::{ConnectionProfile, DbDriverType, introspection::TableInfo};

const TEXT: Color32 = Color32::from_rgb(205, 207, 212);
const MUTED: Color32 = Color32::from_rgb(112, 115, 122);
const BLUE: Color32 = Color32::from_rgb(78, 132, 255);
const GOLD: Color32 = Color32::from_rgb(255, 199, 55);
const SELECTED: Color32 = Color32::from_rgb(47, 67, 109);
const HEIGHT: f32 = 24.0;
const INDENT: f32 = 19.0;

pub(super) enum Action {
    Schema(ConnectionProfile, String, super::schema::Action),
    Connect(ConnectionProfile),
    Password(ConnectionProfile),
    Edit(ConnectionProfile),
    Refresh(String),
    Disconnect(String),
    Console(String, String),
    Describe(String, String, TableInfo, bool),
    Open(ConnectionProfile, String, TableInfo),
}
#[derive(Clone, Copy)]
enum Icon {
    Database,
    Folder,
    Table,
    View,
    Column {
        nullable: bool,
        primary: bool,
        foreign: bool,
    },
    Key,
    ForeignKey,
    Index {
        unique: bool,
    },
}

struct Tree<'a> {
    selected: &'a mut Option<egui::Id>,
    actions: Vec<Action>,
}
impl Tree<'_> {
    #[allow(
        clippy::too_many_arguments,
        reason = "One row paints independent icon, label, detail and disclosure state"
    )]
    fn row(
        &mut self,
        ui: &mut Ui,
        id: egui::Id,
        depth: u16,
        icon: Icon,
        name: &str,
        detail: &str,
        open: Option<bool>,
    ) -> Response {
        let label = ui
            .painter()
            .layout_no_wrap(name.into(), FontId::proportional(14.0), TEXT);
        // The proportional UI font has no arrow glyph. Paint references as vectors
        // between native text segments, so they never become missing-glyph squares.
        let secondary: Vec<_> = detail
            .split('→')
            .map(|part| {
                ui.painter()
                    .layout_no_wrap(part.into(), FontId::proportional(12.0), MUTED)
            })
            .collect();
        let secondary_width: f32 = secondary
            .iter()
            .enumerate()
            .map(|(index, part)| part.size().x + if index > 0 { 12.0 } else { 0.0 })
            .sum();
        let offset = f32::from(depth) * INDENT;
        let width = (offset + 52.0 + label.size().x + secondary_width).max(ui.available_width());
        let (rect, _) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::hover());
        let response = ui.interact(rect, id, Sense::click());
        if response.clicked() || response.secondary_clicked() {
            *self.selected = Some(id);
            response.request_focus();
        }
        let selected = *self.selected == Some(id);
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, name)
        });
        let painter = ui.painter();
        if selected || response.hovered() {
            painter.rect_filled(
                rect,
                0.0,
                if selected {
                    SELECTED
                } else {
                    crate::theme::HOVER
                },
            );
        }
        let x = rect.left() + offset;
        let y = rect.center().y;
        if let Some(open) = open {
            let points = if open {
                vec![
                    pos2(x + 6.0, y - 2.0),
                    pos2(x + 9.5, y + 1.5),
                    pos2(x + 13.0, y - 2.0),
                ]
            } else {
                vec![
                    pos2(x + 8.0, y - 4.0),
                    pos2(x + 11.5, y),
                    pos2(x + 8.0, y + 4.0),
                ]
            };
            painter.add(Shape::line(points, Stroke::new(1.0_f32, TEXT)));
        }
        paint_icon(painter, pos2(x + 24.0, y), icon);
        let position = pos2(x + 38.0, y - label.size().y / 2.0);
        let label_width = label.size().x;
        painter.galley(position, label, TEXT);
        let mut detail_x = position.x + label_width + 8.0;
        let color = if selected { TEXT } else { MUTED };
        for (index, part) in secondary.iter().enumerate() {
            if index > 0 {
                painter.line_segment(
                    [pos2(detail_x + 1.0, y), pos2(detail_x + 10.0, y)],
                    Stroke::new(1.0_f32, color),
                );
                painter.add(Shape::line(
                    vec![
                        pos2(detail_x + 6.0, y - 3.0),
                        pos2(detail_x + 10.0, y),
                        pos2(detail_x + 6.0, y + 3.0),
                    ],
                    Stroke::new(1.0_f32, color),
                ));
                detail_x += 12.0;
            }
            painter.galley_with_override_text_color(
                pos2(detail_x, y - part.size().y / 2.0),
                part.clone(),
                color,
            );
            detail_x += part.size().x;
        }
        response.on_hover_text(if detail.is_empty() {
            name.into()
        } else {
            format!("{name}  {detail}")
        })
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "Tree branch shares the row's explicit visual fields"
    )]
    fn branch(
        &mut self,
        ui: &mut Ui,
        id: egui::Id,
        depth: u16,
        icon: Icon,
        name: &str,
        detail: &str,
        body: impl FnOnce(&mut Self, &mut Ui),
    ) -> Response {
        let mut state =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
        let response = self.row(ui, id, depth, icon, name, detail, Some(state.is_open()));
        if response.clicked() {
            state.toggle(ui);
        }
        if response.has_focus() {
            if ui.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
                state.set_open(true);
            }
            if ui.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
                state.set_open(false);
            }
        }
        state.show_body_unindented(ui, |ui| body(self, ui));
        response
    }
    fn tables(
        &mut self,
        ui: &mut Ui,
        db: &DatabaseUi,
        profile: &ConnectionProfile,
        schema: &nozhik_database::introspection::SchemaInfo,
        depth: u16,
    ) {
        for (views, name) in [(false, "tables"), (true, "views")] {
            let count = schema.tables.iter().filter(|t| t.is_view == views).count();
            if views && count == 0 {
                continue;
            }
            let id = ui.id().with((&profile.name, &schema.name, name));
            self.branch(
                ui,
                id,
                depth,
                Icon::Folder,
                name,
                &count.to_string(),
                |tree, ui| {
                    for table in schema.tables.iter().filter(|t| t.is_view == views) {
                        let id = id.with(&table.name);
                        let response = tree.branch(
                            ui,
                            id,
                            depth + 1,
                            if views { Icon::View } else { Icon::Table },
                            &table.name,
                            "",
                            |tree, ui| {
                                if table.details_loaded {
                                    tree.details(ui, id, depth + 2, profile, &schema.name, table);
                                } else if let Some(error) = db.table_errors.get(&(
                                    profile.name.clone(),
                                    schema.name.clone(),
                                    table.name.clone(),
                                )) {
                                    ui.colored_label(crate::theme::ERROR, error);
                                    if ui.button("Retry table metadata").clicked() {
                                        tree.actions.push(Action::Describe(
                                            profile.name.clone(),
                                            schema.name.clone(),
                                            table.clone(),
                                            true,
                                        ));
                                    }
                                } else {
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.label("Loading table metadata…");
                                    });
                                    tree.actions.push(Action::Describe(
                                        profile.name.clone(),
                                        schema.name.clone(),
                                        table.clone(),
                                        false,
                                    ));
                                }
                            },
                        );
                        if response.double_clicked() {
                            tree.actions.push(Action::Open(
                                profile.clone(),
                                schema.name.clone(),
                                table.clone(),
                            ));
                        }
                        response.context_menu(|ui| {
                            if ui.button("Open Table (First 500 rows)").clicked() {
                                tree.actions.push(Action::Open(
                                    profile.clone(),
                                    schema.name.clone(),
                                    table.clone(),
                                ));
                                ui.close();
                            }
                            if ui.button("New SQL Console").clicked() {
                                tree.actions.push(Action::Console(
                                    profile.name.clone(),
                                    schema.name.clone(),
                                ));
                                ui.close();
                            }
                        });
                    }
                },
            );
        }
    }
    fn details(
        &mut self,
        ui: &mut Ui,
        id: egui::Id,
        depth: u16,
        profile: &ConnectionProfile,
        schema: &str,
        table: &TableInfo,
    ) {
        self.branch(
            ui,
            id.with("columns"),
            depth,
            Icon::Folder,
            "columns",
            &table.columns.len().to_string(),
            |tree, ui| {
                for column in &table.columns {
                    tree.row(
                        ui,
                        id.with(("column", &column.name)),
                        depth + 1,
                        Icon::Column {
                            nullable: column.nullable,
                            primary: column.is_primary_key,
                            foreign: column.is_foreign_key,
                        },
                        &column.name,
                        &column.display_type(),
                        None,
                    );
                }
            },
        );
        if let Some(error) = &table.objects.error {
            ui.colored_label(
                crate::theme::ERROR,
                format!("Keys/DDL unavailable: {error}"),
            );
            if ui.button("Retry table metadata").clicked() {
                self.actions.push(Action::Describe(
                    profile.name.clone(),
                    schema.into(),
                    table.clone(),
                    true,
                ));
            }
            return;
        }
        for (name, objects, icon) in [
            ("keys", &table.objects.keys, Icon::Key),
            (
                "foreign keys",
                &table.objects.foreign_keys,
                Icon::ForeignKey,
            ),
            (
                "indexes",
                &table.objects.indexes,
                Icon::Index { unique: false },
            ),
        ] {
            self.branch(
                ui,
                id.with(name),
                depth,
                Icon::Folder,
                name,
                &objects.len().to_string(),
                |tree, ui| {
                    for object in objects {
                        let icon = if name == "indexes" {
                            Icon::Index {
                                unique: object.definition.ends_with(" UNIQUE")
                                    || object.definition.starts_with("CREATE UNIQUE INDEX"),
                            }
                        } else {
                            icon
                        };
                        tree.row(
                            ui,
                            id.with((name, &object.name)),
                            depth + 1,
                            icon,
                            &object.name,
                            &object.definition,
                            None,
                        );
                    }
                },
            );
        }
    }
}

pub(super) fn show(ui: &mut Ui, db: &DatabaseUi, selected: &mut Option<egui::Id>) -> Vec<Action> {
    let mut tree = Tree {
        selected,
        actions: Vec::new(),
    };
    ui.spacing_mut().item_spacing.y = 0.0;
    for profile in &db.profiles {
        let catalog = db.trees.get(&profile.name);
        let pending = db
            .pending
            .values()
            .any(|p| !p.test && p.profile.name == profile.name);
        let id = ui.id().with((&profile.name, "db"));
        let response = tree.branch(ui, id, 0, Icon::Database, &profile.name, "", |tree, ui| {
            if let Some(catalog) = catalog {
                for schema in &catalog.schemas {
                    let response = tree.branch(
                        ui,
                        id.with(&schema.name),
                        1,
                        Icon::Database,
                        &schema.name,
                        "",
                        |tree, ui| tree.tables(ui, db, profile, schema, 2),
                    );
                    response.context_menu(|ui| {
                        for (label, action) in [
                            ("New SQL console", super::schema::Action::Console),
                            ("Modify Schema...", super::schema::Action::Modify),
                            ("Generate DDL to Clipboard", super::schema::Action::Ddl),
                            ("Run SQL Script...", super::schema::Action::Run),
                            ("Export Data to File", super::schema::Action::Export),
                        ] {
                            if ui.button(label).clicked() {
                                tree.actions.push(Action::Schema(
                                    profile.clone(),
                                    schema.name.clone(),
                                    action,
                                ));
                                ui.close();
                            }
                        }
                    });
                }
            } else if pending {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Connecting…");
                });
            } else if ui.button("Connect").clicked() {
                tree.actions.push(Action::Connect(profile.clone()));
            }
        });
        if response.double_clicked() && catalog.is_none() && !pending {
            tree.actions.push(Action::Connect(profile.clone()));
        }
        response
            .on_hover_text(if pending {
                "Connecting…"
            } else if catalog.is_some() {
                "Online · Right-click for connection actions"
            } else {
                "Offline · Double-click to connect"
            })
            .context_menu(|ui| {
                for label in [
                    "Connect",
                    "Refresh metadata",
                    "Disconnect",
                    "Edit Connection…",
                    "Connect with password…",
                    "New SQL Console",
                ] {
                    let enabled = match label {
                        "Connect" => !pending,
                        "Refresh metadata" | "Disconnect" => catalog.is_some(),
                        "Connect with password…" => {
                            profile.driver != DbDriverType::Sqlite && !pending
                        }
                        _ => true,
                    };
                    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                        tree.actions.push(match label {
                            "Connect" => Action::Connect(profile.clone()),
                            "Refresh metadata" => Action::Refresh(profile.name.clone()),
                            "Disconnect" => Action::Disconnect(profile.name.clone()),
                            "Edit Connection…" => Action::Edit(profile.clone()),
                            "Connect with password…" => Action::Password(profile.clone()),
                            _ => Action::Console(profile.name.clone(), String::new()),
                        });
                        ui.close();
                    }
                }
            });
    }
    tree.actions
}

#[allow(
    clippy::too_many_lines,
    reason = "Small original vector glyphs share a fixed 16 px coordinate space"
)]
fn paint_icon(p: &egui::Painter, center: Pos2, icon: Icon) {
    let origin = center - vec2(8.0, 8.0);
    let pt = |x, y| origin + vec2(x, y);
    let line = |a, b, color| {
        p.line_segment([a, b], Stroke::new(1.0_f32, color));
    };
    let outline = |rect, color| {
        p.rect_stroke(rect, 1.0, Stroke::new(1.0_f32, color), StrokeKind::Inside);
    };
    let key = |x, y, color| {
        p.circle_stroke(pt(x, y), 3.4, Stroke::new(1.0_f32, color));
        line(pt(x, y + 3.5), pt(x, y + 10.0), color);
        line(pt(x, y + 7.0), pt(x + 2.5, y + 7.0), color);
        line(pt(x, y + 9.0), pt(x + 2.5, y + 9.0), color);
    };
    match icon {
        Icon::Folder => {
            let points = vec![
                pt(1.0, 14.0),
                pt(1.0, 3.0),
                pt(6.0, 3.0),
                pt(8.5, 5.0),
                pt(14.0, 5.0),
                pt(14.0, 14.0),
            ];
            p.add(Shape::convex_polygon(
                points.clone(),
                Color32::from_rgb(39, 49, 70),
                Stroke::NONE,
            ));
            p.add(Shape::closed_line(points, Stroke::new(1.0_f32, BLUE)));
        }
        Icon::Table | Icon::View => {
            outline(Rect::from_min_max(pt(1.0, 2.0), pt(14.0, 14.0)), TEXT);
            line(pt(1.0, 5.0), pt(14.0, 5.0), TEXT);
            line(pt(5.0, 2.0), pt(5.0, 14.0), TEXT);
            line(pt(10.0, 2.0), pt(10.0, 14.0), TEXT);
            if matches!(icon, Icon::View) {
                p.circle_filled(pt(12.5, 12.0), 2.5, BLUE);
            }
        }
        Icon::Database => {
            line(pt(4.0, 8.0), pt(4.0, 4.0), TEXT);
            line(pt(4.0, 4.0), pt(11.0, 4.0), TEXT);
            line(pt(8.0, 11.0), pt(11.0, 11.0), TEXT);
            outline(Rect::from_min_max(pt(1.0, 7.0), pt(8.0, 14.0)), TEXT);
            outline(Rect::from_min_max(pt(10.0, 1.0), pt(15.0, 6.0)), TEXT);
            outline(Rect::from_min_max(pt(10.0, 9.0), pt(15.0, 14.0)), TEXT);
        }
        Icon::Column {
            nullable,
            primary,
            foreign,
        } => {
            outline(Rect::from_min_max(pt(2.0, 2.0), pt(15.0, 14.0)), TEXT);
            line(
                pt(6.0, 2.0),
                pt(6.0, 14.0),
                if primary || foreign { BLUE } else { TEXT },
            );
            if nullable {
                p.circle_filled(pt(3.0, 14.0), 3.0, crate::theme::PANEL);
                p.circle_stroke(pt(3.0, 14.0), 2.2, Stroke::new(1.0_f32, TEXT));
            }
            if primary || foreign {
                p.circle_filled(pt(13.5, 9.0), 3.0, crate::theme::PANEL);
                p.circle_stroke(
                    pt(13.5, 9.0),
                    2.1,
                    Stroke::new(1.0_f32, if primary { GOLD } else { BLUE }),
                );
                line(
                    pt(13.5, 11.0),
                    pt(13.5, 15.5),
                    if primary { GOLD } else { BLUE },
                );
                line(
                    pt(13.5, 14.0),
                    pt(16.0, 14.0),
                    if primary { GOLD } else { BLUE },
                );
            }
        }
        Icon::Key => key(7.5, 4.5, GOLD),
        Icon::ForeignKey => key(7.5, 4.5, BLUE),
        Icon::Index { unique } => {
            p.circle_filled(pt(7.0, 2.0), 0.9, BLUE);
            line(pt(4.0, 6.0), pt(7.0, 6.0), BLUE);
            line(pt(7.0, 6.0), pt(7.0, 14.0), BLUE);
            line(pt(4.0, 14.0), pt(11.0, 14.0), BLUE);
            if unique {
                p.text(
                    pt(10.0, 7.0),
                    egui::Align2::LEFT_TOP,
                    "U",
                    FontId::proportional(8.0),
                    TEXT,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_arrow_is_painted_without_a_font_glyph() {
        let ctx = egui::Context::default();
        let mut selected = None;
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                Tree {
                    selected: &mut selected,
                    actions: Vec::new(),
                }
                .row(
                    ui,
                    egui::Id::new("fk"),
                    0,
                    Icon::ForeignKey,
                    "fk_city",
                    "(city_id) → cityes (id)",
                    None,
                );
            });
        });
        let text: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                Shape::Text(text) => Some(text.galley.text()),
                _ => None,
            })
            .collect();
        assert!(text.contains(&"(city_id) "));
        assert!(text.contains(&" cityes (id)"));
        assert!(text.iter().all(|part| !part.contains('→')));
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, Shape::Path(path) if path.points.len() == 3 && path.stroke.color == egui::epaint::ColorMode::Solid(MUTED))));
    }

    #[test]
    fn rows_follow_reference_geometry_and_selection() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        let mut selected = None;
        let mut rows = Vec::new();
        let draw = |ctx: &egui::Context, selected: &mut Option<egui::Id>, rows: &mut Vec<Rect>| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let mut tree = Tree {
                    selected,
                    actions: Vec::new(),
                };
                rows.clear();
                rows.push(
                    tree.row(
                        ui,
                        egui::Id::new("folder"),
                        1,
                        Icon::Folder,
                        "columns",
                        "4",
                        Some(true),
                    )
                    .rect,
                );
                rows.push(
                    tree.row(
                        ui,
                        egui::Id::new("id"),
                        2,
                        Icon::Column {
                            nullable: false,
                            primary: true,
                            foreign: false,
                        },
                        "id",
                        "int unsigned (auto increment)",
                        None,
                    )
                    .rect,
                );
                rows.push(
                    tree.row(
                        ui,
                        egui::Id::new("city"),
                        2,
                        Icon::Column {
                            nullable: true,
                            primary: false,
                            foreign: true,
                        },
                        "city_id",
                        "int unsigned",
                        None,
                    )
                    .rect,
                );
            });
        };
        let first = ctx.run(egui::RawInput::default(), |ctx| {
            draw(ctx, &mut selected, &mut rows);
        });
        let label = |output: &egui::FullOutput, name: &str| {
            output.shapes.iter().find_map(|shape| match &shape.shape {
                Shape::Text(text) if text.galley.text() == name => Some(text.pos),
                _ => None,
            })
        };
        let folder = label(&first, "columns").ok_or("columns")?;
        let id = label(&first, "id").ok_or("id")?;
        let city = label(&first, "city_id").ok_or("city_id")?;
        assert_eq!(id.x - folder.x, INDENT);
        assert_eq!(city.y - id.y, HEIGHT);
        assert_eq!(rows[1].height(), HEIGHT);
        let point = rows[1].center();
        let output = ctx.run(
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            },
            |ctx| draw(ctx, &mut selected, &mut rows),
        );
        assert_eq!(selected, Some(egui::Id::new("id")));
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, Shape::Rect(rect) if rect.fill == SELECTED && rect.rect.height() == HEIGHT)));
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, Shape::Text(text) if text.galley.text() == "int unsigned (auto increment)" && text.override_text_color == Some(TEXT))));
        Ok(())
    }
}
