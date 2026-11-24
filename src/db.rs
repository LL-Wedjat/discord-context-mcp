use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use chrono::DateTime;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::models::{
    DiscordChannel, DiscordGuild, DiscordMessage, DiscordUser, DmChannel, PartialDiscordMessage,
};

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct MessageRow {
    pub id: String,
    pub channel_id: String,
    pub author_id: String,
    pub content: String,
    pub timestamp: String,
    pub edited_timestamp: Option<String>,
    pub attachments_json: Option<String>,
    pub embeds_json: Option<String>,
    pub reply_to: Option<String>,
    pub created_at_ms: i64,
    pub author_username: Option<String>,
    pub author_global_name: Option<String>,
    pub guild_id: Option<String>,
    pub channel_name: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SearchFilters {
    pub channel_id: Option<String>,
    pub guild_id: Option<String>,
    pub author_id: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct DbStats {
    pub users: i64,
    pub guilds: i64,
    pub channels: i64,
    pub dm_channels: i64,
    pub messages: i64,
    pub fts_mode: String,
}

pub struct Db {
    conn: Mutex<Connection>,
}

fn current_timestamp_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn parse_iso_timestamp_ms(iso: &str) -> i64 {
    DateTime::parse_from_rfc3339(iso)
        .map(|dt| dt.timestamp_millis())
        .unwrap_or_else(|_| current_timestamp_ms())
}

pub fn open_db(data_dir: &Path) -> Result<Db> {
    fs::create_dir_all(data_dir)
        .with_context(|| format!("Failed to create data dir at {}", data_dir.display()))?;
    let db_path = data_dir.join("cache.db");
    let conn = Connection::open(&db_path)
        .with_context(|| format!("Failed to open sqlite database at {}", db_path.display()))?;

    let db = Db {
        conn: Mutex::new(conn),
    };
    db.init_schema()?;
    Ok(db)
}

impl Db {
    pub fn new_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Db {
            conn: Mutex::new(conn),
        };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        let _ = conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()));
        let _ = conn.busy_timeout(std::time::Duration::from_millis(5000));

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS users (
                 id TEXT PRIMARY KEY,
                 username TEXT,
                 global_name TEXT,
                 discriminator TEXT,
                 avatar TEXT,
                 updated_at INTEGER
             );

             CREATE TABLE IF NOT EXISTS guilds (
                 id TEXT PRIMARY KEY,
                 name TEXT,
                 icon TEXT,
                 updated_at INTEGER
             );

             CREATE TABLE IF NOT EXISTS channels (
                 id TEXT PRIMARY KEY,
                 guild_id TEXT,
                 name TEXT,
                 type INTEGER,
                 topic TEXT,
                 parent_id TEXT,
                 updated_at INTEGER
             );

             CREATE TABLE IF NOT EXISTS dm_channels (
                 id TEXT PRIMARY KEY,
                 user_id TEXT,
                 name TEXT,
                 kind INTEGER NOT NULL DEFAULT 1,
                 updated_at INTEGER
             );

             CREATE TABLE IF NOT EXISTS messages (
                 id TEXT PRIMARY KEY,
                 channel_id TEXT,
                 author_id TEXT,
                 content TEXT,
                 timestamp TEXT,
                 edited_timestamp TEXT,
                 attachments_json TEXT,
                 embeds_json TEXT,
                 reply_to TEXT,
                 created_at_ms INTEGER,
                 synced_at INTEGER
             );

             CREATE INDEX IF NOT EXISTS idx_messages_channel_created
                 ON messages(channel_id, created_at_ms);

             CREATE TABLE IF NOT EXISTS meta (
                 key TEXT PRIMARY KEY,
                 value TEXT
             );",
        )?;

        // Legacy migration: check if dm_channels has 'kind' column
        let mut has_kind = false;
        let mut stmt = conn.prepare("PRAGMA table_info(dm_channels)")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
        for col in rows {
            if col? == "kind" {
                has_kind = true;
                break;
            }
        }
        if !has_kind {
            conn.execute(
                "ALTER TABLE dm_channels ADD COLUMN kind INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }

        // Setup FTS5 or fallback to LIKE
        let fts_setup = conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                 content,
                 id UNINDEXED
             );

             CREATE TRIGGER IF NOT EXISTS trg_messages_ai AFTER INSERT ON messages BEGIN
                 INSERT INTO messages_fts(id, content) VALUES (new.id, new.content);
             END;

             CREATE TRIGGER IF NOT EXISTS trg_messages_ad AFTER DELETE ON messages BEGIN
                 DELETE FROM messages_fts WHERE id = old.id;
             END;

             CREATE TRIGGER IF NOT EXISTS trg_messages_au AFTER UPDATE ON messages BEGIN
                 DELETE FROM messages_fts WHERE id = old.id;
                 INSERT INTO messages_fts(id, content) VALUES (new.id, new.content);
             END;",
        );

        let fts_mode = if fts_setup.is_ok() { "fts5" } else { "like" };
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('fts_mode', ?1)",
            params![fts_mode],
        )?;

        Ok(())
    }

    pub fn upsert_user(&self, user: &DiscordUser) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = current_timestamp_ms();
        conn.execute(
            "INSERT INTO users (id, username, global_name, discriminator, avatar, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                 username = excluded.username,
                 global_name = excluded.global_name,
                 discriminator = excluded.discriminator,
                 avatar = excluded.avatar,
                 updated_at = excluded.updated_at",
            params![
                user.id,
                user.username,
                user.global_name,
                user.discriminator,
                user.avatar,
                now
            ],
        )?;
        Ok(())
    }

    pub fn upsert_guild(&self, guild: &DiscordGuild) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = current_timestamp_ms();
        conn.execute(
            "INSERT INTO guilds (id, name, icon, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                 name = COALESCE(NULLIF(excluded.name, ''), guilds.name, excluded.name),
                 icon = COALESCE(excluded.icon, guilds.icon),
                 updated_at = excluded.updated_at",
            params![guild.id, guild.name, guild.icon, now],
        )?;
        Ok(())
    }

    pub fn upsert_channel(&self, channel: &DiscordChannel) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = current_timestamp_ms();
        conn.execute(
            "INSERT INTO channels (id, guild_id, name, type, topic, parent_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                 guild_id = excluded.guild_id,
                 name = excluded.name,
                 type = excluded.type,
                 topic = excluded.topic,
                 parent_id = excluded.parent_id,
                 updated_at = excluded.updated_at",
            params![
                channel.id,
                channel.guild_id,
                channel.name,
                channel.kind,
                channel.topic,
                channel.parent_id,
                now
            ],
        )?;
        Ok(())
    }

    pub fn upsert_dm_channel(&self, dm: &DmChannel) -> Result<()> {
        self.upsert_user(&dm.recipient)?;
        let conn = self.conn.lock().unwrap();
        let now = current_timestamp_ms();
        let display_name = dm
            .recipient
            .global_name
            .clone()
            .unwrap_or_else(|| dm.recipient.username.clone());

        conn.execute(
            "INSERT INTO dm_channels (id, user_id, name, kind, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                 user_id = excluded.user_id,
                 name = excluded.name,
                 kind = excluded.kind,
                 updated_at = excluded.updated_at",
            params![dm.id, dm.recipient.id, display_name, dm.kind, now],
        )?;
        Ok(())
    }

    pub fn delete_message(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM messages WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn update_partial_message(&self, partial: &PartialDiscordMessage) -> Result<()> {
        let now = current_timestamp_ms();

        if let Some(ref author) = partial.author {
            self.upsert_user(author)?;
            return self.update_partial_message_fields(partial, now);
        }

        self.update_partial_message_fields(partial, now)
    }

    fn update_partial_message_fields(
        &self,
        partial: &PartialDiscordMessage,
        now: i64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let mut sets = vec!["synced_at = ?".to_string()];
        let mut query_params: Vec<rusqlite::types::Value> = vec![now.into()];

        if let Some(ref content) = partial.content {
            sets.push("content = ?".to_string());
            query_params.push(content.clone().into());
        }
        if let Some(ref channel_id) = partial.channel_id {
            sets.push("channel_id = ?".to_string());
            query_params.push(channel_id.clone().into());
        }
        if let Some(ref author) = partial.author {
            sets.push("author_id = ?".to_string());
            query_params.push(author.id.clone().into());
        }
        if let Some(ref ts) = partial.timestamp {
            sets.push("timestamp = ?".to_string());
            query_params.push(ts.clone().into());
        }
        if let Some(ref edited) = partial.edited_timestamp {
            sets.push("edited_timestamp = ?".to_string());
            query_params.push(edited.clone().into());
        }
        if let Some(ref atts) = partial.attachments {
            sets.push("attachments_json = ?".to_string());
            if atts.is_empty() {
                query_params.push(rusqlite::types::Value::Null);
            } else {
                query_params.push(serde_json::to_string(atts)?.into());
            }
        }
        let reply_to = partial
            .referenced_message
            .as_ref()
            .map(|rm| rm.id.clone())
            .or_else(|| {
                partial
                    .message_reference
                    .as_ref()
                    .and_then(|mr| mr.message_id.clone())
            });
        if let Some(ref rid) = reply_to {
            sets.push("reply_to = ?".to_string());
            query_params.push(rid.clone().into());
        }

        query_params.push(partial.id.clone().into());
        let sql = format!("UPDATE messages SET {} WHERE id = ?", sets.join(", "));
        conn.execute(&sql, rusqlite::params_from_iter(query_params))?;
        Ok(())
    }

    pub fn upsert_message(&self, message: &DiscordMessage) -> Result<()> {
        self.upsert_user(&message.author)?;
        let conn = self.conn.lock().unwrap();
        let now = current_timestamp_ms();
        let created_at_ms = parse_iso_timestamp_ms(&message.timestamp);

        let attachments_json = if message.attachments.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&message.attachments)?)
        };

        let reply_to = message
            .referenced_message
            .as_ref()
            .map(|rm| rm.id.clone())
            .or_else(|| {
                message
                    .message_reference
                    .as_ref()
                    .and_then(|mr| mr.message_id.clone())
            });

        conn.execute(
            "INSERT INTO messages (
                 id, channel_id, author_id, content, timestamp, edited_timestamp,
                 attachments_json, embeds_json, reply_to, created_at_ms, synced_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET
                 channel_id = excluded.channel_id,
                 author_id = excluded.author_id,
                 content = excluded.content,
                 timestamp = COALESCE(NULLIF(excluded.timestamp, ''), messages.timestamp),
                 edited_timestamp = excluded.edited_timestamp,
                 attachments_json = excluded.attachments_json,
                 reply_to = excluded.reply_to,
                 created_at_ms = COALESCE(NULLIF(messages.created_at_ms, 0), excluded.created_at_ms),
                 synced_at = excluded.synced_at",
            params![
                message.id,
                message.channel_id,
                message.author.id,
                message.content,
                message.timestamp,
                message.edited_timestamp,
                attachments_json,
                Option::<String>::None,
                reply_to,
                created_at_ms,
                now
            ],
        )?;
        Ok(())
    }

    pub fn prune_guilds_not_in(&self, ids: &[String]) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        if ids.is_empty() {
            let removed_channels =
                conn.execute("DELETE FROM channels WHERE guild_id IS NOT NULL", [])?;
            let removed_guilds = conn.execute("DELETE FROM guilds", [])?;
            return Ok(removed_channels + removed_guilds);
        }

        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let del_channels = format!(
            "DELETE FROM channels WHERE guild_id IS NOT NULL AND guild_id NOT IN ({})",
            placeholders
        );
        let del_guilds = format!("DELETE FROM guilds WHERE id NOT IN ({})", placeholders);

        let params = rusqlite::params_from_iter(ids);
        let ch_count = conn.execute(&del_channels, params)?;
        let params_guilds = rusqlite::params_from_iter(ids);
        let g_count = conn.execute(&del_guilds, params_guilds)?;

        Ok(ch_count + g_count)
    }

    pub fn get_messages(
        &self,
        channel_id: &str,
        limit: Option<usize>,
        before: Option<&str>,
        after: Option<&str>,
    ) -> Result<Vec<MessageRow>> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.unwrap_or(50).clamp(1, 100);

        let mut conditions = vec!["m.channel_id = ?1".to_string()];
        let mut query_params: Vec<rusqlite::types::Value> = vec![channel_id.to_string().into()];

        if let Some(b_id) = before {
            let found: Option<(i64, String)> = conn
                .query_row(
                    "SELECT created_at_ms, id FROM messages WHERE id = ?1",
                    params![b_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            match found {
                Some((ms, id)) => {
                    conditions.push("(m.created_at_ms < ? OR (m.created_at_ms = ? AND (LENGTH(m.id) < LENGTH(?) OR (LENGTH(m.id) = LENGTH(?) AND m.id < ?))))".to_string());
                    query_params.push(ms.into());
                    query_params.push(ms.into());
                    query_params.push(id.clone().into());
                    query_params.push(id.clone().into());
                    query_params.push(id.into());
                }
                None => {
                    let valid_snowflake = b_id
                        .parse::<u64>()
                        .ok()
                        .map(|v| v >= 4_194_304)
                        .unwrap_or(false);
                    if valid_snowflake {
                        conditions.push("(LENGTH(m.id) < LENGTH(?) OR (LENGTH(m.id) = LENGTH(?) AND m.id < ?))".to_string());
                        query_params.push(b_id.to_string().into());
                        query_params.push(b_id.to_string().into());
                        query_params.push(b_id.to_string().into());
                    } else {
                        return Err(anyhow::anyhow!(
                            "Anchor message '{}' (before) not found in cache",
                            b_id
                        ));
                    }
                }
            }
        }

        if let Some(a_id) = after {
            let found: Option<(i64, String)> = conn
                .query_row(
                    "SELECT created_at_ms, id FROM messages WHERE id = ?1",
                    params![a_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            match found {
                Some((ms, id)) => {
                    conditions.push("(m.created_at_ms > ? OR (m.created_at_ms = ? AND (LENGTH(m.id) > LENGTH(?) OR (LENGTH(m.id) = LENGTH(?) AND m.id > ?))))".to_string());
                    query_params.push(ms.into());
                    query_params.push(ms.into());
                    query_params.push(id.clone().into());
                    query_params.push(id.clone().into());
                    query_params.push(id.into());
                }
                None => {
                    let valid_snowflake = a_id
                        .parse::<u64>()
                        .ok()
                        .map(|v| v >= 4_194_304)
                        .unwrap_or(false);
                    if valid_snowflake {
                        conditions.push("(LENGTH(m.id) > LENGTH(?) OR (LENGTH(m.id) = LENGTH(?) AND m.id > ?))".to_string());
                        query_params.push(a_id.to_string().into());
                        query_params.push(a_id.to_string().into());
                        query_params.push(a_id.to_string().into());
                    } else {
                        return Err(anyhow::anyhow!(
                            "Anchor message '{}' (after) not found in cache",
                            a_id
                        ));
                    }
                }
            }
        }

        let where_clause = conditions.join(" AND ");
        // Query newest descending first up to limit, then reverse so returned rows are oldest -> newest
        let sql = format!(
            "SELECT m.id, m.channel_id, m.author_id, m.content, m.timestamp, m.edited_timestamp,
                    m.attachments_json, m.embeds_json, m.reply_to, m.created_at_ms,
                    u.username, u.global_name, c.guild_id, c.name
             FROM messages m
             LEFT JOIN users u ON m.author_id = u.id
             LEFT JOIN channels c ON m.channel_id = c.id
             WHERE {}
             ORDER BY m.created_at_ms DESC, LENGTH(m.id) DESC, m.id DESC
             LIMIT {}",
            where_clause, limit
        );

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(query_params), |r| {
            Ok(MessageRow {
                id: r.get(0)?,
                channel_id: r.get(1)?,
                author_id: r.get(2)?,
                content: r.get(3)?,
                timestamp: r.get(4)?,
                edited_timestamp: r.get(5)?,
                attachments_json: r.get(6)?,
                embeds_json: r.get(7)?,
                reply_to: r.get(8)?,
                created_at_ms: r.get(9)?,
                author_username: r.get(10)?,
                author_global_name: r.get(11)?,
                guild_id: r.get(12)?,
                channel_name: r.get(13)?,
            })
        })?;

        let mut messages = Vec::new();
        for msg in rows {
            messages.push(msg?);
        }
        // Oldest -> newest
        messages.reverse();
        Ok(messages)
    }

    pub fn search_messages(&self, query: &str, filters: &SearchFilters) -> Result<Vec<MessageRow>> {
        let conn = self.conn.lock().unwrap();
        let fts_mode: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'fts_mode'", [], |r| {
                r.get(0)
            })
            .unwrap_or_else(|_| "fts5".to_string());

        let limit = filters.limit.unwrap_or(25).clamp(1, 100);

        if fts_mode == "fts5" {
            let escaped_query = escape_fts5_query(query);
            if !escaped_query.is_empty() {
                let mut sql = String::from(
                    "SELECT m.id, m.channel_id, m.author_id, m.content, m.timestamp, m.edited_timestamp,
                            m.attachments_json, m.embeds_json, m.reply_to, m.created_at_ms,
                            u.username, u.global_name, c.guild_id, c.name
                     FROM messages_fts fts
                     JOIN messages m ON fts.id = m.id
                     LEFT JOIN users u ON m.author_id = u.id
                     LEFT JOIN channels c ON m.channel_id = c.id
                     WHERE messages_fts MATCH ?1",
                );

                let mut params: Vec<rusqlite::types::Value> = vec![escaped_query.into()];

                if let Some(ref ch_id) = filters.channel_id {
                    sql.push_str(" AND m.channel_id = ?");
                    params.push(ch_id.clone().into());
                }
                if let Some(ref g_id) = filters.guild_id {
                    sql.push_str(" AND c.guild_id = ?");
                    params.push(g_id.clone().into());
                }
                if let Some(ref auth_id) = filters.author_id {
                    sql.push_str(" AND m.author_id = ?");
                    params.push(auth_id.clone().into());
                }

                sql.push_str(&format!(" ORDER BY m.created_at_ms DESC LIMIT {}", limit));

                let run_query = || -> Result<Vec<MessageRow>> {
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map(rusqlite::params_from_iter(params.clone()), |r| {
                        Ok(MessageRow {
                            id: r.get(0)?,
                            channel_id: r.get(1)?,
                            author_id: r.get(2)?,
                            content: r.get(3)?,
                            timestamp: r.get(4)?,
                            edited_timestamp: r.get(5)?,
                            attachments_json: r.get(6)?,
                            embeds_json: r.get(7)?,
                            reply_to: r.get(8)?,
                            created_at_ms: r.get(9)?,
                            author_username: r.get(10)?,
                            author_global_name: r.get(11)?,
                            guild_id: r.get(12)?,
                            channel_name: r.get(13)?,
                        })
                    })?;
                    let mut result = Vec::new();
                    for r in rows {
                        result.push(r?);
                    }
                    Ok(result)
                };

                if let Ok(res) = run_query() {
                    return Ok(res);
                }
            }
        }

        // Fallback to LIKE
        let like_pat = format!(
            "%{}%",
            query
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let mut sql = String::from(
            "SELECT m.id, m.channel_id, m.author_id, m.content, m.timestamp, m.edited_timestamp,
                    m.attachments_json, m.embeds_json, m.reply_to, m.created_at_ms,
                    u.username, u.global_name, c.guild_id, c.name
             FROM messages m
             LEFT JOIN users u ON m.author_id = u.id
             LEFT JOIN channels c ON m.channel_id = c.id
             WHERE m.content LIKE ?1 ESCAPE '\\'",
        );
        let mut params: Vec<rusqlite::types::Value> = vec![like_pat.into()];

        if let Some(ref ch_id) = filters.channel_id {
            sql.push_str(" AND m.channel_id = ?");
            params.push(ch_id.clone().into());
        }
        if let Some(ref g_id) = filters.guild_id {
            sql.push_str(" AND c.guild_id = ?");
            params.push(g_id.clone().into());
        }
        if let Some(ref auth_id) = filters.author_id {
            sql.push_str(" AND m.author_id = ?");
            params.push(auth_id.clone().into());
        }

        sql.push_str(&format!(" ORDER BY m.created_at_ms DESC LIMIT {}", limit));

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            Ok(MessageRow {
                id: r.get(0)?,
                channel_id: r.get(1)?,
                author_id: r.get(2)?,
                content: r.get(3)?,
                timestamp: r.get(4)?,
                edited_timestamp: r.get(5)?,
                attachments_json: r.get(6)?,
                embeds_json: r.get(7)?,
                reply_to: r.get(8)?,
                created_at_ms: r.get(9)?,
                author_username: r.get(10)?,
                author_global_name: r.get(11)?,
                guild_id: r.get(12)?,
                channel_name: r.get(13)?,
            })
        })?;

        let mut result = Vec::new();
        for r in rows {
            result.push(r?);
        }
        Ok(result)
    }

    pub fn list_guilds(&self, query: Option<&str>) -> Result<Vec<DiscordGuild>> {
        let conn = self.conn.lock().unwrap();
        let (sql, params): (String, Vec<rusqlite::types::Value>) = match query {
            Some(q) if !q.trim().is_empty() => (
                "SELECT id, name, icon FROM guilds WHERE name LIKE ?1 ESCAPE '\\' ORDER BY name ASC".to_string(),
                vec![format!(
                    "%{}%",
                    q.trim()
                        .replace('\\', "\\\\")
                        .replace('%', "\\%")
                        .replace('_', "\\_")
                )
                .into()],
            ),
            _ => ("SELECT id, name, icon FROM guilds ORDER BY name ASC".to_string(), vec![]),
        };

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            Ok(DiscordGuild {
                id: r.get(0)?,
                name: r.get(1)?,
                icon: r.get(2)?,
            })
        })?;

        let mut guilds = Vec::new();
        for g in rows {
            guilds.push(g?);
        }
        Ok(guilds)
    }

    pub fn list_channels(
        &self,
        guild_id: &str,
        query: Option<&str>,
    ) -> Result<Vec<DiscordChannel>> {
        let conn = self.conn.lock().unwrap();
        let (sql, params): (String, Vec<rusqlite::types::Value>) = match query {
            Some(q) if !q.trim().is_empty() => (
                "SELECT id, guild_id, name, type, topic, parent_id FROM channels
                 WHERE guild_id = ?1 AND name LIKE ?2 ESCAPE '\\' ORDER BY name ASC"
                    .to_string(),
                vec![
                    guild_id.to_string().into(),
                    format!(
                        "%{}%",
                        q.trim()
                            .replace('\\', "\\\\")
                            .replace('%', "\\%")
                            .replace('_', "\\_")
                    )
                    .into(),
                ],
            ),
            _ => (
                "SELECT id, guild_id, name, type, topic, parent_id FROM channels
                 WHERE guild_id = ?1 ORDER BY name ASC"
                    .to_string(),
                vec![guild_id.to_string().into()],
            ),
        };

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            Ok(DiscordChannel {
                id: r.get(0)?,
                guild_id: r.get(1)?,
                name: r.get(2)?,
                kind: r.get(3)?,
                topic: r.get(4)?,
                parent_id: r.get(5)?,
            })
        })?;

        let mut channels = Vec::new();
        for c in rows {
            channels.push(c?);
        }
        Ok(channels)
    }

    pub fn list_dm_people(&self, query: Option<&str>) -> Result<Vec<DmChannel>> {
        let conn = self.conn.lock().unwrap();
        let (sql, params): (String, Vec<rusqlite::types::Value>) = match query {
            Some(q) if !q.trim().is_empty() => {
                let pat = format!(
                    "%{}%",
                    q.trim()
                        .replace('\\', "\\\\")
                        .replace('%', "\\%")
                        .replace('_', "\\_")
                );
                (
                    "SELECT dm.id, u.id, u.username, u.global_name, u.discriminator, u.avatar, dm.kind
                     FROM dm_channels dm
                     JOIN users u ON dm.user_id = u.id
                     WHERE u.username LIKE ?1 ESCAPE '\\' OR u.global_name LIKE ?1 ESCAPE '\\' OR dm.name LIKE ?1 ESCAPE '\\'
                     ORDER BY dm.updated_at DESC".to_string(),
                    vec![pat.into()],
                )
            }
            _ => (
                "SELECT dm.id, u.id, u.username, u.global_name, u.discriminator, u.avatar, dm.kind
                 FROM dm_channels dm
                 JOIN users u ON dm.user_id = u.id
                 ORDER BY dm.updated_at DESC"
                    .to_string(),
                vec![],
            ),
        };

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            Ok(DmChannel {
                id: r.get(0)?,
                recipient: DiscordUser {
                    id: r.get(1)?,
                    username: r.get(2)?,
                    global_name: r.get(3)?,
                    discriminator: r.get(4)?,
                    avatar: r.get(5)?,
                },
                kind: r.get(6)?,
            })
        })?;

        let mut dms = Vec::new();
        for d in rows {
            dms.push(d?);
        }
        Ok(dms)
    }

    pub fn find_user(&self, query: &str) -> Result<Option<DiscordUser>> {
        let conn = self.conn.lock().unwrap();
        let q = query.trim();
        // Exact match by ID or username first
        let exact: Option<DiscordUser> = conn
            .query_row(
                "SELECT id, username, global_name, discriminator, avatar FROM users
                 WHERE id = ?1 OR username = ?1 COLLATE NOCASE OR global_name = ?1 COLLATE NOCASE
                 LIMIT 1",
                params![q],
                |r| {
                    Ok(DiscordUser {
                        id: r.get(0)?,
                        username: r.get(1)?,
                        global_name: r.get(2)?,
                        discriminator: r.get(3)?,
                        avatar: r.get(4)?,
                    })
                },
            )
            .optional()?;

        if exact.is_some() {
            return Ok(exact);
        }

        // Fuzzy match via LIKE
        let pat = format!(
            "%{}%",
            q.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let fuzzy: Option<DiscordUser> = conn
            .query_row(
                "SELECT id, username, global_name, discriminator, avatar FROM users
                 WHERE username LIKE ?1 ESCAPE '\\' OR global_name LIKE ?1 ESCAPE '\\'
                 ORDER BY updated_at DESC LIMIT 1",
                params![pat],
                |r| {
                    Ok(DiscordUser {
                        id: r.get(0)?,
                        username: r.get(1)?,
                        global_name: r.get(2)?,
                        discriminator: r.get(3)?,
                        avatar: r.get(4)?,
                    })
                },
            )
            .optional()?;

        Ok(fuzzy)
    }

    pub fn find_guild(&self, query: &str) -> Result<Option<DiscordGuild>> {
        let conn = self.conn.lock().unwrap();
        let q = query.trim();
        let exact: Option<DiscordGuild> = conn
            .query_row(
                "SELECT id, name, icon FROM guilds WHERE id = ?1 OR name = ?1 COLLATE NOCASE LIMIT 1",
                params![q],
                |r| {
                    Ok(DiscordGuild {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        icon: r.get(2)?,
                    })
                },
            )
            .optional()?;

        if exact.is_some() {
            return Ok(exact);
        }

        let pat = format!(
            "%{}%",
            q.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let fuzzy: Option<DiscordGuild> = conn
            .query_row(
                "SELECT id, name, icon FROM guilds WHERE name LIKE ?1 ESCAPE '\\' ORDER BY updated_at DESC LIMIT 1",
                params![pat],
                |r| {
                    Ok(DiscordGuild {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        icon: r.get(2)?,
                    })
                },
            )
            .optional()?;

        Ok(fuzzy)
    }

    pub fn find_channel(&self, guild_id: &str, query: &str) -> Result<Option<DiscordChannel>> {
        let conn = self.conn.lock().unwrap();
        let q = query.trim().trim_start_matches('#');
        let exact: Option<DiscordChannel> = conn
            .query_row(
                "SELECT id, guild_id, name, type, topic, parent_id FROM channels
                 WHERE guild_id = ?1 AND (id = ?2 OR name = ?2 COLLATE NOCASE) LIMIT 1",
                params![guild_id, q],
                |r| {
                    Ok(DiscordChannel {
                        id: r.get(0)?,
                        guild_id: r.get(1)?,
                        name: r.get(2)?,
                        kind: r.get(3)?,
                        topic: r.get(4)?,
                        parent_id: r.get(5)?,
                    })
                },
            )
            .optional()?;

        if exact.is_some() {
            return Ok(exact);
        }

        let pat = format!(
            "%{}%",
            q.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let fuzzy: Option<DiscordChannel> = conn
            .query_row(
                "SELECT id, guild_id, name, type, topic, parent_id FROM channels
                 WHERE guild_id = ?1 AND name LIKE ?2 ESCAPE '\\' ORDER BY updated_at DESC LIMIT 1",
                params![guild_id, pat],
                |r| {
                    Ok(DiscordChannel {
                        id: r.get(0)?,
                        guild_id: r.get(1)?,
                        name: r.get(2)?,
                        kind: r.get(3)?,
                        topic: r.get(4)?,
                        parent_id: r.get(5)?,
                    })
                },
            )
            .optional()?;

        Ok(fuzzy)
    }

    pub fn get_message_by_id(&self, id: &str) -> Result<Option<MessageRow>> {
        let conn = self.conn.lock().unwrap();
        let sql = "SELECT m.id, m.channel_id, m.author_id, m.content, m.timestamp, m.edited_timestamp,
                          m.attachments_json, m.embeds_json, m.reply_to, m.created_at_ms,
                          u.username, u.global_name, c.guild_id, c.name
                   FROM messages m
                   LEFT JOIN users u ON m.author_id = u.id
                   LEFT JOIN channels c ON m.channel_id = c.id
                   WHERE m.id = ?1";
        let mut stmt = conn.prepare(sql)?;
        let row = stmt
            .query_row(params![id], |r| {
                Ok(MessageRow {
                    id: r.get(0)?,
                    channel_id: r.get(1)?,
                    author_id: r.get(2)?,
                    content: r.get(3)?,
                    timestamp: r.get(4)?,
                    edited_timestamp: r.get(5)?,
                    attachments_json: r.get(6)?,
                    embeds_json: r.get(7)?,
                    reply_to: r.get(8)?,
                    created_at_ms: r.get(9)?,
                    author_username: r.get(10)?,
                    author_global_name: r.get(11)?,
                    guild_id: r.get(12)?,
                    channel_name: r.get(13)?,
                })
            })
            .optional()?;
        Ok(row)
    }

    pub fn find_dm_channels_exact(&self, query: &str) -> Result<Vec<DmChannel>> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(vec![]);
        }
        let conn = self.conn.lock().unwrap();
        let sql = "SELECT dm.id, u.id, u.username, u.global_name, u.discriminator, u.avatar, dm.kind
                   FROM dm_channels dm
                   JOIN users u ON dm.user_id = u.id
                   WHERE dm.kind = 1 AND (dm.id = ?1 OR u.id = ?1 OR u.username = ?1 COLLATE NOCASE OR u.global_name = ?1 COLLATE NOCASE)
                   ORDER BY dm.updated_at DESC";
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params![q], |r| {
            Ok(DmChannel {
                id: r.get(0)?,
                recipient: DiscordUser {
                    id: r.get(1)?,
                    username: r.get(2)?,
                    global_name: r.get(3)?,
                    discriminator: r.get(4)?,
                    avatar: r.get(5)?,
                },
                kind: r.get(6)?,
            })
        })?;
        let mut dms = Vec::new();
        for d in rows {
            dms.push(d?);
        }
        Ok(dms)
    }

    pub fn find_guilds_exact(&self, query: &str) -> Result<Vec<DiscordGuild>> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(vec![]);
        }
        let conn = self.conn.lock().unwrap();
        let sql = "SELECT id, name, icon FROM guilds WHERE id = ?1 OR name = ?1 COLLATE NOCASE ORDER BY name ASC";
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params![q], |r| {
            Ok(DiscordGuild {
                id: r.get(0)?,
                name: r.get(1)?,
                icon: r.get(2)?,
            })
        })?;
        let mut guilds = Vec::new();
        for g in rows {
            guilds.push(g?);
        }
        Ok(guilds)
    }

    pub fn find_channels_exact(&self, guild_id: &str, query: &str) -> Result<Vec<DiscordChannel>> {
        let q = query.trim().trim_start_matches('#');
        if q.is_empty() {
            return Ok(vec![]);
        }
        let conn = self.conn.lock().unwrap();
        let sql = "SELECT id, guild_id, name, type, topic, parent_id FROM channels
                   WHERE guild_id = ?1 AND (id = ?2 OR name = ?2 COLLATE NOCASE)
                   ORDER BY name ASC";
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params![guild_id, q], |r| {
            Ok(DiscordChannel {
                id: r.get(0)?,
                guild_id: r.get(1)?,
                name: r.get(2)?,
                kind: r.get(3)?,
                topic: r.get(4)?,
                parent_id: r.get(5)?,
            })
        })?;
        let mut channels = Vec::new();
        for c in rows {
            channels.push(c?);
        }
        Ok(channels)
    }

    pub fn stats(&self) -> Result<DbStats> {
        let conn = self.conn.lock().unwrap();
        let users: i64 = conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
        let guilds: i64 = conn.query_row("SELECT COUNT(*) FROM guilds", [], |r| r.get(0))?;
        let channels: i64 = conn.query_row("SELECT COUNT(*) FROM channels", [], |r| r.get(0))?;
        let dm_channels: i64 =
            conn.query_row("SELECT COUNT(*) FROM dm_channels", [], |r| r.get(0))?;
        let messages: i64 = conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;
        let fts_mode: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'fts_mode'", [], |r| {
                r.get(0)
            })
            .unwrap_or_else(|_| "fts5".to_string());

        Ok(DbStats {
            users,
            guilds,
            channels,
            dm_channels,
            messages,
            fts_mode,
        })
    }
}

pub fn escape_fts5_query(query: &str) -> String {
    let clean = query.replace(['"', '*', '^', ':', '(', ')', '-', '+'], " ");

    let terms: Vec<String> = clean
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .filter(|t| {
            let upper = t.to_ascii_uppercase();
            upper != "AND" && upper != "OR" && upper != "NOT"
        })
        .map(|t| format!("\"{}\"", t.replace('"', "")))
        .collect();

    terms.join(" AND ")
}
