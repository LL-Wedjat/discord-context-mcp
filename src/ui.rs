use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use eframe::egui::{self, Color32, RichText, ViewportCommand};
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::app_state::{AppState, TrayAction};
use crate::config;
use crate::discord::DiscordClient;
use crate::token;

pub struct SettingsApp {
    pub state: AppState,
    pub token_input: String,
    pub status_text: String,
    pub autostart_toggle: bool,
    pub gateway_toggle: bool,
    pub allow_writes_toggle: bool,
    pub is_saving: bool,
    gateway_tx: mpsc::UnboundedSender<bool>,
    gateway_in_flight: Arc<AtomicUsize>,
    write_tx: mpsc::UnboundedSender<bool>,
    write_in_flight: Arc<AtomicUsize>,
}

impl SettingsApp {
    pub fn new(state: AppState) -> Self {
        let initial_autostart = state
            .autostart
            .try_lock()
            .map(|g| *g)
            .unwrap_or(state.config.autostart);

        let initial_status = state
            .status
            .try_lock()
            .map(|g| g.clone())
            .unwrap_or_else(|_| "Ready".to_string());

        let initial_gateway = state.gateway_enabled.load(Ordering::SeqCst);
        let initial_writes = state.allow_writes.load(Ordering::SeqCst);

        let (gateway_tx, mut gateway_rx) = mpsc::unbounded_channel::<bool>();
        let gateway_in_flight = Arc::new(AtomicUsize::new(0));

        let (write_tx, mut write_rx) = mpsc::unbounded_channel::<bool>();
        let write_in_flight = Arc::new(AtomicUsize::new(0));

        let state_for_writes = state.clone();
        let in_flight_for_writes = Arc::clone(&write_in_flight);
        state.tokio_handle.spawn(async move {
            while let Some(enabled) = write_rx.recv().await {
                let data_dir = state_for_writes.config.data_dir.clone();
                match config::save_allow_writes(&data_dir, enabled) {
                    Ok(()) => {
                        info!("Allow writes preference updated: {}", enabled);
                        if enabled {
                            state_for_writes.allow_writes.store(true, Ordering::SeqCst);
                            state_for_writes.write_circuit_open.store(false, Ordering::SeqCst);
                            state_for_writes.set_status("Write actions enabled (sending & modifying messages)");
                        } else {
                            state_for_writes.allow_writes.store(false, Ordering::SeqCst);
                            state_for_writes.set_status("Write actions disabled — MCP server is read-only");
                        }
                    }
                    Err(e) => {
                        error!("Failed to save allow_writes setting: {}", e);
                        if enabled {
                            state_for_writes.refresh_allow_writes();
                        } else {
                            state_for_writes.allow_writes.store(false, Ordering::SeqCst);
                        }
                        state_for_writes.set_status(format!("Allow writes save error: {}", e));
                    }
                }
                in_flight_for_writes.fetch_sub(1, Ordering::SeqCst);
                let guard = state_for_writes.repaint_signal.lock().await;
                if let Some(ctx) = guard.as_ref() {
                    ctx.request_repaint();
                }
            }
        });

        let state_for_gw = state.clone();
        let in_flight_for_gw = Arc::clone(&gateway_in_flight);
        state.tokio_handle.spawn(async move {
            while let Some(enabled) = gateway_rx.recv().await {
                let data_dir = state_for_gw.config.data_dir.clone();
                match config::save_gateway(&data_dir, enabled) {
                    Ok(()) => {
                        info!("Gateway preference updated: {}", enabled);
                        state_for_gw.refresh_gateway_enabled();
                        if !enabled {
                            {
                                let client_lock = state_for_gw.client.lock().await;
                                if let Some(c) = client_lock.as_ref() {
                                    // stop() sets the shutdown flag and fires the
                                    // shutdown Notify, so the gateway wakes from
                                    // any read/backoff immediately. Only report the
                                    // gateway as disabled after stop() has run.
                                    c.stop();
                                }
                            }
                            state_for_gw
                                .set_status("Gateway preference saved — live gateway disabled");
                        } else {
                            state_for_gw.set_status(
                                "Gateway preference saved — use Reconnect to start the gateway",
                            );
                        }
                    }
                    Err(e) => {
                        error!("Failed to save gateway setting: {}", e);
                        state_for_gw.set_status(format!("Gateway save error: {}", e));
                    }
                }
                in_flight_for_gw.fetch_sub(1, Ordering::SeqCst);
                {
                    let guard = state_for_gw.repaint_signal.lock().await;
                    if let Some(ctx) = guard.as_ref() {
                        ctx.request_repaint();
                    }
                }
            }
        });

        Self {
            state,
            token_input: String::new(),
            status_text: initial_status,
            autostart_toggle: initial_autostart,
            gateway_toggle: initial_gateway,
            allow_writes_toggle: initial_writes,
            is_saving: false,
            gateway_tx,
            gateway_in_flight,
            write_tx,
            write_in_flight,
        }
    }
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Register repaint signal context
        if let Ok(mut lock) = self.state.repaint_signal.try_lock() {
            if lock.is_none() {
                *lock = Some(ctx.clone());
            }
        }

        // Close request handling: check shutdown first, then intercept close and hide window
        if self.state.is_shutdown() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }

        // Update status text from shared state
        if let Ok(guard) = self.state.status.try_lock() {
            self.status_text = guard.clone();
        }

        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        }

        // Update validating state from shared AtomicBool
        self.is_saving = self.state.validating.load(Ordering::SeqCst);

        // Sync gateway_toggle from state when no save task is in flight
        if self.gateway_in_flight.load(Ordering::SeqCst) == 0 {
            self.gateway_toggle = self.state.gateway_enabled.load(Ordering::SeqCst);
        }

        // Sync allow_writes_toggle from state when no save task is in flight
        if self.write_in_flight.load(Ordering::SeqCst) == 0 {
            self.allow_writes_toggle = self.state.allow_writes.load(Ordering::SeqCst);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.heading(RichText::new("Discord Context MCP").strong().size(18.0));
            });
            ui.add_space(8.0);

            // Status display
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.label("Status:");
                    ui.colored_label(Color32::from_rgb(100, 200, 255), &self.status_text);
                });
            });

            ui.add_space(8.0);

            // Token input
            ui.label("Discord User Token:");
            ui.add(
                egui::TextEdit::singleline(&mut self.token_input)
                    .password(true)
                    .hint_text("Paste Discord user token")
                    .desired_width(f32::INFINITY),
            );

            ui.add_space(4.0);

            // Save Token Button
            ui.horizontal(|ui| {
                let save_enabled = !self.is_saving && !self.token_input.trim().is_empty();
                if ui
                    .add_enabled(save_enabled, egui::Button::new("Save Token"))
                    .clicked()
                {
                    self.state.validating.store(true, Ordering::SeqCst);
                    self.is_saving = true;
                    let input_token = self.token_input.trim().to_string();
                    self.token_input.clear();

                    let state_clone = self.state.clone();
                    let repaint_ctx = ctx.clone();

                    state_clone.set_status("Validating token...");

                    self.state.tokio_handle.spawn(async move {
                        match DiscordClient::validate_token(&input_token).await {
                            Ok(user) => {
                                if let Err(e) = token::store_token(&input_token) {
                                    error!("Failed to store token: {}", e);
                                    state_clone.set_status(format!("Error saving token: {}", e));
                                } else {
                                    info!(
                                        "Token validated and stored successfully for user {}",
                                        user.username
                                    );
                                    let new_client = match DiscordClient::new(
                                        input_token.clone(),
                                        Arc::clone(&state_clone.db),
                                    ) {
                                        Ok(c) => Arc::new(c),
                                        Err(e) => {
                                            state_clone.set_status(format!("Client error: {}", e));
                                            state_clone.validating.store(false, Ordering::SeqCst);
                                            repaint_ctx.request_repaint();
                                            return;
                                        }
                                    };

                                    // Replace client in state
                                    {
                                        let mut client_lock = state_clone.client.lock().await;
                                        if let Some(old_client) = client_lock.as_ref() {
                                            old_client.stop();
                                        }
                                        *client_lock = Some(Arc::clone(&new_client));
                                    }

                                    // Replace or update AppContext
                                    {
                                        let ctx_lock = state_clone.ctx.lock().await;
                                        if let Some(ref mcp_ctx) = *ctx_lock {
                                            mcp_ctx.set_client(Some(Arc::clone(&new_client))).await;
                                        }
                                    }

                                    // Update token present flag
                                    {
                                        let mut tp_lock = state_clone.token_present.lock().await;
                                        *tp_lock = true;
                                    }

                                    // Reset write circuit open flag on successful token save
                                    state_clone
                                        .write_circuit_open
                                        .store(false, Ordering::SeqCst);

                                    // Clear any cache belonging to a previous account before
                                    // spawning the background init task, so MCP tools see a
                                    // clean cache for the new account immediately.
                                    if let Err(e) = state_clone.db.ensure_account(&user.id) {
                                        error!(
                                            "Failed to verify cached account in database: {}",
                                            e
                                        );
                                        state_clone.set_status(format!("Cache error: {}", e));
                                        state_clone.validating.store(false, Ordering::SeqCst);
                                        repaint_ctx.request_repaint();
                                        return;
                                    }

                                    // Start the gateway in the background only when the user
                                    // explicitly enabled it. REST-only is the default and must
                                    // also apply to this primary token-setup flow.
                                    let gw_client = Arc::clone(&new_client);
                                    let gw_token = input_token.clone();
                                    let state_gw = state_clone.clone();
                                    tokio::spawn(async move {
                                        let _ = gw_client.init().await;
                                        if state_gw.refresh_gateway_enabled() {
                                            let _ = gw_client.start_gateway(gw_token);
                                        }
                                    });

                                    state_clone
                                        .set_status(format!("Connected as {}", user.username));
                                }
                            }
                            Err(e) => {
                                error!("Token validation failed: {}", e);
                                state_clone.set_status(format!("Validation failed: {}", e));
                            }
                        }
                        state_clone.validating.store(false, Ordering::SeqCst);
                        repaint_ctx.request_repaint();
                    });
                }

                if self.is_saving {
                    ui.spinner();
                }
            });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);

            // Autostart Checkbox
            if ui
                .checkbox(&mut self.autostart_toggle, "Start on Windows login")
                .changed()
            {
                let enabled = self.autostart_toggle;
                let data_dir = self.state.config.data_dir.clone();
                let state_clone = self.state.clone();

                self.state.tokio_handle.spawn(async move {
                    if let Err(e) = config::set_autostart(enabled) {
                        error!("Failed to update autostart registry: {}", e);
                        state_clone.set_status(format!("Autostart registry error: {}", e));
                    } else if let Err(e) = config::save_autostart(&data_dir, enabled) {
                        error!("Failed to save autostart setting: {}", e);
                        state_clone.set_status(format!("Autostart save error: {}", e));
                    } else {
                        info!("Autostart setting updated: {}", enabled);
                        let mut auto_lock = state_clone.autostart.lock().await;
                        *auto_lock = enabled;
                        state_clone.set_status(if enabled {
                            "Autostart enabled"
                        } else {
                            "Autostart disabled"
                        });
                    }
                });
            }

            // Gateway Checkbox
            if ui
                .checkbox(
                    &mut self.gateway_toggle,
                    "Enable live gateway (higher detection risk)",
                )
                .changed()
            {
                let enabled = self.gateway_toggle;
                self.gateway_in_flight.fetch_add(1, Ordering::SeqCst);
                if self.gateway_tx.send(enabled).is_err() {
                    // The receiver is gone, so its decrement will never run.
                    // Balance the counter here to avoid leaking an in-flight slot.
                    self.gateway_in_flight.fetch_sub(1, Ordering::SeqCst);
                }
            }

            // Allow Writes Checkbox
            if ui
                .checkbox(
                    &mut self.allow_writes_toggle,
                    "Allow sending & modifying messages (ToS / ban risk)",
                )
                .changed()
            {
                let enabled = self.allow_writes_toggle;
                if !enabled {
                    self.state.allow_writes.store(false, Ordering::SeqCst);
                }
                self.write_in_flight.fetch_add(1, Ordering::SeqCst);
                if self.write_tx.send(enabled).is_err() {
                    self.write_in_flight.fetch_sub(1, Ordering::SeqCst);
                }
            }

            if self.state.write_circuit_open.load(Ordering::SeqCst) {
                ui.colored_label(
                    Color32::from_rgb(255, 170, 0),
                    "Write circuit paused (Discord CAPTCHA/challenge) — Reconnect or re-toggle to resume",
                );
            } else if self.state.allow_writes.load(Ordering::SeqCst) {
                ui.colored_label(
                    Color32::from_rgb(255, 170, 0),
                    "Writes enabled: sending or modifying messages carries account ban risk",
                );
            }

            ui.add_space(12.0);

            // Bottom action buttons: Hide and Quit
            ui.horizontal(|ui| {
                if ui.button("Hide Window").clicked() {
                    ctx.send_viewport_cmd(ViewportCommand::Visible(false));
                }

                let reconnect_enabled = !self.status_text.contains("Reconnecting");
                if ui
                    .add_enabled(reconnect_enabled, egui::Button::new("Reconnect"))
                    .clicked()
                {
                    if let Some(ref tx) = self.state.action_tx {
                        let _ = tx.send(TrayAction::Reconnect);
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Quit App").clicked() {
                        self.state.trigger_shutdown();
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                });
            });
        });
    }
}

pub fn run_ui(state: AppState) -> eframe::Result<()> {
    let token_present = state.token_present.try_lock().map(|g| *g).unwrap_or(false);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Discord Context MCP")
            .with_inner_size([420.0, 310.0])
            .with_min_inner_size([360.0, 275.0])
            .with_visible(!token_present)
            .with_resizable(true),
        ..Default::default()
    };

    eframe::run_native(
        "Discord Context MCP",
        native_options,
        Box::new(|cc| {
            *state.repaint_signal.blocking_lock() = Some(cc.egui_ctx.clone());
            cc.egui_ctx.set_visuals(egui::Visuals::dark());

            // Capture the native window handle so the tray can unhide the
            // window directly when eframe's repaint loop cannot run.
            #[cfg(windows)]
            match raw_window_handle::HasWindowHandle::window_handle(cc) {
                Ok(handle) => {
                    if let raw_window_handle::RawWindowHandle::Win32(win32) = handle.as_raw() {
                        state.set_window_hwnd(win32.hwnd.get());
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to capture native window handle: {}", e);
                }
            }

            Ok(Box::new(SettingsApp::new(state)))
        }),
    )
}
