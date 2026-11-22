# discord-context-mcp

Read-only Discord context Model Context Protocol (MCP) server for AI coding agents.

> **Discord TOS Notice:** This tool uses a Discord user account token (self-bot); using automated access on personal user tokens violates Discord's Terms of Service—use at your own discretion.

## What You Can Do

Give your AI coding assistant direct, read-only context from your Discord workspace:

- *"Look at my Discord DMs with Theo and implement whatever he said."*
- *"Read #general in DevServer and summarize recent discussions about the migration."*
- *"Search Discord messages for the latest error logs regarding Redis."*

## Requirements

- **Node.js:** v26+ (leverages built-in `node:sqlite`)
- **Discord User Token:** Personal account token
- **Client:** OpenCode or T3 Code (or any MCP-compatible client)

## Installation

```bash
git clone https://github.com/Wedjat/discord-context-mcp.git
cd discord-context-mcp
npm install
npm run setup
```

Then restart T3 Code or OpenCode.

`npm run setup` securely prompts for your Discord token, stores it outside the repository with restricted permissions (at `~/.config/discord-context-mcp/token.txt`), and automatically registers the MCP server in your local client configuration (`~/.config/opencode/opencode.json` or T3 Code settings).

### Manual / Alternative Configuration

Alternatively, you can provide the token via:
- The `DISCORD_TOKEN` environment variable, or
- Manually creating `~/.config/discord-context-mcp/token.txt`

Example MCP server configuration:

```json
{
  "mcp": {
    "servers": {
      "discord-context": {
        "type": "local",
        "command": ["node", "path/to/discord-context-mcp/src/index.js"],
        "environment": {
          "DISCORD_CONTEXT_GATEWAY": "0"
        }
      }
    }
  }
}
```

## Available Tools

| Name | Arguments | Description | Example Prompt |
| :--- | :--- | :--- | :--- |
| `list_servers` | `query?` (string) | Lists joined Discord servers and their IDs. | *"List my Discord servers"* |
| `list_channels` | `server` (string), `query?` (string) | Lists channels in a server by server name or ID. | *"List channels in the OpenAI server"* |
| `list_dm_people` | `query?` (string) | Lists contacts and users from recent DM conversations. | *"Who have I been direct messaging with?"* |
| `read_dm_with` | `person` (string), `lastN?` (number, 1–100) | Fetches recent messages from direct messages with a specific person. | *"Look at my Discord DMs with Theo and implement whatever he said"* |
| `read_channel` | `server` (string), `channel` (string), `lastN?` (number, 1–100) | Reads recent messages from a channel in a server. | *"Read #general in DevServer"* |
| `search_messages` | `text` (string), `server?`, `channel?`, `person?`, `limit?` | Searches message history across channels and DMs with optional filters. | *"Search Discord for deployment errors"* |
| `cache_stats` | *(none)* | Returns cached SQLite statistics (user, guild, channel, message counts). | *"Check Discord cache status"* |
| `refresh` | `scope?` (`all` \| `servers` \| `dms`), `perChannel?` | Manually synchronizes Discord data into the local SQLite cache. | *"Refresh Discord cache for DMs"* |

## Environment Variables

| Variable | Description | Default |
| :--- | :--- | :--- |
| `DISCORD_TOKEN` | Discord account token (takes priority over token file). | *(none)* |
| `DISCORD_TOKEN_FILE` | Custom path to the file containing the Discord token. | `~/.config/discord-context-mcp/token.txt` |
| `DISCORD_CONTEXT_DATA_DIR` | Directory storing SQLite database (`cache.db`) and credentials. | `~/.config/discord-context-mcp` |
| `DISCORD_CONTEXT_GATEWAY` | Enable Discord WebSocket Gateway for real-time message caching (`1` or `0`). | `1` (Set to `0` for on-demand REST only) |
| `DISCORD_CONTEXT_OFFLINE` | Run strictly against local SQLite cache without making network calls (`1` or `0`). | `0` |
| `DISCORD_CONTEXT_GATEWAY_LOG` | Enable verbose Gateway logging in stderr (`1` or `0`). | `0` |

## Security & Privacy

- **Safe Token Storage:** Token is stored outside the repository directory (`~/.config/discord-context-mcp/token.txt`) with restricted file permissions (`0600`).
- **Read-Only by Design:** Exposes only read-only endpoints. There are no tools or functions to send, edit, or delete messages or perform server actions.
- **Local SQLite Cache:** Message and contact data stay on your machine in a local SQLite file (`cache.db`).
- **Zero Telemetry:** No tracking, metrics, analytics, or external calls outside the official Discord API.

## Troubleshooting & Verification

- **Run Diagnostics:**
  ```bash
  npm run check
  ```
  Validates your token against Discord API `@me`, inspects the SQLite cache, and verifies MCP registration.

- **Run Automated Tests:**
  ```bash
  npm test
  ```
  Executes unit tests and offline stdio MCP smoke tests.

- **Token Invalid or Missing:** Verify `~/.config/discord-context-mcp/token.txt` or set `DISCORD_TOKEN`. Ensure there are no extra whitespace or newline characters.
- **Gateway Issues:** If WebSocket connections are blocked or rate-limited by your network, set `DISCORD_CONTEXT_GATEWAY=0`. On-demand REST queries will continue functioning.
- **Offline Operation:** Set `DISCORD_CONTEXT_OFFLINE=1` to query cached Discord data without any network connection.

## License

MIT License. See [LICENSE](LICENSE) for details.
