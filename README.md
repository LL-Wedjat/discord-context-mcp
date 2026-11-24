# discord-context-mcp

A read-only Windows tray app that gives an AI coding agent direct access to your own Discord DMs and server channels.

> This tool uses a Discord user token (self-bot); automated access on a personal account violates Discord's Terms of Service. Use at your own risk.

## What you can do

Once connected, you can ask your agent things like:

- *"Look at my Discord DMs with Theo and implement whatever he said."*
- *"Read #general in &lt;server&gt; and summarize recent discussions."*
- *"Search my Discord messages for the latest error logs."*

Everything is **read-only** — the tool cannot send, edit, or delete messages.

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
        "enabled": true,
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

- **Open Settings** — token field, connection status, autostart toggle
- **Reconnect** — create a fresh Discord session (re-validate the token, refresh the client, and restart the live gateway if enabled)
- **Quit** — shut down the service

Closing the settings window hides it to the tray; the MCP server keeps running. The **autostart** toggle in Settings controls whether the app launches on Windows login.

## Tools (8)

| Tool | Args | What it does |
|---|---|---|
| `list_servers` | `query?` | List Discord servers you're in |
| `list_channels` | `server`, `query?` | List channels in a server |
| `list_dm_people` | `query?` | List DM contacts |
| `read_dm_with` | `person`, `lastN?` | Read recent messages in a DM |
| `read_channel` | `server`, `channel`, `lastN?` | Read recent messages in a channel |
| `search_messages` | `text`, `server?`, `channel?`, `person?`, `limit?` | Full-text search across messages |
| `cache_stats` | — | Local cache statistics |
| `refresh` | `scope?` (`all`/`servers`/`dms`) | Backfill recent messages into the cache |

## Environment variables

- `DISCORD_MCP_PORT` — MCP HTTP port (default `3774`)
- `RUST_LOG` — tracing filter (e.g. `info`, `discord_context_mcp=debug`)

## Security & privacy

- Token lives in **Windows Credential Manager** — no plaintext token file, never logged or printed.
- **Read-only** scope: no send/edit/delete Discord API calls.
- Messages are cached locally in SQLite in your config directory.
- The MCP server binds to `127.0.0.1` only and validates `Host`/`Origin` exactly (blocks DNS-rebinding/cross-origin reads).
- No telemetry.

## Troubleshooting

- **Settings window opens on launch** — no token is stored yet; paste one and Save.
- **Status shows an error after Save** — the token was rejected by Discord; check it and try again.
- **"Port in use" / client can't connect** — set `DISCORD_MCP_PORT` to another port; the MCP URL is updated automatically in supported client configs on launch/Reconnect (edit manually only as a fallback).
- **Logs** — written to a rotating file in your config directory (`discord-context-mcp/logs`).
