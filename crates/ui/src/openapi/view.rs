//! Swagger-style `OpenAPI` surface. The JSON editor stays the source of the document.
use super::{Draft, FileState, Parsed, http, spec};
use crate::theme;
use egui::{
    Align, Color32, FontId, Layout, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind,
    TextEdit, TextFormat, Ui, vec2,
};
use std::{collections::HashMap, path::Path, time::Duration};

const POST: Color32 = Color32::from_rgb(73, 204, 144);
const CARD: Color32 = Color32::from_rgb(36, 38, 40);
const CODE: Color32 = Color32::from_rgb(24, 25, 26);

pub fn show(
    ui: &mut Ui,
    ctx: &egui::Context,
    path: &Path,
    state: &mut FileState,
    hub: &mut http::Hub,
    enabled: bool,
    transfers: &mut Vec<Result<crate::http_requests::Incoming, String>>,
) {
    if !state.pending.is_empty() {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
    let mut authorize = false;
    egui::ScrollArea::vertical()
        .id_salt(("openapi-gui", path.to_path_buf()))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_enabled_ui(enabled, |ui| match &state.parsed {
                Parsed::Invalid(error) => {
                    ui.colored_label(theme::ERROR, error);
                }
                Parsed::Spec(_) => {
                    authorize = document(ui, state, hub, transfers);
                }
            });
        });
    if authorize {
        state.auth_open = true;
    }
    if state.auth_open && enabled {
        authorize_modal(ctx, state);
    }
}

fn document(
    ui: &mut Ui,
    state: &mut FileState,
    hub: &mut http::Hub,
    transfers: &mut Vec<Result<crate::http_requests::Incoming, String>>,
) -> bool {
    sync_server_vars(state);
    let (title, version, openapi, description, extra, docs, warnings) = {
        let Parsed::Spec(spec) = &state.parsed else {
            return false;
        };
        (
            spec.title.clone(),
            spec.version.clone(),
            spec.openapi.clone(),
            spec.description.clone(),
            spec.info_extra.clone(),
            spec.external_docs.clone(),
            spec.warnings.clone(),
        )
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(title).size(26.0).strong());
        pill(ui, &version, Color32::from_rgb(90, 92, 96), Color32::WHITE);
        pill(
            ui,
            &oas_label(&openapi),
            Color32::from_rgb(137, 196, 74),
            Color32::from_rgb(16, 32, 16),
        );
    });
    if !description.is_empty() {
        ui.add_space(6.0);
        ui.label(description);
    }
    if !extra.is_empty() {
        ui.label(RichText::new(extra).color(theme::MUTED).small());
    }
    if !docs.is_empty() && ui.link(docs.clone()).clicked() {
        crate::editor_view::open_documentation(docs);
    }
    for warning in warnings.iter().take(8) {
        ui.colored_label(theme::WARNING, warning);
    }
    ui.add_space(8.0);
    let authorize = servers_bar(ui, state);
    ui.add_space(10.0);
    let groups: Vec<(String, String, Vec<usize>)> = {
        match &state.parsed {
            Parsed::Spec(spec) => spec
                .groups
                .iter()
                .map(|group| {
                    (
                        group.name.clone(),
                        group.description.clone(),
                        group.operations.clone(),
                    )
                })
                .collect(),
            Parsed::Invalid(_) => Vec::new(),
        }
    };
    for (name, description, operations) in groups {
        tag_block(ui, state, hub, &name, &description, &operations, transfers);
        ui.add_space(8.0);
    }
    schemas_block(ui, state);
    authorize
}

fn sync_server_vars(state: &mut FileState) {
    let Parsed::Spec(spec) = &state.parsed else {
        return;
    };
    let Some(server) = spec.servers.get(state.server) else {
        state.vars.clear();
        return;
    };
    let server_changed = state.vars_server != state.server;
    let width_changed = state.vars.len() != server.variables.len();
    if server_changed || width_changed {
        state.vars = server
            .variables
            .iter()
            .map(|variable| variable.default_value.clone())
            .collect();
        state.vars_server = state.server;
    }
}

fn oas_label(version: &str) -> String {
    let mut parts = version.split('.');
    let major = parts.next().unwrap_or(version);
    let minor = parts.next().unwrap_or("0");
    format!("OAS {major}.{minor}")
}

fn pill(ui: &mut Ui, text: &str, fill: Color32, color: Color32) {
    if text.is_empty() {
        return;
    }
    egui::Frame::new()
        .fill(fill)
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.0).color(color));
        });
}

fn servers_bar(ui: &mut Ui, state: &mut FileState) -> bool {
    let mut authorize = false;
    let Parsed::Spec(spec) = &state.parsed else {
        return false;
    };
    let labels: Vec<String> = spec.servers.iter().map(spec::server_label).collect();
    let variables = spec
        .servers
        .get(state.server)
        .map(|server| server.variables.clone())
        .unwrap_or_default();
    egui::Frame::new()
        .fill(Color32::from_rgb(46, 48, 52))
        .corner_radius(6)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("Servers").small().color(theme::MUTED));
                    let selected = labels.get(state.server).cloned().unwrap_or_default();
                    egui::ComboBox::from_id_salt("openapi-server")
                        .selected_text(selected)
                        .width(420.0)
                        .show_ui(ui, |ui| {
                            for (index, label) in labels.iter().enumerate() {
                                ui.selectable_value(&mut state.server, index, label);
                            }
                        });
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if authorize_button(ui).clicked() {
                        authorize = true;
                    }
                });
            });
            variable_fields(ui, "openapi-server-var", &variables, &mut state.vars);
        });
    authorize
}

fn variable_fields(ui: &mut Ui, salt: &str, variables: &[spec::ServerVar], values: &mut [String]) {
    for (index, variable) in variables.iter().enumerate() {
        let Some(value) = values.get_mut(index) else {
            continue;
        };
        ui.horizontal(|ui| {
            ui.label(&variable.name);
            if variable.choices.is_empty() {
                ui.add(
                    TextEdit::singleline(value)
                        .desired_width(220.0)
                        .hint_text(&variable.default_value),
                );
            } else {
                let choices = variable.choices.clone();
                egui::ComboBox::from_id_salt((salt, index))
                    .selected_text(value.clone())
                    .show_ui(ui, |ui| {
                        for choice in choices {
                            ui.selectable_value(value, choice.clone(), choice);
                        }
                    });
            }
            if !variable.description.is_empty() {
                ui.label(
                    RichText::new(&variable.description)
                        .small()
                        .color(theme::MUTED),
                );
            }
        });
    }
}

fn authorize_button(ui: &mut Ui) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(132.0, 34.0), Sense::click());
    let fill = if response.hovered() {
        Color32::from_rgba_unmultiplied(73, 204, 144, 36)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect(
        rect,
        6.0,
        fill,
        Stroke::new(1.0_f32, POST),
        StrokeKind::Inside,
    );
    ui.painter().text(
        Pos2::new(rect.left() + 16.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "Authorize",
        FontId::proportional(14.0),
        POST,
    );
    paint_lock(
        ui.painter(),
        Pos2::new(rect.right() - 18.0, rect.center().y),
        POST,
    );
    response
}

fn tag_block(
    ui: &mut Ui,
    state: &mut FileState,
    hub: &mut http::Hub,
    name: &str,
    description: &str,
    operations: &[usize],
    transfers: &mut Vec<Result<crate::http_requests::Incoming, String>>,
) {
    let open = !state.collapsed_tags.contains(name);
    let response = section_header(ui, name, open);
    if response.clicked() {
        if open {
            state.collapsed_tags.insert(name.to_string());
        } else {
            state.collapsed_tags.remove(name);
        }
    }
    if !open {
        return;
    }
    if !description.is_empty() {
        ui.label(RichText::new(description).color(theme::MUTED));
    }
    for index in operations {
        operation_card(ui, state, hub, *index, transfers);
        ui.add_space(6.0);
    }
}

fn section_header(ui: &mut Ui, title: &str, open: bool) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 32.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 4.0, theme::HOVER);
    }
    ui.painter().text(
        Pos2::new(rect.left() + 4.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        FontId::proportional(20.0),
        theme::TEXT,
    );
    paint_chevron(
        ui.painter(),
        Pos2::new(rect.right() - 12.0, rect.center().y),
        open,
        theme::TEXT,
    );
    response
}

fn operation_card(
    ui: &mut Ui,
    state: &mut FileState,
    hub: &mut http::Hub,
    index: usize,
    transfers: &mut Vec<Result<crate::http_requests::Incoming, String>>,
) {
    let Some(operation) = operation_clone(state, index) else {
        return;
    };
    let color = method_color(&operation.method);
    let open = state.open_ops.contains(&operation.key);
    let secured = operation_secured(state, &operation);
    let satisfied = secured && security_satisfied(state, &operation);
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0_f32, color.linear_multiply(0.75)))
        .corner_radius(6)
        .inner_margin(8)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if operation_header(ui, &operation, color, secured, satisfied, open).clicked() {
                toggle_set(&mut state.open_ops, &operation.key);
            }
            if open {
                ui.add_space(6.0);
                if ui.button("To HTTP Requests").clicked() {
                    transfers.push(capture_request(state, &operation));
                }
                if !operation.description.is_empty() {
                    ui.label(&operation.description);
                }
                if !operation.identifier.is_empty() {
                    ui.label(
                        RichText::new(&operation.identifier)
                            .small()
                            .color(theme::MUTED),
                    );
                }
                if !operation.callbacks.is_empty() {
                    ui.label(
                        RichText::new(format!("Callbacks: {}", operation.callbacks.join(", ")))
                            .small()
                            .color(theme::MUTED),
                    );
                }
                if !operation.external_docs.is_empty() {
                    let url = operation.external_docs.clone();
                    if ui.link(&url).clicked() {
                        crate::editor_view::open_documentation(url);
                    }
                }
                parameters_block(ui, state, &operation, color);
                body_block(ui, state, &operation);
                if state.trying.contains(&operation.key) && execute_button(ui).clicked() {
                    execute(state, hub, &operation);
                }
                responses_block(ui, &operation);
                live_block(ui, state, &operation.key);
            }
        });
}

fn operation_clone(state: &FileState, index: usize) -> Option<spec::Operation> {
    match &state.parsed {
        Parsed::Spec(spec) => spec.operations.get(index).cloned(),
        Parsed::Invalid(_) => None,
    }
}

fn operation_header(
    ui: &mut Ui,
    operation: &spec::Operation,
    color: Color32,
    secured: bool,
    satisfied: bool,
    open: bool,
) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 32.0), Sense::click());
    let painter = ui.painter();
    let badge = method_badge_rect(
        painter,
        Pos2::new(rect.left() + 4.0, rect.center().y),
        &operation.method,
        color,
    );
    let mut x = badge.right() + 8.0;
    let path = painter.layout_no_wrap(operation.path.clone(), FontId::monospace(14.0), theme::TEXT);
    painter.galley(
        Pos2::new(x, rect.center().y - path.size().y / 2.0),
        path.clone(),
        theme::TEXT,
    );
    x += path.size().x + 8.0;
    if operation.deprecated {
        painter.text(
            Pos2::new(x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            "deprecated",
            FontId::proportional(12.0),
            theme::WARNING,
        );
        x += 78.0;
    }
    let summary_width = (rect.right() - x - 48.0).max(0.0);
    if summary_width > 8.0 && !operation.summary.is_empty() {
        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = summary_width;
        job.append(
            &operation.summary,
            0.0,
            TextFormat {
                font_id: FontId::proportional(13.0),
                color: theme::MUTED,
                ..Default::default()
            },
        );
        let galley = painter.layout_job(job);
        painter.galley(
            Pos2::new(x, rect.center().y - galley.size().y / 2.0),
            galley,
            theme::MUTED,
        );
    }
    if secured {
        paint_lock(
            painter,
            Pos2::new(rect.right() - 28.0, rect.center().y),
            if satisfied {
                theme::SUCCESS
            } else {
                theme::MUTED
            },
        );
    }
    paint_chevron(
        painter,
        Pos2::new(rect.right() - 10.0, rect.center().y),
        open,
        theme::TEXT,
    );
    response
}

fn method_badge_rect(
    painter: &egui::Painter,
    left_center: Pos2,
    method: &str,
    color: Color32,
) -> Rect {
    let galley = painter.layout_no_wrap(
        method.to_string(),
        FontId::proportional(13.0),
        Color32::WHITE,
    );
    let rect = Rect::from_min_size(
        Pos2::new(left_center.x, left_center.y - 12.0),
        vec2(galley.size().x + 16.0, 24.0),
    );
    painter.rect_filled(rect, 4.0, color);
    painter.galley(
        Pos2::new(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0),
        galley,
        Color32::WHITE,
    );
    rect
}

fn parameters_block(
    ui: &mut Ui,
    state: &mut FileState,
    operation: &spec::Operation,
    color: Color32,
) {
    let trying = state.trying.contains(&operation.key);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Parameters").size(16.0).strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let label = if trying { "Cancel" } else { "Try it out" };
            if ui.button(label).clicked() {
                if trying {
                    state.trying.remove(&operation.key);
                } else {
                    state.trying.insert(operation.key.clone());
                    state
                        .drafts
                        .entry(operation.key.clone())
                        .or_insert_with(|| Draft::from_operation(operation));
                }
            }
        });
    });
    underline(ui, color);
    if operation.parameters.is_empty() {
        ui.label("No parameters");
        return;
    }
    if !operation.servers.is_empty() {
        operation_servers(ui, state, operation);
    }
    for parameter in &operation.parameters {
        parameter_row(ui, state, operation, parameter, trying);
    }
}

fn operation_servers(ui: &mut Ui, state: &mut FileState, operation: &spec::Operation) {
    let draft = state
        .drafts
        .entry(operation.key.clone())
        .or_insert_with(|| Draft::from_operation(operation));
    let variable_count = operation
        .servers
        .first()
        .map_or(0, |server| server.variables.len());
    if draft.vars.len() != variable_count
        && draft.server == 0
        && draft.vars.is_empty()
        && let Some(server) = operation.servers.first()
    {
        draft.vars = server
            .variables
            .iter()
            .map(|variable| variable.default_value.clone())
            .collect();
    }
    let labels: Vec<String> = operation.servers.iter().map(spec::server_label).collect();
    let selected = labels.get(draft.server).cloned().unwrap_or_default();
    ui.label(
        RichText::new("Operation server")
            .small()
            .color(theme::MUTED),
    );
    egui::ComboBox::from_id_salt(("openapi-op-server", operation.key.clone()))
        .selected_text(selected)
        .show_ui(ui, |ui| {
            for (index, label) in labels.iter().enumerate() {
                ui.selectable_value(&mut draft.server, index, label);
            }
        });
    let variables = operation
        .servers
        .get(draft.server)
        .map(|server| server.variables.clone())
        .unwrap_or_default();
    if draft.vars.len() != variables.len() {
        draft.vars = variables
            .iter()
            .map(|variable| variable.default_value.clone())
            .collect();
    }
    variable_fields(ui, &operation.key, &variables, &mut draft.vars);
}

fn parameter_row(
    ui: &mut Ui,
    state: &mut FileState,
    operation: &spec::Operation,
    parameter: &spec::Parameter,
    trying: bool,
) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(&parameter.name).strong());
        if parameter.required {
            ui.label(RichText::new("*").color(theme::ERROR));
            ui.label(RichText::new("required").small().color(theme::ERROR));
        }
    });
    ui.label(
        RichText::new(&parameter.type_name)
            .small()
            .color(theme::MUTED),
    );
    if !parameter.description.is_empty() {
        ui.label(
            RichText::new(&parameter.description)
                .small()
                .color(theme::MUTED),
        );
    }
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("({})", parameter.location.label()))
                .italics()
                .color(theme::MUTED),
        );
        let mut value = state
            .drafts
            .get(&operation.key)
            .and_then(|draft| draft.params.get(&parameter.id).cloned())
            .unwrap_or_else(|| parameter.example.clone());
        let changed = widget_for(ui, &operation.key, parameter, trying, &mut value);
        if trying && changed {
            let draft = state
                .drafts
                .entry(operation.key.clone())
                .or_insert_with(|| Draft::from_operation(operation));
            draft.params.insert(parameter.id.clone(), value);
        }
    });
}

fn widget_for(
    ui: &mut Ui,
    operation: &str,
    parameter: &spec::Parameter,
    trying: bool,
    value: &mut String,
) -> bool {
    ui.push_id((operation, &parameter.id), |ui| match parameter.shape {
        spec::Shape::Boolean => {
            let mut flag = value == "true";
            let response = ui.add_enabled(trying, egui::Checkbox::new(&mut flag, ""));
            if response.changed() {
                *value = if flag { "true".into() } else { "false".into() };
            }
            response.changed()
        }
        spec::Shape::Text | spec::Shape::Json if !parameter.choices.is_empty() => {
            let mut changed = false;
            egui::ComboBox::from_id_salt("choice")
                .selected_text(if value.is_empty() {
                    parameter.name.clone()
                } else {
                    value.clone()
                })
                .show_ui(ui, |ui| {
                    if !trying {
                        return;
                    }
                    for choice in &parameter.choices {
                        if ui.selectable_value(value, choice.clone(), choice).clicked() {
                            changed = true;
                        }
                    }
                });
            changed
        }
        _ => {
            let response = ui.add_enabled(
                trying,
                TextEdit::singleline(value)
                    .hint_text(&parameter.name)
                    .desired_width(280.0)
                    .font(egui::TextStyle::Monospace),
            );
            response.changed()
        }
    })
    .inner
}

fn body_block(ui: &mut Ui, state: &mut FileState, operation: &spec::Operation) {
    let Some(body) = &operation.body else {
        return;
    };
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Request body").size(16.0).strong());
        if body.required {
            ui.label(RichText::new("required").small().color(theme::ERROR));
        }
        if body.contents.len() > 1 {
            let trying = state.trying.contains(&operation.key);
            let mut index = state
                .drafts
                .get(&operation.key)
                .map_or(0, |draft| draft.content)
                .min(body.contents.len().saturating_sub(1));
            let selected = body
                .contents
                .get(index)
                .map(|media| media.content_type.clone())
                .unwrap_or_default();
            egui::ComboBox::from_id_salt(("openapi-content", operation.key.clone()))
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for (item, media) in body.contents.iter().enumerate() {
                        if ui
                            .selectable_value(&mut index, item, &media.content_type)
                            .changed()
                        {
                            let draft = state
                                .drafts
                                .entry(operation.key.clone())
                                .or_insert_with(|| Draft::from_operation(operation));
                            draft.content = index;
                            if trying {
                                draft.body.clone_from(&media.example);
                            }
                        }
                    }
                });
        } else if let Some(media) = body.contents.first() {
            ui.label(&media.content_type);
        }
    });
    if !body.description.is_empty() {
        ui.label(RichText::new(&body.description).color(theme::MUTED));
    }
    let index = state
        .drafts
        .get(&operation.key)
        .map_or(0, |draft| draft.content);
    let Some(media) = body.contents.get(index).or_else(|| body.contents.first()) else {
        return;
    };
    let schema_view = state.schema_tab.contains(&operation.key);
    ui.horizontal(|ui| {
        if ui.selectable_label(!schema_view, "Example Value").clicked() {
            state.schema_tab.remove(&operation.key);
        }
        if ui.selectable_label(schema_view, "Schema").clicked() {
            state.schema_tab.insert(operation.key.clone());
        }
    });
    if schema_view {
        code_block(ui, &media.schema_text, true);
        return;
    }
    let trying = state.trying.contains(&operation.key);
    if trying {
        let mut text = state
            .drafts
            .get(&operation.key)
            .map_or_else(|| media.example.clone(), |draft| draft.body.clone());
        let response = ui.add(
            TextEdit::multiline(&mut text)
                .font(egui::TextStyle::Monospace)
                .desired_rows(8)
                .desired_width(f32::INFINITY),
        );
        if response.changed() {
            let draft = state
                .drafts
                .entry(operation.key.clone())
                .or_insert_with(|| Draft::from_operation(operation));
            draft.body = text;
        }
    } else {
        code_block(ui, &media.example, media.content_type.contains("json"));
    }
}

fn execute_button(ui: &mut Ui) -> egui::Response {
    ui.add_space(8.0);
    ui.add(egui::Button::new(RichText::new("Execute").color(Color32::WHITE)).fill(POST))
}

fn responses_block(ui: &mut Ui, operation: &spec::Operation) {
    ui.add_space(10.0);
    ui.label(RichText::new("Responses").size(16.0).strong());
    ui.add_space(4.0);
    egui::Grid::new(("openapi-responses", operation.key.clone()))
        .num_columns(3)
        .min_col_width(72.0)
        .striped(false)
        .show(ui, |ui| {
            ui.strong("Code");
            ui.strong("Description");
            ui.strong("Links");
            ui.end_row();
            for response in &operation.responses {
                ui.label(&response.code);
                ui.label(&response.description);
                if response.links.is_empty() {
                    ui.label(RichText::new("No links").italics().color(theme::MUTED));
                } else {
                    ui.label(response.links.join(", "));
                }
                ui.end_row();
            }
        });
}

fn live_block(ui: &mut Ui, state: &FileState, key: &str) {
    if state.pending.contains_key(key) {
        ui.add_space(8.0);
        ui.label("Sending…");
    }
    let Some(outcome) = state.live.get(key) else {
        return;
    };
    ui.add_space(8.0);
    ui.label(RichText::new("Server response").strong());
    if let Some(error) = &outcome.transport_error {
        ui.colored_label(theme::ERROR, error);
    } else {
        let color = outcome.status_code.map_or(theme::TEXT, |code| {
            if code >= 400 {
                theme::ERROR
            } else {
                theme::SUCCESS
            }
        });
        ui.horizontal(|ui| {
            ui.label("Code");
            ui.label(RichText::new(&outcome.status_line).color(color).strong());
            ui.label(
                RichText::new(format!("{} ms", outcome.elapsed_ms))
                    .small()
                    .color(theme::MUTED),
            );
        });
    }
    if !outcome.headers.is_empty() {
        ui.label(
            RichText::new("Response headers")
                .small()
                .color(theme::MUTED),
        );
        code_block(ui, &outcome.headers, false);
    }
    if !outcome.body.is_empty() {
        ui.label(RichText::new("Response body").small().color(theme::MUTED));
        let json = outcome.body.trim_start().starts_with('{')
            || outcome.body.trim_start().starts_with('[');
        code_block(ui, &outcome.body, json);
    }
    if !outcome.curl.is_empty() {
        ui.label(RichText::new("curl").small().color(theme::MUTED));
        code_block(ui, &outcome.curl, false);
    }
}

fn schemas_block(ui: &mut Ui, state: &mut FileState) {
    let names: Vec<String> = match &state.parsed {
        Parsed::Spec(spec) => spec
            .schemas
            .iter()
            .map(|schema| schema.name.clone())
            .collect(),
        Parsed::Invalid(_) => Vec::new(),
    };
    if names.is_empty() {
        return;
    }
    ui.add_space(12.0);
    egui::Frame::new()
        .stroke(Stroke::new(1.0_f32, theme::BORDER))
        .corner_radius(6)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let open = state.schemas_open;
            if section_header(ui, "Schemas", open).clicked() {
                state.schemas_open = !open;
            }
            if !open {
                return;
            }
            for name in names {
                schema_entry(ui, state, &name);
                ui.add_space(8.0);
            }
        });
}

fn schema_entry(ui: &mut Ui, state: &mut FileState, name: &str) {
    let open = state.open_schemas.contains(name);
    let response = section_header(ui, name, open);
    if response.clicked() {
        toggle_set(&mut state.open_schemas, name);
    }
    if !open {
        return;
    }
    let Some(value) = schema_value(state, name) else {
        return;
    };
    show_schema(ui, state, &value, name, 0);
}

fn schema_value(state: &FileState, name: &str) -> Option<serde_json::Value> {
    match &state.parsed {
        Parsed::Spec(spec) => spec::schema_named(spec, name).cloned(),
        Parsed::Invalid(_) => None,
    }
}

fn show_schema(
    ui: &mut Ui,
    state: &mut FileState,
    value: &serde_json::Value,
    prefix: &str,
    depth: usize,
) {
    if depth > 6 {
        ui.label("…");
        return;
    }
    if let Some(reference) = value.get("$ref").and_then(serde_json::Value::as_str) {
        let name = reference.rsplit('/').next().unwrap_or(reference);
        ui.label(RichText::new(name).color(theme::INFO));
        if let Some(target) = schema_value(state, name)
            && target.get("$ref").and_then(serde_json::Value::as_str) != Some(reference)
        {
            show_schema(ui, state, &target, prefix, depth + 1);
        }
        return;
    }
    let kind = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if kind == "array" {
        ui.label("[");
        if let Some(items) = value.get("items") {
            ui.indent(prefix, |ui| {
                show_schema(ui, state, items, &format!("{prefix}[]"), depth + 1);
            });
        }
        ui.label("]");
        return;
    }
    let Some(properties) = value
        .get("properties")
        .and_then(serde_json::Value::as_object)
    else {
        ui.label(if kind.is_empty() { "object" } else { kind });
        return;
    };
    let required = value
        .get("required")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    ui.label("{");
    for (name, property) in properties {
        let id = format!("{prefix}.{name}");
        let nested = property.get("properties").is_some()
            || property.get("items").is_some()
            || property.get("$ref").is_some()
            || property.get("allOf").is_some();
        ui.horizontal(|ui| {
            ui.label(RichText::new(name).strong());
            if required.iter().any(|item| item == name) {
                ui.label(RichText::new("*").color(theme::ERROR));
            }
            let summary = schema_summary(property);
            if nested {
                let open = state.schema_nodes.contains(&id);
                if ui.selectable_label(open, summary).clicked() {
                    toggle_set(&mut state.schema_nodes, &id);
                }
            } else {
                ui.label(summary);
            }
        });
        if nested && state.schema_nodes.contains(&id) {
            ui.indent(&id, |ui| show_schema(ui, state, property, &id, depth + 1));
        }
    }
    ui.label("}");
}

fn schema_summary(value: &serde_json::Value) -> String {
    if let Some(reference) = value.get("$ref").and_then(serde_json::Value::as_str) {
        return reference
            .rsplit('/')
            .next()
            .unwrap_or(reference)
            .to_string();
    }
    match value.get("type").and_then(serde_json::Value::as_str) {
        Some("object") | None if value.get("properties").is_some() => "{…}".into(),
        Some("array") => "[…]".into(),
        Some(kind) => kind.to_string(),
        None if value.get("allOf").is_some() => "allOf".into(),
        None => "object".into(),
    }
}

fn code_block(ui: &mut Ui, text: &str, json: bool) {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = (ui.available_width() - 28.0).max(40.0);
    if json {
        append_json(&mut job, text);
    } else {
        job.append(
            text,
            0.0,
            TextFormat {
                font_id: FontId::monospace(13.0),
                color: theme::TEXT,
                ..Default::default()
            },
        );
    }
    egui::Frame::new()
        .fill(CODE)
        .corner_radius(6)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.add(egui::Label::new(job).selectable(true));
        });
}

fn append_json(job: &mut egui::text::LayoutJob, text: &str) {
    let spans = nozhik_language_support::json::highlight(text);
    let mut start = 0;
    for span in spans {
        if span.range.start < start
            || !text.is_char_boundary(span.range.start)
            || !text.is_char_boundary(span.range.end)
        {
            continue;
        }
        append_plain(job, &text[start..span.range.start], theme::TEXT);
        append_plain(
            job,
            &text[span.range.clone()],
            theme::syntax_color(&span.token),
        );
        start = span.range.end;
    }
    append_plain(job, &text[start..], theme::TEXT);
}

fn append_plain(job: &mut egui::text::LayoutJob, text: &str, color: Color32) {
    if text.is_empty() {
        return;
    }
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: FontId::monospace(13.0),
            color,
            ..Default::default()
        },
    );
}

fn underline(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 2.0), Sense::hover());
    let bar = Rect::from_min_max(rect.min, Pos2::new(rect.left() + 88.0, rect.bottom()));
    ui.painter().rect_filled(bar, 0.0, color);
}

fn capture_request(
    state: &FileState,
    operation: &spec::Operation,
) -> Result<crate::http_requests::Incoming, String> {
    let draft = state
        .drafts
        .get(&operation.key)
        .cloned()
        .unwrap_or_else(|| Draft::from_operation(operation));
    let Parsed::Spec(spec) = &state.parsed else {
        return Err("OpenAPI document is not valid".into());
    };
    let (server_index, server_values) = if operation.servers.is_empty() {
        (state.server, state.vars.clone())
    } else {
        (draft.server, draft.vars.clone())
    };
    spec::transfer_request(
        spec,
        operation,
        &spec::CallDraft {
            server_index,
            server_values: &server_values,
            params: &draft.params,
            content_index: draft.content,
            body_text: &draft.body,
            auth: &state.auth,
        },
    )
}

fn execute(state: &mut FileState, hub: &mut http::Hub, operation: &spec::Operation) {
    let draft = state
        .drafts
        .get(&operation.key)
        .cloned()
        .unwrap_or_else(|| Draft::from_operation(operation));
    let Parsed::Spec(spec) = &state.parsed else {
        return;
    };
    let (server_index, server_values) = if operation.servers.is_empty() {
        (state.server, state.vars.clone())
    } else {
        (draft.server, draft.vars.clone())
    };
    let auth = state.auth.clone();
    let prepared = spec::prepare(
        spec,
        operation,
        &spec::CallDraft {
            server_index,
            server_values: &server_values,
            params: &draft.params,
            content_index: draft.content,
            body_text: &draft.body,
            auth: &auth,
        },
    );
    let key = operation.key.clone();
    match prepared {
        Ok(prepared) => match hub.submit(prepared) {
            Ok(rx) => {
                state.pending.insert(key, super::Pending { rx });
            }
            Err(error) => {
                state
                    .live
                    .insert(key, http::Outcome::transport(error, String::new()));
            }
        },
        Err(error) => {
            state
                .live
                .insert(key, http::Outcome::transport(error, String::new()));
        }
    }
}

fn operation_secured(state: &FileState, operation: &spec::Operation) -> bool {
    match &operation.security {
        spec::Security::None => false,
        spec::Security::Requirements(items) => items.iter().any(|group| !group.is_empty()),
        spec::Security::Inherit => match &state.parsed {
            Parsed::Spec(spec) => match &spec.global_security {
                spec::Security::Requirements(items) => items.iter().any(|group| !group.is_empty()),
                _ => false,
            },
            Parsed::Invalid(_) => false,
        },
    }
}

fn security_satisfied(state: &FileState, operation: &spec::Operation) -> bool {
    let Parsed::Spec(spec) = &state.parsed else {
        return false;
    };
    let requirements = match &operation.security {
        spec::Security::None => return true,
        spec::Security::Inherit => match &spec.global_security {
            spec::Security::Requirements(items) => items,
            _ => return true,
        },
        spec::Security::Requirements(items) => items,
    };
    requirements.iter().any(|group| {
        !group.is_empty()
            && group.iter().all(|name| {
                state.auth.get(name).is_some_and(|input| {
                    input.authorized
                        && match spec.schemes.iter().find(|scheme| &scheme.name == name) {
                            Some(spec::Scheme {
                                kind: spec::SchemeKind::Basic,
                                ..
                            }) => !input.username.is_empty(),
                            Some(_) => !input.token.is_empty(),
                            None => false,
                        }
                })
            })
    })
}

fn authorize_modal(ctx: &egui::Context, state: &mut FileState) {
    let mut close = false;
    let schemes = match &state.parsed {
        Parsed::Spec(spec) => spec.schemes.clone(),
        Parsed::Invalid(_) => Vec::new(),
    };
    egui::Modal::new(egui::Id::new("openapi-auth")).show(ctx, |ui| {
        ui.set_width(520.0);
        ui.horizontal(|ui| {
            ui.heading("Available authorizations");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("×").clicked() {
                    close = true;
                }
            });
        });
        ui.separator();
        if schemes.is_empty() {
            ui.label("This document does not declare security schemes.");
        }
        for scheme in &schemes {
            ui.add_space(8.0);
            ui.label(RichText::new(&scheme.title).size(20.0).strong());
            if !scheme.description.is_empty() {
                ui.label(&scheme.description);
            }
            scheme_fields(ui, state, scheme);
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let authorized = schemes.iter().any(|scheme| {
                state
                    .auth
                    .get(&scheme.name)
                    .is_some_and(|input| input.authorized)
            });
            let label = if authorized { "Logout" } else { "Authorize" };
            if ui.button(label).clicked() {
                apply_auth_button(state, &schemes, authorized);
            }
            if ui.button("Close").clicked() {
                close = true;
            }
        });
    });
    if close {
        state.auth_open = false;
    }
}

fn scheme_fields(ui: &mut Ui, state: &mut FileState, scheme: &spec::Scheme) {
    match &scheme.kind {
        spec::SchemeKind::ApiKey { name, location } => {
            labeled_chip(ui, "Name:", name);
            labeled_chip(
                ui,
                "In:",
                match location {
                    spec::KeyIn::Query => "query",
                    spec::KeyIn::Header => "header",
                    spec::KeyIn::Cookie => "cookie",
                },
            );
            token_field(ui, state, &scheme.name);
        }
        spec::SchemeKind::Basic => {
            let input = state.auth.entry(scheme.name.clone()).or_default();
            ui.label("Username");
            ui.add(TextEdit::singleline(&mut input.username).desired_width(f32::INFINITY));
            ui.label("Password");
            ui.add(
                TextEdit::singleline(&mut input.password)
                    .password(true)
                    .desired_width(f32::INFINITY),
            );
        }
        spec::SchemeKind::Bearer { format } => {
            if !format.is_empty() {
                ui.label(
                    RichText::new(format!("Bearer format: {format}"))
                        .small()
                        .color(theme::MUTED),
                );
            }
            token_field(ui, state, &scheme.name);
        }
        spec::SchemeKind::OAuth2 { flows } | spec::SchemeKind::OpenId { url: flows } => {
            ui.label(RichText::new(flows).small().color(theme::MUTED));
            token_field(ui, state, &scheme.name);
        }
        spec::SchemeKind::Http {
            scheme: http_scheme,
        } => {
            ui.label(
                RichText::new(format!("HTTP scheme: {http_scheme}"))
                    .small()
                    .color(theme::MUTED),
            );
            token_field(ui, state, &scheme.name);
        }
        spec::SchemeKind::Other { type_name } => {
            ui.label(
                RichText::new(format!("Unsupported scheme type: {type_name}"))
                    .color(theme::WARNING),
            );
            token_field(ui, state, &scheme.name);
        }
    }
}

fn labeled_chip(ui: &mut Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(label);
        egui::Frame::new()
            .fill(Color32::from_rgb(58, 60, 64))
            .corner_radius(4)
            .inner_margin(egui::Margin::symmetric(6, 2))
            .show(ui, |ui| {
                ui.label(RichText::new(value).strong());
            });
    });
}

fn token_field(ui: &mut Ui, state: &mut FileState, name: &str) {
    ui.label("Value:");
    let input = state.auth.entry(name.to_string()).or_default();
    ui.add(
        TextEdit::singleline(&mut input.token)
            .password(true)
            .desired_width(f32::INFINITY),
    );
}

fn apply_auth_button(state: &mut FileState, schemes: &[spec::Scheme], logout: bool) {
    for scheme in schemes {
        let input = state.auth.entry(scheme.name.clone()).or_default();
        if logout {
            input.authorized = false;
            continue;
        }
        let ready = match scheme.kind {
            spec::SchemeKind::Basic => !input.username.is_empty(),
            _ => !input.token.is_empty(),
        };
        input.authorized = ready;
    }
}

fn toggle_set(set: &mut std::collections::HashSet<String>, key: &str) {
    if !set.insert(key.to_string()) {
        set.remove(key);
    }
}

fn method_color(method: &str) -> Color32 {
    match method {
        "GET" => Color32::from_rgb(97, 175, 254),
        "POST" => POST,
        "PUT" => Color32::from_rgb(252, 161, 48),
        "DELETE" => Color32::from_rgb(249, 62, 62),
        "PATCH" => Color32::from_rgb(80, 227, 194),
        "HEAD" => Color32::from_rgb(144, 18, 254),
        "OPTIONS" => Color32::from_rgb(13, 90, 167),
        _ => Color32::from_rgb(120, 120, 120),
    }
}

fn paint_chevron(painter: &egui::Painter, center: Pos2, open: bool, color: Color32) {
    let points = if open {
        vec![
            center + vec2(-4.0, -2.0),
            center + vec2(0.0, 3.0),
            center + vec2(4.0, -2.0),
        ]
    } else {
        vec![
            center + vec2(-2.0, -4.0),
            center + vec2(3.0, 0.0),
            center + vec2(-2.0, 4.0),
        ]
    };
    painter.add(Shape::line(points, Stroke::new(1.4_f32, color)));
}

fn paint_lock(painter: &egui::Painter, center: Pos2, color: Color32) {
    let stroke = Stroke::new(1.3_f32, color);
    painter.rect_stroke(
        Rect::from_center_size(center + vec2(0.0, 3.0), vec2(9.0, 7.0)),
        1.0,
        stroke,
        StrokeKind::Inside,
    );
    painter.line_segment(
        [center + vec2(-3.0, 1.0), center + vec2(-3.0, -2.0)],
        stroke,
    );
    painter.line_segment(
        [center + vec2(-3.0, -2.0), center + vec2(3.0, -2.0)],
        stroke,
    );
    painter.line_segment([center + vec2(3.0, -2.0), center + vec2(3.0, 1.0)], stroke);
}

impl Draft {
    fn from_operation(operation: &spec::Operation) -> Self {
        let mut params = HashMap::new();
        for parameter in &operation.parameters {
            if !parameter.example.is_empty() {
                params.insert(parameter.id.clone(), parameter.example.clone());
            }
        }
        let (content, body) = operation
            .body
            .as_ref()
            .and_then(|body| body.contents.first())
            .map_or((0, String::new()), |media| (0, media.example.clone()));
        let vars = operation
            .servers
            .first()
            .map(|server| {
                server
                    .variables
                    .iter()
                    .map(|variable| variable.default_value.clone())
                    .collect()
            })
            .unwrap_or_default();
        Self {
            params,
            content,
            body,
            server: 0,
            vars,
        }
    }
}
