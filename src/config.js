/**
 * @fileoverview Configuration loader for discord-context-mcp.
 * Manages directories, token resolution (from env or external token file), and options.
 */

import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';

export const DEFAULT_DATA_DIR = path.join(os.homedir(), '.config', 'discord-context-mcp');

/**
 * Loads configuration for discord-context-mcp.
 * Never throws if token is missing; returns '' instead.
 * Ensures dataDir directory exists. Never creates tokenFile.
 *
 * @param {Object} [options={}]
 * @param {string} [options.dataDir]
 * @param {string} [options.tokenFile]
 * @param {string} [options.token]
 * @param {boolean} [options.gatewayEnabled]
 * @param {boolean} [options.gatewayLog]
 * @param {string} [options.restBase]
 * @param {boolean} [options.selfBot]
 * @param {boolean} [options.offline]
 * @returns {{
 *   dataDir: string,
 *   tokenFile: string,
 *   token: string,
 *   gatewayEnabled: boolean,
 *   gatewayLog: boolean,
 *   restBase: string,
 *   selfBot: boolean,
 *   offline: boolean
 * }}
 */
export function loadConfig(options = {}) {
  const offline = options.offline !== undefined
    ? Boolean(options.offline)
    : process.env.DISCORD_CONTEXT_OFFLINE === '1';

  const dataDir = options.dataDir || process.env.DISCORD_CONTEXT_DATA_DIR || DEFAULT_DATA_DIR;
  const tokenFile = options.tokenFile || process.env.DISCORD_TOKEN_FILE || path.join(dataDir, 'token.txt');

  let token = (options.token || process.env.DISCORD_TOKEN || '').trim();
  if (!token && tokenFile) {
    try {
      if (fs.existsSync(tokenFile)) {
        token = fs.readFileSync(tokenFile, 'utf8').trim();
      }
    } catch (err) {
      process.stderr.write(`[discord-config] Warning: could not read token file: ${err.message}\n`);
    }
  }

  const gatewayEnabled = options.gatewayEnabled !== undefined
    ? Boolean(options.gatewayEnabled)
    : process.env.DISCORD_CONTEXT_GATEWAY !== '0';

  const gatewayLog = options.gatewayLog !== undefined
    ? Boolean(options.gatewayLog)
    : process.env.DISCORD_CONTEXT_GATEWAY_LOG === '1';

  const restBase = options.restBase || 'https://discord.com/api/v10';
  const selfBot = options.selfBot !== undefined ? Boolean(options.selfBot) : true;

  try {
    fs.mkdirSync(dataDir, { recursive: true });
  } catch (err) {
    process.stderr.write(`[discord-config] Error creating data directory: ${err.message}\n`);
  }

  return {
    dataDir,
    tokenFile,
    token: token || '',
    gatewayEnabled,
    gatewayLog,
    restBase,
    selfBot,
    offline
  };
}
