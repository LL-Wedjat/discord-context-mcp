# v1.0.0 — first release (Rust)

The first release of `discord-context-mcp`: a single-file Windows tray app that gives an AI coding agent read-only access to your own Discord DMs and server channels. It replaces the earlier Node.js prototype entirely — no Node runtime is required.

## Quick start

1. Download `discord-context-mcp.exe` from this release and run it.
2. A tray icon appears. Click it → **Open Settings**, paste your Discord user token, and click **Save**. The token is stored in Windows Credential Manager, never in a plaintext file.
3. Register the MCP server in your client. For T3 Code / OpenCode, add to `~/.config/opencode/opencode.json`:

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

Or: `opencode mcp add --global discord-context --url http://127.0.0.1:3774/mcp`

4. Restart T3 Code / OpenCode and try: *"Look at my Discord DMs with Theo and implement whatever he said."*

## Included tools

`list_servers`, `list_channels`, `list_dm_people`, `read_dm_with`, `read_channel`, `search_messages`, `cache_stats`, `refresh` — all read-only, backed by a local SQLite cache with full-text search.

## Security & privacy

- Token stored in Windows Credential Manager; never logged, printed, or written to a file.
- Read-only scope — the app cannot send, edit, or delete messages.
- Local SQLite cache only; no telemetry.
- MCP server binds to `127.0.0.1` and validates `Host`/`Origin` exactly, blocking cross-origin and DNS-rebinding reads.

## Notes

- This uses a Discord **user token** (self-bot). Automated access on a personal account is against Discord's Terms of Service and carries a ban risk — use at your own discretion.
- You can only read servers and channels your account is a member of.
- Windows is supported in this first release. `DISCORD_MCP_PORT` overrides the default port `3774`.
- Built with Rust 1.100; 27 tests passing.
