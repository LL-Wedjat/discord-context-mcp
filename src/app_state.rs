use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::Mutex;

use crate::config::Config;
use crate::db::Db;
use crate::discord::DiscordClient;
use crate::mcp::AppContext;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    OpenSettings,
    Reconnect,
    Quit,
}

#[derive(Clone)]
pub struct AppState {
    pub status: Arc<StdMutex<String>>,
    pub token_present: Arc<Mutex<bool>>,
    pub autostart: Arc<Mutex<bool>>,
    pub gateway_enabled: Arc<AtomicBool>,
    pub validating: Arc<AtomicBool>,
    pub shutdown: Arc<AtomicBool>,
    pub ctx: Arc<Mutex<Option<AppContext>>>,
    pub config: Config,
    pub db: Arc<Db>,
    pub client: Arc<Mutex<Option<Arc<DiscordClient>>>>,
    pub tokio_handle: tokio::runtime::Handle,
    pub repaint_signal: Arc<Mutex<Option<egui::Context>>>,
    pub action_tx: Option<tokio::sync::mpsc::UnboundedSender<TrayAction>>,
}

impl AppState {
    pub fn new(
        config: Config,
        db: Arc<Db>,
        token_present: bool,
        tokio_handle: tokio::runtime::Handle,
    ) -> Self {
        let initial_status = if token_present {
            "Ready".to_string()
        } else {
            "No token configured".to_string()
        };

        Self {
            status: Arc::new(StdMutex::new(initial_status)),
            token_present: Arc::new(Mutex::new(token_present)),
            autostart: Arc::new(Mutex::new(config.autostart)),
            gateway_enabled: Arc::new(AtomicBool::new(config.gateway_enabled)),
            validating: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(AtomicBool::new(false)),
            ctx: Arc::new(Mutex::new(None)),
            config,
            db,
            client: Arc::new(Mutex::new(None)),
            tokio_handle,
            repaint_signal: Arc::new(Mutex::new(None)),
            action_tx: None,
        }
    }

    pub fn refresh_gateway_enabled(&self) -> bool {
        let persisted = crate::config::load_gateway(&self.config.data_dir);
        self.gateway_enabled.store(persisted, Ordering::SeqCst);
        persisted
    }

    pub fn set_status(&self, status: impl Into<String>) {
        let status_str = status.into();
        {
            let mut guard = self
                .status
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *guard = status_str;
        }

        // Spawn a task only to notify repaint if repaint context is registered
        let repaint_arc = Arc::clone(&self.repaint_signal);
        self.tokio_handle.spawn(async move {
            if let Some(ctx) = repaint_arc.lock().await.as_ref() {
                ctx.request_repaint();
            }
        });
    }

    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    pub fn trigger_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let repaint_arc = Arc::clone(&self.repaint_signal);
        self.tokio_handle.spawn(async move {
            if let Some(ctx) = repaint_arc.lock().await.as_ref() {
                ctx.request_repaint();
            }
        });
    }
}
