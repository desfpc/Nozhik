//! One action catalog drives embedded and native menus and command search.
use crate::NozhikUi;

type MenuEntry = (&'static str, &'static str, &'static str);
pub const MENUS: &[(&str, &[MenuEntry])] = &[
    (
        "File",
        &[
            ("ui.new_project", "New…", ""),
            ("ui.new_file", "New File", ""),
            ("ui.open", "Open File / Folder…", ""),
            ("ui.open_file_dialog", "Open File…", ""),
            ("ui.recent_projects", "Recent Projects", ""),
            ("ui.save_file", "Save", "editorOpen"),
            ("ui.save_all", "Save All", ""),
            ("ui.close_tab", "Close Tab", "editorOpen"),
            ("ui.settings", "Settings…", ""),
            ("ui.exit", "Exit", ""),
        ],
    ),
    (
        "Edit",
        &[
            ("ui.undo", "Undo", "editorOpen"),
            ("ui.redo", "Redo", "editorOpen"),
            ("ui.cut", "Cut", "editorOpen"),
            ("ui.copy", "Copy", "editorOpen"),
            ("ui.paste", "Paste", "editorOpen"),
            ("ui.find", "Find in File", "editorOpen"),
            ("ui.replace", "Replace", "editorOpen"),
        ],
    ),
    (
        "View",
        &[
            ("ui.project", "Project", ""),
            ("ui.git", "Git", ""),
            ("ui.database", "Database", ""),
            ("ui.terminal", "Terminal", ""),
            ("ui.remote", "Remote SSH", ""),
            ("ui.fullscreen", "Toggle Full Screen", ""),
        ],
    ),
    (
        "Navigate",
        &[
            ("ui.search_everywhere", "Search Everywhere", ""),
            ("ui.go_file", "Go to File", ""),
            ("ui.recent_files", "Recent Files", ""),
        ],
    ),
    (
        "Code",
        &[("ui.definition", "Go to Definition", "editorOpen")],
    ),
    (
        "Git",
        &[
            ("ui.commit", "Commit", "projectOpen"),
            ("ui.fetch", "Fetch", "projectOpen"),
            ("ui.pull", "Pull (fast-forward)", "projectOpen"),
            ("ui.push", "Push", "projectOpen"),
            ("ui.branches", "Branches", "projectOpen"),
        ],
    ),
    (
        "Database",
        &[
            ("ui.new_connection", "New Connection", ""),
            ("ui.sql_console", "Open SQL Console", ""),
        ],
    ),
    ("Terminal", &[("ui.new_terminal", "New Terminal", "")]),
    ("Tools", &[("ui.plugins", "Plugins", "")]),
    ("Help", &[("ui.about", "About Nozhik", "")]),
];
pub fn shortcut(id: &str, bindings: &[nozhik_plugin_host::keymap::KeyBinding]) -> String {
    let labels = bindings
        .iter()
        .filter(|b| b.action == id)
        .map(|b| b.chord.label())
        .collect::<Vec<_>>()
        .join(", ");
    if id == "ui.search_everywhere" {
        format!("Double Shift {labels}")
    } else {
        labels
    }
}
impl NozhikUi {
    #[allow(
        clippy::needless_pass_by_ref_mut,
        reason = "macOS updates native menu state; embedded menus are read-only on other platforms"
    )]
    pub(crate) fn menu_bar(&mut self, ctx: &egui::Context) {
        let Some(manager) = &self.plugins else {
            return;
        };
        let context = self.action_context();
        let available = manager.actions().available(context).unwrap_or_default();
        let available = if self.global_modal() {
            Vec::new()
        } else {
            available
        };
        let bindings = manager.keymap().bindings().unwrap_or_default();
        #[cfg(target_os = "macos")]
        if let Some(native) = &mut self.native_menu {
            let actions = native.tick(&available, &bindings, &self.recent_projects.entries);
            for id in actions {
                if id.starts_with("ui.open_project:") {
                    if !self.global_modal() {
                        self.integration.actions.push(id);
                        ctx.request_repaint();
                    }
                } else {
                    self.action(&id);
                }
            }
            return;
        }
        egui::TopBottomPanel::top("main_menu")
            .exact_height(27.0)
            .show(ctx, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    for (title, items) in MENUS {
                        ui.menu_button(*title, |ui| {
                            for (id, title, _) in *items {
                                if *id == "ui.recent_projects" {
                                    ui.add_enabled_ui(!self.global_modal(), |ui| {
                                        ui.menu_button(*title, |ui| {
                                            egui::ScrollArea::vertical().max_height(400.0).show(
                                                ui,
                                                |ui| {
                                                    if let Some(path) =
                                                        self.recent_projects.list(ui)
                                                    {
                                                        self.open_project(path);
                                                        ctx.request_repaint();
                                                        ui.close();
                                                    }
                                                },
                                            );
                                        });
                                    });
                                    continue;
                                }
                                let enabled =
                                    available.iter().any(|a| a.id == *id) && !self.global_modal();
                                if ui
                                    .add_enabled(
                                        enabled,
                                        egui::Button::new(*title)
                                            .shortcut_text(shortcut(id, &bindings)),
                                    )
                                    .clicked()
                                {
                                    self.action(id);
                                    ctx.request_repaint();
                                    ui.close();
                                }
                            }
                        });
                    }
                });
            });
    }
}

#[cfg(target_os = "macos")]
pub mod native {
    use super::{MENUS, shortcut};
    use muda::{
        Menu, MenuEvent, MenuItem, Submenu,
        accelerator::{Accelerator, Code, Modifiers},
    };
    use nozhik_plugin_host::{keymap::KeyBinding, registry::Action};
    use std::sync::mpsc;
    pub struct NativeMenu {
        _menu: Menu,
        items: Vec<(&'static str, &'static str, MenuItem)>,
        events: mpsc::Receiver<String>,
        signature: String,
        recent: Submenu,
        recent_entries: Option<Vec<crate::recent_projects::RecentProject>>,
        recent_enabled: bool,
    }
    impl NativeMenu {
        pub fn new(ctx: &egui::Context) -> muda::Result<Self> {
            let menu = Menu::new();
            let mut items = Vec::new();
            let application = Submenu::new("Nozhik", true);
            for (id, title) in [
                ("ui.about", "About Nozhik"),
                ("ui.settings", "Settings…"),
                ("ui.exit", "Quit Nozhik"),
            ] {
                let item = MenuItem::with_id(id, title, true, None);
                application.append(&item)?;
                items.push((id, title, item));
            }
            menu.append(&application)?;
            let recent = Submenu::new("Recent Projects", true);
            for (title, entries) in MENUS {
                let submenu = Submenu::new(*title, true);
                for (id, title, _) in *entries {
                    if *id == "ui.recent_projects" {
                        submenu.append(&recent)?;
                        continue;
                    }
                    let item = MenuItem::with_id(*id, *title, true, None);
                    submenu.append(&item)?;
                    items.push((*id, *title, item));
                }
                menu.append(&submenu)?;
            }
            menu.init_for_nsapp();
            let (sender, events) = mpsc::sync_channel(32);
            let ctx = ctx.clone();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                let _sent = sender.try_send(event.id.0);
                ctx.request_repaint();
            }));
            Ok(Self {
                _menu: menu,
                items,
                events,
                signature: String::new(),
                recent,
                recent_entries: None,
                recent_enabled: true,
            })
        }
        pub fn tick(
            &mut self,
            available: &[Action],
            bindings: &[KeyBinding],
            projects: &[crate::recent_projects::RecentProject],
        ) -> Vec<String> {
            let enabled = available.iter().any(|a| a.id == "ui.recent_projects");
            if enabled != self.recent_enabled {
                self.recent.set_enabled(enabled);
                self.recent_enabled = enabled;
            }
            if self.recent_entries.as_deref() != Some(projects) {
                self.recent_entries = Some(projects.to_vec());
                while self.recent.remove_at(0).is_some() {}
                if projects.is_empty()
                    && let Err(e) =
                        self.recent
                            .append(&MenuItem::new("No recent projects", false, None))
                {
                    eprintln!("Recent projects menu: {e}");
                }
                for project in projects {
                    let path = project.path.display();
                    let item = MenuItem::with_id(
                        format!("ui.open_project:{path}"),
                        path.to_string(),
                        true,
                        None,
                    );
                    if let Err(e) = self.recent.append(&item) {
                        eprintln!("Recent projects menu: {e}");
                    }
                }
            }
            let signature = format!(
                "{:?}{:?}",
                available.iter().map(|a| &a.id).collect::<Vec<_>>(),
                bindings
                    .iter()
                    .map(|b| (&b.action, b.chord.label()))
                    .collect::<Vec<_>>()
            );
            if self.signature == signature {
                return self.events.try_iter().take(32).collect();
            }
            self.signature = signature;
            for (id, title, item) in &self.items {
                let title = *title;
                item.set_enabled(available.iter().any(|a| a.id == *id));
                let binding = bindings.iter().find(|b| b.action == *id);
                let accelerator = binding.and_then(|b| {
                    let code = match b.chord.key.as_str() {
                        "Comma" => Some(Code::Comma),
                        key => format!("Key{key}")
                            .parse()
                            .ok()
                            .or_else(|| key.parse().ok()),
                    }?;
                    let mut modifiers = Modifiers::empty();
                    if b.chord.primary || b.chord.cmd {
                        modifiers |= Modifiers::META;
                    }
                    if b.chord.ctrl {
                        modifiers |= Modifiers::CONTROL;
                    }
                    if b.chord.shift {
                        modifiers |= Modifiers::SHIFT;
                    }
                    if b.chord.alt {
                        modifiers |= Modifiers::ALT;
                    }
                    Some(Accelerator::new(modifiers, code))
                });
                if let Err(e) = item.set_accelerator(accelerator) {
                    eprintln!("Native accelerator: {e}");
                }
                item.set_text(if accelerator.is_none() {
                    format!("{title}  {}", shortcut(id, bindings))
                } else {
                    title.into()
                });
            }
            self.events.try_iter().take(32).collect()
        }
    }
}
