//! Small, font-independent outlines for the existing tool windows and actions.
use crate::theme;
use egui::{Color32, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, Widget};

#[derive(Clone, Copy)]
pub enum Icon {
    Project,
    Git,
    Remote,
    Database,
    Bell,
    Settings,
    Search,
    Debug,
    Run,
}

impl Widget for Icon {
    fn ui(self, ui: &mut Ui) -> Response {
        button(ui, self, false, "")
    }
}

pub fn button(ui: &mut Ui, icon: Icon, selected: bool, title: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, title)
    });
    let painter = ui.painter();
    if selected || response.hovered() {
        painter.rect_filled(
            rect,
            5.0,
            if selected {
                theme::HOVER
            } else {
                theme::BORDER
            },
        );
    }
    let color = if ui.is_enabled() {
        theme::TEXT
    } else {
        theme::MUTED.gamma_multiply(0.5)
    };
    paint(painter, rect.center(), icon, color);
    response.on_hover_text(title)
}

#[allow(
    clippy::too_many_lines,
    reason = "Compact outlines share a normalized 20-pixel coordinate system"
)]
pub fn paint(painter: &egui::Painter, center: Pos2, icon: Icon, color: Color32) {
    let origin = center - Vec2::splat(10.0);
    let point = |x, y| origin + Vec2::new(x, y);
    let stroke = Stroke::new(1.3_f32, color);
    let line = |coords: &[(f32, f32)]| {
        painter.add(Shape::line(
            coords.iter().map(|&(x, y)| point(x, y)).collect(),
            stroke,
        ));
    };
    let circle = |x, y, radius| {
        painter.circle_stroke(point(x, y), radius, stroke);
    };
    match icon {
        Icon::Project => {
            line(&[
                (2.0, 5.0),
                (8.0, 5.0),
                (10.0, 7.0),
                (18.0, 7.0),
                (18.0, 16.0),
                (2.0, 16.0),
                (2.0, 5.0),
            ]);
        }
        Icon::Git => {
            circle(5.0, 4.0, 2.0);
            circle(5.0, 16.0, 2.0);
            circle(15.0, 4.0, 2.0);
            line(&[(5.0, 6.0), (5.0, 14.0)]);
            line(&[(15.0, 6.0), (15.0, 9.0), (5.0, 12.0)]);
        }
        Icon::Remote => {
            line(&[(2.0, 6.0), (17.0, 6.0), (13.0, 2.0)]);
            line(&[(17.0, 14.0), (2.0, 14.0), (6.0, 18.0)]);
        }
        Icon::Database => {
            painter.add(egui::epaint::EllipseShape::stroke(
                point(10.0, 5.0),
                Vec2::new(7.0, 3.0),
                stroke,
            ));
            line(&[
                (3.0, 5.0),
                (3.0, 15.0),
                (5.0, 17.0),
                (10.0, 18.0),
                (15.0, 17.0),
                (17.0, 15.0),
                (17.0, 5.0),
            ]);
            for y in [9.0, 13.0] {
                line(&[
                    (3.0, y),
                    (5.0, y + 2.0),
                    (10.0, y + 3.0),
                    (15.0, y + 2.0),
                    (17.0, y),
                ]);
            }
        }
        Icon::Bell => {
            line(&[
                (3.0, 15.0),
                (5.0, 12.0),
                (5.0, 7.0),
                (7.0, 4.0),
                (13.0, 4.0),
                (15.0, 7.0),
                (15.0, 12.0),
                (17.0, 15.0),
                (3.0, 15.0),
            ]);
            circle(10.0, 17.0, 1.0);
        }
        Icon::Settings => {
            circle(10.0, 10.0, 5.0);
            circle(10.0, 10.0, 2.0);
            for (x, y) in [(0.0_f32, 1.0_f32), (1.0, 0.0), (0.7, 0.7), (-0.7, 0.7)] {
                for sign in [-1.0, 1.0] {
                    line(&[
                        ((x * sign).mul_add(6.0, 10.0), (y * sign).mul_add(6.0, 10.0)),
                        ((x * sign).mul_add(8.0, 10.0), (y * sign).mul_add(8.0, 10.0)),
                    ]);
                }
            }
        }
        Icon::Search => {
            circle(8.0, 8.0, 5.0);
            line(&[(12.0, 12.0), (18.0, 18.0)]);
        }
        Icon::Run => {
            painter.add(Shape::closed_line(
                vec![point(6.0, 3.0), point(16.0, 10.0), point(6.0, 17.0)],
                stroke,
            ));
        }
        Icon::Debug => {
            painter.rect_stroke(
                Rect::from_min_max(point(6.0, 6.0), point(14.0, 16.0)),
                4.0,
                stroke,
                StrokeKind::Inside,
            );
            for y in [7.0, 11.0, 15.0] {
                line(&[(2.0, y), (6.0, y)]);
                line(&[(14.0, y), (18.0, y)]);
            }
            line(&[(7.0, 3.0), (10.0, 6.0), (13.0, 3.0)]);
        }
    }
}
