use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
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
    pub allow_writes: Arc<AtomicBool>,
    pub write_circuit_open: Arc<AtomicBool>,
    pub validating: Arc<AtomicBool>,
    pub shutdown: Arc<AtomicBool>,
    pub ctx: Arc<Mutex<Option<AppContext>>>,
    pub config: Config,
    pub db: Arc<Db>,
    pub client: Arc<Mutex<Option<Arc<DiscordClient>>>>,
    pub tokio_handle: tokio::runtime::Handle,
    pub repaint_signal: Arc<Mutex<Option<egui::Context>>>,
    /// Native window handle (HWND on Windows), captured when the UI is created.
    pub window_hwnd: Arc<AtomicIsize>,
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
            allow_writes: Arc::new(AtomicBool::new(config.allow_writes)),
            write_circuit_open: Arc::new(AtomicBool::new(false)),
            validating: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(AtomicBool::new(false)),
            ctx: Arc::new(Mutex::new(None)),
            config,
            db,
            client: Arc::new(Mutex::new(None)),
            tokio_handle,
            repaint_signal: Arc::new(Mutex::new(None)),
            window_hwnd: Arc::new(AtomicIsize::new(0)),
            action_tx: None,
        }
    }

    pub fn refresh_gateway_enabled(&self) -> bool {
        let persisted = crate::config::load_gateway(&self.config.data_dir);
        self.gateway_enabled.store(persisted, Ordering::SeqCst);
        persisted
    }

    pub fn refresh_allow_writes(&self) -> bool {
        let persisted = crate::config::load_allow_writes(&self.config.data_dir);
        self.allow_writes.store(persisted, Ordering::SeqCst);
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

    pub fn set_window_hwnd(&self, hwnd: isize) {
        self.window_hwnd.store(hwnd, Ordering::SeqCst);
    }

    pub fn window_hwnd(&self) -> isize {
        self.window_hwnd.load(Ordering::SeqCst)
    }

    /// Reveal the native window directly, bypassing eframe's viewport-command
    /// queue. A hidden Win32 window is not guaranteed to produce a frame
    /// (`WM_PAINT`), so queued viewport commands may not flush until the window
    /// is shown first.
    pub fn show_native_window(&self) -> bool {
        #[cfg(windows)]
        {
            show_window_by_hwnd(self.window_hwnd())
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
}

#[cfg(windows)]
pub fn show_window_by_hwnd(hwnd: isize) -> bool {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsIconic, IsWindow, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
    };

    if hwnd == 0 {
        return false;
    }

    unsafe {
        if IsWindow(hwnd as HWND) == 0 {
            return false;
        }
        let cmd = if IsIconic(hwnd as HWND) != 0 {
            SW_RESTORE
        } else {
            SW_SHOW
        };
        ShowWindow(hwnd as HWND, cmd);
        SetForegroundWindow(hwnd as HWND);
    }
    true
}
