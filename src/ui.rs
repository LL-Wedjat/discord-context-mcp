use std::sync::atomic::Ordering;
use std::sync::Arc;

use eframe::egui::{self, Color32, RichText, ViewportCommand};
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
    pub is_saving: bool,
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

        let initial_gateway = config::load_gateway(&state.config.data_dir);
        state
            .gateway_enabled
            .store(initial_gateway, Ordering::SeqCst);

        Self {
            state,
            token_input: String::new(),
            status_text: initial_status,
            autostart_toggle: initial_autostart,
            gateway_toggle: initial_gateway,
            is_saving: false,
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

                                    // Start the gateway in the background only when the user
                                    // explicitly enabled it. REST-only is the default and must
                                    // also apply to this primary token-setup flow.
                                    let gateway_enabled = state_clone.refresh_gateway_enabled();
                                    let gw_client = Arc::clone(&new_client);
                                    let gw_token = input_token.clone();
                                    tokio::spawn(async move {
                                        let _ = gw_client.init().await;
                                        if gateway_enabled {
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
                let data_dir = self.state.config.data_dir.clone();
                let state_clone = self.state.clone();
                let repaint_ctx = ctx.clone();

                self.state.tokio_handle.spawn(async move {
                    match config::save_gateway(&data_dir, enabled) {
                        Ok(()) => {
                            info!("Gateway preference updated: {}", enabled);
                            state_clone.refresh_gateway_enabled();
                            if !enabled {
                                {
                                    let client_lock = state_clone.client.lock().await;
                                    if let Some(c) = client_lock.as_ref() {
                                        // stop() sets the shutdown flag and fires the
                                        // shutdown Notify, so the gateway wakes from
                                        // any read/backoff immediately. Only report the
                                        // gateway as disabled after stop() has run.
                                        c.stop();
                                    }
                                }
                                state_clone
                                    .set_status("Gateway preference saved — live gateway disabled");
                            } else {
                                state_clone.set_status(
                                    "Gateway preference saved — use Reconnect to start the gateway",
                                );
                            }
                        }
                        Err(e) => {
                            error!("Failed to save gateway setting: {}", e);
                            state_clone.set_status(format!("Gateway save error: {}", e));
                        }
                    }
                    repaint_ctx.request_repaint();
                });
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
            .with_inner_size([420.0, 260.0])
            .with_min_inner_size([360.0, 220.0])
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
            Ok(Box::new(SettingsApp::new(state)))
        }),
    )
}
