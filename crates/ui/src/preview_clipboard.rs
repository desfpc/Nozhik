//! Copy-only context menu shared by rendered file previews.
use egui::text_selection::LabelSelectionState;

/// Run before rendering labels so a menu click can use egui's multi-label copy
/// in this pass. Preserve the selection while the popup owns pointer input.
pub fn menu(ui: &egui::Ui, path: &std::path::Path, enabled: bool) {
    let id = ui.id().with(("preview-copy-menu", path));
    let saved_id = id.with("selection");
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
    if opened || popup.is_open() {
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
}
