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
pub struct ChannelSyncState {
    pub channel_id: String,
    pub oldest_synced_id: Option<String>,
    pub newest_synced_id: Option<String>,
    pub fully_backfilled: bool,
    pub last_synced_at: Option<i64>,
    pub total_cached: i64,
}

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
    pub message_type: i32,
    pub call_json: Option<String>,
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
    pub data_dir: Option<std::path::PathBuf>,
}

pub const MESSAGE_ROW_SELECT: &str = "m.id, m.channel_id, m.author_id, m.content, m.timestamp, m.edited_timestamp, \
     m.attachments_json, m.embeds_json, m.reply_to, m.created_at_ms, \
     u.username, u.global_name, c.guild_id, c.name, m.message_type, m.call_json";

pub fn map_message_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<MessageRow> {
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
        message_type: r.get(14)?,
        call_json: r.get(15)?,
    })
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
        data_dir: Some(data_dir.to_path_buf()),
    };
    db.init_schema()?;
    Ok(db)
}

impl Db {
    pub fn new_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Db {
            conn: Mutex::new(conn),
            data_dir: None,
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
                 message_type INTEGER NOT NULL DEFAULT 0,
                 call_json TEXT,
                 reply_to TEXT,
                 created_at_ms INTEGER,
                 synced_at INTEGER
             );

             CREATE INDEX IF NOT EXISTS idx_messages_channel_created
                 ON messages(channel_id, created_at_ms);

             CREATE TABLE IF NOT EXISTS meta (
                 key TEXT PRIMARY KEY,
                 value TEXT
             );

             CREATE TABLE IF NOT EXISTS channel_sync_state (
                 channel_id TEXT PRIMARY KEY,
                 oldest_synced_id TEXT,
                 newest_synced_id TEXT,
                 fully_backfilled INTEGER NOT NULL DEFAULT 0,
                 last_synced_at INTEGER,
                 total_cached INTEGER NOT NULL DEFAULT 0
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

        // Legacy migration: older caches had no message type / call payload columns.
        let message_columns: std::collections::HashSet<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(messages)")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
            let mut cols = std::collections::HashSet::new();
            for col in rows {
                cols.insert(col?);
            }
            cols
        };
        if !message_columns.contains("message_type") {
            conn.execute(
                "ALTER TABLE messages ADD COLUMN message_type INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        if !message_columns.contains("call_json") {
            conn.execute("ALTER TABLE messages ADD COLUMN call_json TEXT", [])?;
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

    /// Delete every cached row, preserving the `fts_mode` meta key.
    ///
    /// Bypasses the per-row `trg_messages_ad` trigger to avoid an O(N^2) full
    /// table scan on unindexed FTS tables during mass deletion, clearing
    /// `messages_fts` directly and recreating the trigger in the same
    /// transaction. Everything runs in one transaction so a failure cannot
    /// leave a partially cleared cache behind.
    pub fn clear_cache(&self) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;

        tx.execute_batch("DROP TRIGGER IF EXISTS trg_messages_ad;")?;
        tx.execute("DELETE FROM messages", [])?;

        let has_fts: bool = tx
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'messages_fts'",
                [],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);

        if has_fts {
            tx.execute("DELETE FROM messages_fts", [])?;
            tx.execute_batch(
                "CREATE TRIGGER IF NOT EXISTS trg_messages_ad AFTER DELETE ON messages BEGIN
                     DELETE FROM messages_fts WHERE id = old.id;
                 END;",
            )?;
        }

        tx.execute("DELETE FROM channel_sync_state", [])?;
        tx.execute("DELETE FROM dm_channels", [])?;
        tx.execute("DELETE FROM channels", [])?;
        tx.execute("DELETE FROM guilds", [])?;
        tx.execute("DELETE FROM users", [])?;
        tx.execute("DELETE FROM meta WHERE key != 'fts_mode'", [])?;
        tx.commit()?;
        Ok(())
    }

    /// Ensure the cache belongs to `user_id`, clearing it when it does not.
    ///
    /// Returns `true` when the cache was cleared (an account switch, or a
    /// legacy cache with no recorded account) and `false` when the cached
    /// account already matches.
    pub fn ensure_account(&self, user_id: &str) -> Result<bool> {
        let cached_user_id: Option<String> = {
            let conn = self.conn.lock().unwrap();
            conn.query_row(
                "SELECT value FROM meta WHERE key = 'account_user_id'",
                [],
                |r| r.get(0),
            )
            .optional()?
        };

        if cached_user_id.as_deref() == Some(user_id) {
            return Ok(false);
        }

        self.clear_cache()?;

        {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('account_user_id', ?1)",
                params![user_id],
            )?;
        }

        Ok(true)
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
        if let Some(ref embs) = partial.embeds {
            sets.push("embeds_json = ?".to_string());
            if embs.is_empty() {
                query_params.push(rusqlite::types::Value::Null);
            } else {
                query_params.push(serde_json::to_string(embs)?.into());
            }
        }
        if let Some(k) = partial.kind {
            sets.push("message_type = ?".to_string());
            query_params.push(k.into());
        }
        if let Some(ref c) = partial.call {
            sets.push("call_json = ?".to_string());
            query_params.push(serde_json::to_string(c)?.into());
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
        self.batch_upsert_messages(std::slice::from_ref(message))
            .map(|_| ())
    }

    pub fn batch_upsert_messages(&self, messages: &[DiscordMessage]) -> Result<usize> {
        if messages.is_empty() {
            return Ok(0);
        }
        let now = current_timestamp_ms();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().context("Failed to begin transaction for batch upsert")?;

        {
            let mut user_stmt = tx.prepare(
                "INSERT INTO users (id, username, global_name, discriminator, avatar, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                     username = excluded.username,
                     global_name = excluded.global_name,
                     discriminator = excluded.discriminator,
                     avatar = excluded.avatar,
                     updated_at = excluded.updated_at",
            ).context("Failed to prepare user upsert statement")?;

            let mut msg_stmt = tx.prepare(
                "INSERT INTO messages (
                     id, channel_id, author_id, content, timestamp, edited_timestamp,
                     attachments_json, embeds_json, message_type, call_json, reply_to,
                     created_at_ms, synced_at
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT(id) DO UPDATE SET
                     channel_id = excluded.channel_id,
                     author_id = excluded.author_id,
                     content = excluded.content,
                     timestamp = COALESCE(NULLIF(excluded.timestamp, ''), messages.timestamp),
                     edited_timestamp = excluded.edited_timestamp,
                     attachments_json = excluded.attachments_json,
                     embeds_json = excluded.embeds_json,
                     message_type = excluded.message_type,
                     call_json = excluded.call_json,
                     reply_to = excluded.reply_to,
                     created_at_ms = COALESCE(NULLIF(messages.created_at_ms, 0), excluded.created_at_ms),
                     synced_at = excluded.synced_at
                 WHERE messages.channel_id IS NOT excluded.channel_id
                    OR messages.author_id IS NOT excluded.author_id
                    OR messages.content IS NOT excluded.content
                    OR messages.edited_timestamp IS NOT excluded.edited_timestamp
                    OR messages.attachments_json IS NOT excluded.attachments_json
                    OR messages.embeds_json IS NOT excluded.embeds_json
                    OR messages.reply_to IS NOT excluded.reply_to
                    OR messages.message_type IS NOT excluded.message_type
                    OR messages.call_json IS NOT excluded.call_json
                    OR (messages.timestamp = '' AND excluded.timestamp != '')
                    OR (messages.created_at_ms = 0 AND excluded.created_at_ms != 0)",
            ).context("Failed to prepare message upsert statement")?;

            for message in messages {
                user_stmt.execute(params![
                    message.author.id,
                    message.author.username,
                    message.author.global_name,
                    message.author.discriminator,
                    message.author.avatar,
                    now,
                ]).with_context(|| format!("Failed to upsert author user {} in batch", message.author.id))?;

                let created_at_ms = parse_iso_timestamp_ms(&message.timestamp);

                let attachments_json = if message.attachments.is_empty() {
                    None
                } else {
                    Some(serde_json::to_string(&message.attachments)
                        .with_context(|| format!("Failed to serialize attachments for message {}", message.id))?)
                };

                let embeds_json = if message.embeds.is_empty() {
                    None
                } else {
                    Some(serde_json::to_string(&message.embeds)
                        .with_context(|| format!("Failed to serialize embeds for message {}", message.id))?)
                };

                let call_json = message
                    .call
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .with_context(|| format!("Failed to serialize call for message {}", message.id))?;

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

                msg_stmt.execute(params![
                    message.id,
                    message.channel_id,
                    message.author.id,
                    message.content,
                    message.timestamp,
                    message.edited_timestamp,
                    attachments_json,
                    embeds_json,
                    message.kind,
                    call_json,
                    reply_to,
                    created_at_ms,
                    now,
                ]).with_context(|| format!("Failed to upsert message {} in batch", message.id))?;
            }
        }

        tx.commit().context("Failed to commit batch upsert transaction")?;
        Ok(messages.len())
    }

    pub fn get_channel_sync_state(&self, channel_id: &str) -> Result<Option<ChannelSyncState>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT channel_id, oldest_synced_id, newest_synced_id, fully_backfilled, last_synced_at, total_cached
             FROM channel_sync_state WHERE channel_id = ?1",
        )?;
        let row = stmt
            .query_row(params![channel_id], |r| {
                let backfilled_int: i64 = r.get(3)?;
                Ok(ChannelSyncState {
                    channel_id: r.get(0)?,
                    oldest_synced_id: r.get(1)?,
                    newest_synced_id: r.get(2)?,
                    fully_backfilled: backfilled_int != 0,
                    last_synced_at: r.get(4)?,
                    total_cached: r.get(5)?,
                })
            })
            .optional()?;
        Ok(row)
    }

    pub fn upsert_channel_sync_state(&self, state: &ChannelSyncState) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO channel_sync_state (
                 channel_id, oldest_synced_id, newest_synced_id, fully_backfilled, last_synced_at, total_cached
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(channel_id) DO UPDATE SET
                 oldest_synced_id = excluded.oldest_synced_id,
                 newest_synced_id = excluded.newest_synced_id,
                 fully_backfilled = excluded.fully_backfilled,
                 last_synced_at = excluded.last_synced_at,
                 total_cached = excluded.total_cached",
            params![
                state.channel_id,
                state.oldest_synced_id,
                state.newest_synced_id,
                if state.fully_backfilled { 1 } else { 0 },
                state.last_synced_at,
                state.total_cached,
            ],
        )?;
        Ok(())
    }

    pub fn count_channel_messages(&self, channel_id: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE channel_id = ?1",
            params![channel_id],
            |r| r.get(0),
        )?;
        Ok(count)
    }

    pub fn prune_guilds_not_in(&self, ids: &[String]) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        if ids.is_empty() {
            conn.execute(
                "DELETE FROM channel_sync_state WHERE channel_id IN (SELECT id FROM channels WHERE guild_id IS NOT NULL)",
                [],
            )?;
            let removed_channels =
                conn.execute("DELETE FROM channels WHERE guild_id IS NOT NULL", [])?;
            let removed_guilds = conn.execute("DELETE FROM guilds", [])?;
            return Ok(removed_channels + removed_guilds);
        }

        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let del_sync_state = format!(
            "DELETE FROM channel_sync_state WHERE channel_id IN (SELECT id FROM channels WHERE guild_id IS NOT NULL AND guild_id NOT IN ({}))",
            placeholders
        );
        let del_channels = format!(
            "DELETE FROM channels WHERE guild_id IS NOT NULL AND guild_id NOT IN ({})",
            placeholders
        );
        let del_guilds = format!("DELETE FROM guilds WHERE id NOT IN ({})", placeholders);

        let params_sync = rusqlite::params_from_iter(ids);
        conn.execute(&del_sync_state, params_sync)?;
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
                    let valid_anchor = b_id == "0" || (!b_id.starts_with('0') && b_id.parse::<u64>().is_ok());
                    if valid_anchor {
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
                    let valid_anchor = a_id == "0" || (!a_id.starts_with('0') && a_id.parse::<u64>().is_ok());
                    if valid_anchor {
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
            "SELECT {}
             FROM messages m
             LEFT JOIN users u ON m.author_id = u.id
             LEFT JOIN channels c ON m.channel_id = c.id
             WHERE {}
             ORDER BY m.created_at_ms DESC, LENGTH(m.id) DESC, m.id DESC
             LIMIT {}",
            MESSAGE_ROW_SELECT, where_clause, limit
        );

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(query_params), map_message_row)?;

        let mut messages = Vec::new();
        for msg in rows {
            messages.push(msg?);
        }
        // Oldest -> newest
        messages.reverse();
        Ok(messages)
    }

    pub fn get_channel_messages_paginated(
        &self,
        channel_id: &str,
        before: Option<&str>,
        after: Option<&str>,
        limit: usize,
        ascending: bool,
    ) -> Result<Vec<MessageRow>> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.max(1);

        let mut conditions = vec!["m.channel_id = ?1".to_string()];
        let mut query_params: Vec<rusqlite::types::Value> = vec![channel_id.to_string().into()];

        if let Some(b_id) = before {
            let in_cache: bool = conn
                .query_row(
                    "SELECT 1 FROM messages WHERE id = ?1",
                    params![b_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();

            let valid_anchor = b_id == "0" || (!b_id.starts_with('0') && b_id.parse::<u64>().is_ok());

            if in_cache || valid_anchor {
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

        if let Some(a_id) = after {
            let in_cache: bool = conn
                .query_row(
                    "SELECT 1 FROM messages WHERE id = ?1",
                    params![a_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();

            let valid_anchor = a_id == "0" || (!a_id.starts_with('0') && a_id.parse::<u64>().is_ok());

            if in_cache || valid_anchor {
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

        let order_dir = if ascending { "ASC" } else { "DESC" };
        let where_clause = conditions.join(" AND ");
        let sql = format!(
            "SELECT {}
             FROM messages m
             LEFT JOIN users u ON m.author_id = u.id
             LEFT JOIN channels c ON m.channel_id = c.id
             WHERE {}
             ORDER BY LENGTH(m.id) {}, m.id {}
             LIMIT {}",
            MESSAGE_ROW_SELECT, where_clause, order_dir, order_dir, limit
        );

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(query_params), map_message_row)?;

        let mut messages = Vec::new();
        for msg in rows {
            messages.push(msg?);
        }
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
                let mut sql = format!(
                    "SELECT {}
                     FROM messages_fts fts
                     JOIN messages m ON fts.id = m.id
                     LEFT JOIN users u ON m.author_id = u.id
                     LEFT JOIN channels c ON m.channel_id = c.id
                     WHERE messages_fts MATCH ?1",
                    MESSAGE_ROW_SELECT
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
                    let rows = stmt.query_map(rusqlite::params_from_iter(params.clone()), map_message_row)?;
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
        let mut sql = format!(
            "SELECT {}
             FROM messages m
             LEFT JOIN users u ON m.author_id = u.id
             LEFT JOIN channels c ON m.channel_id = c.id
             WHERE m.content LIKE ?1 ESCAPE '\\'",
            MESSAGE_ROW_SELECT
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
        let rows = stmt.query_map(rusqlite::params_from_iter(params), map_message_row)?;

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
        let q = query.trim();
        if q.is_empty() {
            return Ok(None);
        }
        let conn = self.conn.lock().unwrap();
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
        let q = query.trim();
        if q.is_empty() {
            return Ok(None);
        }
        let conn = self.conn.lock().unwrap();
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
        let q = query.trim().trim_start_matches('#');
        if q.is_empty() {
            return Ok(None);
        }
        let conn = self.conn.lock().unwrap();
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
        let sql = format!(
            "SELECT {}
             FROM messages m
             LEFT JOIN users u ON m.author_id = u.id
             LEFT JOIN channels c ON m.channel_id = c.id
             WHERE m.id = ?1",
            MESSAGE_ROW_SELECT
        );
        let mut stmt = conn.prepare(&sql)?;
        let row = stmt.query_row(params![id], map_message_row).optional()?;
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
