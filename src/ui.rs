use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use eframe::egui::{self, Color32, RichText, ViewportCommand};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::app_state::{AppState, TrayAction};
use crate::config;
use crate::discord::DiscordClient;
use crate::token;

pub struct ValidatingGuard {
    validating: Arc<AtomicBool>,
    repaint_ctx: egui::Context,
}

impl Drop for ValidatingGuard {
    fn drop(&mut self) {
        self.validating.store(false, Ordering::SeqCst);
        self.repaint_ctx.request_repaint();
        debug!("ValidatingGuard dropped: validating flag cleared and repaint requested");
    }
}

pub struct ExportGuard {
    in_flight: Arc<AtomicBool>,
    repaint_ctx: egui::Context,
}

impl Drop for ExportGuard {
    fn drop(&mut self) {
        self.in_flight.store(false, Ordering::SeqCst);
        self.repaint_ctx.request_repaint();
        debug!("ExportGuard dropped: export flag cleared and repaint requested");
    }
}

pub fn export_logs_to_dir(log_dir: &Path, dest_dir: &Path) -> Result<PathBuf> {
    let read_dir = std::fs::read_dir(log_dir)
        .with_context(|| format!("Failed to read log directory: {}", log_dir.display()))?;

    let mut log_files: Vec<(String, PathBuf)> = Vec::new();
    for entry in read_dir {
        let entry = entry.with_context(|| "Failed to read directory entry")?;
        let path = entry.path();
        if path.is_file() {
            if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                // Ignore any previously exported log bundles to prevent re-bundling
                if file_name.starts_with("discord-context-mcp-logs-") {
                    continue;
                }
                // Include files with .log in the name or starting with discord-mcp
                if file_name.contains(".log") || file_name.starts_with("discord-mcp") {
                    log_files.push((file_name.to_string(), path));
                }
            }
        }
    }

    if log_files.is_empty() {
        anyhow::bail!("No log files found in {}", log_dir.display());
    }

    // Sort by filename (which sorts ISO-date daily logs chronologically)
    log_files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut combined = String::new();
    for (file_name, path) in &log_files {
        combined.push_str(&format!("=== {} ===\n", file_name));
        let bytes = std::fs::read(path)
            .with_context(|| format!("Failed to read log file {}", path.display()))?;
        let content = String::from_utf8_lossy(&bytes);
        combined.push_str(&content);
        if !content.ends_with('\n') {
            combined.push('\n');
        }
    }

    std::fs::create_dir_all(dest_dir).with_context(|| {
        format!(
            "Failed to create destination directory: {}",
            dest_dir.display()
        )
    })?;

    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let dest_filename = format!("discord-context-mcp-logs-{}.log", timestamp);
    let dest_path = dest_dir.join(dest_filename);

    std::fs::write(&dest_path, combined).with_context(|| {
        format!(
            "Failed to write exported logs to {}",
            dest_path.display()
        )
    })?;

    Ok(dest_path)
}

#[cfg(windows)]
fn reveal_in_explorer(path: &Path) {
    if let Err(e) = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn()
    {
        warn!("Failed to reveal exported logs in Explorer: {}", e);
    }
}

#[cfg(not(windows))]
fn reveal_in_explorer(_path: &Path) {}

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
    export_in_flight: Arc<AtomicBool>,
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
            export_in_flight: Arc::new(AtomicBool::new(false)),
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
                ui.heading(
                    RichText::new(format!("Discord Context MCP v{}", env!("CARGO_PKG_VERSION")))
                        .strong()
                        .size(18.0),
                );
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
                        let _guard = ValidatingGuard {
                            validating: Arc::clone(&state_clone.validating),
                            repaint_ctx: repaint_ctx.clone(),
                        };

                        debug!("Starting token validation workflow...");
                        let user = match DiscordClient::validate_token(&input_token).await {
                            Ok(user) => {
                                info!(
                                    "Token validated successfully for user {}",
                                    user.username
                                );
                                user
                            }
                            Err(e) => {
                                error!("Token validation failed: {}", e);
                                state_clone.set_status(format!("Validation failed: {}", e));
                                return;
                            }
                        };

                        debug!("Storing token in Windows Credential Manager...");
                        if let Err(e) = token::store_token(&input_token) {
                            error!("Failed to store token: {}", e);
                            state_clone.set_status(format!("Error saving token: {}", e));
                            return;
                        }
                        info!(
                            "Token stored successfully for user {}",
                            user.username
                        );

                        debug!("Creating new DiscordClient instance...");
                        let new_client = match DiscordClient::new(
                            input_token.clone(),
                            Arc::clone(&state_clone.db),
                        ) {
                            Ok(c) => Arc::new(c),
                            Err(e) => {
                                error!("Failed to create Discord client: {}", e);
                                state_clone.set_status(format!("Client error: {}", e));
                                return;
                            }
                        };

                        // Replace client in state
                        debug!("Replacing DiscordClient in state...");
                        {
                            let mut client_lock = state_clone.client.lock().await;
                            if let Some(old_client) = client_lock.as_ref() {
                                debug!("Stopping previous DiscordClient...");
                                old_client.stop();
                            }
                            *client_lock = Some(Arc::clone(&new_client));
                        }
                        debug!("DiscordClient successfully replaced in state");

                        // Replace or update AppContext
                        debug!("Updating AppContext with new client...");
                        let mcp_ctx = {
                            let ctx_lock = state_clone.ctx.lock().await;
                            ctx_lock.clone()
                        };
                        if let Some(ref mcp_ctx) = mcp_ctx {
                            mcp_ctx.set_client(Some(Arc::clone(&new_client))).await;
                        }
                        debug!("AppContext updated with new client");

                        // Update token present flag
                        debug!("Updating token_present flag...");
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
                        debug!("Ensuring account for user {} in database...", user.id);
                        let db_clone = Arc::clone(&state_clone.db);
                        let user_id = user.id.clone();
                        let ensure_res = tokio::task::spawn_blocking(move || {
                            db_clone.ensure_account(&user_id)
                        })
                        .await;

                        match ensure_res {
                            Ok(Ok(cleared)) => {
                                if cleared {
                                    info!(
                                        "Cached account changed to {}; cleared stale cache from previous account",
                                        user.id
                                    );
                                } else {
                                    debug!(
                                        "Cached account verified in database for user {}",
                                        user.id
                                    );
                                }
                            }
                            Ok(Err(e)) => {
                                error!("Failed to verify cached account in database: {}", e);
                                state_clone.set_status(format!("Cache error: {}", e));
                                return;
                            }
                            Err(e) => {
                                error!("Background task failed to verify cached account: {}", e);
                                state_clone.set_status(format!("Cache task error: {}", e));
                                return;
                            }
                        }

                        // Start the gateway in the background only when the user
                        // explicitly enabled it. REST-only is the default and must
                        // also apply to this primary token-setup flow.
                        debug!("Spawning background DiscordClient initialization...");
                        let gw_client = Arc::clone(&new_client);
                        let gw_token = input_token.clone();
                        let state_gw = state_clone.clone();
                        tokio::spawn(async move {
                            debug!("Initializing DiscordClient background cache (@me)...");
                            match gw_client.init().await {
                                Ok(me) => {
                                    info!(
                                        "DiscordClient background initialization succeeded for @{}",
                                        me.username
                                    );
                                    if state_gw.refresh_gateway_enabled() {
                                        info!("Live gateway is enabled; starting gateway connection...");
                                        std::mem::drop(gw_client.start_gateway(gw_token));
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to initialize Discord client (@me): {}", e);
                                    state_gw.set_status(format!("Discord init error: {}", e));
                                }
                            }
                        });

                        info!(
                            "Save token workflow completed successfully for user {}",
                            user.username
                        );
                        state_clone
                            .set_status(format!("Connected as {}", user.username));
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

            // Bottom action buttons: Hide, Reconnect, Export Logs, and Quit
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

                let export_in_flight = self.export_in_flight.load(Ordering::SeqCst);
                let export_enabled = !export_in_flight;
                if ui
                    .add_enabled(export_enabled, egui::Button::new("Export Logs"))
                    .clicked()
                    && self
                        .export_in_flight
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                {
                    let state_clone = self.state.clone();
                    let in_flight = Arc::clone(&self.export_in_flight);
                    let repaint_ctx = ctx.clone();
                    let log_dir = self.state.config.log_dir.clone();
                    let dest_dir = dirs::desktop_dir()
                        .or_else(dirs::download_dir)
                        .unwrap_or_else(|| self.state.config.data_dir.clone());

                    state_clone.set_status("Exporting logs...");

                    self.state.tokio_handle.spawn(async move {
                        let _guard = ExportGuard {
                            in_flight,
                            repaint_ctx,
                        };

                        let res = tokio::task::spawn_blocking(move || {
                            export_logs_to_dir(&log_dir, &dest_dir)
                        })
                        .await;

                        match res {
                            Ok(Ok(path)) => {
                                info!("Logs exported successfully to {}", path.display());
                                state_clone
                                    .set_status(format!("Logs exported to {}", path.display()));
                                #[cfg(windows)]
                                {
                                    reveal_in_explorer(&path);
                                }
                            }
                            Ok(Err(e)) => {
                                error!("Export logs failed: {}", e);
                                state_clone.set_status(format!("Export logs failed: {}", e));
                            }
                            Err(e) => {
                                error!("Export logs task panicked: {}", e);
                                state_clone.set_status(format!("Export logs failed: {}", e));
                            }
                        }
                    });
                }

                if export_in_flight {
                    ui.spinner();
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
    let app_title = format!("Discord Context MCP v{}", env!("CARGO_PKG_VERSION"));

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(app_title.clone())
            .with_inner_size([460.0, 320.0])
            .with_min_inner_size([400.0, 280.0])
            .with_visible(!token_present)
            .with_resizable(true),
        ..Default::default()
    };

    eframe::run_native(
        &app_title,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_export_logs_to_dir_combines_files_chronologically() {
        let log_dir = tempdir().expect("log tempdir");
        let dest_dir = tempdir().expect("dest tempdir");

        let f1 = log_dir.path().join("discord-mcp.log.2026-11-23");
        let f2 = log_dir.path().join("discord-mcp.log.2026-11-24");
        let f3 = log_dir.path().join("discord-mcp.log.2026-11-25");

        fs::write(&f1, "Line 1 in 2026-11-23\nLine 2 in 2026-11-23\n").unwrap();
        fs::write(&f2, "Line 1 in 2026-11-24\n").unwrap();
        fs::write(&f3, "Line 1 in 2026-11-25").unwrap(); // no trailing newline

        let exported_path =
            export_logs_to_dir(log_dir.path(), dest_dir.path()).expect("export should succeed");
        assert!(exported_path.exists());
        let file_name = exported_path.file_name().unwrap().to_str().unwrap();
        assert!(file_name.starts_with("discord-context-mcp-logs-"));
        assert!(file_name.ends_with(".log"));

        let content = fs::read_to_string(&exported_path).expect("read exported logs");
        assert!(content.contains(
            "=== discord-mcp.log.2026-11-23 ===\nLine 1 in 2026-11-23\nLine 2 in 2026-11-23\n"
        ));
        assert!(content.contains("=== discord-mcp.log.2026-11-24 ===\nLine 1 in 2026-11-24\n"));
        assert!(content.contains("=== discord-mcp.log.2026-11-25 ===\nLine 1 in 2026-11-25\n"));

        // Verify chronological ordering: 23 before 24 before 25
        let pos1 = content.find("=== discord-mcp.log.2026-11-23 ===").unwrap();
        let pos2 = content.find("=== discord-mcp.log.2026-11-24 ===").unwrap();
        let pos3 = content.find("=== discord-mcp.log.2026-11-25 ===").unwrap();
        assert!(pos1 < pos2);
        assert!(pos2 < pos3);
    }

    #[test]
    fn test_export_logs_to_dir_empty_dir_returns_error() {
        let log_dir = tempdir().expect("log tempdir");
        let dest_dir = tempdir().expect("dest tempdir");

        let res = export_logs_to_dir(log_dir.path(), dest_dir.path());
        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(err_msg.contains("No log files found"));
    }

    #[test]
    fn test_export_logs_to_dir_nonexistent_dir_returns_error() {
        let dest_dir = tempdir().expect("dest tempdir");
        let non_existent = dest_dir.path().join("does_not_exist");

        let res = export_logs_to_dir(&non_existent, dest_dir.path());
        assert!(res.is_err());
    }

    #[test]
    fn test_export_logs_to_dir_ignores_previous_bundles() {
        let log_dir = tempdir().expect("log tempdir");
        let dest_dir = tempdir().expect("dest tempdir");

        let prev_bundle = log_dir
            .path()
            .join("discord-context-mcp-logs-20261120-100000.log");
        let real_log = log_dir.path().join("discord-mcp.log.2026-11-25");

        fs::write(&prev_bundle, "OLD EXPORT DATA").unwrap();
        fs::write(&real_log, "REAL LOG DATA\n").unwrap();

        let exported_path =
            export_logs_to_dir(log_dir.path(), dest_dir.path()).expect("export should succeed");
        let content = fs::read_to_string(&exported_path).unwrap();

        assert!(!content.contains("=== discord-context-mcp-logs-20261120-100000.log ==="));
        assert!(!content.contains("OLD EXPORT DATA"));
        assert!(content.contains("=== discord-mcp.log.2026-11-25 ===\nREAL LOG DATA\n"));
    }

    #[test]
    fn test_validating_guard_clears_flag_on_drop_and_panic() {
        let validating = Arc::new(AtomicBool::new(true));
        let ctx = egui::Context::default();

        {
            let _guard = ValidatingGuard {
                validating: Arc::clone(&validating),
                repaint_ctx: ctx.clone(),
            };
            assert!(validating.load(Ordering::SeqCst));
        }
        assert!(!validating.load(Ordering::SeqCst));

        // Test panic unwind
        validating.store(true, Ordering::SeqCst);
        let validating_clone = Arc::clone(&validating);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = ValidatingGuard {
                validating: validating_clone,
                repaint_ctx: ctx,
            };
            panic!("simulated task panic");
        }));
        assert!(!validating.load(Ordering::SeqCst));
    }

    #[test]
    fn test_export_guard_clears_flag_on_drop_and_panic() {
        let in_flight = Arc::new(AtomicBool::new(true));
        let ctx = egui::Context::default();

        {
            let _guard = ExportGuard {
                in_flight: Arc::clone(&in_flight),
                repaint_ctx: ctx.clone(),
            };
            assert!(in_flight.load(Ordering::SeqCst));
        }
        assert!(!in_flight.load(Ordering::SeqCst));

        // Test panic unwind
        in_flight.store(true, Ordering::SeqCst);
        let in_flight_clone = Arc::clone(&in_flight);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = ExportGuard {
                in_flight: in_flight_clone,
                repaint_ctx: ctx,
            };
            panic!("simulated task panic");
        }));
        assert!(!in_flight.load(Ordering::SeqCst));
    }
}
