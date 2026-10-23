//! Composition root: settings, async event observer, native egui shell.
use anyhow::{Context, Result};
use nozhik_config::ConfigManager;
use nozhik_event_bus::{Event, EventBus};
use nozhik_ui::{NozhikUi, theme};
use std::time::Duration;
use tokio::{runtime::Runtime, sync::broadcast::error::RecvError};

fn main() -> Result<()> {
    let bus = EventBus::new(1024)?;
    let settings = ConfigManager::load(bus.clone()).context("Cannot load user settings")?;
    let config = settings.snapshot()?;
    eprintln!("Settings: {}", settings.path().display());
    let mut events = bus.subscribe();
    let runtime = Runtime::new().context("Cannot start Tokio runtime")?;
    let consumer = runtime.spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => eprintln!("Event: {event:?}"),
                Err(RecvError::Lagged(count)) => {
                    eprintln!("Event logger missed {count} notifications");
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
    let options = eframe::NativeOptions {
        viewport: egui_viewport(),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    let result = eframe::run_native(
        "Nozhik",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            let ui = NozhikUi::new(bus.clone(), settings, config);
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

fn egui_viewport() -> eframe::egui::ViewportBuilder {
    eframe::egui::ViewportBuilder::default()
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([800.0, 540.0])
        .with_title("Nozhik")
}
