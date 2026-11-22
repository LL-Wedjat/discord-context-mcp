use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
    pub status: Arc<Mutex<String>>,
    pub token_present: Arc<Mutex<bool>>,
    pub autostart: Arc<Mutex<bool>>,
    pub shutdown: Arc<AtomicBool>,
    pub ctx: Arc<Mutex<Option<AppContext>>>,
    pub config: Config,
    pub db: Arc<Db>,
    pub client: Arc<Mutex<Option<Arc<DiscordClient>>>>,
    pub tokio_handle: tokio::runtime::Handle,
    pub repaint_signal: Arc<Mutex<Option<egui::Context>>>,
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
            status: Arc::new(Mutex::new(initial_status)),
            token_present: Arc::new(Mutex::new(token_present)),
            autostart: Arc::new(Mutex::new(config.autostart)),
            shutdown: Arc::new(AtomicBool::new(false)),
            ctx: Arc::new(Mutex::new(None)),
            config,
            db,
            client: Arc::new(Mutex::new(None)),
            tokio_handle,
            repaint_signal: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_status(&self, status: impl Into<String>) {
        let status_str = status.into();
        let status_arc = Arc::clone(&self.status);
        let repaint_arc = Arc::clone(&self.repaint_signal);

        self.tokio_handle.spawn(async move {
            {
                let mut guard = status_arc.lock().await;
                *guard = status_str;
            }
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
