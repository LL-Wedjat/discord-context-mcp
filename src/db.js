/**
 * @fileoverview SQLite database cache using node:sqlite DatabaseSync.
 * Stores Discord users, guilds, channels, DM channels, and messages with full-text search.
 */

import { DatabaseSync } from 'node:sqlite';
import fs from 'node:fs';
import path from 'node:path';

/**
 * Initializes and migrates SQLite database cache.
 * @param {string} dataDir
 * @returns {DatabaseSync}
 */
export function openDb(dataDir) {
  fs.mkdirSync(dataDir, { recursive: true });
  const dbPath = path.join(dataDir, 'cache.db');
  const db = new DatabaseSync(dbPath);

  db.exec('PRAGMA journal_mode = WAL;');
  db.exec('PRAGMA busy_timeout = 5000;');

  db.exec(`
    CREATE TABLE IF NOT EXISTS users (
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

    CREATE INDEX IF NOT EXISTS idx_messages_channel_time ON messages(channel_id, created_at_ms);

    CREATE TABLE IF NOT EXISTS meta (
      key TEXT PRIMARY KEY,
      value TEXT
    );
  `);

  // FTS5 Setup with fallback
  let ftsEnabled = false;
  try {
    db.exec(`
      CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
        content,
        id UNINDEXED,
        content='messages',
        content_rowid='rowid'
      );

      CREATE TRIGGER IF NOT EXISTS trg_messages_ai AFTER INSERT ON messages BEGIN
        INSERT INTO messages_fts(rowid, content, id) VALUES (new.rowid, new.content, new.id);
      END;

      CREATE TRIGGER IF NOT EXISTS trg_messages_ad AFTER DELETE ON messages BEGIN
        INSERT INTO messages_fts(messages_fts, rowid, content, id) VALUES ('delete', old.rowid, old.content, old.id);
      END;

      CREATE TRIGGER IF NOT EXISTS trg_messages_au AFTER UPDATE ON messages BEGIN
        INSERT INTO messages_fts(messages_fts, rowid, content, id) VALUES ('delete', old.rowid, old.content, old.id);
        INSERT INTO messages_fts(rowid, content, id) VALUES (new.rowid, new.content, new.id);
      END;
    `);
    ftsEnabled = true;
  } catch (err) {
    process.stderr.write(`[discord-db] FTS5 setup failed, falling back to LIKE: ${err.message}\n`);
    ftsEnabled = false;
  }

  const ftsMode = ftsEnabled ? 'fts5' : 'like';
  const setMeta = db.prepare('INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value');
  setMeta.run('fts_mode', ftsMode);

  return db;
}

/**
 * Upserts a Discord user record.
 * @param {DatabaseSync} db
 * @param {Object} u
 */
export function upsertUser(db, u) {
  if (!u || !u.id) return;
  const now = Date.now();
  const stmt = db.prepare(`
    INSERT INTO users (id, username, global_name, discriminator, avatar, updated_at)
    VALUES (?, ?, ?, ?, ?, ?)
    ON CONFLICT(id) DO UPDATE SET
      username = excluded.username,
      global_name = excluded.global_name,
      discriminator = excluded.discriminator,
      avatar = excluded.avatar,
      updated_at = excluded.updated_at
  `);
  stmt.run(
    String(u.id),
    u.username ?? null,
    u.global_name ?? null,
    u.discriminator ?? null,
    u.avatar ?? null,
    now
  );
}

/**
 * Upserts a Discord guild record.
 * @param {DatabaseSync} db
 * @param {Object} g
 */
export function upsertGuild(db, g) {
  if (!g || !g.id) return;
  const now = Date.now();
  const name = (typeof g.name === 'string' && g.name.trim() !== '') ? g.name : null;
  const icon = (typeof g.icon === 'string' && g.icon.trim() !== '') ? g.icon : (g.icon ?? null);
  const stmt = db.prepare(`
    INSERT INTO guilds (id, name, icon, updated_at)
    VALUES (?, ?, ?, ?)
    ON CONFLICT(id) DO UPDATE SET
      name = COALESCE(excluded.name, guilds.name),
      icon = COALESCE(excluded.icon, guilds.icon),
      updated_at = excluded.updated_at
  `);
  stmt.run(String(g.id), name, icon, now);
}

/**
 * Prunes guilds (and their channels) not present in the provided list of IDs.
 * @param {DatabaseSync} db
 * @param {Array<string|number>} ids
 * @returns {number} Number of guilds removed
 */
export function pruneGuildsNotIn(db, ids) {
  if (!Array.isArray(ids)) return 0;
  const validIds = new Set(ids.map(String));
  const cachedGuilds = db.prepare('SELECT id FROM guilds').all();
  let removed = 0;

  const deleteChannels = db.prepare('DELETE FROM channels WHERE guild_id = ?');
  const deleteGuild = db.prepare('DELETE FROM guilds WHERE id = ?');

  for (const g of cachedGuilds) {
    const gid = String(g.id);
    if (!validIds.has(gid)) {
      deleteChannels.run(gid);
      deleteGuild.run(gid);
      removed++;
    }
  }

  return removed;
}

/**
 * Upserts a Discord channel record.
 * @param {DatabaseSync} db
 * @param {Object} c
 */
export function upsertChannel(db, c) {
  if (!c || !c.id) return;
  const now = Date.now();
  const stmt = db.prepare(`
    INSERT INTO channels (id, guild_id, name, type, topic, parent_id, updated_at)
    VALUES (?, ?, ?, ?, ?, ?, ?)
    ON CONFLICT(id) DO UPDATE SET
      guild_id = excluded.guild_id,
      name = excluded.name,
      type = excluded.type,
      topic = excluded.topic,
      parent_id = excluded.parent_id,
      updated_at = excluded.updated_at
  `);
  stmt.run(
    String(c.id),
    c.guild_id ? String(c.guild_id) : null,
    c.name ?? null,
    c.type !== undefined ? Number(c.type) : null,
    c.topic ?? null,
    c.parent_id ? String(c.parent_id) : null,
    now
  );
}

/**
 * Upserts a DM channel record.
 * @param {DatabaseSync} db
 * @param {string} dmId
 * @param {string} otherUserId
 * @param {string} name
 */
export function upsertDmChannel(db, dmId, otherUserId, name) {
  if (!dmId) return;
  const now = Date.now();
  const stmt = db.prepare(`
    INSERT INTO dm_channels (id, user_id, name, updated_at)
    VALUES (?, ?, ?, ?)
    ON CONFLICT(id) DO UPDATE SET
      user_id = excluded.user_id,
      name = excluded.name,
      updated_at = excluded.updated_at
  `);
  stmt.run(String(dmId), otherUserId ? String(otherUserId) : null, name ?? null, now);
}

/**
 * Upserts a message.
 * @param {DatabaseSync} db
 * @param {Object} m
 */
export function upsertMessage(db, m) {
  if (!m || !m.id) return;
  const now = Date.now();
  const createdAtMs = m.created_at_ms || (m.timestamp ? Date.parse(m.timestamp) : now);
  const stmt = db.prepare(`
    INSERT INTO messages (
      id, channel_id, author_id, content, timestamp, edited_timestamp,
      attachments_json, embeds_json, reply_to, created_at_ms, synced_at
    )
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    ON CONFLICT(id) DO UPDATE SET
      channel_id = excluded.channel_id,
      author_id = excluded.author_id,
      content = excluded.content,
      timestamp = excluded.timestamp,
      edited_timestamp = excluded.edited_timestamp,
      attachments_json = excluded.attachments_json,
      embeds_json = excluded.embeds_json,
      reply_to = excluded.reply_to,
      created_at_ms = excluded.created_at_ms,
      synced_at = excluded.synced_at
  `);

  stmt.run(
    String(m.id),
    String(m.channel_id),
    m.author_id ? String(m.author_id) : null,
    m.content ?? '',
    m.timestamp ?? null,
    m.edited_timestamp ?? null,
    typeof m.attachments_json === 'string' ? m.attachments_json : JSON.stringify(m.attachments_json || []),
    typeof m.embeds_json === 'string' ? m.embeds_json : JSON.stringify(m.embeds_json || []),
    m.reply_to ? String(m.reply_to) : null,
    createdAtMs,
    now
  );
}

/**
 * Retrieves messages from a channel ordered oldest -> newest.
 * @param {DatabaseSync} db
 * @param {Object} options
 * @param {string} options.channelId
 * @param {number} [options.limit=50]
 * @param {number|string} [options.before]
 * @param {number|string} [options.after]
 * @returns {Array<Object>}
 */
export function getMessages(db, { channelId, limit = 50, before, after }) {
  if (!channelId) return [];
  const safeLimit = Math.min(Math.max(1, limit || 50), 500);

  let whereClauses = ['m.channel_id = ?'];
  let params = [String(channelId)];

  if (before !== undefined && before !== null) {
    const beforeMs = typeof before === 'number' ? before : (Date.parse(before) || Number(before));
    if (!Number.isNaN(beforeMs)) {
      whereClauses.push('m.created_at_ms < ?');
      params.push(beforeMs);
    }
  }

  if (after !== undefined && after !== null) {
    const afterMs = typeof after === 'number' ? after : (Date.parse(after) || Number(after));
    if (!Number.isNaN(afterMs)) {
      whereClauses.push('m.created_at_ms > ?');
      params.push(afterMs);
    }
  }

  params.push(safeLimit);

  // We select the latest matching safeLimit rows, but return them oldest->newest
  const query = `
    SELECT * FROM (
      SELECT
        m.id, m.channel_id, m.author_id, m.content, m.timestamp, m.edited_timestamp,
        m.attachments_json, m.embeds_json, m.reply_to, m.created_at_ms, m.synced_at,
        u.username AS author_username,
        u.global_name AS author_global_name
      FROM messages m
      LEFT JOIN users u ON m.author_id = u.id
      WHERE ${whereClauses.join(' AND ')}
      ORDER BY m.created_at_ms DESC
      LIMIT ?
    )
    ORDER BY created_at_ms ASC
  `;

  return db.prepare(query).all(...params);
}

/**
 * Searches messages matching a query.
 * @param {DatabaseSync} db
 * @param {string} query
 * @param {Object} [options={}]
 * @param {string} [options.channelId]
 * @param {string} [options.guildId]
 * @param {string} [options.userId]
 * @param {number} [options.limit=30]
 * @returns {Array<Object>}
 */
export function searchMessages(db, query, { channelId, guildId, userId, limit = 30 } = {}) {
  const safeLimit = Math.min(Math.max(1, limit || 30), 100);
  const metaMode = db.prepare("SELECT value FROM meta WHERE key = 'fts_mode'").get();
  const ftsMode = metaMode ? metaMode.value : 'like';

  let whereClauses = [];
  let params = [];

  if (channelId) {
    whereClauses.push('m.channel_id = ?');
    params.push(String(channelId));
  }
  if (guildId) {
    whereClauses.push('c.guild_id = ?');
    params.push(String(guildId));
  }
  if (userId) {
    whereClauses.push('m.author_id = ?');
    params.push(String(userId));
  }

  if (ftsMode === 'fts5') {
    // Escape double quotes in FTS query and match tokens
    const escapedQuery = query
      ? query.replace(/"/g, '""').trim()
      : '';
    if (escapedQuery) {
      whereClauses.push('m.rowid IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?)');
      // Wrap tokenized phrases in quotes for literal phrase or prefix search
      const ftsToken = `"${escapedQuery}"`;
      params.push(ftsToken);
    }
  } else {
    if (query) {
      whereClauses.push('m.content LIKE ?');
      params.push(`%${query}%`);
    }
  }

  params.push(safeLimit);

  const whereSql = whereClauses.length > 0 ? `WHERE ${whereClauses.join(' AND ')}` : '';
  const sql = `
    SELECT
      m.id, m.channel_id, m.author_id, m.content, m.timestamp, m.edited_timestamp,
      m.attachments_json, m.embeds_json, m.reply_to, m.created_at_ms, m.synced_at,
      u.username AS author_username,
      u.global_name AS author_global_name,
      c.guild_id,
      c.name AS channel_name
    FROM messages m
    LEFT JOIN users u ON m.author_id = u.id
    LEFT JOIN channels c ON m.channel_id = c.id
    ${whereSql}
    ORDER BY m.created_at_ms DESC
    LIMIT ?
  `;

  try {
    return db.prepare(sql).all(...params);
  } catch (err) {
    // If FTS fails at runtime (e.g. malformed syntax), fallback to LIKE
    if (ftsMode === 'fts5') {
      process.stderr.write(`[discord-db] FTS query failed (${err.message}), falling back to LIKE\n`);
      let fallbackParams = [];
      let fallbackWhere = [];
      if (channelId) {
        fallbackWhere.push('m.channel_id = ?');
        fallbackParams.push(String(channelId));
      }
      if (guildId) {
        fallbackWhere.push('c.guild_id = ?');
        fallbackParams.push(String(guildId));
      }
      if (userId) {
        fallbackWhere.push('m.author_id = ?');
        fallbackParams.push(String(userId));
      }
      if (query) {
        fallbackWhere.push('m.content LIKE ?');
        fallbackParams.push(`%${query}%`);
      }
      fallbackParams.push(safeLimit);
      const fallbackWhereSql = fallbackWhere.length > 0 ? `WHERE ${fallbackWhere.join(' AND ')}` : '';
      const fallbackSql = `
        SELECT
          m.id, m.channel_id, m.author_id, m.content, m.timestamp, m.edited_timestamp,
          m.attachments_json, m.embeds_json, m.reply_to, m.created_at_ms, m.synced_at,
          u.username AS author_username,
          u.global_name AS author_global_name,
          c.guild_id,
          c.name AS channel_name
        FROM messages m
        LEFT JOIN users u ON m.author_id = u.id
        LEFT JOIN channels c ON m.channel_id = c.id
        ${fallbackWhereSql}
        ORDER BY m.created_at_ms DESC
        LIMIT ?
      `;
      return db.prepare(fallbackSql).all(...fallbackParams);
    }
    throw err;
  }
}

/**
 * Lists guilds filtered by case-insensitive name substring.
 * @param {DatabaseSync} db
 * @param {string} [query='']
 * @returns {Array<{id: string, name: string, icon: string}>}
 */
export function listGuilds(db, query = '') {
  if (query && query.trim()) {
    return db.prepare(`
      SELECT id, name, icon FROM guilds
      WHERE LOWER(name) LIKE LOWER(?)
      ORDER BY name ASC
    `).all(`%${query.trim()}%`);
  }
  return db.prepare(`
    SELECT id, name, icon FROM guilds
    ORDER BY name ASC
  `).all();
}

/**
 * Lists channels for a guild, filtered by name substring.
 * @param {DatabaseSync} db
 * @param {string} guildId
 * @param {string} [query='']
 * @returns {Array<Object>}
 */
export function listChannels(db, guildId, query = '') {
  const cleanQuery = (query || '').replace(/^#/, '').trim();
  if (cleanQuery) {
    return db.prepare(`
      SELECT id, guild_id, name, type, topic, parent_id
      FROM channels
      WHERE guild_id = ? AND LOWER(name) LIKE LOWER(?)
      ORDER BY name ASC
    `).all(String(guildId), `%${cleanQuery}%`);
  }
  return db.prepare(`
    SELECT id, guild_id, name, type, topic, parent_id
    FROM channels
    WHERE guild_id = ?
    ORDER BY name ASC
  `).all(String(guildId));
}

/**
 * Lists direct message contacts.
 * @param {DatabaseSync} db
 * @param {string} [query='']
 * @returns {Array<{userId: string, username: string, global_name: string, channelId: string}>}
 */
export function listDmPeople(db, query = '') {
  const clean = (query || '').replace(/^@/, '').trim();
  const sql = `
    SELECT
      dm.user_id AS userId,
      u.username,
      u.global_name,
      dm.id AS channelId
    FROM dm_channels dm
    LEFT JOIN users u ON dm.user_id = u.id
    ${clean ? 'WHERE LOWER(u.username) LIKE LOWER(?) OR LOWER(u.global_name) LIKE LOWER(?) OR LOWER(dm.name) LIKE LOWER(?)' : ''}
    ORDER BY COALESCE(u.global_name, u.username, dm.name) ASC
  `;

  if (clean) {
    const pattern = `%${clean}%`;
    return db.prepare(sql).all(pattern, pattern, pattern);
  }
  return db.prepare(sql).all();
}

/**
 * Finds best user match by username/global_name/id (case-insensitive exact first, then substring).
 * @param {DatabaseSync} db
 * @param {string} query
 * @returns {Object|null}
 */
export function findUser(db, query) {
  if (!query) return null;
  const clean = query.replace(/^@/, '').trim();
  if (!clean) return null;

  // Exact ID
  let row = db.prepare('SELECT * FROM users WHERE id = ?').get(clean);
  if (row) return row;

  // Exact username or global_name
  row = db.prepare('SELECT * FROM users WHERE LOWER(username) = LOWER(?) OR LOWER(global_name) = LOWER(?) LIMIT 1').get(clean, clean);
  if (row) return row;

  // Substring
  const pattern = `%${clean}%`;
  return db.prepare('SELECT * FROM users WHERE LOWER(username) LIKE LOWER(?) OR LOWER(global_name) LIKE LOWER(?) LIMIT 1').get(pattern, pattern) || null;
}

/**
 * Finds best guild match by name/id (exact first, then substring).
 * @param {DatabaseSync} db
 * @param {string} query
 * @returns {Object|null}
 */
export function findGuild(db, query) {
  if (!query) return null;
  const clean = query.trim();
  if (!clean) return null;

  // Exact ID
  let row = db.prepare('SELECT * FROM guilds WHERE id = ?').get(clean);
  if (row) return row;

  // Exact name
  row = db.prepare('SELECT * FROM guilds WHERE LOWER(name) = LOWER(?) LIMIT 1').get(clean);
  if (row) return row;

  // Substring
  return db.prepare('SELECT * FROM guilds WHERE LOWER(name) LIKE LOWER(?) LIMIT 1').get(`%${clean}%`) || null;
}

/**
 * Finds best channel match in guild by name/id (strips leading #; exact first, then substring).
 * @param {DatabaseSync} db
 * @param {string} guildId
 * @param {string} query
 * @returns {Object|null}
 */
export function findChannel(db, guildId, query) {
  if (!query) return null;
  const clean = query.replace(/^#/, '').trim();
  if (!clean) return null;

  // Exact ID
  let row = db.prepare('SELECT * FROM channels WHERE guild_id = ? AND id = ?').get(String(guildId), clean);
  if (row) return row;

  // Exact name
  row = db.prepare('SELECT * FROM channels WHERE guild_id = ? AND LOWER(name) = LOWER(?) LIMIT 1').get(String(guildId), clean);
  if (row) return row;

  // Substring
  return db.prepare('SELECT * FROM channels WHERE guild_id = ? AND LOWER(name) LIKE LOWER(?) LIMIT 1').get(String(guildId), `%${clean}%`) || null;
}

/**
 * Gets cache row counts and fts mode.
 * @param {DatabaseSync} db
 * @returns {{users: number, guilds: number, channels: number, dm_channels: number, messages: number, fts_mode: string}}
 */
export function stats(db) {
  const getCount = (table) => {
    try {
      const res = db.prepare(`SELECT COUNT(*) AS c FROM ${table}`).get();
      return res ? Number(res.c) : 0;
    } catch {
      return 0;
    }
  };

  const metaMode = db.prepare("SELECT value FROM meta WHERE key = 'fts_mode'").get();

  return {
    users: getCount('users'),
    guilds: getCount('guilds'),
    channels: getCount('channels'),
    dm_channels: getCount('dm_channels'),
    messages: getCount('messages'),
    fts_mode: metaMode ? metaMode.value : 'unknown'
  };
}
