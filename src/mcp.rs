use std::sync::Arc;

use anyhow::Result;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use tracing::info;

use crate::db::{Db, MessageRow, SearchFilters};
use crate::discord::DiscordClient;
use crate::models::{DiscordChannel, DiscordGuild};

#[derive(Clone)]
pub struct AppContext {
    pub db: Arc<Db>,
    pub client: Arc<tokio::sync::RwLock<Option<Arc<DiscordClient>>>>,
}

impl AppContext {
    pub fn new(db: Arc<Db>, client: Option<Arc<DiscordClient>>) -> Self {
        Self {
            db,
            client: Arc::new(tokio::sync::RwLock::new(client)),
        }
    }

    pub async fn live_client(&self) -> Option<Arc<DiscordClient>> {
        self.client.read().await.clone()
    }

    pub async fn set_client(&self, client: Option<Arc<DiscordClient>>) {
        let mut guard = self.client.write().await;
        *guard = client;
    }
}

pub fn text_result(text: &str) -> Value {
    json!({
        "content": [
            {
                "type": "text",
                "text": text
            }
        ]
    })
}

pub fn json_result(value: Value) -> Value {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    json!({
        "content": [
            {
                "type": "text",
                "text": text
            }
        ]
    })
}

pub fn error_result(msg: &str) -> Value {
    json!({
        "content": [
            {
                "type": "text",
                "text": format!("Error: {}", msg)
            }
        ],
        "isError": true
    })
}

pub fn format_messages(rows: &[MessageRow], title: &str) -> String {
    if rows.is_empty() {
        return format!("# {} (0)\n\n_No messages found._", title);
    }

    let mut out = format!("# {} ({})\n\n", title, rows.len());
    for row in rows {
        let ts = DateTime::<Utc>::from_timestamp_millis(row.created_at_ms)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| row.timestamp.clone());

        let display_name = row
            .author_global_name
            .as_deref()
            .or(row.author_username.as_deref())
            .unwrap_or("Unknown");

        let edited = if row.edited_timestamp.is_some() {
            " (edited)"
        } else {
            ""
        };

        out.push_str(&format!(
            "[{}] {} ({}): {}{}\n",
            ts, display_name, row.author_id, row.content, edited
        ));

        if let Some(ref att_json) = row.attachments_json {
            if let Ok(attachments) = serde_json::from_str::<Vec<Value>>(att_json) {
                for att in attachments {
                    let filename = att
                        .get("filename")
                        .and_then(|v| v.as_str())
                        .unwrap_or("attachment");
                    let url = att.get("url").and_then(|v| v.as_str()).unwrap_or("");
                    out.push_str(&format!("  ↳ [Attachment: {}]({})\n", filename, url));
                }
            }
        }
    }

    out.trim_end().to_string()
}

pub fn format_items<T: Serialize>(items: &[T], title: &str, fields: &[&str]) -> String {
    if items.is_empty() {
        return format!("# {} (0)\n\n_No items found._", title);
    }

    let mut out = format!("# {} ({})\n\n", title, items.len());
    for item in items {
        let val = serde_json::to_value(item).unwrap_or(Value::Null);
        let mut field_strs = Vec::new();
        for field in fields {
            if let Some(v) = val.get(*field) {
                let formatted = match v {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    Value::Null => "null".to_string(),
                    _ => v.to_string(),
                };
                field_strs.push(format!("**{}**: {}", field, formatted));
            }
        }
        out.push_str(&format!("- {}\n", field_strs.join(", ")));
    }

    out.trim_end().to_string()
}

pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "list_servers",
            "title": "List Servers",
            "description": "Lists Discord servers (guilds) the user belongs to with server ID and name.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Optional server name substring filter"
                    }
                }
            }
        }),
        json!({
            "name": "list_channels",
            "title": "List Channels",
            "description": "Lists channels in a Discord server by server name or server ID.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "server": {
                        "type": "string",
                        "description": "Server name or ID"
                    },
                    "query": {
                        "type": "string",
                        "description": "Optional channel name substring filter"
                    }
                },
                "required": ["server"]
            }
        }),
        json!({
            "name": "list_dm_people",
            "title": "List DM Contacts",
            "description": "Lists DM contacts and recent direct message channels.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Optional person name/username substring filter"
                    }
                }
            }
        }),
        json!({
            "name": "read_dm_with",
            "title": "Read DM With Person",
            "description": "Reads recent direct messages with a specific person by user ID, username, or display name.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "person": {
                        "type": "string",
                        "description": "User ID, username, or display name"
                    },
                    "lastN": {
                        "type": "integer",
                        "description": "Number of messages to fetch (1-100, default 50)",
                        "minimum": 1,
                        "maximum": 100
                    }
                },
                "required": ["person"]
            }
        }),
        json!({
            "name": "read_channel",
            "title": "Read Channel Messages",
            "description": "Reads recent messages from a channel in a server.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "server": {
                        "type": "string",
                        "description": "Server name or ID"
                    },
                    "channel": {
                        "type": "string",
                        "description": "Channel name or ID"
                    },
                    "lastN": {
                        "type": "integer",
                        "description": "Number of messages to fetch (1-100, default 50)",
                        "minimum": 1,
                        "maximum": 100
                    }
                },
                "required": ["server", "channel"]
            }
        }),
        json!({
            "name": "search_messages",
            "title": "Search Messages",
            "description": "Searches message contents using full-text search with optional server, channel, or person filters.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "Search text / keyword"
                    },
                    "server": {
                        "type": "string",
                        "description": "Optional server name or ID filter"
                    },
                    "channel": {
                        "type": "string",
                        "description": "Optional channel name or ID filter"
                    },
                    "person": {
                        "type": "string",
                        "description": "Optional user ID, username, or display name filter"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max number of messages to return (1-100, default 25)",
                        "minimum": 1,
                        "maximum": 100
                    }
                },
                "required": ["text"]
            }
        }),
        json!({
            "name": "cache_stats",
            "title": "Cache Statistics",
            "description": "Returns cached database statistics including count of users, guilds, channels, DM channels, and messages.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        }),
        json!({
            "name": "refresh",
            "title": "Refresh Cache",
            "description": "Manually triggers a sync/refresh of Discord data from the API into local cache.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "scope": {
                        "type": "string",
                        "enum": ["all", "servers", "dms"],
                        "description": "Scope to refresh: all, servers, or dms (default: all)"
                    },
                    "perChannel": {
                        "type": "integer",
                        "description": "Number of messages to fetch per channel (1-100, default 100)",
                        "minimum": 1,
                        "maximum": 100
                    }
                }
            }
        }),
    ]
}

fn sanitize_err(client: Option<&Arc<DiscordClient>>, err: &str) -> String {
    if let Some(c) = client {
        c.sanitize(err)
    } else {
        err.to_string()
    }
}

pub async fn call_tool(
    ctx: &AppContext,
    name: &str,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let client_opt = ctx.live_client().await;
    let client_ref = client_opt.as_ref();

    match name {
        "list_servers" => {
            let query = args.get("query").and_then(|v| v.as_str());

            let current_guilds = ctx
                .db
                .list_guilds(None)
                .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;

            let mut refresh_note: Option<String> = None;
            if current_guilds.is_empty() {
                if let Some(c) = client_ref {
                    if let Err(e) = c.list_guilds().await {
                        refresh_note = Some(format!(
                            "\n\n_Note: live refresh failed: {}; showing cached data._",
                            c.sanitize(&e.to_string())
                        ));
                    }
                }
            }

            let guilds = ctx
                .db
                .list_guilds(query)
                .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;

            let mut formatted = format_items(&guilds, "Discord Servers", &["id", "name"]);
            formatted.push_str("\n\nUse read_channel(server, channel, lastN) to read messages from a specific channel.");
            if let Some(note) = refresh_note {
                formatted.push_str(&note);
            }
            Ok(text_result(&formatted))
        }

        "list_channels" => {
            let server_arg = args
                .get("server")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required parameter 'server'".to_string())?;
            let query = args.get("query").and_then(|v| v.as_str());

            let mut refresh_note: Option<String> = None;
            let mut guild = ctx
                .db
                .find_guild(server_arg)
                .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;

            if guild.is_none() {
                if let Some(c) = client_ref {
                    if let Err(e) = c.list_guilds().await {
                        refresh_note = Some(format!(
                            "\n\n_Note: live refresh failed: {}; showing cached data._",
                            c.sanitize(&e.to_string())
                        ));
                    }
                    guild = ctx
                        .db
                        .find_guild(server_arg)
                        .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;
                }
            }

            let guild = guild.ok_or_else(|| {
                format!(
                    "Server not found: '{}'. Use list_servers to see available servers.{}",
                    server_arg,
                    refresh_note.as_deref().unwrap_or("")
                )
            })?;

            let current_channels = ctx
                .db
                .list_channels(&guild.id, None)
                .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;

            if current_channels.is_empty() {
                if let Some(c) = client_ref {
                    if let Err(e) = c.list_channels(&guild.id).await {
                        refresh_note = Some(format!(
                            "\n\n_Note: live refresh failed: {}; showing cached data._",
                            c.sanitize(&e.to_string())
                        ));
                    }
                }
            }

            let channels = ctx
                .db
                .list_channels(&guild.id, query)
                .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;

            let title = format!("Channels in {}", guild.name);
            let mut formatted = format_items(&channels, &title, &["id", "name", "type", "topic"]);
            if let Some(note) = refresh_note {
                formatted.push_str(&note);
            }
            Ok(text_result(&formatted))
        }

        "list_dm_people" => {
            let query = args.get("query").and_then(|v| v.as_str());

            let current_dms = ctx
                .db
                .list_dm_people(None)
                .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;

            let mut refresh_note: Option<String> = None;
            if current_dms.is_empty() {
                if let Some(c) = client_ref {
                    if let Err(e) = c.list_dm_channels().await {
                        refresh_note = Some(format!(
                            "\n\n_Note: live refresh failed: {}; showing cached data._",
                            c.sanitize(&e.to_string())
                        ));
                    }
                }
            }

            let dms = ctx
                .db
                .list_dm_people(query)
                .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;

            let mapped: Vec<Value> = dms
                .into_iter()
                .map(|dm| {
                    json!({
                        "userId": dm.recipient.id,
                        "username": dm.recipient.username,
                        "global_name": dm.recipient.global_name,
                        "channelId": dm.id
                    })
                })
                .collect();

            let mut formatted = format_items(
                &mapped,
                "DM Contacts",
                &["userId", "username", "global_name", "channelId"],
            );
            if let Some(note) = refresh_note {
                formatted.push_str(&note);
            }
            Ok(text_result(&formatted))
        }

        "read_dm_with" => {
            let person_arg = args
                .get("person")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required parameter 'person'".to_string())?;

            let last_n = args
                .get("lastN")
                .and_then(|v| v.as_u64())
                .unwrap_or(50)
                .clamp(1, 100) as usize;

            let mut refresh_note: Option<String> = None;
            let mut user = ctx
                .db
                .find_user(person_arg)
                .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;

            let mut dms = ctx
                .db
                .list_dm_people(None)
                .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;

            let mut matched_dm = user
                .as_ref()
                .and_then(|u| dms.iter().find(|d| d.recipient.id == u.id).cloned());

            if user.is_none() || matched_dm.is_none() {
                if let Some(c) = client_ref {
                    if let Err(e) = c.list_dm_channels().await {
                        refresh_note = Some(format!(
                            "\n\n_Note: live refresh failed: {}; showing cached data._",
                            c.sanitize(&e.to_string())
                        ));
                    }
                    user = ctx
                        .db
                        .find_user(person_arg)
                        .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;
                    dms = ctx
                        .db
                        .list_dm_people(None)
                        .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;
                    matched_dm = user
                        .as_ref()
                        .and_then(|u| dms.iter().find(|d| d.recipient.id == u.id).cloned());
                }
            }

            let user = user.ok_or_else(|| {
                format!(
                    "User not found: '{}'. Use list_dm_people to see recent contacts.{}",
                    person_arg,
                    refresh_note.as_deref().unwrap_or("")
                )
            })?;

            let dm = matched_dm.ok_or_else(|| {
                format!(
                    "No DM channel found with user '{}' ({}).{}",
                    user.username,
                    user.id,
                    refresh_note.as_deref().unwrap_or("")
                )
            })?;

            if let Some(c) = client_ref {
                if let Err(e) = c.fetch_messages(&dm.id, last_n, None).await {
                    refresh_note = Some(format!(
                        "\n\n_Note: live refresh failed: {}; showing cached data._",
                        c.sanitize(&e.to_string())
                    ));
                }
            }

            let rows = ctx
                .db
                .get_messages(&dm.id, Some(last_n), None, None)
                .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;

            let display = user.global_name.as_deref().unwrap_or(&user.username);
            let title = format!("DM with {}", display);
            let mut formatted = format_messages(&rows, &title);
            if let Some(note) = refresh_note {
                formatted.push_str(&note);
            }
            Ok(text_result(&formatted))
        }

        "read_channel" => {
            let server_arg = args
                .get("server")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required parameter 'server'".to_string())?;
            let channel_arg = args
                .get("channel")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required parameter 'channel'".to_string())?;

            let last_n = args
                .get("lastN")
                .and_then(|v| v.as_u64())
                .unwrap_or(50)
                .clamp(1, 100) as usize;

            let mut refresh_note: Option<String> = None;
            let mut guild: Option<DiscordGuild> = ctx
                .db
                .find_guild(server_arg)
                .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;

            if guild.is_none() {
                if let Some(c) = client_ref {
                    if let Err(e) = c.list_guilds().await {
                        refresh_note = Some(format!(
                            "\n\n_Note: live refresh failed: {}; showing cached data._",
                            c.sanitize(&e.to_string())
                        ));
                    }
                    guild = ctx
                        .db
                        .find_guild(server_arg)
                        .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;
                }
            }

            let guild = guild.ok_or_else(|| {
                format!(
                    "Server not found: '{}'. Use list_servers to see available servers.{}",
                    server_arg,
                    refresh_note.as_deref().unwrap_or("")
                )
            })?;

            let mut channel: Option<DiscordChannel> =
                ctx.db
                    .find_channel(&guild.id, channel_arg)
                    .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;

            if channel.is_none() {
                if let Some(c) = client_ref {
                    if let Err(e) = c.list_channels(&guild.id).await {
                        refresh_note = Some(format!(
                            "\n\n_Note: live refresh failed: {}; showing cached data._",
                            c.sanitize(&e.to_string())
                        ));
                    }
                    channel = ctx
                        .db
                        .find_channel(&guild.id, channel_arg)
                        .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;
                }
            }

            let channel = channel.ok_or_else(|| {
                format!(
                    "Channel not found: '{}' in server '{}'. Use list_channels to see available channels.{}",
                    channel_arg, guild.name,
                    refresh_note.as_deref().unwrap_or("")
                )
            })?;

            if let Some(c) = client_ref {
                if let Err(e) = c.fetch_messages(&channel.id, last_n, None).await {
                    refresh_note = Some(format!(
                        "\n\n_Note: live refresh failed: {}; showing cached data._",
                        c.sanitize(&e.to_string())
                    ));
                }
            }

            let rows = ctx
                .db
                .get_messages(&channel.id, Some(last_n), None, None)
                .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;

            let title = format!("#{} in {}", channel.name, guild.name);
            let mut formatted = format_messages(&rows, &title);
            if let Some(note) = refresh_note {
                formatted.push_str(&note);
            }
            Ok(text_result(&formatted))
        }

        "search_messages" => {
            let text_arg = args
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required parameter 'text'".to_string())?;

            let server_arg = args.get("server").and_then(|v| v.as_str());
            let channel_arg = args.get("channel").and_then(|v| v.as_str());
            let person_arg = args.get("person").and_then(|v| v.as_str());
            let limit_arg = args
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|v| v.clamp(1, 100) as usize);

            let stats = ctx
                .db
                .stats()
                .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;

            let mut refresh_note: Option<String> = None;
            if stats.messages == 0 {
                if let Some(c) = client_ref {
                    if let Err(e) = c.sync_recent(2, 5, 10, 100).await {
                        refresh_note = Some(format!(
                            "\n\n_Note: live refresh failed: {}; showing cached data._",
                            c.sanitize(&e.to_string())
                        ));
                    }
                }
            }

            let mut guild_id = None;
            if let Some(s) = server_arg {
                let mut found = ctx
                    .db
                    .find_guild(s)
                    .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;
                if found.is_none() {
                    if let Some(c) = client_ref {
                        if let Err(e) = c.list_guilds().await {
                            refresh_note = Some(format!(
                                "\n\n_Note: live refresh failed: {}; showing cached data._",
                                c.sanitize(&e.to_string())
                            ));
                        }
                        found = ctx
                            .db
                            .find_guild(s)
                            .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;
                    }
                }
                match found {
                    Some(g) => guild_id = Some(g.id),
                    None => {
                        return Err(format!(
                            "Server not found: '{}'. Use list_servers to see available servers.{}",
                            s,
                            refresh_note.as_deref().unwrap_or("")
                        ))
                    }
                }
            }

            let mut channel_id = None;
            if let Some(ch) = channel_arg {
                if let Some(ref g_id) = guild_id {
                    let mut found = ctx
                        .db
                        .find_channel(g_id, ch)
                        .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;
                    if found.is_none() {
                        if let Some(c) = client_ref {
                            if let Err(e) = c.list_channels(g_id).await {
                                refresh_note = Some(format!(
                                    "\n\n_Note: live refresh failed: {}; showing cached data._",
                                    c.sanitize(&e.to_string())
                                ));
                            }
                            found = ctx
                                .db
                                .find_channel(g_id, ch)
                                .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;
                        }
                    }
                    match found {
                        Some(c) => channel_id = Some(c.id),
                        None => {
                            return Err(format!(
                                "Channel not found: '{}' in server. Use list_channels to see available channels.{}",
                                ch,
                                refresh_note.as_deref().unwrap_or("")
                            ))
                        }
                    }
                } else {
                    return Err(
                        "Specifying 'channel' filter requires specifying 'server' as well."
                            .to_string(),
                    );
                }
            }

            let mut author_id = None;
            if let Some(p) = person_arg {
                let mut found = ctx
                    .db
                    .find_user(p)
                    .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;
                if found.is_none() {
                    if let Some(c) = client_ref {
                        if let Err(e) = c.list_dm_channels().await {
                            refresh_note = Some(format!(
                                "\n\n_Note: live refresh failed: {}; showing cached data._",
                                c.sanitize(&e.to_string())
                            ));
                        }
                        found = ctx
                            .db
                            .find_user(p)
                            .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;
                    }
                }
                match found {
                    Some(u) => author_id = Some(u.id),
                    None => {
                        return Err(format!(
                            "User not found: '{}'. Use list_dm_people to see contacts.{}",
                            p,
                            refresh_note.as_deref().unwrap_or("")
                        ))
                    }
                }
            }

            let filters = SearchFilters {
                channel_id,
                guild_id,
                author_id,
                limit: limit_arg,
            };

            let rows = ctx
                .db
                .search_messages(text_arg, &filters)
                .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;

            let title = format!("Search: {}", text_arg);
            let mut formatted = format_messages(&rows, &title);
            if let Some(note) = refresh_note {
                formatted.push_str(&note);
            }
            Ok(text_result(&formatted))
        }

        "cache_stats" => {
            let stats = ctx
                .db
                .stats()
                .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;

            let stats_val = serde_json::to_value(&stats)
                .map_err(|e| sanitize_err(client_ref, &e.to_string()))?;

            Ok(json_result(stats_val))
        }

        "refresh" => {
            let c = client_opt
                .ok_or_else(|| "No Discord token configured — open Settings".to_string())?;

            let scope = args.get("scope").and_then(|v| v.as_str()).unwrap_or("all");

            let per_channel = args
                .get("perChannel")
                .and_then(|v| v.as_u64())
                .unwrap_or(100)
                .clamp(1, 100) as usize;

            let summary = match scope {
                "servers" => {
                    let guilds = c
                        .list_guilds()
                        .await
                        .map_err(|e| c.sanitize(&e.to_string()))?;
                    let mut channel_count = 0;
                    for g in guilds.iter().take(5) {
                        if let Ok(channels) = c.list_channels(&g.id).await {
                            channel_count += channels.len();
                        }
                    }
                    format!(
                        "Refreshed {} servers and {} channels",
                        guilds.len(),
                        channel_count
                    )
                }
                "dms" => {
                    let dms = c
                        .list_dm_channels()
                        .await
                        .map_err(|e| c.sanitize(&e.to_string()))?;
                    format!("Refreshed {} DM channels", dms.len())
                }
                _ => {
                    let report = c
                        .sync_recent(3, 10, 20, per_channel)
                        .await
                        .map_err(|e| c.sanitize(&e.to_string()))?;
                    format!(
                        "Synced {} channels and {} messages (errors: {})",
                        report.channels_fetched,
                        report.messages_fetched,
                        report.errors.len()
                    )
                }
            };

            let stats = ctx.db.stats().map_err(|e| c.sanitize(&e.to_string()))?;

            Ok(json_result(json!({
                "summary": summary,
                "scope": scope,
                "stats": stats
            })))
        }

        _ => Err(format!("Unknown tool: {}", name)),
    }
}

pub async fn mcp_post(
    State(ctx): State<AppContext>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let req: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return Json(json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": {
                    "code": -32700,
                    "message": "Parse error"
                }
            }))
            .into_response();
        }
    };

    let id = req.get("id").cloned();

    // JSON-RPC 2.0 conformance: validate jsonrpc field
    if req.get("jsonrpc").and_then(|v| v.as_str()) != Some("2.0") {
        return Json(json!({
            "jsonrpc": "2.0",
            "id": id.unwrap_or(Value::Null),
            "error": {
                "code": -32600,
                "message": "Invalid Request"
            }
        }))
        .into_response();
    }

    // JSON-RPC 2.0 conformance: validate method is a non-empty string
    let method = match req.get("method").and_then(|v| v.as_str()) {
        Some(m) if !m.is_empty() => m,
        _ => {
            return Json(json!({
                "jsonrpc": "2.0",
                "id": id.unwrap_or(Value::Null),
                "error": {
                    "code": -32600,
                    "message": "Invalid Request"
                }
            }))
            .into_response();
        }
    };

    // Under JSON-RPC 2.0, a notification is any request WITHOUT an 'id' member (or id is null)
    let is_notification = req.get("id").is_none() || req.get("id") == Some(&Value::Null);
    if is_notification {
        return StatusCode::ACCEPTED.into_response();
    }

    match method {
        "initialize" => {
            let requested_version = req
                .get("params")
                .and_then(|p| p.get("protocolVersion"))
                .and_then(|v| v.as_str());

            let protocol_version = match requested_version {
                Some("2025-06-18") | Some("2025-03-26") => requested_version.unwrap(),
                _ => "2025-06-18",
            };

            let result = json!({
                "protocolVersion": protocol_version,
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "discord-context",
                    "version": "1.0.0"
                }
            });
            Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result
            }))
            .into_response()
        }
        "tools/list" => {
            let result = json!({
                "tools": tool_definitions()
            });
            Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result
            }))
            .into_response()
        }
        "tools/call" => {
            let params = req.get("params").cloned().unwrap_or(Value::Null);
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

            let res = call_tool(&ctx, name, &arguments).await;
            let result = match res {
                Ok(val) => val,
                Err(err) => error_result(&err),
            };

            Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result
            }))
            .into_response()
        }
        _ => Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32601,
                "message": format!("Method not found: {}", method)
            }
        }))
        .into_response(),
    }
}

pub async fn health() -> &'static str {
    "ok"
}

pub fn is_allowed_host_or_origin(val: &str) -> bool {
    let mut s = val.trim();
    if let Some(idx) = s.find("://") {
        s = &s[idx + 3..];
    }
    // Strip path, query, and fragment if present
    if let Some(idx) = s.find(|c| c == '/' || c == '?' || c == '#') {
        s = &s[..idx];
    }
    // Strip userinfo if present
    if let Some(idx) = s.rfind('@') {
        s = &s[idx + 1..];
    }

    let host = if s.starts_with('[') {
        let close_bracket = match s.find(']') {
            Some(idx) => idx,
            None => return false,
        };
        let host_inner = &s[1..close_bracket];
        let rest = &s[close_bracket + 1..];
        if !rest.is_empty() {
            if !rest.starts_with(':')
                || rest[1..].is_empty()
                || !rest[1..].chars().all(|c| c.is_ascii_digit())
            {
                return false;
            }
        }
        host_inner
    } else if s == "::1" {
        "::1"
    } else {
        match s.split_once(':') {
            Some((h, port)) => {
                if port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
                    return false;
                }
                h
            }
            None => s,
        }
    };

    let host = host.trim_end_matches('.');
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
}

pub async fn validate_local_headers(
    headers: HeaderMap,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, StatusCode> {
    if let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) {
        if !is_allowed_host_or_origin(origin) {
            return Err(StatusCode::FORBIDDEN);
        }
    }

    if let Some(host) = headers.get("host").and_then(|v| v.to_str().ok()) {
        if !is_allowed_host_or_origin(host) {
            return Err(StatusCode::FORBIDDEN);
        }
    }

    Ok(next.run(request).await)
}

pub async fn run_http_server(ctx: AppContext, port: u16) -> Result<()> {
    let app = Router::new()
        .route("/mcp", post(mcp_post))
        .route("/health", get(health))
        .layer(axum::middleware::from_fn(validate_local_headers))
        .with_state(ctx);

    let addr = format!("127.0.0.1:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("Discord Context MCP server listening on http://{}", addr);

    axum::serve(listener, app).await?;
    Ok(())
}
