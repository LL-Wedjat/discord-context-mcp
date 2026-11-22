/**
 * @fileoverview End-to-end stdio MCP smoke test for discord-context-mcp.
 */

import { spawn } from 'node:child_process';
import os from 'node:os';
import path from 'node:path';
import fs from 'node:fs';
import readline from 'node:readline';
import assert from 'node:assert/strict';

// Create temp directory and seed DB
const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'discord-mcp-smoke-'));

async function setupDatabase() {
  process.env.DISCORD_CONTEXT_OFFLINE = '1';
  process.env.DISCORD_CONTEXT_DATA_DIR = tempDir;
  process.env.DISCORD_CONTEXT_GATEWAY = '0';

  const { openDb, upsertUser, upsertGuild, upsertChannel, upsertDmChannel, upsertMessage } = await import('../src/db.js');
  const db = openDb(tempDir);

  const user = { id: '7001', username: 'alice', global_name: 'Alice' };
  upsertUser(db, user);

  const guild = { id: '8001', name: 'Smoke Guild' };
  upsertGuild(db, guild);

  const channel = { id: '9001', guild_id: '8001', name: 'general', type: 0 };
  upsertChannel(db, channel);

  upsertDmChannel(db, '9901', user.id, 'Alice');

  upsertMessage(db, {
    id: '12345',
    channel_id: '9001',
    author_id: user.id,
    content: 'hello from smoke test message',
    created_at_ms: Date.now()
  });

  db.close();
}

async function run() {
  await setupDatabase();

  const scriptPath = path.resolve('src/index.js');
  const child = spawn(process.execPath, [scriptPath], {
    env: {
      ...process.env,
      DISCORD_CONTEXT_OFFLINE: '1',
      DISCORD_CONTEXT_DATA_DIR: tempDir,
      DISCORD_CONTEXT_GATEWAY: '0'
    },
    stdio: ['pipe', 'pipe', 'inherit']
  });

  const timeoutTimer = setTimeout(() => {
    console.error('Smoke test timed out after 15s');
    child.kill('SIGTERM');
    process.exit(1);
  }, 15000);

  const pendingRequests = new Map();
  let nextId = 1;

  const rl = readline.createInterface({ input: child.stdout });

  rl.on('line', (line) => {
    const trimmed = line.trim();
    if (!trimmed.startsWith('{')) {
      // Skip non-JSON lines
      return;
    }
    let msg;
    try {
      msg = JSON.parse(trimmed);
    } catch {
      return;
    }

    if (msg.id !== undefined && pendingRequests.has(msg.id)) {
      const { resolve, reject } = pendingRequests.get(msg.id);
      pendingRequests.delete(msg.id);
      if (msg.error) {
        reject(new Error(`RPC error: ${JSON.stringify(msg.error)}`));
      } else {
        resolve(msg.result);
      }
    }
  });

  function sendRequest(method, params) {
    const id = nextId++;
    return new Promise((resolve, reject) => {
      pendingRequests.set(id, { resolve, reject });
      const payload = JSON.stringify({
        jsonrpc: '2.0',
        id,
        method,
        params
      });
      child.stdin.write(payload + '\n');
    });
  }

  function sendNotification(method, params) {
    const payload = JSON.stringify({
      jsonrpc: '2.0',
      method,
      params
    });
    child.stdin.write(payload + '\n');
  }

  try {
    // 1. initialize
    const initResult = await sendRequest('initialize', {
      protocolVersion: '2025-06-18',
      capabilities: {},
      clientInfo: {
        name: 'smoke-test-client',
        version: '1.0.0'
      }
    });
    assert.ok(initResult, 'initialize should succeed');
    console.log('PASS: initialize');

    sendNotification('notifications/initialized');

    // 2. tools/list
    const toolsList = await sendRequest('tools/list', {});
    assert.ok(toolsList && Array.isArray(toolsList.tools), 'tools/list returned tool list');
    const toolNames = toolsList.tools.map((t) => t.name);

    const expectedTools = [
      'list_servers',
      'list_channels',
      'list_dm_people',
      'read_dm_with',
      'read_channel',
      'search_messages',
      'cache_stats',
      'refresh'
    ];

    for (const expected of expectedTools) {
      assert.ok(toolNames.includes(expected), `Tool ${expected} should be registered`);
    }
    assert.equal(toolNames.length, 8, 'Exactly 8 tools should be registered');
    console.log('PASS: tools/list includes all 8 tools');

    // 3. tools/call cache_stats
    const statsCall = await sendRequest('tools/call', {
      name: 'cache_stats',
      arguments: {}
    });
    assert.ok(statsCall && Array.isArray(statsCall.content), 'cache_stats call returned content');
    const statsText = statsCall.content[0].text;
    const parsedStats = JSON.parse(statsText);
    assert.equal(parsedStats.messages, 1, 'Stats should show 1 message');
    assert.equal(parsedStats.guilds, 1, 'Stats should show 1 guild');
    console.log('PASS: tools/call cache_stats');

    // 4. tools/call search_messages with seeded text
    const searchCall = await sendRequest('tools/call', {
      name: 'search_messages',
      arguments: {
        text: 'smoke test message'
      }
    });
    assert.ok(searchCall && Array.isArray(searchCall.content), 'search_messages returned content');
    const searchText = searchCall.content[0].text;
    assert.ok(searchText.includes('smoke test message'), 'Search output must contain seeded message content');
    assert.ok(searchText.includes('Alice'), 'Search output must contain author Alice');
    console.log('PASS: tools/call search_messages');

    console.log('SMOKE TEST PASSED');
  } finally {
    clearTimeout(timeoutTimer);
    child.kill('SIGTERM');
    try {
      fs.rmSync(tempDir, { recursive: true, force: true });
    } catch {
      // ignore
    }
  }
}

run().catch((err) => {
  console.error('SMOKE TEST FAILED:', err);
  process.exit(1);
});
