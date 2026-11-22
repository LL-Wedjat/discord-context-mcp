/**
 * @fileoverview Discord REST client and Gateway receiver for discord-context-mcp.
 * Implements read-only REST calls with rate-limiting and a real-time event cache via WebSocket.
 */

import WebSocket from 'ws';
import {
  upsertUser,
  upsertGuild,
  upsertChannel,
  upsertDmChannel,
  upsertMessage,
  pruneGuildsNotIn
} from './db.js';

// Realistic browser user agent and super properties
const BROWSER_USER_AGENT = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36';

const SUPER_PROPERTIES_JSON = JSON.stringify({
  os: 'Windows',
  browser: 'Chrome',
  device: '',
  system_locale: 'en-US',
  browser_user_agent: BROWSER_USER_AGENT,
  browser_version: '131.0.0.0',
  os_version: '10',
  referrer: '',
  referring_domain: '',
  referrer_current: '',
  referring_domain_current: '',
  release_channel: 'stable',
  client_build_number: 351234,
  client_event_source: null
});
const SUPER_PROPERTIES_B64 = Buffer.from(SUPER_PROPERTIES_JSON).toString('base64');

export class DiscordClient {
  /**
   * @param {Object} config
   * @param {import('node:sqlite').DatabaseSync} db
   */
  constructor(config, db) {
    this.config = config;
    this.db = db;
    this.me = null;

    // Rate limiting queue state
    this.requestQueues = new Map(); // bucket or route -> Promise chain
    this.globalRateLimitReset = 0;
    this.routeBuckets = new Map(); // routeKey -> bucketId
    this.bucketRemaining = new Map(); // bucketId -> number
    this.bucketResetTime = new Map(); // bucketId -> timestamp ms

    // Gateway state
    this.gatewayWs = null;
    this.heartbeatTimer = null;
    this.reconnectTimer = null;
    this.lastSequence = null;
    this.sessionId = null;
    this.resumeGatewayUrl = null;
    this.lastIdentifyTime = 0;
    this.isClosedManually = false;
  }

  /**
   * Validates token and fetches self profile.
   * Throws clear error if token is missing or invalid.
   */
  async init() {
    if (!this.config.token || !this.config.token.trim()) {
      throw new Error('Discord token missing: Please set DISCORD_TOKEN or configure DISCORD_TOKEN_FILE');
    }

    try {
      const user = await this.rest('GET', '/users/@me');
      if (!user || !user.id) {
        throw new Error('Failed to fetch self user profile: empty response');
      }

      this.me = {
        id: user.id,
        username: user.username,
        global_name: user.global_name || null
      };

      upsertUser(this.db, user);
      return this.me;
    } catch (err) {
      const sanitized = this.sanitizeError(err.message || String(err));
      throw new Error(`Failed to initialize Discord client: ${sanitized}`);
    }
  }

  /**
   * Sanitizes error message so token never leaks.
   * @param {string} msg
   * @returns {string}
   */
  sanitizeError(msg) {
    if (!msg) return '';
    let result = String(msg);
    if (this.config.token) {
      result = result.replaceAll(this.config.token, '[REDACTED_TOKEN]');
    }
    return result;
  }

  /**
   * Makes a rate-limited REST API call against Discord API.
   * @param {'GET'|'POST'|'PUT'|'PATCH'|'DELETE'} method
   * @param {string} apiPath
   * @param {Object} [options={}]
   * @param {Object} [options.query]
   * @param {Object} [options.body]
   * @param {string} [options.route]
   * @returns {Promise<any>}
   */
  async rest(method, apiPath, { query, body, route } = {}) {
    const routeKey = route || `${method} ${apiPath.replace(/\/\d+/g, '/:id')}`;

    // Queue request to honor rate limits sequentially per route
    const currentQueue = this.requestQueues.get(routeKey) || Promise.resolve();
    const executeCall = currentQueue.then(async () => {
      return this._executeRestWithRetry(method, apiPath, { query, body, routeKey });
    });

    // Catch errors on the queue tail so it doesn't break subsequent calls
    this.requestQueues.set(routeKey, executeCall.catch(() => {}));
    return executeCall;
  }

  async _executeRestWithRetry(method, apiPath, { query, body, routeKey }, retries = 0) {
    if (retries > 5) {
      throw new Error(`Exceeded maximum retries (5) for request ${method} ${apiPath}`);
    }

    // Check global rate limit
    const now = Date.now();
    if (this.globalRateLimitReset > now) {
      const delay = this.globalRateLimitReset - now;
      await new Promise((resolve) => setTimeout(resolve, delay));
    }

    // Check bucket rate limit
    const bucketId = this.routeBuckets.get(routeKey);
    if (bucketId) {
      const remaining = this.bucketRemaining.get(bucketId);
      const resetTime = this.bucketResetTime.get(bucketId) || 0;
      if (remaining !== undefined && remaining <= 0 && resetTime > Date.now()) {
        const waitMs = resetTime - Date.now();
        await new Promise((resolve) => setTimeout(resolve, waitMs));
      }
    }

    let url = `${this.config.restBase}${apiPath}`;
    if (query) {
      const params = new URLSearchParams();
      for (const [k, v] of Object.entries(query)) {
        if (v !== undefined && v !== null) {
          params.append(k, String(v));
        }
      }
      const qs = params.toString();
      if (qs) {
        url += (url.includes('?') ? '&' : '?') + qs;
      }
    }

    const headers = {
      Authorization: this.config.token,
      'User-Agent': BROWSER_USER_AGENT,
      'X-Super-Properties': SUPER_PROPERTIES_B64,
      Accept: '*/*'
    };

    let bodyStr = null;
    if (body) {
      headers['Content-Type'] = 'application/json';
      bodyStr = JSON.stringify(body);
    }

    let res;
    try {
      res = await fetch(url, {
        method,
        headers,
        body: bodyStr
      });
    } catch (networkErr) {
      throw new Error(`Network error calling Discord: ${this.sanitizeError(networkErr.message)}`);
    }

    // Update rate limit state from headers
    const resBucket = res.headers.get('x-ratelimit-bucket');
    const resRemaining = res.headers.get('x-ratelimit-remaining');
    const resResetAfter = res.headers.get('x-ratelimit-reset-after');
    const isGlobal = res.headers.get('x-ratelimit-global');

    if (resBucket) {
      this.routeBuckets.set(routeKey, resBucket);
      if (resRemaining !== null) {
        this.bucketRemaining.set(resBucket, Number(resRemaining));
      }
      if (resResetAfter !== null) {
        this.bucketResetTime.set(resBucket, Date.now() + Math.ceil(Number(resResetAfter) * 1000));
      }
    }

    if (res.status === 429) {
      const retryAfterHeader = res.headers.get('retry-after');
      let retryAfterMs = 1000;
      try {
        const errJson = await res.json();
        if (errJson && errJson.retry_after) {
          retryAfterMs = Math.ceil(Number(errJson.retry_after) * 1000);
        }
      } catch {
        if (retryAfterHeader) {
          retryAfterMs = Math.ceil(Number(retryAfterHeader) * 1000);
        }
      }

      if (isGlobal) {
        this.globalRateLimitReset = Date.now() + retryAfterMs;
      }

      await new Promise((r) => setTimeout(r, retryAfterMs));
      return this._executeRestWithRetry(method, apiPath, { query, body, routeKey }, retries + 1);
    }

    if (!res.ok) {
      let errText = '';
      try {
        errText = await res.text();
      } catch {
        // ignore
      }
      const sanitized = this.sanitizeError(`Discord API ${res.status} ${res.statusText}: ${errText}`);
      throw new Error(sanitized);
    }

    if (res.status === 204) {
      return null;
    }

    return await res.json();
  }

  /**
   * Fetches guilds for current user and updates cache.
   * @returns {Promise<Array<Object>>}
   */
  async listGuilds() {
    const guilds = await this.rest('GET', '/users/@me/guilds');
    if (Array.isArray(guilds) && guilds.length > 0) {
      for (const g of guilds) {
        upsertGuild(this.db, g);
      }
      pruneGuildsNotIn(this.db, guilds.map((g) => g.id));
    }
    return guilds || [];
  }

  /**
   * Fetches channels for a guild and updates cache.
   * @param {string} guildId
   * @returns {Promise<Array<Object>>}
   */
  async listChannels(guildId) {
    const channels = await this.rest('GET', `/guilds/${guildId}/channels`);
    if (Array.isArray(channels)) {
      for (const c of channels) {
        upsertChannel(this.db, c);
      }
    }
    return channels || [];
  }

  /**
   * Fetches user DM channels and updates cache.
   * @returns {Promise<Array<Object>>}
   */
  async listDmChannels() {
    const dms = await this.rest('GET', '/users/@me/channels');
    if (Array.isArray(dms)) {
      for (const dm of dms) {
        // type 1 = DM, type 3 = GROUP_DM
        if (dm.type === 1) {
          const recipient = dm.recipients && dm.recipients[0];
          if (recipient) {
            upsertUser(this.db, recipient);
            upsertDmChannel(this.db, dm.id, recipient.id, recipient.username);
          }
        } else if (dm.type === 3) {
          if (Array.isArray(dm.recipients)) {
            for (const r of dm.recipients) {
              upsertUser(this.db, r);
            }
          }
          upsertDmChannel(this.db, dm.id, null, dm.name || 'Group DM');
        }
      }
    }
    return dms || [];
  }

  /**
   * Fetches up to 100 messages for a channel and updates cache.
   * @param {string} channelId
   * @param {number} [limit=100]
   * @param {string} [before]
   * @returns {Promise<Array<Object>>}
   */
  async fetchMessages(channelId, limit = 100, before) {
    const safeLimit = Math.min(Math.max(1, limit), 100);
    const query = { limit: safeLimit };
    if (before) query.before = before;

    const messages = await this.rest('GET', `/channels/${channelId}/messages`, { query });
    if (Array.isArray(messages)) {
      for (const m of messages) {
        if (m.author) {
          upsertUser(this.db, m.author);
        }
        upsertMessage(this.db, {
          id: m.id,
          channel_id: m.channel_id,
          author_id: m.author?.id,
          content: m.content,
          timestamp: m.timestamp,
          edited_timestamp: m.edited_timestamp,
          attachments_json: m.attachments,
          embeds_json: m.embeds,
          reply_to: m.referenced_message?.id,
          created_at_ms: m.timestamp ? Date.parse(m.timestamp) : Date.now()
        });
      }
    }
    return messages || [];
  }

  /**
   * Best-effort sync for recent guilds, DMs, and their recent messages.
   * @param {Object} [options={}]
   * @returns {Promise<{ ok: boolean, errors: Array<string>, fetched: number, messages: number }>}
   */
  async syncRecent({ guildLimit = 3, channelsPerGuild = 10, dmLimit = 20, perChannel = 100 } = {}) {
    const errors = [];
    let fetchedChannels = 0;
    let totalMessages = 0;

    let guilds = [];
    try {
      guilds = await this.listGuilds();
    } catch (err) {
      errors.push(`Guilds list error: ${this.sanitizeError(err.message)}`);
    }

    let dms = [];
    try {
      dms = await this.listDmChannels();
    } catch (err) {
      errors.push(`DM list error: ${this.sanitizeError(err.message)}`);
    }

    // Sync DMs
    const targetDms = (dms || []).slice(0, dmLimit);
    for (const dm of targetDms) {
      try {
        const msgs = await this.fetchMessages(dm.id, perChannel);
        fetchedChannels++;
        totalMessages += (msgs ? msgs.length : 0);
      } catch (err) {
        errors.push(`DM ${dm.id} fetch error: ${this.sanitizeError(err.message)}`);
      }
    }

    // Sync Guilds
    const targetGuilds = (guilds || []).slice(0, guildLimit);
    for (const g of targetGuilds) {
      let channels = [];
      try {
        channels = await this.listChannels(g.id);
      } catch (err) {
        errors.push(`Guild ${g.id} channels error: ${this.sanitizeError(err.message)}`);
        continue;
      }

      // Filter text channels (type 0 = GUILD_TEXT, 5 = GUILD_ANNOUNCEMENT)
      const textChannels = (channels || [])
        .filter((c) => c.type === 0 || c.type === 5)
        .slice(0, channelsPerGuild);

      for (const ch of textChannels) {
        try {
          const msgs = await this.fetchMessages(ch.id, perChannel);
          fetchedChannels++;
          totalMessages += (msgs ? msgs.length : 0);
        } catch (err) {
          errors.push(`Channel ${ch.id} fetch error: ${this.sanitizeError(err.message)}`);
        }
      }
    }

    return {
      ok: errors.length === 0,
      errors,
      fetched: fetchedChannels,
      messages: totalMessages
    };
  }

  /**
   * Connects to Discord Gateway WebSocket and maintains real-time sync.
   */
  startGateway() {
    if (!this.config.gatewayEnabled) {
      return;
    }
    if (this.gatewayWs && this.gatewayWs.readyState === WebSocket.OPEN) {
      return;
    }

    this.isClosedManually = false;
    this._connectGateway();
  }

  _connectGateway() {
    if (this.isClosedManually) return;

    const gatewayUrl = this.resumeGatewayUrl || 'wss://gateway.discord.gg/?v=10&encoding=json';
    try {
      this.gatewayWs = new WebSocket(gatewayUrl);
    } catch (err) {
      process.stderr.write(`[discord-gateway] Connection creation error: ${this.sanitizeError(err.message)}\n`);
      this._scheduleReconnect(5000);
      return;
    }

    this.gatewayWs.on('open', () => {
      if (this.config.gatewayLog) {
        process.stderr.write('[discord-gateway] Connected to Gateway\n');
      }
    });

    this.gatewayWs.on('message', (data) => {
      this._handleGatewayMessage(data);
    });

    this.gatewayWs.on('close', (code, reason) => {
      this._cleanupHeartbeat();
      if (this.config.gatewayLog) {
        process.stderr.write(`[discord-gateway] Disconnected (code: ${code}, reason: ${reason})\n`);
      }
      // Reconnect if not closed manually
      if (!this.isClosedManually) {
        // Re-identify if invalidated session or reconnect
        const delay = code === 4004 ? 0 : 5000;
        if (code === 4004) {
          process.stderr.write('[discord-gateway] Authentication failed (code 4004). Token is invalid.\n');
          return;
        }
        this._scheduleReconnect(delay);
      }
    });

    this.gatewayWs.on('error', (err) => {
      process.stderr.write(`[discord-gateway] WebSocket error: ${this.sanitizeError(err.message)}\n`);
    });
  }

  _handleGatewayMessage(raw) {
    let payload;
    try {
      payload = JSON.parse(raw.toString());
    } catch {
      return;
    }

    const { op, d, s, t } = payload;
    if (s !== null && s !== undefined) {
      this.lastSequence = s;
    }

    switch (op) {
      case 10: { // HELLO
        const heartbeatInterval = d.heartbeat_interval;
        this._startHeartbeat(heartbeatInterval);
        this._identifyOrResume();
        break;
      }
      case 1: { // Heartbeat requested
        this._sendHeartbeat();
        break;
      }
      case 11: { // Heartbeat ACK
        break;
      }
      case 7: { // RECONNECT
        if (this.config.gatewayLog) {
          process.stderr.write('[discord-gateway] Reconnect requested by Discord\n');
        }
        this.gatewayWs?.close();
        break;
      }
      case 9: { // INVALID_SESSION
        const canResume = Boolean(d);
        if (this.config.gatewayLog) {
          process.stderr.write(`[discord-gateway] Invalid session (resumable: ${canResume})\n`);
        }
        if (!canResume) {
          this.sessionId = null;
          this.lastSequence = null;
          this.resumeGatewayUrl = null;
        }
        setTimeout(() => {
          this._identifyOrResume();
        }, 4000);
        break;
      }
      case 0: { // DISPATCH
        this._handleDispatch(t, d);
        break;
      }
      default:
        break;
    }
  }

  _handleDispatch(eventType, data) {
    if (!data) return;

    try {
      switch (eventType) {
        case 'READY': {
          this.sessionId = data.session_id;
          if (data.resume_gateway_url) {
            this.resumeGatewayUrl = `${data.resume_gateway_url}?v=10&encoding=json`;
          }
          if (data.user) {
            this.me = {
              id: data.user.id,
              username: data.user.username,
              global_name: data.user.global_name || null
            };
            upsertUser(this.db, data.user);
          }
          if (Array.isArray(data.guilds)) {
            for (const g of data.guilds) {
              if (g && typeof g.name === 'string' && g.name.trim() !== '') {
                upsertGuild(this.db, g);
              }
            }
          }
          if (Array.isArray(data.private_channels)) {
            for (const dm of data.private_channels) {
              if (dm.type === 1 && dm.recipients && dm.recipients[0]) {
                upsertUser(this.db, dm.recipients[0]);
                upsertDmChannel(this.db, dm.id, dm.recipients[0].id, dm.recipients[0].username);
              }
            }
          }
          if (Array.isArray(data.users)) {
            for (const u of data.users) {
              upsertUser(this.db, u);
            }
          }
          break;
        }

        case 'GUILD_CREATE':
        case 'GUILD_UPDATE': {
          upsertGuild(this.db, data);
          if (Array.isArray(data.channels)) {
            for (const c of data.channels) {
              c.guild_id = data.id;
              upsertChannel(this.db, c);
            }
          }
          break;
        }

        case 'CHANNEL_CREATE':
        case 'CHANNEL_UPDATE': {
          if (data.type === 1) { // DM
            const recipient = data.recipients && data.recipients[0];
            if (recipient) {
              upsertUser(this.db, recipient);
              upsertDmChannel(this.db, data.id, recipient.id, recipient.username);
            }
          } else {
            upsertChannel(this.db, data);
          }
          break;
        }

        case 'MESSAGE_CREATE':
        case 'MESSAGE_UPDATE': {
          if (data.author) {
            upsertUser(this.db, data.author);
          }
          upsertMessage(this.db, {
            id: data.id,
            channel_id: data.channel_id,
            author_id: data.author?.id,
            content: data.content,
            timestamp: data.timestamp,
            edited_timestamp: data.edited_timestamp,
            attachments_json: data.attachments,
            embeds_json: data.embeds,
            reply_to: data.referenced_message?.id,
            created_at_ms: data.timestamp ? Date.parse(data.timestamp) : Date.now()
          });
          break;
        }

        case 'MESSAGE_DELETE': {
          if (data.id) {
            this.db.prepare('DELETE FROM messages WHERE id = ?').run(String(data.id));
          }
          break;
        }

        case 'USER_UPDATE': {
          upsertUser(this.db, data);
          break;
        }

        default:
          break;
      }
    } catch (err) {
      process.stderr.write(`[discord-gateway] Error handling dispatch ${eventType}: ${this.sanitizeError(err.message)}\n`);
    }
  }

  _identifyOrResume() {
    if (!this.gatewayWs || this.gatewayWs.readyState !== WebSocket.OPEN) return;

    if (this.sessionId && this.lastSequence !== null) {
      // RESUME
      const resumePayload = {
        op: 6,
        d: {
          token: this.config.token,
          session_id: this.sessionId,
          seq: this.lastSequence
        }
      };
      this.gatewayWs.send(JSON.stringify(resumePayload));
      return;
    }

    // Rate-limit identify: 1 per 5s
    const now = Date.now();
    const wait = Math.max(0, 5000 - (now - this.lastIdentifyTime));
    setTimeout(() => {
      if (!this.gatewayWs || this.gatewayWs.readyState !== WebSocket.OPEN) return;
      this.lastIdentifyTime = Date.now();

      const identifyPayload = {
        op: 2,
        d: {
          token: this.config.token,
          capabilities: 16381,
          properties: JSON.parse(SUPER_PROPERTIES_JSON),
          presence: {
            status: 'unknown',
            since: 0,
            activities: [],
            afk: false
          },
          compress: false,
          client_state: {
            guild_versions: {}
          }
        }
      };
      this.gatewayWs.send(JSON.stringify(identifyPayload));
    }, wait);
  }

  _startHeartbeat(intervalMs) {
    this._cleanupHeartbeat();
    this.heartbeatTimer = setInterval(() => {
      this._sendHeartbeat();
    }, intervalMs);
  }

  _sendHeartbeat() {
    if (this.gatewayWs && this.gatewayWs.readyState === WebSocket.OPEN) {
      this.gatewayWs.send(JSON.stringify({ op: 1, d: this.lastSequence }));
    }
  }

  _cleanupHeartbeat() {
    if (this.heartbeatTimer) {
      clearInterval(this.heartbeatTimer);
      this.heartbeatTimer = null;
    }
  }

  _scheduleReconnect(delayMs) {
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = setTimeout(() => {
      this._connectGateway();
    }, delayMs);
  }

  /**
   * Stops gateway connection and cancels heartbeats.
   */
  stop() {
    this.isClosedManually = true;
    this._cleanupHeartbeat();
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.gatewayWs) {
      try {
        this.gatewayWs.close();
      } catch {
        // ignore
      }
      this.gatewayWs = null;
    }
  }
}
