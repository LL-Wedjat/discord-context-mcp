# discord-context-mcp

A Windows tray app that gives an AI coding agent direct access to your own Discord DMs and server channels (read-only by default, with opt-in write capabilities).

> **Warning:** This tool uses a Discord user token (self-bot); automated access on a personal account violates Discord's Terms of Service. Use at your own risk.

### Risk Acceptance & Safeguards

Automating a user account violates Discord's Terms of Service. Write actions (sending, editing, deleting messages, adding reactions) carry materially higher detection and account ban risk than passive reading. By default, **all write tools are strictly disabled**. You must explicitly opt in via the **Allow sending & modifying messages** setting.

To minimize account risk and prevent runaway AI actions, `discord-context-mcp` implements strict built-in safeguards:
- **Burst Rate Limiting**: Hard cap of 5 write operations per 60-second sliding window.
- **Fail-Fast on 429s**: Backs off without aggressive retry loops on rate limits.
- **CAPTCHA / Challenge Circuit Breaker**: If Discord returns a CAPTCHA, Cloudflare challenge, or verification requirement (HTTP 400/403/429 with challenge payloads), the write circuit breaker trips instantly, pausing all subsequent write operations until manually reconnected or re-toggled.
- **No Cold DMs**: DMs can only be sent to users with existing open DM channels; cold DM creation is blocked.
- **Target Disambiguation**: Requires exact server, channel, and user resolution. Ambiguous matches return a list of matching candidates instead of guessing.
- **Own-Message-Only Mutation**: You can only edit or delete messages originally authored by your own user ID (verified against local cache and live Discord API).
- **Mass Mention Prevention**: Content containing `@everyone`, `@here`, or role mentions (`<@&...>`) is rejected before dispatch.
- **Prompt Injection Defense**: Discord message content is wrapped in `<untrusted_discord_messages>` tags with closing tags sanitized (`escape_untrusted_closing_tag`) so untrusted user message data cannot break prompt containment.
- **Advisory MCP Annotations**: All tools declare explicit MCP annotations (`readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`) and descriptions warning client models about ToS and untrusted user data.

## What you can do

Once connected, you can ask your agent things like:

- *"Look at my Discord DMs with Theo and implement whatever he said."*
- *"Read #general in &lt;server&gt; and summarize recent discussions."*
- *"Search my Discord messages for the latest error logs."*
- *"Reply to Theo saying the PR is ready for review."* *(requires writes enabled)*

By default, operations are **read-only** — the tool cannot send, edit, delete, or react to messages unless **Allow sending & modifying messages** is explicitly enabled in Settings.

## Quick start (release binary)

1. Download `discord-context-mcp.exe` from the [Releases](https://github.com/LL-Wedjat/discord-context-mcp/releases) tab.
2. Run it. A tray icon appears (the settings window opens automatically if no token is configured).
3. Click the tray icon → **Open Settings**, paste your Discord user token, and click **Save**. The token is stored in Windows Credential Manager — never in a plaintext file.
4. **Client registration**: `discord-context-mcp` automatically detects and registers itself on startup (and on **Reconnect**) for installed coding agents:
   - **OpenCode / T3 Code** (`~/.config/opencode/opencode.json`)
   - **Codex** (`~/.codex/config.toml`)
   - **Claude Code** (`~/.claude.json`)
   - **Antigravity** (`~/.gemini/config/mcp_config.json`)

   Only config directories that already exist are touched (no unused dotfiles are created).

   *Manual registration fallback:* If your client is not auto-detected or you prefer manual setup, add to `~/.config/opencode/opencode.json`:

```json
{
  "mcp": {
    "servers": {
      "discord-context": {
        "type": "remote",
        "url": "http://127.0.0.1:3774/mcp",
        "timeout": 60000
      }
    }
  }
}
```

Or from the CLI:

```bash
opencode mcp add --global discord-context --url http://127.0.0.1:3774/mcp
```

5. Restart T3 Code / OpenCode, then try the example prompts.

## Build from source

```bash
cargo build --release
cargo test
```

The release binary is at `target/release/discord-context-mcp.exe`.

## Requirements

- Windows (first release; macOS/Linux planned)
- Rust 1.100+ (only to build from source — the release binary needs no runtime)
- A Discord user token (from your own account)

## Tray menu

- **Open Settings** — token field, connection status, autostart toggle, and the **Allow sending & modifying messages (ToS / ban risk)** write gate.
- **Reconnect** — create a fresh Discord session (re-validate the token, refresh the client, restart the live gateway if enabled, and reset the write circuit breaker).
- **Quit** — shut down the service

Closing the settings window hides it to the tray; the MCP server keeps running.
- The **autostart** toggle in Settings controls whether the app launches on Windows login.
- The **Allow sending & modifying messages (ToS / ban risk)** toggle enables or disables write tools. If a Discord CAPTCHA/challenge is detected, the write circuit breaker automatically opens and pauses writes; clicking **Reconnect** or toggling the setting off and on resets the circuit breaker.

## Tools (15)

### Read Tools (Available by default)

| Tool | Args | What it does |
|---|---|---|
| `list_servers` | `query?` | List Discord servers (guilds) you belong to with server ID and name |
| `list_channels` | `server`, `query?` | List channels in a server by server name or server ID |
| `list_dm_people` | `query?` | List DM contacts and recent direct message channels |
| `read_dm_with` | `person`, `lastN?`, `before?`, `after?`, `cacheOnly?`, `compact?` | Read recent messages in a DM with a user (`before`/`after` snowflakes for pagination, `cacheOnly` for offline cache reads; compact local-12hr burst format is the default) |
| `read_channel` | `server`, `channel`, `lastN?`, `before?`, `after?`, `cacheOnly?`, `compact?` | Read recent messages from a channel in a server (same pagination/cache/format options as `read_dm_with`) |
| `search_messages` | `text`, `server?`, `channel?`, `person?`, `limit?` | Full-text search across messages with optional server, channel, or person/DM filter |
| `cache_stats` | — | Return cached database counts (users, guilds, channels, messages) |
| `refresh` | `scope?` (`all`/`servers`/`dms`), `perChannel?` | Manually sync recent Discord data from the API into local SQLite cache |
| `sync_channel_history` | `server` + `channel` OR `person`, `maxMessages?`, `before?`, `after?` | Paginate full channel/DM history into the local cache with `channel_sync_state` tracking oldest and newest cursors (1-10000 messages, default 2000; pass `before`/`after` to continue) |
| `export_cached_chat` | `server` + `channel` OR `person`, `after?`, `before?`, `limit?`, `outputPath?`, `includeUrls?` | Export cached messages in compact format to a path-confined local file under `<data_dir>/exports/` |

`read_dm_with` and `read_channel` use the **compact local-12hr format by default** (`compact: true`). Non-empty compact output ends with both `prev_before: <first_message_id>` and `next_after: <last_message_id>` cursors, so backward (`before`) and forward (`after`) pagination work directly from it. Pass `compact: false` for the verbose format with per-message snowflake IDs (needed for `replyTo` and write tools) and raw attachment URLs.

### Write Tools (Require "Allow sending & modifying messages")

| Tool | Args | What it does |
|---|---|---|
| `send_dm` | `person`, `content`, `replyTo?`, `mentionReplyAuthor?` | Send a direct message to an existing DM contact (up to 2000 characters) |
| `send_channel_message` | `server`, `channel`, `content`, `replyTo?`, `mentionReplyAuthor?` | Send a message to a text channel in a server (up to 2000 characters) |
| `edit_message` | `channelId`, `messageId`, `content` | Edit a message previously sent by the current user (up to 2000 characters) |
| `delete_message` | `channelId`, `messageId` | Delete a message previously sent by the current user |
| `react_message` | `channelId`, `messageId`, `emoji`, `action?` (`add`/`remove`) | Add or remove a reaction emoji on a message |

## Environment variables

- `DISCORD_MCP_PORT` — MCP HTTP port (default `3774`)
- `RUST_LOG` — tracing filter (e.g. `info`, `discord_context_mcp=debug`)

## Security & privacy

- Token lives in **Windows Credential Manager** — no plaintext token file, never logged or printed.
- **Opt-in writes**: Disabled by default. Write actions (`send_dm`, `send_channel_message`, `edit_message`, `delete_message`, `react_message`) are gated behind the "Allow sending & modifying messages" toggle in Settings.
- **Prompt injection containment**: Message content is framed in `<untrusted_discord_messages>` blocks with closing tags sanitized (`escape_untrusted_closing_tag`), preventing malicious chat messages from breaking prompt framing.
- Messages are cached locally in SQLite in your config directory.
- The MCP server binds to `127.0.0.1` only and validates `Host`/`Origin` exactly (blocks DNS-rebinding/cross-origin reads).
- No telemetry.

## Troubleshooting

- **Settings window opens on launch** — no token is stored yet; paste one and Save.
- **Status shows an error after Save** — the token was rejected by Discord; check it and try again.
- **"Port in use" / client can't connect** — set `DISCORD_MCP_PORT` to another port; the MCP URL is updated automatically in supported client configs on launch/Reconnect (edit manually only as a fallback).
- **Writes return "Writes are disabled"** — open Settings and enable the **Allow sending & modifying messages** toggle.
- **Writes return "Write circuit paused"** — a Discord CAPTCHA/challenge was detected. Click **Reconnect** or re-toggle writes in Settings once resolved in the Discord client.
- **Logs** — written to a rotating file in your config directory (`discord-context-mcp/logs`).
