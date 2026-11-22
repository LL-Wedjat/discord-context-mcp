use discord_context_mcp::db::Db;
use discord_context_mcp::discord::{
    base64_encode, build_rest_headers, compute_bucket_wait_ms, make_super_properties,
    normalize_route_key, parse_search_response, rate_limit_delay_ms, Bucket, DiscordClient,
    SyncReport, USER_AGENT,
};
use std::sync::Arc;

#[test]
fn test_compute_bucket_wait_ms() {
    let now = 10000i64;

    // Remaining > 0: no wait
    let b1 = Bucket {
        remaining: 5,
        reset_at_ms: now + 5000,
    };
    assert_eq!(compute_bucket_wait_ms(&b1, now), 0);
    assert_eq!(b1.remaining, 5);

    // Remaining <= 0, reset_at_ms in future: wait delta, no mutation
    let b2 = Bucket {
        remaining: 0,
        reset_at_ms: now + 3500,
    };
    assert_eq!(compute_bucket_wait_ms(&b2, now), 3500);
    assert_eq!(b2.remaining, 0);
    assert_eq!(b2.reset_at_ms, now + 3500);

    // Remaining <= 0, reset_at_ms in past: no wait
    let b3 = Bucket {
        remaining: 0,
        reset_at_ms: now - 1000,
    };
    assert_eq!(compute_bucket_wait_ms(&b3, now), 0);
    assert_eq!(b3.remaining, 0);
    assert_eq!(b3.reset_at_ms, now - 1000);

    // Explicit test proving bucket with future reset_at_ms produces positive wait and does not mutate
    let b_future = Bucket {
        remaining: 0,
        reset_at_ms: now + 2500,
    };
    let wait = compute_bucket_wait_ms(&b_future, now);
    assert_eq!(wait, 2500);
    assert_eq!(b_future.remaining, 0);
    assert_eq!(b_future.reset_at_ms, now + 2500);
}

#[test]
fn test_sanitize_redacts_token() {
    let db = Arc::new(Db::new_in_memory().unwrap());
    let token = "super_secret_discord_token_12345".to_string();
    let client = DiscordClient::new(token.clone(), db).unwrap();

    let raw_log = format!("Connecting with Authorization: {} to endpoint", token);
    let sanitized = client.sanitize(&raw_log);

    assert!(!sanitized.contains(&token));
    assert!(sanitized.contains("[REDACTED]"));
    assert_eq!(
        sanitized,
        "Connecting with Authorization: [REDACTED] to endpoint"
    );
}

#[test]
fn test_normalize_route_key() {
    let get = reqwest::Method::GET;
    let post = reqwest::Method::POST;

    assert_eq!(normalize_route_key(&get, "/users/@me"), "GET:/users/@me");
    assert_eq!(
        normalize_route_key(&get, "/guilds/123456789/channels"),
        "GET:/guilds/:id/channels"
    );
    assert_eq!(
        normalize_route_key(&get, "channels/987654321/messages/11223344"),
        "GET:/channels/:id/messages/:id"
    );
    assert_eq!(
        normalize_route_key(&post, "/channels/555/messages"),
        "POST:/channels/:id/messages"
    );
}

#[test]
fn test_sync_report_default() {
    let report = SyncReport::default();
    assert_eq!(report.channels_fetched, 0);
    assert_eq!(report.messages_fetched, 0);
    assert!(report.errors.is_empty());
}

#[tokio::test]
async fn test_app_state_set_status_synchronous_ordering() {
    use discord_context_mcp::app_state::AppState;
    use discord_context_mcp::config::Config;

    let db = Arc::new(Db::new_in_memory().unwrap());
    let handle = tokio::runtime::Handle::current();
    let state = AppState::new(Config::default(), db, true, handle);

    assert_eq!(*state.status.lock().unwrap(), "Ready");
    state.set_status("Connecting...");
    // Immediate synchronous visibility without awaiting any spawned task
    assert_eq!(*state.status.lock().unwrap(), "Connecting...");

    state.set_status("Connected");
    assert_eq!(*state.status.lock().unwrap(), "Connected");
}

#[test]
fn test_base64_encode_and_super_properties() {
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_encode(b"f"), "Zg==");
    assert_eq!(base64_encode(b"fo"), "Zm8=");
    assert_eq!(base64_encode(b"foo"), "Zm9v");
    assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
    assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
    assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");

    let sp = make_super_properties();
    assert!(!sp.is_empty());
}

#[test]
fn test_build_rest_headers_has_no_client_spoofing() {
    let headers = build_rest_headers().unwrap();

    // REST headers must not carry browser client spoofing properties.
    assert!(headers.get("x-super-properties").is_none());

    // The User-Agent must identify this client honestly, not masquerade as a browser.
    let ua = headers
        .get("user-agent")
        .expect("REST headers should include a User-Agent")
        .to_str()
        .unwrap();
    assert!(!ua.contains("Mozilla"));
    assert_eq!(ua, USER_AGENT);
}

#[test]
fn test_parse_search_response_flattens_results() {
    let fixture = serde_json::json!({
        "total_results": 1,
        "messages": [[
            {
                "id": "1",
                "channel_id": "2",
                "author": {
                    "id": "3",
                    "username": "u"
                },
                "content": "hi",
                "timestamp": "2026-01-01T00:00:00Z"
            }
        ]]
    });

    let messages = parse_search_response(&fixture).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, "1");
    assert_eq!(messages[0].content, "hi");
}

#[test]
fn test_parse_search_response_missing_messages_is_empty() {
    let fixture = serde_json::json!({
        "total_results": 0
    });

    let messages = parse_search_response(&fixture).unwrap();
    assert!(messages.is_empty());
}

#[test]
fn test_rate_limit_delay_ms() {
    // retry_after is in seconds and gets the 1000ms buffer: 0.5s -> 1500ms.
    assert_eq!(rate_limit_delay_ms(Some(0.5), None), 1500);

    // Large retry_after values are capped at 60 seconds.
    assert_eq!(rate_limit_delay_ms(Some(600.0), None), 60_000);

    // No headers at all: the buffer alone provides a 1000ms fallback.
    assert_eq!(rate_limit_delay_ms(None, None), 1000);

    // reset_after (milliseconds) is used when retry_after is absent.
    assert_eq!(rate_limit_delay_ms(None, Some(2000)), 3000);
}

#[tokio::test]
async fn test_search_messages_persistent_202_returns_err() {
    // A cold Discord search index answers 202 with code 110000. After the
    // retry budget is exhausted, search_messages must return Err so callers
    // (e.g. the MCP tool) can fall back to the local FTS cache and show a note.
    let app = axum::Router::new().fallback(|| async {
        (
            axum::http::StatusCode::ACCEPTED,
            axum::Json(serde_json::json!({
                "code": 110000,
                "message": "Search index unavailable",
                "retry_after": 0.0
            })),
        )
    });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let db = Arc::new(Db::new_in_memory().unwrap());
    let mut client = DiscordClient::new("test-token".to_string(), db).unwrap();
    client.base = format!("http://{}", addr);

    let result = client
        .search_messages("hello", None, Some("123456789"), None, 10)
        .await;
    server.abort();

    assert!(
        result.is_err(),
        "expected Err on persistent 202/110000, got {:?}",
        result
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("search index unavailable"),
        "unexpected error message"
    );
}

#[test]
fn test_version_strings_use_cargo_package_version() {
    assert!(USER_AGENT.starts_with("discord-context-mcp/"));
    assert!(USER_AGENT.contains(env!("CARGO_PKG_VERSION")));
    assert!(include_str!("../Cargo.toml").contains("version = \"1.1.0\""));
}

#[test]
fn test_parse_search_response_dedupes_by_id() {
    let msg_json = serde_json::json!({
        "id": "dup_123",
        "channel_id": "c1",
        "author": {
            "id": "u1",
            "username": "tester"
        },
        "content": "duplicate message content",
        "timestamp": "2026-01-01T00:00:00Z"
    });

    let fixture = serde_json::json!({
        "total_results": 2,
        "messages": [
            [msg_json.clone()],
            [msg_json]
        ]
    });

    let messages = parse_search_response(&fixture).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, "dup_123");
}

#[tokio::test]
async fn test_rest_permit_dropped_during_429_backoff() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    let hit_count = Arc::new(AtomicUsize::new(0));
    let hit_count_clone = Arc::clone(&hit_count);

    let app = axum::Router::new()
        .route(
            "/channels/slow/messages/search",
            axum::routing::get(move || {
                let count = Arc::clone(&hit_count_clone);
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    (
                        axum::http::StatusCode::TOO_MANY_REQUESTS,
                        axum::Json(serde_json::json!({
                            "message": "You are being rate limited.",
                            "retry_after": 2.0
                        })),
                    )
                }
            }),
        )
        .route(
            "/users/@me",
            axum::routing::get(|| async {
                axum::Json(serde_json::json!({
                    "id": "12345",
                    "username": "testuser"
                }))
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let db = Arc::new(Db::new_in_memory().unwrap());
    let mut client = DiscordClient::new("test-token".to_string(), db).unwrap();
    client.base = format!("http://{}", addr);
    let client = Arc::new(client);

    let c1 = Arc::clone(&client);
    let task1 = tokio::spawn(async move {
        let _ = c1.search_messages("x", None, Some("slow"), None, 5).await;
    });

    let c2 = Arc::clone(&client);
    let task2 = tokio::spawn(async move {
        let _ = c2.search_messages("x", None, Some("slow"), None, 5).await;
    });

    // Poll counter with 10ms sleeps up to 1s until it reaches 2
    let poll_start = Instant::now();
    while hit_count.load(Ordering::SeqCst) < 2 {
        if poll_start.elapsed() >= std::time::Duration::from_secs(1) {
            panic!(
                "Timed out waiting for hit_count to reach 2, was {}",
                hit_count.load(Ordering::SeqCst)
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let init_start = Instant::now();
    let init_res = client.init().await;
    let init_duration = init_start.elapsed();

    assert!(
        init_res.is_ok(),
        "client.init() failed: {:?}",
        init_res.err()
    );
    assert!(
        init_duration < std::time::Duration::from_secs(1),
        "client.init() took {:?}, expected < 1s",
        init_duration
    );

    task1.abort();
    task2.abort();
    server.abort();
}

#[tokio::test]
async fn test_gateway_toggle_reread_from_disk_across_clones() {
    use discord_context_mcp::app_state::AppState;
    use discord_context_mcp::config::Config;
    use std::sync::atomic::Ordering;

    let temp_dir = tempfile::tempdir().unwrap();
    let data_dir = temp_dir.path().to_path_buf();
    let log_dir = data_dir.join("logs");

    let settings_path = data_dir.join("settings.json");
    std::fs::write(&settings_path, r#"{"autostart":false,"gateway":true}"#).unwrap();

    let config = Config {
        data_dir: data_dir.clone(),
        log_dir,
        port: 3774,
        autostart: false,
        gateway_enabled: false,
    };

    let db = Arc::new(Db::new_in_memory().unwrap());
    let handle = tokio::runtime::Handle::current();
    let state = AppState::new(config, db, true, handle);
    let state_clone = state.clone();

    // Assert atomic starts false
    assert!(!state.gateway_enabled.load(Ordering::SeqCst));
    assert!(!state_clone.gateway_enabled.load(Ordering::SeqCst));

    // refresh returns true and both atomics true
    let refreshed = state.refresh_gateway_enabled();
    assert!(refreshed);
    assert!(state.gateway_enabled.load(Ordering::SeqCst));
    assert!(state_clone.gateway_enabled.load(Ordering::SeqCst));

    // rewrite settings.json with gateway false
    std::fs::write(&settings_path, r#"{"autostart":false,"gateway":false}"#).unwrap();

    // refresh returns false and both atomics false
    let refreshed = state.refresh_gateway_enabled();
    assert!(!refreshed);
    assert!(!state.gateway_enabled.load(Ordering::SeqCst));
    assert!(!state_clone.gateway_enabled.load(Ordering::SeqCst));

    // assert discord_context_mcp::ui::SettingsApp::new(state.clone()).gateway_toggle == false
    let ui_app = discord_context_mcp::ui::SettingsApp::new(state.clone());
    assert!(!ui_app.gateway_toggle);
}
