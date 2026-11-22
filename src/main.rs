#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

use discord_context_mcp::app_state::{AppState, TrayAction};
use discord_context_mcp::config::load_config;
use discord_context_mcp::db::open_db;
use discord_context_mcp::discord::DiscordClient;
use discord_context_mcp::mcp::{run_http_server, AppContext};
use discord_context_mcp::token::{self, load_token};
use discord_context_mcp::tray::run_tray;
use discord_context_mcp::ui::run_ui;

fn init_tracing(log_dir: &std::path::Path) -> Result<tracing_appender::non_blocking::WorkerGuard> {
    let file_appender = tracing_appender::rolling::daily(log_dir, "discord-mcp.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,discord_context_mcp=debug"));

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false);

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(true);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer)
        .with(stderr_layer)
        .init();

    Ok(guard)
}

fn main() -> Result<()> {
    let config = load_config().context("Failed to load configuration")?;

    let _log_guard = init_tracing(&config.log_dir).context("Failed to initialize logging")?;
    info!("Starting Discord Context MCP server");
    info!(
        "Discord gateway is {} by user setting",
        if config.gateway_enabled {
            "ENABLED"
        } else {
            "DISABLED"
        }
    );

    let db = open_db(&config.data_dir).context("Failed to open local database")?;
    let db = Arc::new(db);

    let token_opt = match load_token() {
        Ok(t) => t,
        Err(e) => {
            warn!("Failed to read token from keyring: {}", e);
            None
        }
    };
    let token_present = token_opt.is_some();

    // Create multi-threaded tokio runtime
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Failed to build tokio runtime")?;

    let tokio_handle = rt.handle().clone();

    // Create AppState
    let mut state = AppState::new(
        config.clone(),
        Arc::clone(&db),
        token_present,
        tokio_handle.clone(),
    );

    // MPSC channel for tray actions
    let (tray_tx, mut tray_rx) = mpsc::unbounded_channel::<TrayAction>();
    state.action_tx = Some(tray_tx.clone());

    // Start background services in tokio runtime if token is present
    let initial_client: Option<Arc<DiscordClient>> = if let Some(ref token) = token_opt {
        match DiscordClient::new(token.clone(), Arc::clone(&db)) {
            Ok(client) => {
                let client = Arc::new(client);
                Some(client)
            }
            Err(e) => {
                error!("Failed to create Discord client: {}", e);
                state.set_status(format!("Client error: {}", e));
                None
            }
        }
    } else {
        None
    };

    let server_port = config.port;

    // Set initial client and AppContext into AppState, without creating dummy clients with empty tokens
    let app_ctx = AppContext::new(Arc::clone(&db), initial_client.clone());

    let state_ctx = Arc::clone(&state.ctx);
    let state_client = Arc::clone(&state.client);

    tokio_handle.block_on(async {
        *state_ctx.lock().await = Some(app_ctx.clone());
        if let Some(ref c) = initial_client {
            *state_client.lock().await = Some(Arc::clone(c));
        }
    });

    let server_app_ctx = app_ctx.clone();
    tokio_handle.spawn(async move {
        info!("Running HTTP MCP server on port {}", server_port);
        if let Err(e) = run_http_server(server_app_ctx, server_port).await {
            error!("HTTP server encountered error: {}", e);
        }
    });

    // Initialize Discord client & start gateway only if token is present
    if let (true, Some(ref client)) = (token_present, &initial_client) {
        let c_init = Arc::clone(client);
        let c_token = token_opt.clone().unwrap_or_default();
        let state_clone = state.clone();

        tokio_handle.spawn(async move {
            state_clone.set_status("Connecting to Discord...");
            match c_init.init().await {
                Ok(user) => {
                    info!("Logged in as {}", user.username);
                    state_clone.set_status(format!("Connected as {}", user.username));
                    if state_clone.refresh_gateway_enabled() {
                        let _ = c_init.start_gateway(c_token);
                    }
                }
                Err(e) => {
                    error!("Discord initialization failed: {}", e);
                    state_clone.set_status(format!("Init error: {}", e));
                }
            }
        });
    }

    // Spawn Tray Action Handler in tokio
    let state_tray_handler = state.clone();
    let db_for_reconnect = Arc::clone(&db);
    tokio_handle.spawn(async move {
        while let Some(action) = tray_rx.recv().await {
            match action {
                TrayAction::OpenSettings => {
                    info!("Tray: OpenSettings requested");
                    if let Some(ctx) = state_tray_handler.repaint_signal.lock().await.as_ref() {
                        ctx.request_repaint();
                        ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Focus);
                    }
                }
                TrayAction::Reconnect => {
                    info!("Tray: Reconnect requested");
                    let current_token = match token::load_token() {
                        Ok(t) => t,
                        Err(e) => {
                            error!("Failed to read token for reconnect: {}", e);
                            None
                        }
                    };

                    if let Some(tok) = current_token {
                        state_tray_handler.set_status("Reconnecting...");
                        match DiscordClient::new(tok.clone(), Arc::clone(&db_for_reconnect)) {
                            Ok(client) => {
                                let client = Arc::new(client);

                                // Stop previous client if any
                                {
                                    let mut client_lock = state_tray_handler.client.lock().await;
                                    if let Some(old) = client_lock.as_ref() {
                                        old.stop();
                                    }
                                    *client_lock = Some(Arc::clone(&client));
                                }

                                // Update AppContext
                                {
                                    let ctx_lock = state_tray_handler.ctx.lock().await;
                                    if let Some(ref mcp_ctx) = *ctx_lock {
                                        mcp_ctx.set_client(Some(Arc::clone(&client))).await;
                                    }
                                }

                                let c_init = Arc::clone(&client);
                                let state_c = state_tray_handler.clone();
                                tokio::spawn(async move {
                                    match c_init.init().await {
                                        Ok(user) => {
                                            state_c.set_status(format!(
                                                "Connected as {}",
                                                user.username
                                            ));
                                            if state_c.refresh_gateway_enabled() {
                                                let _ = c_init.start_gateway(tok);
                                            }
                                        }
                                        Err(e) => {
                                            state_c.set_status(format!("Reconnect failed: {}", e));
                                        }
                                    }
                                });
                            }
                            Err(e) => {
                                state_tray_handler.set_status(format!("Reconnect error: {}", e));
                            }
                        }
                    } else {
                        state_tray_handler.set_status("No token configured to reconnect");
                    }
                }
                TrayAction::Quit => {
                    info!("Tray: Quit requested");
                    state_tray_handler.trigger_shutdown();
                    break;
                }
            }
        }
    });

    // Spawn tray event loop in dedicated OS thread
    let state_tray = state.clone();
    std::thread::spawn(move || {
        if let Err(e) = run_tray(state_tray, tray_tx) {
            error!("Tray error: {}", e);
        }
    });

    // Spawn graceful shutdown listener (Ctrl+C)
    let state_ctrlc = state.clone();
    tokio_handle.spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            info!("Ctrl+C received, shutting down");
            state_ctrlc.trigger_shutdown();
        }
    });

    // Run eframe on the main thread
    let ui_result = run_ui(state.clone());
    if let Err(e) = ui_result {
        error!("UI runner error: {}", e);
    }

    // Graceful shutdown after window / app close
    info!("Waiting for graceful shutdown (up to 3s)...");
    state.trigger_shutdown();

    // Stop Discord client
    tokio_handle.block_on(async {
        if let Some(client) = state.client.lock().await.as_ref() {
            client.stop();
        }
    });

    // Allow tasks to drain up to 3 seconds
    rt.shutdown_timeout(Duration::from_secs(3));
    info!("Discord Context MCP exited gracefully");

    Ok(())
}
