# Release Notes - v1.0.0

## v1.0.0 - first release

First public release of `discord-context-mcp`: a read-only Model Context Protocol (MCP) server that grants AI coding agents (such as OpenCode and T3 Code) direct context from your Discord DMs and server channels.

### Quick Start

```bash
git clone https://github.com/Wedjat/discord-context-mcp.git
cd discord-context-mcp
npm install
npm run setup
```
Restart T3 Code or OpenCode after completing setup.

### Features & Available Tools

Includes 8 MCP tools backed by a local SQLite cache:
1. `list_servers` - List all joined Discord servers and server IDs.
2. `list_channels` - List channels within a server.
3. `list_dm_people` - List DM contacts and recent conversations.
4. `read_dm_with` - Read recent messages from a conversation with a specific user.
5. `read_channel` - Read recent messages from a specific channel in a server.
6. `search_messages` - Search message history across channels and DMs with optional filters.
7. `cache_stats` - View cached message, user, channel, and server counts.
8. `refresh` - Trigger on-demand sync of recent messages into the local cache.

### Security & Privacy

- **Token Protection:** Credentials are kept strictly outside the repository in `~/.config/discord-context-mcp/token.txt` with restricted permissions.
- **Read-Only:** Provides read-only access. It cannot send messages, react, edit, delete, or modify any servers or DMs.
- **Local Storage:** All cached messages and channels are saved locally in SQLite (`cache.db`).
- **No Telemetry:** No analytics, tracking, or external calls beyond the official Discord endpoints.

### TOS Note

This server authenticates using a Discord user account token (self-bot). Using automated access with personal user tokens is contrary to Discord's Terms of Service. Use at your own discretion.

### Known Limitations

- **Self-bot / User Token Risk:** User tokens are subject to Discord rate limits and potential account restrictions.
- **Server Membership:** You can only read servers and channels that your Discord account has access to.
- **Gateway Optional:** Gateway connection (`DISCORD_CONTEXT_GATEWAY=1`) offers real-time caching, but can be disabled (`0`) if your network environment restricts long-lived WebSockets.
- **Engine Requirement:** Requires Node.js v26+ to utilize the built-in `node:sqlite` database module.
