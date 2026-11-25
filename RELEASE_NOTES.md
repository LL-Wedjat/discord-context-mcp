# v1.2.0 - opt-in message sending, reactions, pagination, and account switching

This release adds opt-in Discord write tools with account-protection guardrails, read and pagination improvements, T3 Code Antigravity provider auto-registration, and automatic cache clearing when switching Discord accounts.

## Opt-in Discord write tools

Five new write tools are available but **disabled by default** (`allow_writes: false`) behind a Settings checkbox:

- **`send_dm`** — send a direct message to a user.
- **`send_channel_message`** — send a message to a channel.
- **`edit_message`** — edit one of your own messages.
- **`delete_message`** — delete one of your own messages.
- **`react_message`** — add or remove a reaction on a message.

## Account-protection & safety guardrails

- **5/60s burst cap** — write operations are limited to five per sixty seconds.
- **Fail-fast 429 handling** — rate-limit responses stop the write immediately rather than retrying or backing off blindly.
- **Automatic CAPTCHA/challenge circuit breaker** — detected challenges trip a circuit breaker that halts further writes.
- **Existing 1:1 DMs only** — direct messages can only be sent to conversations that already exist.
- **Strict target resolution** — message and channel targets are resolved strictly to avoid ambiguous writes.
- **Own-message-only edit/delete** — edits and deletions are restricted to messages authored by the current account.
- **`@everyone`/`@here`/role-ping rejection** — mentions that ping broad audiences are rejected.
- **`<untrusted_discord_messages>` prompt-injection framing** — fetched message content is wrapped in untrusted-data framing to reduce prompt-injection risk.

## Read & pagination improvements

- Message and channel IDs are now exposed on read tools.
- Reply references are included when reading messages.
- `before` pagination is available on read tools for paging through history.

## T3 Code Antigravity provider auto-registration

`discord-context` is now registered across all `~/.t3/userdata/providers/antigravity/<id>/config/mcp_config.json` profiles (including 0-byte files), alongside `~/.gemini/config/mcp_config.json`.

## Automatic cache clearing on account switch

The active `account_user_id` is tracked in SQLite. When the Discord account changes, stale cached servers, channels, DMs, and messages are cleared automatically.

# v1.1.2 - automatic MCP client registration

On launch and on the tray Reconnect action, discord-context now registers its streamable-HTTP endpoint with installed AI clients: OpenCode, Codex, Claude Code, and Antigravity.

- **Non-intrusive client detection:** Detects installed clients without creating empty config directories.
- **Preserves existing configuration:** Preserves other config keys and TOML comments.
- **Fails fast on malformed configs:** Reports errors rather than clobbering or truncating invalid configs.
- **Atomic file writes:** Writes config files atomically to prevent corruption.
- **Idempotent registration:** Clean, repeatable registration across restarts and port changes.
- **Broad client compatibility:** Migrates existing stdio entries and handles UTF-8 BOMs as well as Antigravity `serverUrl`.

# v1.1.1 - hardening follow-ups

A follow-up to v1.1.0 that tightens the safety and protocol changes shipped there:

- **Opt-in live gateway with immediate shutdown.** The gateway stays OFF by default; the Settings toggle is now applied live, and disabling it fires the shutdown `Notify` so the gateway wakes and stops immediately.
- **Correct REST 429 backoff accounting.** The concurrency semaphore permit is released during backoff, so rate-limited requests no longer leak permits and stall later calls.
- **Crash-safe settings writes.** Settings are written atomically to avoid truncated or corrupt config after an interrupted save.
- **DM-only search scoping with live discovery.** Search can be scoped to DMs, and cached DM channels are checked first with a live `list_dm_channels()` refresh when none is cached yet, falling back to local FTS only if no DM channel is found.
- **JSON-RPC conformance.** Correct handling of `id: null` notifications and the `ping` method.
- **Dynamic version reporting.** The REST user-agent and MCP server info now use `CARGO_PKG_VERSION` instead of a hardcoded string.

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
