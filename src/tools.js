/**
 * @fileoverview MCP Tool Registrations for discord-context-mcp.
 */

import { z } from 'zod';
import {
  formatMessages,
  formatItems,
  textResult,
  jsonResult
} from './format.js';
import {
  listGuilds,
  listChannels,
  listDmPeople,
  findUser,
  findGuild,
  findChannel,
  getMessages,
  searchMessages,
  stats
} from './db.js';

/**
 * Ensures the Discord client has identified its own profile.
 * @param {Object} ctx
 */
async function ensureIdentified(ctx) {
  if (!ctx.client.me) {
    await ctx.client.init();
  }
}

/**
 * Registers all 8 Discord MCP tools on the given McpServer instance.
 *
 * @param {import('@modelcontextprotocol/sdk/server/mcp.js').McpServer} server
 * @param {{
 *   config: Object,
 *   db: import('node:sqlite').DatabaseSync,
 *   client: import('./discord.js').DiscordClient
 * }} ctx
 */
export function registerTools(server, ctx) {
  const { db, client } = ctx;

  // 1. list_servers
  server.registerTool('list_servers', {
    title: 'List Discord Servers',
    description: 'Lists all Discord servers (guilds) the user belongs to. Useful when deciding which server to read or search. Example prompt: "List my Discord servers" or "Which servers am I in?".',
    inputSchema: {
      query: z.string().optional().describe('Optional server name substring filter')
    }
  }, async (args) => {
    try {
      let cached = listGuilds(db, args?.query);
      if (cached.length === 0 && (!args?.query || !args.query.trim())) {
        try {
          await client.listGuilds();
          cached = listGuilds(db, args?.query);
        } catch {
          // ignore error to return whatever is available
        }
      }

      const formatted = formatItems(cached, {
        title: 'Servers',
        fields: ['id', 'name']
      });

      const fullText = `${formatted}\n\nHint: Use read_channel(server, channel, lastN) to read a channel.`;
      return textResult(fullText);
    } catch (err) {
      const msg = client.sanitizeError(err?.message || String(err));
      return {
        content: [{ type: 'text', text: `Error: ${msg}` }],
        isError: true
      };
    }
  });

  // 2. list_channels
  server.registerTool('list_channels', {
    title: 'List Server Channels',
    description: 'Lists channels in a Discord server by server name or server ID. Example prompt: "List channels in the OpenAI server" or "Show channels in server 123456789".',
    inputSchema: {
      server: z.string().describe('Server name or ID'),
      query: z.string().optional().describe('Optional channel name substring filter')
    }
  }, async (args) => {
    try {
      let guild = findGuild(db, args.server);
      if (!guild) {
        try {
          await client.listGuilds();
          guild = findGuild(db, args.server);
        } catch {
          // ignore
        }
      }

      if (!guild) {
        const available = listGuilds(db).map((g) => `- ${g.name} (${g.id})`).join('\n');
        return {
          content: [{
            type: 'text',
            text: `Error: Server "${args.server}" not found.\n\nAvailable servers in cache:\n${available || '(none)'}`
          }],
          isError: true
        };
      }

      let cachedChannels = listChannels(db, guild.id, args.query);
      if (cachedChannels.length === 0) {
        try {
          await client.listChannels(guild.id);
          cachedChannels = listChannels(db, guild.id, args.query);
        } catch {
          // ignore
        }
      }

      return textResult(formatItems(cachedChannels, {
        title: `Channels in ${guild.name}`,
        fields: ['id', 'name', 'type', 'topic']
      }));
    } catch (err) {
      const msg = client.sanitizeError(err?.message || String(err));
      return {
        content: [{ type: 'text', text: `Error: ${msg}` }],
        isError: true
      };
    }
  });

  // 3. list_dm_people
  server.registerTool('list_dm_people', {
    title: 'List DM Contacts',
    description: 'Lists DM contacts and recent direct message channels. Example prompt: "Who have I been direct messaging with?" or "Find the DM channel with Alex".',
    inputSchema: {
      query: z.string().optional().describe('Optional person name/username substring filter')
    }
  }, async (args) => {
    try {
      let cached = listDmPeople(db, args?.query);
      if (cached.length === 0) {
        try {
          await client.listDmChannels();
          cached = listDmPeople(db, args?.query);
        } catch {
          // ignore
        }
      }

      return textResult(formatItems(cached, {
        title: 'DM contacts',
        fields: ['userId', 'username', 'global_name', 'channelId']
      }));
    } catch (err) {
      const msg = client.sanitizeError(err?.message || String(err));
      return {
        content: [{ type: 'text', text: `Error: ${msg}` }],
        isError: true
      };
    }
  });

  // 4. read_dm_with
  server.registerTool('read_dm_with', {
    title: 'Read Direct Messages With Person',
    description: 'Fetches recent messages from a direct message conversation with a specific person. Example prompt: "Look at my Discord DMs with Theo and implement whatever he said" or "Read the last 30 DMs with @sarah".',
    inputSchema: {
      person: z.string().describe('Person name, username, or user ID'),
      lastN: z.number().min(1).max(100).optional().describe('Number of recent messages to retrieve (1-100, default 50)')
    }
  }, async (args) => {
    try {
      const clampedN = Math.min(Math.max(1, args.lastN ?? 50), 100);

      let user = findUser(db, args.person);
      if (!user) {
        try {
          await client.listDmChannels();
          user = findUser(db, args.person);
        } catch {
          // ignore
        }
      }

      if (!user) {
        return {
          content: [{
            type: 'text',
            text: `Error: Person "${args.person}" not found in cache. Try running list_dm_people first to see available contacts.`
          }],
          isError: true
        };
      }

      const dmRow = db.prepare('SELECT id FROM dm_channels WHERE user_id = ?').get(String(user.id));
      if (!dmRow || !dmRow.id) {
        return {
          content: [{
            type: 'text',
            text: `Error: No DM channel found with user "${user.username || user.global_name || args.person}" (${user.id}). Use list_dm_people to see existing conversations.`
          }],
          isError: true
        };
      }

      const dmChannelId = dmRow.id;
      try {
        await client.fetchMessages(dmChannelId, clampedN);
      } catch (fetchErr) {
        process.stderr.write(`[discord-tools] Warning: fetchMessages failed: ${client.sanitizeError(fetchErr.message)}\n`);
      }

      const rows = getMessages(db, { channelId: dmChannelId, limit: clampedN });
      const displayName = user.global_name || user.username || args.person;
      return textResult(formatMessages(rows, { title: `DM with ${displayName}` }));
    } catch (err) {
      const msg = client.sanitizeError(err?.message || String(err));
      return {
        content: [{ type: 'text', text: `Error: ${msg}` }],
        isError: true
      };
    }
  });

  // 5. read_channel
  server.registerTool('read_channel', {
    title: 'Read Server Channel Messages',
    description: 'Reads recent messages from a specific channel in a Discord server. Example prompt: "Read the last 50 messages in #general in DevServer" or "What happened in #announcements?".',
    inputSchema: {
      server: z.string().describe('Server name or ID'),
      channel: z.string().describe('Channel name (e.g. #general) or channel ID'),
      lastN: z.number().min(1).max(100).optional().describe('Number of recent messages to retrieve (1-100, default 50)')
    }
  }, async (args) => {
    try {
      const clampedN = Math.min(Math.max(1, args.lastN ?? 50), 100);

      let guild = findGuild(db, args.server);
      if (!guild) {
        try {
          await client.listGuilds();
          guild = findGuild(db, args.server);
        } catch {
          // ignore
        }
      }

      if (!guild) {
        const available = listGuilds(db).map((g) => `- ${g.name} (${g.id})`).join('\n');
        return {
          content: [{
            type: 'text',
            text: `Error: Server "${args.server}" not found.\n\nAvailable servers:\n${available || '(none)'}`
          }],
          isError: true
        };
      }

      let ch = findChannel(db, guild.id, args.channel);
      if (!ch) {
        try {
          await client.listChannels(guild.id);
          ch = findChannel(db, guild.id, args.channel);
        } catch {
          // ignore
        }
      }

      if (!ch) {
        const availableChannels = listChannels(db, guild.id).map((c) => `- #${c.name} (${c.id})`).join('\n');
        return {
          content: [{
            type: 'text',
            text: `Error: Channel "${args.channel}" not found in server "${guild.name}".\n\nAvailable channels:\n${availableChannels || '(none)'}`
          }],
          isError: true
        };
      }

      try {
        await client.fetchMessages(ch.id, clampedN);
      } catch (fetchErr) {
        process.stderr.write(`[discord-tools] Warning: fetchMessages failed: ${client.sanitizeError(fetchErr.message)}\n`);
      }

      const rows = getMessages(db, { channelId: ch.id, limit: clampedN });
      return textResult(formatMessages(rows, { title: `#${ch.name} in ${guild.name}` }));
    } catch (err) {
      const msg = client.sanitizeError(err?.message || String(err));
      return {
        content: [{ type: 'text', text: `Error: ${msg}` }],
        isError: true
      };
    }
  });

  // 6. search_messages
  server.registerTool('search_messages', {
    title: 'Search Messages',
    description: 'Searches message text across channels or direct messages with optional filters. Example prompt: "Search Discord for deployment errors" or "Find messages about redis in #dev".',
    inputSchema: {
      text: z.string().describe('Search query text or phrase'),
      server: z.string().optional().describe('Optional server name or ID filter'),
      channel: z.string().optional().describe('Optional channel name (e.g. #general) or channel ID filter'),
      person: z.string().optional().describe('Optional person username or ID filter'),
      limit: z.number().min(1).max(100).optional().describe('Maximum number of results to return (default 30, max 100)')
    }
  }, async (args) => {
    try {
      const clampedLimit = Math.min(Math.max(1, args.limit ?? 30), 100);

      // If messages table is empty, try initial sync
      const msgCount = stats(db).messages;
      if (msgCount === 0) {
        try {
          await client.syncRecent({ guildLimit: 2, channelsPerGuild: 8, dmLimit: 15, perChannel: 100 });
        } catch {
          // ignore
        }
      }

      let resolvedGuildId = undefined;
      if (args.server) {
        let guild = findGuild(db, args.server);
        if (!guild) {
          try {
            await client.listGuilds();
            guild = findGuild(db, args.server);
          } catch {
            // ignore
          }
        }
        if (guild) {
          resolvedGuildId = guild.id;
        }
      }

      let resolvedChannelId = undefined;
      if (args.channel) {
        if (resolvedGuildId) {
          let ch = findChannel(db, resolvedGuildId, args.channel);
          if (!ch) {
            try {
              await client.listChannels(resolvedGuildId);
              ch = findChannel(db, resolvedGuildId, args.channel);
            } catch {
              // ignore
            }
          }
          if (ch) {
            resolvedChannelId = ch.id;
          }
        } else {
          // Channel given without guild: search channel by ID or exact/substring name across all channels
          const cleanName = args.channel.replace(/^#/, '').trim();
          let chRow = db.prepare('SELECT id FROM channels WHERE id = ?').get(cleanName);
          if (!chRow) {
            chRow = db.prepare('SELECT id FROM channels WHERE LOWER(name) = LOWER(?) LIMIT 1').get(cleanName);
          }
          if (!chRow) {
            chRow = db.prepare('SELECT id FROM channels WHERE LOWER(name) LIKE LOWER(?) LIMIT 1').get(`%${cleanName}%`);
          }
          if (chRow) {
            resolvedChannelId = chRow.id;
          }
        }
      }

      let resolvedUserId = undefined;
      if (args.person) {
        let user = findUser(db, args.person);
        if (!user) {
          try {
            await client.listDmChannels();
            user = findUser(db, args.person);
          } catch {
            // ignore
          }
        }
        if (user) {
          resolvedUserId = user.id;
        }
      }

      const rows = searchMessages(db, args.text, {
        guildId: resolvedGuildId,
        channelId: resolvedChannelId,
        userId: resolvedUserId,
        limit: clampedLimit
      });

      return textResult(formatMessages(rows, { title: `Search: "${args.text}"` }));
    } catch (err) {
      const msg = client.sanitizeError(err?.message || String(err));
      return {
        content: [{ type: 'text', text: `Error: ${msg}` }],
        isError: true
      };
    }
  });

  // 7. cache_stats
  server.registerTool('cache_stats', {
    title: 'Cache Statistics',
    description: 'Returns cached database statistics including count of users, guilds, channels, DM channels, and messages. Example prompt: "Check Discord cache status".',
    inputSchema: {}
  }, async () => {
    try {
      return jsonResult(stats(db));
    } catch (err) {
      const msg = client.sanitizeError(err?.message || String(err));
      return {
        content: [{ type: 'text', text: `Error: ${msg}` }],
        isError: true
      };
    }
  });

  // 8. refresh
  server.registerTool('refresh', {
    title: 'Refresh Discord Cache',
    description: 'Backfills or refreshes Discord data into local SQLite cache. Example prompt: "Refresh Discord cache for DMs" or "Sync recent Discord messages".',
    inputSchema: {
      scope: z.enum(['all', 'servers', 'dms']).optional().describe('Sync scope: "all", "servers", or "dms" (default "all")'),
      perChannel: z.number().min(1).max(100).optional().describe('Number of messages to fetch per channel (default 100, max 100)')
    }
  }, async (args) => {
    try {
      await ensureIdentified(ctx);
      const scope = args?.scope || 'all';
      const clamped = Math.min(Math.max(1, args?.perChannel ?? 100), 100);

      let summary = '';
      if (scope === 'servers') {
        const guilds = await client.listGuilds();
        let channelsCount = 0;
        const targetGuilds = (guilds || []).slice(0, 5);
        for (const g of targetGuilds) {
          try {
            const chs = await client.listChannels(g.id);
            channelsCount += (chs ? chs.length : 0);
          } catch {
            // ignore channel error
          }
        }
        summary = `Refreshed ${targetGuilds.length} servers and cached ${channelsCount} channels.`;
      } else if (scope === 'dms') {
        const dms = await client.listDmChannels();
        summary = `Refreshed ${dms ? dms.length : 0} DM conversations.`;
      } else {
        const syncRes = await client.syncRecent({
          guildLimit: 3,
          channelsPerGuild: 10,
          dmLimit: 20,
          perChannel: clamped
        });
        summary = `Full sync completed: ${syncRes.fetched} channels checked, ${syncRes.messages} messages fetched. ${syncRes.errors.length > 0 ? `(${syncRes.errors.length} non-fatal errors)` : ''}`;
      }

      const currentStats = stats(db);
      return jsonResult({
        summary,
        scope,
        stats: currentStats
      });
    } catch (err) {
      const msg = client.sanitizeError(err?.message || String(err));
      return {
        content: [{ type: 'text', text: `Error: ${msg}` }],
        isError: true
      };
    }
  });
}
