//! Composition root: settings, async event observer, native egui shell.
use anyhow::{Context, Result};
use nozhik_config::ConfigManager;
use nozhik_event_bus::{Event, EventBus};
use nozhik_ui::{NozhikUi, theme};
use std::time::Duration;
use tokio::{runtime::Runtime, sync::broadcast::error::RecvError};

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
    let mut events = bus.subscribe();
    let runtime = Runtime::new().context("Cannot start Tokio runtime")?;
    let consumer = runtime.spawn(async move {
        loop {
            match events.recv().await {
                Ok(_) => {}
                Err(RecvError::Lagged(count)) => {
                    eprintln!("Event logger missed {count} notifications");
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
    let options = eframe::NativeOptions {
        viewport: egui_viewport()?,
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
            match project_path.map_or_else(std::env::current_dir, Ok) {
                Ok(path) => ui.open_project(path),
                Err(error) => eprintln!("Cannot resolve project folder: {error}"),
            }
            bus.publish(Event::ActionTriggered("app.started".into()));
            Ok(Box::new(ui))
        }),
    )
    .map_err(|error| anyhow::anyhow!("Cannot run native UI: {error}"));
    let shutdown =
        runtime.block_on(async { tokio::time::timeout(Duration::from_secs(2), consumer).await });
    runtime.shutdown_timeout(Duration::from_secs(1));
    result?;
    shutdown
        .context("Event consumer shutdown timed out")?
        .context("Event consumer failed")?;
    Ok(())
}

fn egui_viewport() -> Result<eframe::egui::ViewportBuilder> {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/window-icon.png"))
        .context("Cannot decode the application icon")?;
    Ok(eframe::egui::ViewportBuilder::default()
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([800.0, 540.0])
        .with_title("Nozhik")
        .with_icon(icon))
}
