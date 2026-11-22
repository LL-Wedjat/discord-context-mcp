#!/usr/bin/env node
/**
 * @fileoverview Entry point for discord-context-mcp server.
 * Connects stdio MCP transport with SQLite cache and Discord client.
 */

import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { loadConfig } from './config.js';
import { openDb } from './db.js';
import { DiscordClient } from './discord.js';
import { registerTools } from './tools.js';

async function main() {
  const config = loadConfig();
  const db = openDb(config.dataDir);
  const client = new DiscordClient(config, db);

  const server = new McpServer({
    name: 'discord-context',
    version: '1.0.0'
  });

  registerTools(server, { config, db, client });

  // Best-effort client initialization
  if (!config.offline) {
    let initTimer;
    const timeout = new Promise((_, reject) => {
      initTimer = setTimeout(() => reject(new Error('init timed out')), 10000);
    });
    try {
      await Promise.race([client.init(), timeout]);
      try {
        await client.listGuilds();
      } catch {
        // ignore startup guild backfill errors
      }
    } catch (err) {
      process.stderr.write(`[discord-context] init failed: ${client.sanitizeError(err?.message || String(err))}\n`);
    } finally {
      clearTimeout(initTimer);
    }
  }

  // Best-effort gateway startup if enabled
  if (config.gatewayEnabled && !config.offline) {
    try {
      client.startGateway();
    } catch (err) {
      process.stderr.write(`[discord-context] gateway start failed: ${client.sanitizeError(err.message)}\n`);
    }
  }

  const transport = new StdioServerTransport();
  await server.connect(transport);
  process.stderr.write('[discord-context] ready (stdio)\n');

  const shutdown = () => {
    try {
      client.stop();
    } catch {
      // ignore
    }
    try {
      db.close();
    } catch {
      // ignore
    }
    process.exit(0);
  };

  process.on('SIGINT', shutdown);
  process.on('SIGTERM', shutdown);
}

main().catch((err) => {
  process.stderr.write(`[discord-context] fatal error: ${err?.message || String(err)}\n`);
  process.exit(1);
});
