# v1.1.0 - safer user-token defaults

This release hardens the default posture to match DiscordChatExporter's safety model: live gateway capture is now opt-in, REST traffic no longer impersonates a browser, and message search works without the gateway.

## Highlights

- **DiscordChatExporter-style safety — gateway is OPT-IN and OFF by default.** A new **Enable live gateway (higher detection risk)** checkbox in Settings controls it, and all three start sites respect the toggle.
- **REST calls no longer spoof a browser client.** Removed `x-super-properties` and the Chrome user-agent from REST requests; they now send an honest `discord-context-mcp/1.1.0` UA. Browser properties remain only in the opt-in gateway IDENTIFY.
- **REST message search for DMs and servers.** Includes index-warmup retry plus a local SQLite FTS5 fallback, so a cold index never returns false empty results.
- **Safer 429 handling.** Adds a +1s buffer, a 60s cap, and absolute reset deadlines, backed by a 2-permit concurrency semaphore.

## Quick start

1. Download `discord-context-mcp.exe` from this release and run it.
2. Click the tray icon → **Settings**, paste your Discord user token, and save.
3. Add the MCP server to `opencode.json`:

```json
{
  "mcp": {
    "servers": {
      "discord-context": {
        "type": "remote",
        "url": "http://127.0.0.1:3774/mcp"
      }
    }
  }
}
```

4. Restart T3 Code.

## Note

If you explicitly want live gateway capture, enable the **Enable live gateway (higher detection risk)** toggle in Settings, then use **Reconnect** (or restart the app) to start the gateway.
