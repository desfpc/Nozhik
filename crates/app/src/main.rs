//! Phase 0 composition root: settings, background event consumer, native window.

use std::time::Duration;

use anyhow::{Context, Result};
use nozhik_config::ConfigManager;
use nozhik_event_bus::{Event, EventBus};
use tokio::{runtime::Runtime, sync::broadcast::error::RecvError};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};

struct App {
    window: Option<Window>,
    bus: EventBus,
    settings: ConfigManager,
    startup_error: Option<winit::error::OsError>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        match event_loop.create_window(Window::default_attributes().with_title("Nozhik — Phase 0"))
        {
            Ok(window) => {
                self.window = Some(window);
                self.bus
                    .publish(Event::ActionTriggered("app.started".into()));
                eprintln!("Settings: {}", self.settings.path().display());
            }
            Err(error) => {
                self.startup_error = Some(error);
                event_loop.exit();
            }
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.window = None;
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.window.as_ref().is_some_and(|window| window.id() == id)
            && matches!(event, WindowEvent::CloseRequested)
        {
            event_loop.exit();
        }
    }
}

fn main() -> Result<()> {
    let bus = EventBus::new(1024)?;
    let settings = ConfigManager::load(bus.clone()).context("Cannot load user settings")?;
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

    // Winit stays on the main thread; Tokio workers process async notifications.
    let result = (|| -> Result<()> {
        let event_loop = EventLoop::new().context("Cannot initialize the window event loop")?;
        event_loop.set_control_flow(ControlFlow::Wait);
        let mut app = App {
            window: None,
            bus,
            settings,
            startup_error: None,
        };
        event_loop
            .run_app(&mut app)
            .context("Window event loop failed")?;
        if let Some(error) = app.startup_error.take() {
            return Err(error).context("Cannot create the application window");
        }
        Ok(())
    })();

    // All senders were dropped by the composition root. Drain queued events.
    let shutdown =
        runtime.block_on(async { tokio::time::timeout(Duration::from_secs(2), consumer).await });
    runtime.shutdown_timeout(Duration::from_secs(1));
    result?;
    shutdown
        .context("Event consumer shutdown timed out")?
        .context("Event consumer failed")?;
    Ok(())
}
