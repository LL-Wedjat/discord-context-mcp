use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use toml_edit::{DocumentMut, Item, Table};

pub const SERVER_NAME: &str = "discord-context";

static ATOMIC_WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClientKind {
    OpenCode,
    Codex,
    ClaudeCode,
    Antigravity,
}

impl std::fmt::Display for ClientKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OpenCode => write!(f, "OpenCode"),
            Self::Codex => write!(f, "Codex"),
            Self::ClaudeCode => write!(f, "Claude Code"),
            Self::Antigravity => write!(f, "Antigravity"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationStatus {
    Registered,
    Updated,
    AlreadyConfigured,
    SkippedNotDetected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientRegistrationResult {
    pub client: ClientKind,
    pub config_path: Option<PathBuf>,
    pub status: Result<RegistrationStatus, String>,
}

pub fn target_url(port: u16) -> String {
    format!("http://127.0.0.1:{}/mcp", port)
}

pub fn atomic_write(dest: &Path, content: &str) -> std::io::Result<()> {
    let parent = dest.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Path '{}' has no parent directory", dest.display()),
        )
    })?;

    fs::create_dir_all(parent)?;

    let filename = dest
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("config");
    let count = ATOMIC_WRITE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_path = parent.join(format!(
        "{}.tmp.{}.{}",
        filename,
        std::process::id(),
        count
    ));

    let write_result: std::io::Result<()> = (|| {
        {
            let mut file = File::create(&tmp_path)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(&tmp_path, dest)?;
        Ok(())
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }

    write_result
}

pub fn register_opencode(home_dir: &Path, port: u16) -> ClientRegistrationResult {
    let client = ClientKind::OpenCode;
    let base_dir = home_dir.join(".config").join("opencode");
    let config_path = base_dir.join("opencode.json");

    if !base_dir.is_dir() {
        return ClientRegistrationResult {
            client,
            config_path: Some(config_path),
            status: Ok(RegistrationStatus::SkippedNotDetected),
        };
    }

    let url = target_url(port);

    let status = (|| -> Result<RegistrationStatus, String> {
        if !config_path.exists() {
            let root = serde_json::json!({
                "mcp": {
                    "servers": {
                        SERVER_NAME: {
                            "type": "remote",
                            "url": url
                        }
                    }
                }
            });
            let json = serde_json::to_string_pretty(&root)
                .map_err(|e| format!("Failed to serialize JSON: {}", e))?;
            atomic_write(&config_path, &format!("{}\n", json))
                .map_err(|e| format!("Failed to write {}: {}", config_path.display(), e))?;
            return Ok(RegistrationStatus::Registered);
        }

        let content = fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read {}: {}", config_path.display(), e))?;
        let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
        let mut root: serde_json::Value = serde_json::from_str(content)
            .map_err(|e| format!("Failed to parse JSON from {}: {}", config_path.display(), e))?;

        if !root.is_object() {
            return Err(format!(
                "Root of {} is not a JSON object",
                config_path.display()
            ));
        }

        let root_obj = root.as_object_mut().unwrap();

        let mcp_val = root_obj
            .entry("mcp")
            .or_insert_with(|| serde_json::json!({}));
        if !mcp_val.is_object() {
            return Err(format!(
                "'mcp' field in {} is not an object",
                config_path.display()
            ));
        }
        let mcp_obj = mcp_val.as_object_mut().unwrap();

        let servers_val = mcp_obj
            .entry("servers")
            .or_insert_with(|| serde_json::json!({}));
        if !servers_val.is_object() {
            return Err(format!(
                "'mcp.servers' field in {} is not an object",
                config_path.display()
            ));
        }
        let servers_obj = servers_val.as_object_mut().unwrap();

        let status = if let Some(server_val) = servers_obj.get_mut(SERVER_NAME) {
            if !server_val.is_object() {
                return Err(format!(
                    "'mcp.servers.{}' in {} is not an object",
                    SERVER_NAME,
                    config_path.display()
                ));
            }
            let server_obj = server_val.as_object_mut().unwrap();
            let had_command = server_obj.remove("command").is_some();
            let had_args = server_obj.remove("args").is_some();
            let current_type = server_obj.get("type").and_then(|v| v.as_str());
            let current_url = server_obj.get("url").and_then(|v| v.as_str());

            if current_type == Some("remote")
                && current_url == Some(&url)
                && !had_command
                && !had_args
            {
                return Ok(RegistrationStatus::AlreadyConfigured);
            }

            server_obj.insert(
                "type".to_string(),
                serde_json::Value::String("remote".to_string()),
            );
            server_obj.insert("url".to_string(), serde_json::Value::String(url));
            RegistrationStatus::Updated
        } else {
            servers_obj.insert(
                SERVER_NAME.to_string(),
                serde_json::json!({
                    "type": "remote",
                    "url": url
                }),
            );
            RegistrationStatus::Registered
        };

        let json = serde_json::to_string_pretty(&root)
            .map_err(|e| format!("Failed to serialize JSON: {}", e))?;
        atomic_write(&config_path, &format!("{}\n", json))
            .map_err(|e| format!("Failed to write {}: {}", config_path.display(), e))?;

        Ok(status)
    })();

    ClientRegistrationResult {
        client,
        config_path: Some(config_path),
        status,
    }
}

pub fn register_codex(home_dir: &Path, port: u16) -> ClientRegistrationResult {
    let client = ClientKind::Codex;
    let base_dir = home_dir.join(".codex");
    let config_path = base_dir.join("config.toml");

    if !base_dir.is_dir() {
        return ClientRegistrationResult {
            client,
            config_path: Some(config_path),
            status: Ok(RegistrationStatus::SkippedNotDetected),
        };
    }

    let url = target_url(port);

    let status = (|| -> Result<RegistrationStatus, String> {
        if !config_path.exists() {
            let mut doc = DocumentMut::new();
            let mut mcp_servers = Table::new();
            mcp_servers.set_implicit(true);
            let mut server_table = Table::new();
            server_table.insert("url", toml_edit::value(&url));
            mcp_servers.insert(SERVER_NAME, Item::Table(server_table));
            doc.insert("mcp_servers", Item::Table(mcp_servers));

            atomic_write(&config_path, &doc.to_string())
                .map_err(|e| format!("Failed to write {}: {}", config_path.display(), e))?;
            return Ok(RegistrationStatus::Registered);
        }

        let content = fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read {}: {}", config_path.display(), e))?;
        let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
        let mut doc: DocumentMut = content
            .parse::<DocumentMut>()
            .map_err(|e| format!("Failed to parse TOML in {}: {}", config_path.display(), e))?;

        let root_table = doc.as_table_mut();

        if let Some(mcp_servers_item) = root_table.get_mut("mcp_servers") {
            if let Some(table) = mcp_servers_item.as_table_mut() {
                if let Some(dc_item) = table.get_mut(SERVER_NAME) {
                    if let Some(dc_table) = dc_item.as_table_mut() {
                        let had_command = dc_table.remove("command").is_some();
                        let had_args = dc_table.remove("args").is_some();
                        let current_url = dc_table.get("url").and_then(|v| v.as_str());
                        if current_url == Some(&url) && !had_command && !had_args {
                            return Ok(RegistrationStatus::AlreadyConfigured);
                        }
                        dc_table.insert("url", toml_edit::value(&url));
                    } else if let Some(dc_inline) = dc_item.as_inline_table_mut() {
                        let had_command = dc_inline.remove("command").is_some();
                        let had_args = dc_inline.remove("args").is_some();
                        let current_url = dc_inline.get("url").and_then(|v| v.as_str());
                        if current_url == Some(&url) && !had_command && !had_args {
                            return Ok(RegistrationStatus::AlreadyConfigured);
                        }
                        dc_inline.insert("url", url.into());
                    } else {
                        return Err(format!(
                            "'mcp_servers.{}' is not a table in {}",
                            SERVER_NAME,
                            config_path.display()
                        ));
                    }
                    atomic_write(&config_path, &doc.to_string())
                        .map_err(|e| format!("Failed to write {}: {}", config_path.display(), e))?;
                    return Ok(RegistrationStatus::Updated);
                } else {
                    let mut server_table = Table::new();
                    server_table.insert("url", toml_edit::value(&url));
                    table.insert(SERVER_NAME, Item::Table(server_table));
                    atomic_write(&config_path, &doc.to_string())
                        .map_err(|e| format!("Failed to write {}: {}", config_path.display(), e))?;
                    return Ok(RegistrationStatus::Registered);
                }
            } else if let Some(inline_table) = mcp_servers_item.as_inline_table_mut() {
                if let Some(dc_val) = inline_table.get_mut(SERVER_NAME) {
                    if let Some(dc_inline) = dc_val.as_inline_table_mut() {
                        let had_command = dc_inline.remove("command").is_some();
                        let had_args = dc_inline.remove("args").is_some();
                        let current_url = dc_inline.get("url").and_then(|v| v.as_str());
                        if current_url == Some(&url) && !had_command && !had_args {
                            return Ok(RegistrationStatus::AlreadyConfigured);
                        }
                        dc_inline.insert("url", url.into());
                    } else {
                        return Err(format!(
                            "'mcp_servers.{}' is not a table in {}",
                            SERVER_NAME,
                            config_path.display()
                        ));
                    }
                    atomic_write(&config_path, &doc.to_string())
                        .map_err(|e| format!("Failed to write {}: {}", config_path.display(), e))?;
                    return Ok(RegistrationStatus::Updated);
                } else {
                    let mut dc_inline = toml_edit::InlineTable::new();
                    dc_inline.insert("url", url.into());
                    inline_table.insert(SERVER_NAME, toml_edit::Value::InlineTable(dc_inline));
                    atomic_write(&config_path, &doc.to_string())
                        .map_err(|e| format!("Failed to write {}: {}", config_path.display(), e))?;
                    return Ok(RegistrationStatus::Registered);
                }
            } else {
                return Err(format!(
                    "'mcp_servers' in {} is not a table",
                    config_path.display()
                ));
            }
        } else {
            let mut mcp_servers = Table::new();
            mcp_servers.set_implicit(true);
            let mut server_table = Table::new();
            server_table.insert("url", toml_edit::value(&url));
            mcp_servers.insert(SERVER_NAME, Item::Table(server_table));
            root_table.insert("mcp_servers", Item::Table(mcp_servers));
            atomic_write(&config_path, &doc.to_string())
                .map_err(|e| format!("Failed to write {}: {}", config_path.display(), e))?;
            return Ok(RegistrationStatus::Registered);
        }
    })();

    ClientRegistrationResult {
        client,
        config_path: Some(config_path),
        status,
    }
}

pub fn register_claude_code(home_dir: &Path, port: u16) -> ClientRegistrationResult {
    let client = ClientKind::ClaudeCode;
    let base_dir = home_dir.join(".claude");
    let config_path = home_dir.join(".claude.json");

    if !base_dir.is_dir() && !config_path.exists() {
        return ClientRegistrationResult {
            client,
            config_path: Some(config_path),
            status: Ok(RegistrationStatus::SkippedNotDetected),
        };
    }

    let url = target_url(port);

    let status = (|| -> Result<RegistrationStatus, String> {
        if !config_path.exists() {
            let root = serde_json::json!({
                "mcpServers": {
                    SERVER_NAME: {
                        "type": "http",
                        "url": url
                    }
                }
            });
            let json = serde_json::to_string_pretty(&root)
                .map_err(|e| format!("Failed to serialize JSON: {}", e))?;
            atomic_write(&config_path, &format!("{}\n", json))
                .map_err(|e| format!("Failed to write {}: {}", config_path.display(), e))?;
            return Ok(RegistrationStatus::Registered);
        }

        let content = fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read {}: {}", config_path.display(), e))?;
        let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
        let mut root: serde_json::Value = serde_json::from_str(content)
            .map_err(|e| format!("Failed to parse JSON from {}: {}", config_path.display(), e))?;

        if !root.is_object() {
            return Err(format!(
                "Root of {} is not a JSON object",
                config_path.display()
            ));
        }

        let root_obj = root.as_object_mut().unwrap();

        let mcp_servers_val = root_obj
            .entry("mcpServers")
            .or_insert_with(|| serde_json::json!({}));
        if !mcp_servers_val.is_object() {
            return Err(format!(
                "'mcpServers' field in {} is not an object",
                config_path.display()
            ));
        }
        let mcp_servers_obj = mcp_servers_val.as_object_mut().unwrap();

        let status = if let Some(server_val) = mcp_servers_obj.get_mut(SERVER_NAME) {
            if !server_val.is_object() {
                return Err(format!(
                    "'mcpServers.{}' in {} is not an object",
                    SERVER_NAME,
                    config_path.display()
                ));
            }
            let server_obj = server_val.as_object_mut().unwrap();
            let had_command = server_obj.remove("command").is_some();
            let had_args = server_obj.remove("args").is_some();
            let current_type = server_obj.get("type").and_then(|v| v.as_str());
            let current_url = server_obj.get("url").and_then(|v| v.as_str());

            if current_type == Some("http")
                && current_url == Some(&url)
                && !had_command
                && !had_args
            {
                return Ok(RegistrationStatus::AlreadyConfigured);
            }

            server_obj.insert(
                "type".to_string(),
                serde_json::Value::String("http".to_string()),
            );
            server_obj.insert("url".to_string(), serde_json::Value::String(url));
            RegistrationStatus::Updated
        } else {
            mcp_servers_obj.insert(
                SERVER_NAME.to_string(),
                serde_json::json!({
                    "type": "http",
                    "url": url
                }),
            );
            RegistrationStatus::Registered
        };

        let json = serde_json::to_string_pretty(&root)
            .map_err(|e| format!("Failed to serialize JSON: {}", e))?;
        atomic_write(&config_path, &format!("{}\n", json))
            .map_err(|e| format!("Failed to write {}: {}", config_path.display(), e))?;

        Ok(status)
    })();

    ClientRegistrationResult {
        client,
        config_path: Some(config_path),
        status,
    }
}

pub fn register_antigravity_at(config_dir: &Path, port: u16) -> ClientRegistrationResult {
    let client = ClientKind::Antigravity;
    let config_path = config_dir.join("mcp_config.json");

    if !config_dir.is_dir() && !config_path.exists() {
        return ClientRegistrationResult {
            client,
            config_path: Some(config_path),
            status: Ok(RegistrationStatus::SkippedNotDetected),
        };
    }

    let url = target_url(port);

    let status = (|| -> Result<RegistrationStatus, String> {
        if !config_path.exists() {
            let root = serde_json::json!({
                "mcpServers": {
                    SERVER_NAME: {
                        "url": url
                    }
                }
            });
            let json = serde_json::to_string_pretty(&root)
                .map_err(|e| format!("Failed to serialize JSON: {}", e))?;
            atomic_write(&config_path, &format!("{}\n", json))
                .map_err(|e| format!("Failed to write {}: {}", config_path.display(), e))?;
            return Ok(RegistrationStatus::Registered);
        }

        let content = fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read {}: {}", config_path.display(), e))?;
        let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
        let mut root: serde_json::Value = if content.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(content)
                .map_err(|e| format!("Failed to parse JSON from {}: {}", config_path.display(), e))?
        };

        if !root.is_object() {
            return Err(format!(
                "Root of {} is not a JSON object",
                config_path.display()
            ));
        }

        let root_obj = root.as_object_mut().unwrap();

        let mcp_servers_val = root_obj
            .entry("mcpServers")
            .or_insert_with(|| serde_json::json!({}));
        if !mcp_servers_val.is_object() {
            return Err(format!(
                "'mcpServers' field in {} is not an object",
                config_path.display()
            ));
        }
        let mcp_servers_obj = mcp_servers_val.as_object_mut().unwrap();

        let status = if let Some(server_val) = mcp_servers_obj.get_mut(SERVER_NAME) {
            if !server_val.is_object() {
                return Err(format!(
                    "'mcpServers.{}' in {} is not an object",
                    SERVER_NAME,
                    config_path.display()
                ));
            }
            let server_obj = server_val.as_object_mut().unwrap();
            let had_command = server_obj.remove("command").is_some();
            let had_args = server_obj.remove("args").is_some();
            let uses_server_url = server_obj.contains_key("serverUrl");

            if uses_server_url {
                let had_url = server_obj.remove("url").is_some();
                let current_server_url = server_obj.get("serverUrl").and_then(|v| v.as_str());
                if current_server_url == Some(&url) && !had_command && !had_args && !had_url {
                    return Ok(RegistrationStatus::AlreadyConfigured);
                }
                server_obj.insert(
                    "serverUrl".to_string(),
                    serde_json::Value::String(url),
                );
            } else {
                let current_url = server_obj.get("url").and_then(|v| v.as_str());
                if current_url == Some(&url) && !had_command && !had_args {
                    return Ok(RegistrationStatus::AlreadyConfigured);
                }
                server_obj.insert(
                    "url".to_string(),
                    serde_json::Value::String(url),
                );
            }
            RegistrationStatus::Updated
        } else {
            mcp_servers_obj.insert(
                SERVER_NAME.to_string(),
                serde_json::json!({
                    "url": url
                }),
            );
            RegistrationStatus::Registered
        };

        let json = serde_json::to_string_pretty(&root)
            .map_err(|e| format!("Failed to serialize JSON: {}", e))?;
        atomic_write(&config_path, &format!("{}\n", json))
            .map_err(|e| format!("Failed to write {}: {}", config_path.display(), e))?;

        Ok(status)
    })();

    ClientRegistrationResult {
        client,
        config_path: Some(config_path),
        status,
    }
}

pub fn register_antigravity(home_dir: &Path, port: u16) -> ClientRegistrationResult {
    register_antigravity_at(&home_dir.join(".gemini").join("config"), port)
}

pub fn register_t3_antigravity_providers(
    home_dir: &Path,
    port: u16,
) -> Vec<ClientRegistrationResult> {
    let base_dir = home_dir
        .join(".t3")
        .join("userdata")
        .join("providers")
        .join("antigravity");

    if !base_dir.is_dir() {
        return Vec::new();
    }

    let entries = match fs::read_dir(&base_dir) {
        Ok(read_dir) => {
            let mut paths: Vec<PathBuf> = read_dir
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
            paths.sort();
            paths
        }
        Err(_) => return Vec::new(),
    };

    let mut results = Vec::new();
    for entry in entries {
        let config_dir = entry.join("config");
        if config_dir.is_dir() || config_dir.join("mcp_config.json").exists() {
            results.push(register_antigravity_at(&config_dir, port));
        }
    }

    results
}

pub fn register_all_clients(home_dir: &Path, port: u16) -> Vec<ClientRegistrationResult> {
    let mut results = vec![
        register_opencode(home_dir, port),
        register_codex(home_dir, port),
        register_claude_code(home_dir, port),
        register_antigravity(home_dir, port),
    ];
    results.extend(register_t3_antigravity_providers(home_dir, port));
    results
}

pub fn log_registration_results(results: &[ClientRegistrationResult]) {
    for res in results {
        match &res.status {
            Ok(RegistrationStatus::Registered) => {
                tracing::info!(
                    client = %res.client,
                    path = ?res.config_path.as_ref().map(|p| p.display().to_string()),
                    "Registered MCP client 'discord-context'"
                );
            }
            Ok(RegistrationStatus::Updated) => {
                tracing::info!(
                    client = %res.client,
                    path = ?res.config_path.as_ref().map(|p| p.display().to_string()),
                    "Updated MCP client registration for 'discord-context'"
                );
            }
            Ok(RegistrationStatus::AlreadyConfigured) => {
                tracing::debug!(
                    client = %res.client,
                    path = ?res.config_path.as_ref().map(|p| p.display().to_string()),
                    "MCP client 'discord-context' is already configured"
                );
            }
            Ok(RegistrationStatus::SkippedNotDetected) => {
                tracing::debug!(
                    client = %res.client,
                    "MCP client not detected (skipped)"
                );
            }
            Err(err) => {
                tracing::error!(
                    client = %res.client,
                    path = ?res.config_path.as_ref().map(|p| p.display().to_string()),
                    error = %err,
                    "Failed to register MCP client 'discord-context'"
                );
            }
        }
    }
}
