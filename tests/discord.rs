use discord_context_mcp::db::Db;
use discord_context_mcp::discord::{
    base64_encode, build_rest_headers, compute_bucket_wait_ms, encode_path_segment,
    make_super_properties, normalize_route_key, parse_search_response, rate_limit_delay_ms,
    Bucket, DiscordClient, SyncReport, WriteApiError, USER_AGENT,
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

#[tokio::test]
async fn test_stop_wakes_gateway_shutdown_waiter() {
    use std::sync::atomic::Ordering;

    let db = Arc::new(Db::new_in_memory().unwrap());
    let client = Arc::new(DiscordClient::new("test-token".to_string(), db).unwrap());

    // Mirror the gateway loop: register the notified future (force a single
    // poll so the waiter is in Notify's queue) before awaiting it. The oneshot
    // makes the test deterministic — stop() is only called once registered.
    let (reg_tx, reg_rx) = tokio::sync::oneshot::channel::<()>();
    let c = Arc::clone(&client);
    let waiter = tokio::spawn(async move {
        let mut notified = Box::pin(c.shutdown_notify.notified());
        let _ = futures_util::FutureExt::now_or_never(&mut notified);
        let _ = reg_tx.send(());
        notified.await;
        c.shutdown.load(Ordering::SeqCst)
    });

    reg_rx.await.expect("waiter failed to register");
    assert!(!client.shutdown.load(Ordering::SeqCst));

    client.stop();

    let shutdown_flag = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
        .await
        .expect("stop() did not wake the shutdown waiter")
        .expect("waiter task panicked");
    assert!(shutdown_flag, "shutdown flag should be set after stop()");
}

#[test]
fn test_version_strings_use_cargo_package_version() {
    assert!(USER_AGENT.starts_with("discord-context-mcp/"));
    assert!(USER_AGENT.contains(env!("CARGO_PKG_VERSION")));
    assert!(include_str!("../Cargo.toml").contains(&format!("version = \"{}\"", env!("CARGO_PKG_VERSION"))));
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
        allow_writes: false,
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

#[test]
fn test_normalize_route_key_reactions_and_at_me() {
    let put = reqwest::Method::PUT;
    let delete = reqwest::Method::DELETE;
    let get = reqwest::Method::GET;

    assert_eq!(
        normalize_route_key(&put, "/channels/123/messages/456/reactions/%F0%9F%91%8D/@me"),
        "PUT:/channels/:id/messages/:id/reactions/:emoji/@me"
    );
    assert_eq!(
        normalize_route_key(&delete, "/channels/123/messages/456/reactions/%F0%9F%91%8D/@me"),
        "DELETE:/channels/:id/messages/:id/reactions/:emoji/@me"
    );
    assert_eq!(
        normalize_route_key(&delete, "/channels/123/messages/456/reactions/custom_emoji:789/@me"),
        "DELETE:/channels/:id/messages/:id/reactions/:emoji/@me"
    );
    assert_eq!(
        normalize_route_key(&delete, "/channels/123/messages/456/reactions/%F0%9F%91%8D"),
        "DELETE:/channels/:id/messages/:id/reactions/:emoji"
    );
    assert_eq!(
        normalize_route_key(&get, "/channels/123/messages/456/reactions/%F0%9F%91%8D"),
        "GET:/channels/:id/messages/:id/reactions/:emoji"
    );
}

#[test]
fn test_encode_path_segment() {
    // Valid standard emoji
    assert_eq!(encode_path_segment("👍").unwrap(), "%F0%9F%91%8D");
    assert_eq!(encode_path_segment("🎉").unwrap(), "%F0%9F%8E%89");

    // Custom emoji name:id
    assert_eq!(
        encode_path_segment("pepe_laugh:123456789").unwrap(),
        "pepe_laugh:123456789"
    );

    // Unreserved characters: a-z, A-Z, 0-9, -, _, ., ~, :
    assert_eq!(
        encode_path_segment("test-1_2.3~4:5").unwrap(),
        "test-1_2.3~4:5"
    );

    // Invalid path segments
    assert!(encode_path_segment("").is_err());
    assert!(encode_path_segment("   ").is_err());
    assert!(encode_path_segment(".").is_err());
    assert!(encode_path_segment("foo bar").is_err());
    assert!(encode_path_segment("foo/bar").is_err());
    assert!(encode_path_segment("foo?bar").is_err());
    assert!(encode_path_segment("foo#bar").is_err());
    assert!(encode_path_segment("foo..bar").is_err());
    assert!(encode_path_segment("foo\nbar").is_err());
    assert!(encode_path_segment("foo\rbar").is_err());
}

#[tokio::test]
async fn test_ensure_me_caching_and_deadlock_free() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = Arc::clone(&hits);

    let app = axum::Router::new().route(
        "/users/@me",
        axum::routing::get(move || {
            let h = Arc::clone(&hits_clone);
            async move {
                h.fetch_add(1, Ordering::SeqCst);
                axum::Json(serde_json::json!({
                    "id": "user_42",
                    "username": "tester"
                }))
            }
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

    // First call: hits /users/@me
    let user_id = client.ensure_me().await.unwrap();
    assert_eq!(user_id, "user_42");
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // Second call: cached, no HTTP hit
    let user_id2 = client.ensure_me().await.unwrap();
    assert_eq!(user_id2, "user_42");
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // Concurrent calls should be completely deadlock-free
    let mut handles = Vec::new();
    for _ in 0..10 {
        let c = Arc::clone(&client);
        handles.push(tokio::spawn(async move {
            c.ensure_me().await.unwrap()
        }));
    }
    for h in handles {
        let res = tokio::time::timeout(std::time::Duration::from_secs(2), h)
            .await
            .expect("ensure_me timed out / deadlocked")
            .unwrap();
        assert_eq!(res, "user_42");
    }

    server.abort();
}

#[tokio::test]
async fn test_rest_write_burst_limit_exceeded() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = Arc::clone(&hits);

    let app = axum::Router::new().route(
        "/channels/123/messages",
        axum::routing::post(move || {
            let h = Arc::clone(&hits_clone);
            async move {
                h.fetch_add(1, Ordering::SeqCst);
                (
                    axum::http::StatusCode::OK,
                    axum::Json(serde_json::json!({
                        "id": "999",
                        "channel_id": "123",
                        "author": { "id": "1", "username": "u" },
                        "content": "hello",
                        "timestamp": "2026-01-01T00:00:00Z"
                    })),
                )
            }
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

    // 5 write calls should succeed within 60s
    for i in 0..5 {
        let res = client
            .rest_write(
                reqwest::Method::POST,
                "/channels/123/messages",
                Some(&serde_json::json!({ "content": format!("msg {}", i) })),
            )
            .await;
        assert!(res.is_ok(), "Request {} failed: {:?}", i, res.err());
    }
    assert_eq!(hits.load(Ordering::SeqCst), 5);

    // 6th call exceeds burst limit
    let err = client
        .rest_write(
            reqwest::Method::POST,
            "/channels/123/messages",
            Some(&serde_json::json!({ "content": "msg 6" })),
        )
        .await
        .unwrap_err();

    match err.downcast_ref::<WriteApiError>() {
        Some(WriteApiError::BurstLimitExceeded { retry_after_secs }) => {
            assert!(*retry_after_secs > 0 && *retry_after_secs <= 60);
        }
        other => panic!("Expected BurstLimitExceeded, got {:?}", other),
    }

    // Server should NOT have received the 6th call
    assert_eq!(hits.load(Ordering::SeqCst), 5);

    server.abort();
}

#[tokio::test]
async fn test_rest_write_429_fails_fast_without_retrying() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = Arc::clone(&hits);

    let app = axum::Router::new().route(
        "/channels/123/messages",
        axum::routing::post(move || {
            let h = Arc::clone(&hits_clone);
            async move {
                h.fetch_add(1, Ordering::SeqCst);
                (
                    axum::http::StatusCode::TOO_MANY_REQUESTS,
                    axum::Json(serde_json::json!({
                        "message": "You are being rate limited.",
                        "retry_after": 2.5
                    })),
                )
            }
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

    // First call receives 429
    let err = client
        .rest_write(
            reqwest::Method::POST,
            "/channels/123/messages",
            Some(&serde_json::json!({ "content": "hi" })),
        )
        .await
        .unwrap_err();

    match err.downcast_ref::<WriteApiError>() {
        Some(WriteApiError::RateLimited { retry_after_ms }) => {
            // 2.5s (2500ms) + 1000ms safety buffer = 3500ms
            assert_eq!(*retry_after_ms, 3500);
        }
        other => panic!("Expected RateLimited, got {:?}", other),
    }

    // Server hit only once — NO retry occurred!
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // Second call immediately fails fast before network request
    let err2 = client
        .rest_write(
            reqwest::Method::POST,
            "/channels/123/messages",
            Some(&serde_json::json!({ "content": "hi2" })),
        )
        .await
        .unwrap_err();

    match err2.downcast_ref::<WriteApiError>() {
        Some(WriteApiError::RateLimited { retry_after_ms }) => {
            assert!(*retry_after_ms > 0);
        }
        other => panic!("Expected RateLimited on second call, got {:?}", other),
    }

    // Server still hit only once
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    server.abort();
}

#[tokio::test]
async fn test_rest_write_captcha_detection_400_403_429() {
    let app = axum::Router::new()
        .route(
            "/captcha_400",
            axum::routing::post(|| async {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({
                        "captcha_key": ["captcha-required"]
                    })),
                )
            }),
        )
        .route(
            "/captcha_403",
            axum::routing::post(|| async {
                (
                    axum::http::StatusCode::FORBIDDEN,
                    axum::Json(serde_json::json!({
                        "message": "Captcha required",
                        "captcha_key": "some_hash"
                    })),
                )
            }),
        )
        .route(
            "/captcha_429",
            axum::routing::post(|| async {
                (
                    axum::http::StatusCode::TOO_MANY_REQUESTS,
                    axum::Json(serde_json::json!({
                        "message": "You are being rate limited.",
                        "captcha_key": "rate_limit_captcha"
                    })),
                )
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

    let err400 = client
        .rest_write(reqwest::Method::POST, "/captcha_400", None)
        .await
        .unwrap_err();
    assert!(matches!(
        err400.downcast_ref::<WriteApiError>(),
        Some(WriteApiError::CaptchaOrChallenge(_))
    ));

    let err403 = client
        .rest_write(reqwest::Method::POST, "/captcha_403", None)
        .await
        .unwrap_err();
    assert!(matches!(
        err403.downcast_ref::<WriteApiError>(),
        Some(WriteApiError::CaptchaOrChallenge(_))
    ));

    let err429 = client
        .rest_write(reqwest::Method::POST, "/captcha_429", None)
        .await
        .unwrap_err();
    assert!(matches!(
        err429.downcast_ref::<WriteApiError>(),
        Some(WriteApiError::CaptchaOrChallenge(_))
    ));

    server.abort();
}

#[tokio::test]
async fn test_rest_write_html_cloudflare_challenge_detection() {
    let app = axum::Router::new()
        .route(
            "/cf_403",
            axum::routing::post(|| async {
                (
                    axum::http::StatusCode::FORBIDDEN,
                    "<html><head><title>Just a moment...</title></head><body>cf-browser-verification</body></html>",
                )
            }),
        )
        .route(
            "/cf_503",
            axum::routing::post(|| async {
                (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    "<!DOCTYPE html><html><body>Attention Required! | Cloudflare</body></html>",
                )
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

    let err_cf403 = client
        .rest_write(reqwest::Method::POST, "/cf_403", None)
        .await
        .unwrap_err();
    assert!(matches!(
        err_cf403.downcast_ref::<WriteApiError>(),
        Some(WriteApiError::CaptchaOrChallenge(_))
    ));

    let err_cf503 = client
        .rest_write(reqwest::Method::POST, "/cf_503", None)
        .await
        .unwrap_err();
    assert!(matches!(
        err_cf503.downcast_ref::<WriteApiError>(),
        Some(WriteApiError::CaptchaOrChallenge(_))
    ));

    server.abort();
}

#[tokio::test]
async fn test_rest_write_204_and_reaction_delete_methods() {
    let app = axum::Router::new()
        .route(
            "/channels/123/messages/456",
            axum::routing::delete(|| async { axum::http::StatusCode::NO_CONTENT }),
        )
        .route(
            "/channels/123/messages/456/reactions/%F0%9F%91%8D/@me",
            axum::routing::put(|| async { axum::http::StatusCode::NO_CONTENT })
                .delete(|| async { axum::http::StatusCode::NO_CONTENT }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let db = Arc::new(Db::new_in_memory().unwrap());
    let msg = discord_context_mcp::models::DiscordMessage {
        id: "456".to_string(),
        channel_id: "123".to_string(),
        author: discord_context_mcp::models::DiscordUser {
            id: "1".to_string(),
            username: "tester".to_string(),
            discriminator: None,
            global_name: None,
            avatar: None,
        },
        content: "to be deleted".to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        edited_timestamp: None,
        attachments: vec![],
        referenced_message: None,
        message_reference: None,
    };
    db.upsert_message(&msg).unwrap();
    assert!(db.get_message_by_id("456").unwrap().is_some());

    let mut client = DiscordClient::new("test-token".to_string(), db.clone()).unwrap();
    client.base = format!("http://{}", addr);

    // delete_message succeeds and removes from db
    client.delete_message("123", "456").await.unwrap();
    assert!(db.get_message_by_id("456").unwrap().is_none());

    // add_reaction succeeds with 204
    client.add_reaction("123", "456", "👍").await.unwrap();

    // remove_reaction succeeds with 204
    client.remove_reaction("123", "456", "👍").await.unwrap();

    server.abort();
}

#[tokio::test]
async fn test_send_and_edit_message() {
    let app = axum::Router::new()
        .route(
            "/channels/123/messages",
            axum::routing::post(|axum::Json(body): axum::Json<serde_json::Value>| async move {
                assert_eq!(body["content"], "hello world");
                assert_eq!(body["enforce_nonce"], true);
                let nonce = body["nonce"].as_str().expect("nonce should be string");
                assert!(nonce.contains('_'));
                assert!(nonce.len() <= 25);
                assert_eq!(body["message_reference"]["message_id"], "999");
                assert_eq!(body["allowed_mentions"]["replied_user"], true);
                assert_eq!(body["allowed_mentions"]["parse"], serde_json::json!(["users"]));

                (
                    axum::http::StatusCode::OK,
                    axum::Json(serde_json::json!({
                        "id": "1001",
                        "channel_id": "123",
                        "author": { "id": "1", "username": "tester" },
                        "content": "hello world",
                        "timestamp": "2026-01-01T00:00:00Z"
                    })),
                )
            }),
        )
        .route(
            "/channels/123/messages/1001",
            axum::routing::patch(|axum::Json(body): axum::Json<serde_json::Value>| async move {
                assert_eq!(body["content"], "hello edited");
                assert_eq!(body["allowed_mentions"]["parse"], serde_json::json!(["users"]));

                (
                    axum::http::StatusCode::OK,
                    axum::Json(serde_json::json!({
                        "id": "1001",
                        "channel_id": "123",
                        "author": { "id": "1", "username": "tester" },
                        "content": "hello edited",
                        "timestamp": "2026-01-01T00:00:00Z"
                    })),
                )
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let db = Arc::new(Db::new_in_memory().unwrap());
    let mut client = DiscordClient::new("test-token".to_string(), db.clone()).unwrap();
    client.base = format!("http://{}", addr);

    let msg = client
        .send_message("123", "hello world", Some("999"), true)
        .await
        .unwrap();
    assert_eq!(msg.id, "1001");
    assert_eq!(msg.content, "hello world");
    assert_eq!(
        db.get_message_by_id("1001").unwrap().unwrap().content,
        "hello world"
    );

    let edited = client
        .edit_message("123", "1001", "hello edited")
        .await
        .unwrap();
    assert_eq!(edited.id, "1001");
    assert_eq!(edited.content, "hello edited");
    assert_eq!(
        db.get_message_by_id("1001").unwrap().unwrap().content,
        "hello edited"
    );

    server.abort();
}

#[tokio::test]
async fn test_fetch_message_around() {
    let app = axum::Router::new().route(
        "/channels/123/messages",
        axum::routing::get(
            |axum::extract::Query(params): axum::extract::Query<
                std::collections::HashMap<String, String>,
            >| async move {
                assert_eq!(params.get("around").map(String::as_str), Some("456"));
                assert_eq!(params.get("limit").map(String::as_str), Some("5"));

                axum::Json(serde_json::json!([
                    {
                        "id": "455",
                        "channel_id": "123",
                        "author": { "id": "1", "username": "u1" },
                        "content": "msg 455",
                        "timestamp": "2026-01-01T00:00:00Z"
                    },
                    {
                        "id": "456",
                        "channel_id": "123",
                        "author": { "id": "2", "username": "u2" },
                        "content": "msg 456",
                        "timestamp": "2026-01-01T00:01:00Z"
                    },
                    {
                        "id": "457",
                        "channel_id": "123",
                        "author": { "id": "3", "username": "u3" },
                        "content": "msg 457",
                        "timestamp": "2026-01-01T00:02:00Z"
                    }
                ]))
            },
        ),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let db = Arc::new(Db::new_in_memory().unwrap());
    let mut client = DiscordClient::new("test-token".to_string(), db.clone()).unwrap();
    client.base = format!("http://{}", addr);

    // Matching target id returns Some(msg)
    let found = client
        .fetch_message_around("123", "456")
        .await
        .unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().id, "456");

    // All messages in the around slice were upserted into db
    assert!(db.get_message_by_id("455").unwrap().is_some());
    assert!(db.get_message_by_id("456").unwrap().is_some());
    assert!(db.get_message_by_id("457").unwrap().is_some());

    server.abort();
}
