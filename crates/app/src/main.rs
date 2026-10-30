//! Composition root: settings, async event observer, native egui shell.
use anyhow::{Context, Result};
use nozhik_config::ConfigManager;
use nozhik_event_bus::{Event, EventBus};
use nozhik_ui::{NozhikUi, Workspace, theme};
use std::time::Duration;
use tokio::runtime::Runtime;

fn main() -> Result<()> {
    let bus = EventBus::new(1024)?;
    let mut args = std::env::args_os().skip(1);
    let mut config_path = None;
    let mut project_path = None;
    while let Some(arg) = args.next() {
        if arg == "--config" {
            config_path = Some(std::path::PathBuf::from(
                args.next().context("--config requires a settings path")?,
            ));
        } else {
            project_path = Some(std::path::PathBuf::from(arg));
        }
    }
    let settings = config_path
        .map_or_else(
            || ConfigManager::load(bus.clone()),
            |path| ConfigManager::from_path(path, bus.clone()),
        )
        .context("Cannot load user settings")?;
    let config = settings.snapshot()?;
    eprintln!("Settings: {}", settings.path().display());
    let runtime = Runtime::new().context("Cannot start Tokio runtime")?;
    let lifecycle = nozhik_ai_completion::lifecycle::Lifecycle::start(runtime.handle(), None);
    let ai_settings = lifecycle.controller();
    let options = eframe::NativeOptions {
        viewport: egui_viewport(project_path.is_some())?,
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    let ui_runtime = runtime.handle().clone();
    let result = eframe::run_native(
        "Nozhik",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            let mut ui = NozhikUi::new(bus.clone(), settings, config);
            ui.attach_runtime(&cc.egui_ctx, &ui_runtime);
            let mut ui = Workspace::new(ui, ui_runtime, ai_settings);
            if let Some(path) = project_path {
                ui.open_path(path, &cc.egui_ctx);
            }
            bus.publish(Event::ActionTriggered("app.started".into()));
            Ok(Box::new(ui))
        }),
    )
    .map_err(|error| anyhow::anyhow!("Cannot run native UI: {error}"));
    runtime.block_on(lifecycle.shutdown());
    runtime.shutdown_timeout(Duration::from_secs(1));
    result
}

fn egui_viewport(project: bool) -> Result<eframe::egui::ViewportBuilder> {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/window-icon.png"))
        .context("Cannot decode the application icon")?;
    Ok(eframe::egui::ViewportBuilder::default()
        .with_inner_size(if project {
            [1440.0, 900.0]
        } else {
            [1000.0, 680.0]
        })
        .with_min_inner_size([800.0, 540.0])
        .with_fullsize_content_view(cfg!(target_os = "macos"))
        .with_title_shown(!cfg!(target_os = "macos"))
        .with_titlebar_shown(!cfg!(target_os = "macos"))
        .with_title("Nozhik")
        .with_icon(icon))
}
