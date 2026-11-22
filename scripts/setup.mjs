#!/usr/bin/env node
/**
 * @fileoverview Interactive secure setup & check script for discord-context-mcp.
 */

import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import readline from 'node:readline';
import { execFileSync, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const projectRoot = path.resolve(__dirname, '..');
const srcIndexPath = path.join(projectRoot, 'src', 'index.js');

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

const TOOLS = [
  'list_servers',
  'list_channels',
  'list_dm_people',
  'read_dm_with',
  'read_channel',
  'search_messages',
  'cache_stats',
  'refresh'
];

/**
 * Verify token with Discord API @me endpoint without printing or leaking the token.
 */
async function verifyToken(token) {
  if (!token || !token.trim()) {
    throw new Error('Token is empty.');
  }

  const res = await fetch('https://discord.com/api/v10/users/@me', {
    method: 'GET',
    headers: {
      Authorization: token.trim(),
      'User-Agent': BROWSER_USER_AGENT,
      'X-Super-Properties': SUPER_PROPERTIES_B64,
      Accept: '*/*'
    }
  });

  if (!res.ok) {
    let errDetail = '';
    try {
      const errJson = await res.json();
      errDetail = errJson.message ? `: ${errJson.message}` : '';
    } catch {
      // ignore JSON parse error
    }
    throw new Error(`Discord API returned status ${res.status} (${res.statusText})${errDetail}`);
  }

  const user = await res.json();
  if (!user || !user.id || !user.username) {
    throw new Error('Discord API returned an unexpected user payload');
  }

  return {
    id: user.id,
    username: user.username,
    global_name: user.global_name || null
  };
}

/**
 * Securely prompts for input with masked characters.
 */
async function promptHidden(questionText) {
  return new Promise((resolve) => {
    if (!process.stdin.isTTY) {
      process.stdout.write(questionText);
      const rl = readline.createInterface({
        input: process.stdin,
        output: process.stdout
      });
      rl.question('', (answer) => {
        rl.close();
        resolve(answer.trim());
      });
      return;
    }

    process.stdout.write(questionText);
    const stdin = process.stdin;
    const oldRawMode = stdin.isRaw;
    stdin.setRawMode(true);
    stdin.resume();
    stdin.setEncoding('utf8');

    let input = '';
    const onData = (chunk) => {
      for (const char of chunk) {
        if (char === '\r' || char === '\n') {
          stdin.setRawMode(oldRawMode);
          stdin.pause();
          stdin.removeListener('data', onData);
          process.stdout.write('\n');
          resolve(input.trim());
          return;
        } else if (char === '\u0003') { // Ctrl+C
          stdin.setRawMode(oldRawMode);
          stdin.pause();
          stdin.removeListener('data', onData);
          process.stdout.write('\n');
          process.exit(130);
        } else if (char === '\b' || char === '\x7f') { // Backspace
          if (input.length > 0) {
            input = input.slice(0, -1);
            process.stdout.write('\b \b');
          }
        } else {
          input += char;
          process.stdout.write('*');
        }
      }
    };

    stdin.on('data', onData);
  });
}

/**
 * Get SQLite cache stats if cache.db exists and is readable.
 */
async function getCacheStats(dataDir) {
  const dbPath = path.join(dataDir, 'cache.db');
  if (!fs.existsSync(dbPath)) {
    return null;
  }
  try {
    const { openDb, stats } = await import('../src/db.js');
    const db = openDb(dataDir);
    const currentStats = stats(db);
    db.close();
    return currentStats;
  } catch (err) {
    return { error: err.message };
  }
}

/**
 * Check if discord-context is already registered in OpenCode config.
 */
function isOpenCodeRegistered() {
  try {
    const configPath = path.join(os.homedir(), '.config', 'opencode', 'opencode.json');
    if (fs.existsSync(configPath)) {
      const raw = fs.readFileSync(configPath, 'utf8');
      const parsed = JSON.parse(raw);
      if (parsed?.mcp?.servers && (parsed.mcp.servers['discord-context'] || parsed.mcp.servers['discord-context-mcp'])) {
        return true;
      }
    }
  } catch {
    // ignore
  }
  return false;
}

/**
 * Register with OpenCode using `opencode mcp add`.
 */
function registerWithOpenCode(dataDir) {
  if (isOpenCodeRegistered()) {
    console.log('[setup] OpenCode: already registered in opencode.json');
    return;
  }

  const args = [
    'mcp',
    'add',
    '--global',
    'discord-context',
    '--env',
    `DISCORD_CONTEXT_DATA_DIR=${dataDir}`,
    '--',
    'node',
    srcIndexPath
  ];

  try {
    const result = spawnSync('opencode', args, { stdio: 'inherit' });
    if (result.error) {
      throw result.error;
    }
    if (result.status === 0) {
      console.log('[setup] Successfully registered discord-context with OpenCode.');
    } else {
      console.log(`[setup] OpenCode CLI exited with code ${result.status}.`);
      printManualRegistration(dataDir);
    }
  } catch {
    console.log('[setup] Notice: "opencode" command not found in PATH.');
    printManualRegistration(dataDir);
  }
}

function printManualRegistration(dataDir) {
  console.log('\nTo register manually with OpenCode, run:');
  console.log(`  opencode mcp add --global discord-context --env DISCORD_CONTEXT_DATA_DIR="${dataDir}" -- node "${srcIndexPath}"`);
  console.log('\nOr add to ~/.config/opencode/opencode.json:');
  const sample = {
    mcp: {
      servers: {
        'discord-context': {
          type: 'local',
          command: ['node', srcIndexPath.replace(/\\/g, '/')],
          environment: {
            DISCORD_CONTEXT_DATA_DIR: dataDir.replace(/\\/g, '/')
          }
        }
      }
    }
  };
  console.log(JSON.stringify(sample, null, 2));
}

async function main() {
  const isCheckMode = process.argv.includes('--check');
  const dataDir = process.env.DISCORD_CONTEXT_DATA_DIR || path.join(os.homedir(), '.config', 'discord-context-mcp');
  const tokenFilePath = path.join(dataDir, 'token.txt');

  if (isCheckMode) {
    console.log('=== Checking Discord Context MCP Server Setup ===');
    let token = (process.env.DISCORD_TOKEN || '').trim();
    let source = 'DISCORD_TOKEN environment variable';

    if (!token && fs.existsSync(tokenFilePath)) {
      try {
        token = fs.readFileSync(tokenFilePath, 'utf8').trim();
        source = tokenFilePath;
      } catch (err) {
        console.error(`[check] Error reading token file: ${err.message}`);
        process.exit(1);
      }
    }

    if (!token) {
      console.error('[check] Error: No token found. Set DISCORD_TOKEN or run "npm run setup" to create token.txt');
      process.exit(1);
    }

    console.log(`[check] Token source: ${source}`);
    console.log('[check] Verifying token with Discord API...');
    try {
      const user = await verifyToken(token);
      console.log(`[check] Token valid! Authenticated as: ${user.username} (${user.id})`);
    } catch (err) {
      console.error(`[check] Token verification failed: ${err.message}`);
      process.exit(1);
    }

    const cacheStats = await getCacheStats(dataDir);
    if (cacheStats) {
      if (cacheStats.error) {
        console.log(`[check] SQLite cache: ${cacheStats.error}`);
      } else {
        console.log(`[check] SQLite cache (${dataDir}):`);
        console.log(`  - Users: ${cacheStats.users}`);
        console.log(`  - Guilds: ${cacheStats.guilds}`);
        console.log(`  - Channels: ${cacheStats.channels}`);
        console.log(`  - DM Channels: ${cacheStats.dm_channels}`);
        console.log(`  - Messages: ${cacheStats.messages}`);
        console.log(`  - FTS Mode: ${cacheStats.fts_mode}`);
      }
    } else {
      console.log(`[check] SQLite cache: not yet initialized (${dataDir})`);
    }

    const registered = isOpenCodeRegistered();
    console.log(`[check] OpenCode registration: ${registered ? 'Configured' : 'Not detected in opencode.json'}`);
    console.log('[check] Status: All checks passed!');
    process.exit(0);
  }

  // Interactive setup mode
  console.log('=== Discord Context MCP Server Setup ===');

  let token = (process.env.DISCORD_TOKEN || '').trim();
  if (token) {
    console.log('[setup] Found DISCORD_TOKEN in environment.');
  } else {
    if (!process.stdin.isTTY) {
      console.log('[setup] Warning: Non-interactive terminal detected without DISCORD_TOKEN.');
      console.log('         For non-interactive setups, please set DISCORD_TOKEN before running setup.');
    }
    token = await promptHidden('Enter your Discord User Token (input will be hidden): ');
  }

  token = token.trim();
  if (!token) {
    console.error('[setup] Error: Token cannot be empty.');
    process.exit(1);
  }

  console.log('[setup] Verifying token with Discord API...');
  let authenticatedUser;
  try {
    authenticatedUser = await verifyToken(token);
    console.log(`[setup] Verified! Authenticated as: ${authenticatedUser.username} (${authenticatedUser.id})`);
  } catch (err) {
    console.error(`[setup] Token verification failed: ${err.message}`);
    process.exit(1);
  }

  // Ensure dataDir exists
  fs.mkdirSync(dataDir, { recursive: true });

  // Write token to token.txt with mode 0o600
  try {
    fs.writeFileSync(tokenFilePath, token + '\n', { mode: 0o600 });
    // On Windows, set ACL via icacls (ignore errors, never print token)
    if (process.platform === 'win32') {
      try {
        const username = os.userInfo().username;
        execFileSync('icacls', [tokenFilePath, '/inheritance:r', '/grant:r', `${username}:F`], {
          stdio: 'ignore'
        });
      } catch {
        // ignore Windows permission errors
      }
    }
    console.log(`[setup] Saved token securely to: ${tokenFilePath}`);
  } catch (err) {
    console.error(`[setup] Failed to save token file: ${err.message}`);
    process.exit(1);
  }

  // Register with OpenCode
  registerWithOpenCode(dataDir);

  // Print short success summary listing 8 tools
  console.log('\n=== Setup Complete ===');
  console.log('The Discord Context MCP Server is ready to use.');
  console.log('Available MCP Tools (8 tools):');
  for (const tool of TOOLS) {
    console.log(`  - ${tool}`);
  }
  console.log('\nNext steps:');
  console.log('  1. Restart your OpenCode / T3 Code session if running.');
  console.log('  2. Verify your setup anytime with: npm run check');
  console.log('  3. Start asking your assistant Discord questions!');
}

main().catch((err) => {
  console.error(`[setup] Unexpected error: ${err.message}`);
  process.exit(1);
});
