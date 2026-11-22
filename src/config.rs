use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const DEFAULT_PORT: u16 = 3774;
const APP_NAME: &str = "discord-context-mcp";
const REG_RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

static SETTINGS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub autostart: bool,
    #[serde(default)]
    pub gateway: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            autostart: false,
            gateway: false,
        }
    }
}

fn read_settings(data_dir: &Path) -> Settings {
    let _guard = SETTINGS_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let settings_file = data_dir.join("settings.json");
    if let Ok(content) = fs::read_to_string(&settings_file) {
        if let Ok(settings) = serde_json::from_str::<Settings>(&content) {
            return settings;
        }
    }
    Settings::default()
}

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub port: u16,
    pub autostart: bool,
    pub gateway_enabled: bool,
}

pub fn load_config() -> Result<Config> {
    let base_config_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join(APP_NAME);

    let data_dir = base_config_dir.clone();
    let log_dir = base_config_dir.join("logs");

    fs::create_dir_all(&data_dir)
        .with_context(|| format!("Failed to create config/data dir at {}", data_dir.display()))?;
    fs::create_dir_all(&log_dir)
        .with_context(|| format!("Failed to create log dir at {}", log_dir.display()))?;

    let port: u16 = env::var("DISCORD_MCP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    let settings = read_settings(&data_dir);

    Ok(Config {
        data_dir,
        log_dir,
        port,
        autostart: settings.autostart,
        gateway_enabled: settings.gateway,
    })
}

pub fn load_autostart(data_dir: &Path) -> bool {
    read_settings(data_dir).autostart
}

pub fn load_gateway(data_dir: &Path) -> bool {
    read_settings(data_dir).gateway
}

pub fn save_gateway(data_dir: &Path, enabled: bool) -> Result<()> {
    let _guard = SETTINGS_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let settings_file = data_dir.join("settings.json");
    let mut settings: Settings = fs::read_to_string(&settings_file)
        .ok()
        .and_then(|content| serde_json::from_str::<Settings>(&content).ok())
        .unwrap_or_default();
    settings.gateway = enabled;
    let json = serde_json::to_string_pretty(&settings)?;
    fs::write(&settings_file, json)
        .with_context(|| format!("Failed to save settings to {}", settings_file.display()))?;
    Ok(())
}

pub fn save_autostart(data_dir: &Path, enabled: bool) -> Result<()> {
    let _guard = SETTINGS_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let settings_file = data_dir.join("settings.json");
    let mut settings: Settings = fs::read_to_string(&settings_file)
        .ok()
        .and_then(|content| serde_json::from_str::<Settings>(&content).ok())
        .unwrap_or_default();
    settings.autostart = enabled;
    let json = serde_json::to_string_pretty(&settings)?;
    fs::write(&settings_file, json)
        .with_context(|| format!("Failed to save settings to {}", settings_file.display()))?;
    Ok(())
}

pub fn set_autostart(enabled: bool) -> Result<()> {
    let exe_path = env::current_exe().context("Failed to get current executable path")?;
    let exe_str = exe_path.to_string_lossy().to_string();

    if enabled {
        let status = Command::new("reg")
            .args([
                "add",
                REG_RUN_KEY,
                "/v",
                APP_NAME,
                "/t",
                "REG_SZ",
                "/d",
                &format!("\"{}\"", exe_str),
                "/f",
            ])
            .status()
            .context("Failed to execute reg add command")?;

        if !status.success() {
            anyhow::bail!("reg add exited with status: {}", status);
        }
    } else {
        let status = Command::new("reg")
            .args(["delete", REG_RUN_KEY, "/v", APP_NAME, "/f"])
            .status()
            .context("Failed to execute reg delete command")?;

        // Exit status can be non-zero if key does not exist; check that or treat as ok
        if !status.success() {
            // Check if key existed
            tracing::debug!("reg delete exited with status: {}", status);
        }
    }

    if let Ok(cfg) = load_config() {
        let _ = save_autostart(&cfg.data_dir, enabled);
    }

    Ok(())
}
