use std::sync::Arc;

use anyhow::Result;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use tower_http::cors::CorsLayer;
use tracing::info;

use crate::db::{Db, MessageRow, SearchFilters};
use crate::discord::DiscordClient;
use crate::models::{DiscordChannel, DiscordGuild};

#[derive(Clone)]
pub struct AppContext {
    pub db: Arc<Db>,
    pub client: Arc<DiscordClient>,
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

pub async fn call_tool(
    ctx: &AppContext,
    name: &str,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    match name {
        "list_servers" => {
            let query = args.get("query").and_then(|v| v.as_str());

            let current_guilds = ctx
                .db
                .list_guilds(None)
                .map_err(|e| ctx.client.sanitize(&e.to_string()))?;

            if current_guilds.is_empty() && !ctx.client.token.trim().is_empty() {
                let _ = ctx.client.list_guilds().await;
            }

            let guilds = ctx
                .db
                .list_guilds(query)
                .map_err(|e| ctx.client.sanitize(&e.to_string()))?;

            let mut formatted = format_items(&guilds, "Discord Servers", &["id", "name"]);
            formatted.push_str("\n\nUse read_channel(server, channel, lastN) to read messages from a specific channel.");
            Ok(text_result(&formatted))
        }

        "list_channels" => {
            let server_arg = args
                .get("server")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required parameter 'server'".to_string())?;
            let query = args.get("query").and_then(|v| v.as_str());

            let mut guild = ctx
                .db
                .find_guild(server_arg)
                .map_err(|e| ctx.client.sanitize(&e.to_string()))?;

            if guild.is_none() && !ctx.client.token.trim().is_empty() {
                let _ = ctx.client.list_guilds().await;
                guild = ctx
                    .db
                    .find_guild(server_arg)
                    .map_err(|e| ctx.client.sanitize(&e.to_string()))?;
            }

            let guild = guild.ok_or_else(|| {
                format!(
                    "Server not found: '{}'. Use list_servers to see available servers.",
                    server_arg
                )
            })?;

            let current_channels = ctx
                .db
                .list_channels(&guild.id, None)
                .map_err(|e| ctx.client.sanitize(&e.to_string()))?;

            if current_channels.is_empty() && !ctx.client.token.trim().is_empty() {
                let _ = ctx.client.list_channels(&guild.id).await;
            }

            let channels = ctx
                .db
                .list_channels(&guild.id, query)
                .map_err(|e| ctx.client.sanitize(&e.to_string()))?;

            let title = format!("Channels in {}", guild.name);
            let formatted = format_items(&channels, &title, &["id", "name", "type", "topic"]);
            Ok(text_result(&formatted))
        }

        "list_dm_people" => {
            let query = args.get("query").and_then(|v| v.as_str());

            let current_dms = ctx
                .db
                .list_dm_people(None)
                .map_err(|e| ctx.client.sanitize(&e.to_string()))?;

            if current_dms.is_empty() && !ctx.client.token.trim().is_empty() {
                let _ = ctx.client.list_dm_channels().await;
            }

            let dms = ctx
                .db
                .list_dm_people(query)
                .map_err(|e| ctx.client.sanitize(&e.to_string()))?;

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

            let formatted = format_items(
                &mapped,
                "DM Contacts",
                &["userId", "username", "global_name", "channelId"],
            );
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

            let mut user = ctx
                .db
                .find_user(person_arg)
                .map_err(|e| ctx.client.sanitize(&e.to_string()))?;

            let mut dms = ctx
                .db
                .list_dm_people(None)
                .map_err(|e| ctx.client.sanitize(&e.to_string()))?;

            let mut matched_dm = user
                .as_ref()
                .and_then(|u| dms.iter().find(|d| d.recipient.id == u.id).cloned());

            if (user.is_none() || matched_dm.is_none()) && !ctx.client.token.trim().is_empty() {
                let _ = ctx.client.list_dm_channels().await;
                user = ctx
                    .db
                    .find_user(person_arg)
                    .map_err(|e| ctx.client.sanitize(&e.to_string()))?;
                dms = ctx
                    .db
                    .list_dm_people(None)
                    .map_err(|e| ctx.client.sanitize(&e.to_string()))?;
                matched_dm = user
                    .as_ref()
                    .and_then(|u| dms.iter().find(|d| d.recipient.id == u.id).cloned());
            }

            let user = user.ok_or_else(|| {
                format!(
                    "User not found: '{}'. Use list_dm_people to see recent contacts.",
                    person_arg
                )
            })?;

            let dm = matched_dm.ok_or_else(|| {
                format!(
                    "No DM channel found with user '{}' ({}).",
                    user.username, user.id
                )
            })?;

            if !ctx.client.token.trim().is_empty() {
                let _ = ctx.client.fetch_messages(&dm.id, last_n, None).await;
            }

            let rows = ctx
                .db
                .get_messages(&dm.id, Some(last_n), None, None)
                .map_err(|e| ctx.client.sanitize(&e.to_string()))?;

            let display = user.global_name.as_deref().unwrap_or(&user.username);

            let title = format!("DM with {}", display);
            Ok(text_result(&format_messages(&rows, &title)))
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

            let mut guild: Option<DiscordGuild> = ctx
                .db
                .find_guild(server_arg)
                .map_err(|e| ctx.client.sanitize(&e.to_string()))?;

            if guild.is_none() && !ctx.client.token.trim().is_empty() {
                let _ = ctx.client.list_guilds().await;
                guild = ctx
                    .db
                    .find_guild(server_arg)
                    .map_err(|e| ctx.client.sanitize(&e.to_string()))?;
            }

            let guild = guild.ok_or_else(|| {
                format!(
                    "Server not found: '{}'. Use list_servers to see available servers.",
                    server_arg
                )
            })?;

            let mut channel: Option<DiscordChannel> =
                ctx.db
                    .find_channel(&guild.id, channel_arg)
                    .map_err(|e| ctx.client.sanitize(&e.to_string()))?;

            if channel.is_none() && !ctx.client.token.trim().is_empty() {
                let _ = ctx.client.list_channels(&guild.id).await;
                channel = ctx
                    .db
                    .find_channel(&guild.id, channel_arg)
                    .map_err(|e| ctx.client.sanitize(&e.to_string()))?;
            }

            let channel = channel.ok_or_else(|| {
                format!(
                    "Channel not found: '{}' in server '{}'. Use list_channels to see available channels.",
                    channel_arg, guild.name
                )
            })?;

            if !ctx.client.token.trim().is_empty() {
                let _ = ctx.client.fetch_messages(&channel.id, last_n, None).await;
            }

            let rows = ctx
                .db
                .get_messages(&channel.id, Some(last_n), None, None)
                .map_err(|e| ctx.client.sanitize(&e.to_string()))?;

            let title = format!("#{} in {}", channel.name, guild.name);
            Ok(text_result(&format_messages(&rows, &title)))
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

            let guild_id = if let Some(s) = server_arg {
                ctx.db.find_guild(s).ok().flatten().map(|g| g.id)
            } else {
                None
            };

            let channel_id = if let Some(ch) = channel_arg {
                if let Some(ref g_id) = guild_id {
                    ctx.db.find_channel(g_id, ch).ok().flatten().map(|c| c.id)
                } else {
                    None
                }
            } else {
                None
            };

            let author_id = if let Some(p) = person_arg {
                ctx.db.find_user(p).ok().flatten().map(|u| u.id)
            } else {
                None
            };

            let stats = ctx
                .db
                .stats()
                .map_err(|e| ctx.client.sanitize(&e.to_string()))?;

            if stats.messages == 0 && !ctx.client.token.trim().is_empty() {
                let _ = ctx.client.sync_recent(2, 5, 10, 100).await;
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
                .map_err(|e| ctx.client.sanitize(&e.to_string()))?;

            let title = format!("Search: {}", text_arg);
            Ok(text_result(&format_messages(&rows, &title)))
        }

        "cache_stats" => {
            let stats = ctx
                .db
                .stats()
                .map_err(|e| ctx.client.sanitize(&e.to_string()))?;

            let stats_val =
                serde_json::to_value(&stats).map_err(|e| ctx.client.sanitize(&e.to_string()))?;

            Ok(json_result(stats_val))
        }

        "refresh" => {
            let scope = args.get("scope").and_then(|v| v.as_str()).unwrap_or("all");

            let per_channel = args
                .get("perChannel")
                .and_then(|v| v.as_u64())
                .unwrap_or(100)
                .clamp(1, 100) as usize;

            let summary = match scope {
                "servers" => {
                    let guilds = ctx
                        .client
                        .list_guilds()
                        .await
                        .map_err(|e| ctx.client.sanitize(&e.to_string()))?;
                    let mut channel_count = 0;
                    for g in guilds.iter().take(5) {
                        if let Ok(channels) = ctx.client.list_channels(&g.id).await {
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
                    let dms = ctx
                        .client
                        .list_dm_channels()
                        .await
                        .map_err(|e| ctx.client.sanitize(&e.to_string()))?;
                    format!("Refreshed {} DM channels", dms.len())
                }
                _ => {
                    let report = ctx
                        .client
                        .sync_recent(3, 10, 20, per_channel)
                        .await
                        .map_err(|e| ctx.client.sanitize(&e.to_string()))?;
                    format!(
                        "Synced {} channels and {} messages (errors: {})",
                        report.channels_fetched,
                        report.messages_fetched,
                        report.errors.len()
                    )
                }
            };

            let stats = ctx
                .db
                .stats()
                .map_err(|e| ctx.client.sanitize(&e.to_string()))?;

            Ok(json_result(json!({
                "summary": summary,
                "scope": scope,
                "stats": stats
            })))
        }

        _ => Err(format!("Unknown tool: {}", name)),
    }
}

pub async fn mcp_post(State(ctx): State<AppContext>, Json(req): Json<Value>) -> Json<Value> {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");

    match method {
        "initialize" => {
            let result = json!({
                "protocolVersion": "2025-06-18",
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
        }
        "notifications/initialized" => {
            // Notification: return empty JSON object
            Json(json!({}))
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
        }
        _ => Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32601,
                "message": format!("Method not found: {}", method)
            }
        })),
    }
}

pub async fn health() -> &'static str {
    "ok"
}

pub async fn run_http_server(ctx: AppContext, port: u16) -> Result<()> {
    let app = Router::new()
        .route("/mcp", post(mcp_post))
        .route("/health", get(health))
        .layer(CorsLayer::permissive())
        .with_state(ctx);

    let addr = format!("127.0.0.1:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("Discord Context MCP server listening on http://{}", addr);

    axum::serve(listener, app).await?;
    Ok(())
}
