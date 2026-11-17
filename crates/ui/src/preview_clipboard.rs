//! Copy-only context menu shared by rendered file previews.
use egui::text_selection::LabelSelectionState;

/// Keeps secondary pointer input away from selectable labels while they render.
/// The popup itself sees the original pointer before this guard is created.
pub struct SelectionGuard {
    ctx: egui::Context,
    pointer: Option<egui::PointerState>,
}

impl Drop for SelectionGuard {
    fn drop(&mut self) {
        if let Some(pointer) = self.pointer.take() {
            self.ctx.input_mut(|input| input.pointer = pointer);
        }
    }
}

fn freeze_selection(ctx: &egui::Context) -> SelectionGuard {
    let restore_id = egui::Id::new(("preview-selection-restore", ctx.viewport_id()));
    let installed_id = egui::Id::new("preview-selection-hook");
    let installed = ctx.data(|data| data.get_temp::<bool>(installed_id).unwrap_or(false));
    if !installed {
        // egui's label-selection hook runs first. Restore afterwards as it also
        // clears selection on any button press outside a hovered label.
        ctx.on_end_pass(
            "preview-selection-restore",
            std::sync::Arc::new(|ctx| {
                let id = egui::Id::new(("preview-selection-restore", ctx.viewport_id()));
                let selection = ctx.data_mut(|data| data.remove_temp::<LabelSelectionState>(id));
                if let Some(selection) = selection {
                    selection.store(ctx);
                }
            }),
        );
        ctx.data_mut(|data| data.insert_temp(installed_id, true));
    }
    let selection = LabelSelectionState::load(ctx);
    ctx.data_mut(|data| data.insert_temp(restore_id, selection));
    let pointer = ctx.input_mut(|input| std::mem::take(&mut input.pointer));
    SelectionGuard {
        ctx: ctx.clone(),
        pointer: Some(pointer),
    }
}

/// Run before rendering labels so a menu click can use egui's multi-label copy
/// in this pass. Preserve the selection while the popup owns pointer input.
pub fn menu(ui: &egui::Ui, path: &std::path::Path, enabled: bool) -> SelectionGuard {
    let id = ui.id().with(("preview-copy-menu", path));
    let saved_id = id.with("selection");
    let gesture_id = id.with("secondary-gesture");
    let captured = ui
        .ctx()
        .data(|data| data.get_temp::<bool>(gesture_id).unwrap_or(false));
    let secondary = captured
        || (enabled
            && ui.is_enabled()
            && ui.rect_contains_pointer(ui.available_rect_before_wrap())
            && ui.input(|input| {
                input.pointer.secondary_down()
                    || input.pointer.secondary_pressed()
                    || input.pointer.secondary_released()
            }));
    let held = secondary && ui.input(|input| input.pointer.secondary_down());
    ui.ctx().data_mut(|data| data.insert_temp(gesture_id, held));
    let opened = enabled
        && ui.is_enabled()
        && ui.rect_contains_pointer(ui.available_rect_before_wrap())
        && ui.input(|i| i.pointer.secondary_clicked());
    if opened {
        let selection = LabelSelectionState::load(ui.ctx());
        ui.ctx()
            .data_mut(|data| data.insert_temp(saved_id, selection));
    }
    let popup = egui::Popup::new(id, ui.ctx().clone(), egui::Pos2::ZERO, ui.layer_id())
        .kind(egui::PopupKind::Menu)
        .layout(egui::Layout::top_down_justified(egui::Align::Min))
        .open_memory(opened.then_some(egui::SetOpenCommand::Bool(true)))
        .at_pointer_fixed();
    let menu_active = opened || popup.is_open();
    if menu_active {
        if let Some(selection) = ui
            .ctx()
            .data(|data| data.get_temp::<LabelSelectionState>(saved_id))
        {
            selection.store(ui.ctx());
        }
        let selected = LabelSelectionState::load(ui.ctx()).has_selection();
        popup.show(|ui| {
            if ui
                .add_enabled(selected, egui::Button::new("Copy"))
                .clicked()
            {
                ui.ctx()
                    .input_mut(|input| input.events.push(egui::Event::Copy));
                ui.close();
            }
        });
    } else {
        ui.ctx()
            .data_mut(|data| data.remove::<LabelSelectionState>(saved_id));
    }
    if secondary || menu_active {
        freeze_selection(ui.ctx())
    } else {
        SelectionGuard {
            ctx: ui.ctx().clone(),
            pointer: None,
        }
    }
}
