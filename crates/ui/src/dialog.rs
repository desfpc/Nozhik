//! Shared modal geometry: bounded scrolling, a resize grip and project-owned preferences.
use egui::{Context, Id, Ui, Vec2, vec2};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

/// Preferred content sizes in logical pixels, keyed by stable dialog names.
pub type Sizes = BTreeMap<String, [f32; 2]>;
pub type SharedSizes = Arc<Mutex<Sizes>>;

fn memory_id() -> Id {
    Id::new("nozhik_dialog_sizes")
}

pub fn bind(ctx: &Context, sizes: SharedSizes) {
    ctx.data_mut(|data| data.insert_temp(memory_id(), sizes));
}
fn preferences(ctx: &Context) -> SharedSizes {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<SharedSizes>(memory_id())
            .clone()
    })
}
pub fn snapshot(sizes: &SharedSizes) -> Sizes {
    sizes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}
pub fn replace(sizes: &SharedSizes, values: Sizes) {
    *sizes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = values;
}

/// Modal with a fixed viewport inside its frame; oversized contents scroll rather than grow it.
pub struct Dialog {
    key: &'static str,
    id: Id,
    default_size: Vec2,
    frame: Option<egui::Frame>,
}
impl Dialog {
    pub fn new(key: &'static str, size: [f32; 2]) -> Self {
        Self {
            key,
            id: Id::new(key),
            default_size: size.into(),
            frame: None,
        }
    }
    pub const fn frame(mut self, frame: egui::Frame) -> Self {
        self.frame = Some(frame);
        self
    }
    pub const fn id(mut self, id: Id) -> Self {
        self.id = id;
        self
    }
    pub fn show<T>(
        self,
        ctx: &Context,
        content: impl FnOnce(&mut Ui) -> T,
    ) -> egui::ModalResponse<T> {
        let preferences = preferences(ctx);
        let preferred = snapshot(&preferences)
            .get(self.key)
            .copied()
            .map_or(self.default_size, Vec2::from);
        let frame = self
            .frame
            .unwrap_or_else(|| egui::Frame::popup(&ctx.style()));
        let bounds = ctx.screen_rect();
        let maximum = (bounds.size() - frame.total_margin().sum()).max(Vec2::ZERO);
        let size = preferred.max(Vec2::splat(32.0)).min(maximum);
        let outer_size = size + frame.total_margin().sum();
        let area = egui::Area::new(self.id)
            .kind(egui::UiKind::Modal)
            .order(egui::Order::Foreground)
            .sense(egui::Sense::hover())
            .fixed_pos(bounds.center() - outer_size * 0.5)
            .default_size(outer_size)
            .constrain(false);
        egui::Modal::new(self.id)
            .area(area)
            .frame(frame)
            .show(ctx, |ui| {
                ui.set_width(size.x);
                ui.set_min_height(size.y);
                let grip_height = 20.0_f32.min(size.y);
                let body_height = (size.y - grip_height - ui.spacing().item_spacing.y).max(0.0);
                let inner = egui::ScrollArea::both()
                    .id_salt("dialog_contents")
                    .auto_shrink([false, false])
                    .min_scrolled_width(0.0)
                    .min_scrolled_height(0.0)
                    .max_width(size.x)
                    .max_height(body_height)
                    .show_viewport(ui, |ui, viewport| {
                        ui.set_max_size(viewport.size());
                        content(ui)
                    })
                    .inner;
                let (footer, _) =
                    ui.allocate_exact_size(vec2(size.x, grip_height), egui::Sense::hover());
                let grip = egui::Rect::from_min_max(
                    footer.right_top() - vec2(grip_height, 0.0),
                    footer.right_bottom(),
                );
                let response = ui
                    .interact(grip, self.id.with("resize_grip"), egui::Sense::drag())
                    .on_hover_cursor(egui::CursorIcon::ResizeNwSe)
                    .on_hover_text("Drag to resize");
                let corner = grip.right_bottom() - vec2(3.0, 3.0);
                let stroke = egui::Stroke::new(
                    1.0_f32,
                    if response.hovered() || response.dragged() {
                        crate::theme::text()
                    } else {
                        crate::theme::muted()
                    },
                );
                for offset in [4.0, 8.0, 12.0] {
                    ui.painter().line_segment(
                        [corner - vec2(offset, 0.0), corner - vec2(0.0, offset)],
                        stroke,
                    );
                }
                if response.dragged() {
                    // Centered modals grow on both sides: the grip follows the pointer 1:1.
                    let resized = (size + response.drag_delta() * 2.0)
                        .max(Vec2::splat(32.0))
                        .min(maximum);
                    if resized != size {
                        preferences
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .insert(self.key.into(), resized.into());
                        ctx.request_repaint();
                    }
                }
                inner
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Pos2, Rect};

    fn frame(ctx: &Context, screen: Vec2, events: Vec<Event>, key: &'static str) -> Rect {
        let mut rect = Rect::NOTHING;
        let _output = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, screen)),
                events,
                ..Default::default()
            },
            |ctx| {
                rect = Dialog::new(key, [640.0, 650.0])
                    .show(ctx, |ui| {
                        ui.heading("Connection settings");
                        for i in 0..40 {
                            ui.label(format!("Field {i}"));
                        }
                        // Wide contents must scroll horizontally without enlarging the modal.
                        ui.allocate_exact_size(vec2(1800.0, 20.0), egui::Sense::hover());
                    })
                    .response
                    .rect;
            },
        );
        rect
    }
    fn button(point: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }
    #[test]
    fn drag_resize_reopen_other_dialog_and_smaller_viewport()
    -> Result<(), Box<dyn std::error::Error>> {
        let ctx = Context::default();
        let screen = vec2(1600.0, 1100.0);
        frame(&ctx, screen, vec![], "connection");
        let original = frame(&ctx, screen, vec![], "connection");
        assert!(original.height() > 640.0);
        let grip = ctx
            .read_response(Id::new("connection").with("resize_grip"))
            .ok_or("grip")?
            .rect;
        assert!(original.contains_rect(grip));
        let point = grip.center();
        frame(
            &ctx,
            screen,
            vec![Event::PointerMoved(point), button(point, true)],
            "connection",
        );
        let end = point + vec2(100.0, 80.0);
        frame(&ctx, screen, vec![Event::PointerMoved(end)], "connection");
        frame(&ctx, screen, vec![button(end, false)], "connection");
        let enlarged = frame(&ctx, screen, vec![], "connection");
        assert!(
            enlarged.width() > original.width() + 190.0,
            "{original:?} -> {enlarged:?}"
        );
        assert!(enlarged.height() > original.height() + 150.0);
        let saved = snapshot(&preferences(&ctx));
        assert!(saved.contains_key("connection"));
        // A different form has an independent default; reopening restores the first form.
        frame(&ctx, screen, vec![], "other");
        let other = frame(&ctx, screen, vec![], "other");
        assert!((other.width() - original.width()).abs() < 1.0);
        frame(&ctx, screen, vec![], "connection");
        assert_eq!(
            frame(&ctx, screen, vec![], "connection").size(),
            enlarged.size()
        );
        let small = vec2(320.0, 240.0);
        frame(&ctx, small, vec![], "connection");
        let small_rect = frame(&ctx, small, vec![], "connection");
        assert!(
            Rect::from_min_size(Pos2::ZERO, small)
                .expand(1.0)
                .contains_rect(small_rect),
            "{small_rect:?}"
        );
        assert_eq!(snapshot(&preferences(&ctx)), saved);
        frame(&ctx, screen, vec![], "connection");
        assert_eq!(
            frame(&ctx, screen, vec![], "connection").size(),
            enlarged.size()
        );
        Ok(())
    }
    #[test]
    fn maximum_resize_stays_inside_window_and_backdrop_still_closes()
    -> Result<(), Box<dyn std::error::Error>> {
        let ctx = Context::default();
        let screen = vec2(1000.0, 800.0);
        frame(&ctx, screen, vec![], "connection");
        frame(&ctx, screen, vec![], "connection");
        let point = ctx
            .read_response(Id::new("connection").with("resize_grip"))
            .ok_or("grip")?
            .rect
            .center();
        frame(
            &ctx,
            screen,
            vec![Event::PointerMoved(point), button(point, true)],
            "connection",
        );
        let end = point + vec2(2000.0, 2000.0);
        frame(&ctx, screen, vec![Event::PointerMoved(end)], "connection");
        frame(&ctx, screen, vec![button(end, false)], "connection");
        let rect = frame(&ctx, screen, vec![], "connection");
        assert!(
            Rect::from_min_size(Pos2::ZERO, screen)
                .expand(1.0)
                .contains_rect(rect),
            "{rect:?}"
        );
        assert!(rect.width() >= screen.x - 1.0);
        let mut close = false;
        let _output = ctx.run(
            egui::RawInput {
                events: vec![Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                close = Dialog::new("connection", [640.0, 650.0])
                    .show(ctx, |ui| {
                        ui.label("Settings");
                    })
                    .should_close();
            },
        );
        assert!(close);
        Ok(())
    }
}
