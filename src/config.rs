use std::env;
use std::fs::{self, File};
use std::io::Write;
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

fn update_settings(data_dir: &Path, update: impl FnOnce(&mut Settings)) -> Result<()> {
    let _guard = SETTINGS_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let settings_file = data_dir.join("settings.json");
    let mut settings: Settings = match fs::read_to_string(&settings_file) {
        Ok(content) => serde_json::from_str::<Settings>(&content)
            .with_context(|| format!("Failed to parse settings JSON from {}", settings_file.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Settings::default(),
        Err(e) => return Err(e).with_context(|| format!("Failed to read settings from {}", settings_file.display())),
    };

    update(&mut settings);

    let json = serde_json::to_string_pretty(&settings)?;

    // Write to a temporary file and rename it into place so readers never
    // observe a partially written settings.json.
    let tmp_path = data_dir.join(format!("settings.json.tmp.{}", std::process::id()));
    let write_result: std::io::Result<()> = (|| {
        {
            let mut file = File::create(&tmp_path)?;
            file.write_all(json.as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(&tmp_path, &settings_file)?;
        Ok(())
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }

    write_result
        .with_context(|| format!("Failed to save settings to {}", settings_file.display()))?;
    Ok(())
}

pub fn save_gateway(data_dir: &Path, enabled: bool) -> Result<()> {
    update_settings(data_dir, |settings| {
        settings.gateway = enabled;
    })
}

pub fn save_autostart(data_dir: &Path, enabled: bool) -> Result<()> {
    update_settings(data_dir, |settings| {
        settings.autostart = enabled;
    })
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
