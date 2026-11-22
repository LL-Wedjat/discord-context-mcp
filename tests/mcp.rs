use std::sync::Arc;

use discord_context_mcp::db::{Db, MessageRow};
use discord_context_mcp::discord::DiscordClient;
use discord_context_mcp::mcp::{
    call_tool, format_items, format_live_messages, format_messages, tool_definitions, AppContext,
};
use serde_json::json;

#[test]
fn test_format_messages() {
    let empty = format_messages(&[], "Empty Test");
    assert_eq!(empty, "# Empty Test (0)\n\n_No messages found._");

    let rows = vec![
        MessageRow {
            id: "msg_1".to_string(),
            channel_id: "c1".to_string(),
            author_id: "u1".to_string(),
            content: "Hello world!".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            edited_timestamp: None,
            attachments_json: Some(
                json!([
                    {
                        "id": "att1",
                        "filename": "screenshot.png",
                        "url": "https://cdn.discordapp.com/attachments/test/screenshot.png"
                    }
                ])
                .to_string(),
            ),
            embeds_json: None,
            reply_to: None,
            created_at_ms: 1767225600000,
            author_username: Some("alice".to_string()),
            author_global_name: Some("Alice Smith".to_string()),
            guild_id: Some("g1".to_string()),
            channel_name: Some("general".to_string()),
        },
        MessageRow {
            id: "msg_2".to_string(),
            channel_id: "c1".to_string(),
            author_id: "u2".to_string(),
            content: "Second message".to_string(),
            timestamp: "2026-01-01T00:05:00Z".to_string(),
            edited_timestamp: Some("2026-01-01T00:06:00Z".to_string()),
            attachments_json: None,
            embeds_json: None,
            reply_to: Some("msg_1".to_string()),
            created_at_ms: 1767225900000,
            author_username: Some("bob".to_string()),
            author_global_name: None,
            guild_id: Some("g1".to_string()),
            channel_name: Some("general".to_string()),
        },
    ];

    let formatted = format_messages(&rows, "General Chat");
    assert!(formatted.starts_with("# General Chat (2)\n\n"));
    assert!(formatted.contains("Alice Smith (u1): Hello world!"));
    assert!(formatted.contains("  ↳ [Attachment: screenshot.png](https://cdn.discordapp.com/attachments/test/screenshot.png)"));
    assert!(formatted.contains("bob (u2): Second message (edited)"));
}

#[test]
fn test_format_live_messages_includes_author_and_content() {
    use discord_context_mcp::models::{Attachment, DiscordMessage, DiscordUser};

    let msgs = vec![DiscordMessage {
        id: "m1".to_string(),
        channel_id: "c1".to_string(),
        author: DiscordUser {
            id: "u1".to_string(),
            username: "alice".to_string(),
            global_name: Some("Alice Smith".to_string()),
            discriminator: None,
            avatar: None,
        },
        content: "live search hit".to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        edited_timestamp: None,
        attachments: vec![Attachment {
            id: "a1".to_string(),
            filename: "pic.png".to_string(),
            url: "https://example.com/pic.png".to_string(),
            proxy_url: None,
        }],
        referenced_message: None,
    }];

    let formatted = format_live_messages(&msgs, "Live");
    assert!(formatted.starts_with("# Live (1)\n\n"));
    assert!(formatted.contains("[2026-01-01T00:00:00Z] Alice Smith (u1): live search hit"));
    assert!(formatted.contains("  ↳ [Attachment: pic.png](https://example.com/pic.png)"));

    let empty = format_live_messages(&[], "Live");
    assert_eq!(empty, "# Live (0)\n\n_No messages found._");
}

#[test]
fn test_format_items() {
    let empty: Vec<serde_json::Value> = vec![];
    let res = format_items(&empty, "Items", &["id", "name"]);
    assert_eq!(res, "# Items (0)\n\n_No items found._");

    let items = vec![
        json!({"id": "1", "name": "Alpha", "extra": "ignored"}),
        json!({"id": "2", "name": "Beta", "extra": "ignored"}),
    ];

    let formatted = format_items(&items, "Test List", &["id", "name"]);
    assert_eq!(
        formatted,
        "# Test List (2)\n\n- **id**: 1, **name**: Alpha\n- **id**: 2, **name**: Beta"
    );
}

#[test]
fn test_tool_definitions() {
    let tools = tool_definitions();
    assert_eq!(tools.len(), 8);

    let names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t.get("name").and_then(|v| v.as_str()))
        .collect();

    assert!(names.contains(&"list_servers"));
    assert!(names.contains(&"list_channels"));
    assert!(names.contains(&"list_dm_people"));
    assert!(names.contains(&"read_dm_with"));
    assert!(names.contains(&"read_channel"));
    assert!(names.contains(&"search_messages"));
    assert!(names.contains(&"cache_stats"));
    assert!(names.contains(&"refresh"));
}

#[tokio::test]
async fn test_call_tool_cache_stats() {
    let db = Arc::new(Db::new_in_memory().expect("in-memory db failed"));
    let client = Arc::new(DiscordClient::new("dummy_token".to_string(), db.clone()).unwrap());
    let ctx = AppContext::new(db, Some(client));

    let res = call_tool(&ctx, "cache_stats", &json!({})).await;
    assert!(res.is_ok());
    let val = res.unwrap();

    let content = val.get("content").and_then(|v| v.as_array()).unwrap();
    assert_eq!(content.len(), 1);
    let text = content[0].get("text").and_then(|v| v.as_str()).unwrap();
    assert!(text.contains("\"users\": 0"));
    assert!(text.contains("\"messages\": 0"));
}

#[tokio::test]
async fn test_http_jsonrpc_flow() {
    use axum::extract::State;
    use discord_context_mcp::mcp::mcp_post;

    let db = Arc::new(Db::new_in_memory().expect("in-memory db failed"));
    let client = Arc::new(DiscordClient::new("dummy_token".to_string(), db.clone()).unwrap());
    let ctx = AppContext::new(db, Some(client));

    // Test initialize without version (defaults to 2025-06-18)
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {}
    });
    let resp = mcp_post(
        State(ctx.clone()),
        axum::body::Bytes::from(serde_json::to_vec(&init_req).unwrap()),
    )
    .await;
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let init_resp: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(init_resp["id"], 1);
    assert_eq!(init_resp["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init_resp["result"]["serverInfo"]["name"], "discord-context");

    // Test initialize echoing supported version
    let init_echo_req = json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26"
        }
    });
    let resp = mcp_post(
        State(ctx.clone()),
        axum::body::Bytes::from(serde_json::to_vec(&init_echo_req).unwrap()),
    )
    .await;
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let echo_resp: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(echo_resp["result"]["protocolVersion"], "2025-03-26");

    // Test notifications/initialized returns 202
    let notif_req = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    let notif_resp = mcp_post(
        State(ctx.clone()),
        axum::body::Bytes::from(serde_json::to_vec(&notif_req).unwrap()),
    )
    .await;
    assert_eq!(notif_resp.status(), axum::http::StatusCode::ACCEPTED);
    let notif_bytes = axum::body::to_bytes(notif_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(
        notif_bytes.is_empty(),
        "notification response body should be empty"
    );

    // Test any request without 'id' member is a notification returning 202 with no body (e.g. tools/list without id)
    let tools_notif_req = json!({
        "jsonrpc": "2.0",
        "method": "tools/list"
    });
    let tools_notif_resp = mcp_post(
        State(ctx.clone()),
        axum::body::Bytes::from(serde_json::to_vec(&tools_notif_req).unwrap()),
    )
    .await;
    assert_eq!(tools_notif_resp.status(), axum::http::StatusCode::ACCEPTED);
    let tools_notif_bytes = axum::body::to_bytes(tools_notif_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(
        tools_notif_bytes.is_empty(),
        "notification body must be empty"
    );

    // Test a request with id: null is also treated as a notification returning 202 with no body
    let null_id_notif_req = json!({
        "jsonrpc": "2.0",
        "id": null,
        "method": "notifications/custom_event",
        "params": {}
    });
    let null_id_resp = mcp_post(
        State(ctx.clone()),
        axum::body::Bytes::from(serde_json::to_vec(&null_id_notif_req).unwrap()),
    )
    .await;
    assert_eq!(null_id_resp.status(), axum::http::StatusCode::ACCEPTED);
    let null_id_bytes = axum::body::to_bytes(null_id_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(null_id_bytes.is_empty());

    // Test notifications/foo WITH an id returns normal JSON-RPC response, not 202
    let notif_with_id_req = json!({
        "jsonrpc": "2.0",
        "id": 99,
        "method": "notifications/custom_event",
        "params": {}
    });
    let notif_with_id_resp = mcp_post(
        State(ctx.clone()),
        axum::body::Bytes::from(serde_json::to_vec(&notif_with_id_req).unwrap()),
    )
    .await;
    assert_eq!(notif_with_id_resp.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(notif_with_id_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let notif_id_res: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(notif_id_res["id"], 99);
    assert_eq!(notif_id_res["error"]["code"], -32601);

    // Test tools/list
    let list_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });
    let resp = mcp_post(
        State(ctx.clone()),
        axum::body::Bytes::from(serde_json::to_vec(&list_req).unwrap()),
    )
    .await;
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let list_resp: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(list_resp["id"], 2);
    let tools = list_resp["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 8);

    // Test tools/call cache_stats
    let call_req = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "cache_stats",
            "arguments": {}
        }
    });
    let resp = mcp_post(
        State(ctx.clone()),
        axum::body::Bytes::from(serde_json::to_vec(&call_req).unwrap()),
    )
    .await;
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let call_resp: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(call_resp["id"], 3);
    assert!(call_resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("messages"));

    // Test unknown method
    let bad_req = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "non_existent_method"
    });
    let resp = mcp_post(
        State(ctx.clone()),
        axum::body::Bytes::from(serde_json::to_vec(&bad_req).unwrap()),
    )
    .await;
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let bad_resp: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(bad_resp["id"], 4);
    assert_eq!(bad_resp["error"]["code"], -32601);

    // Test missing/wrong jsonrpc field -> -32600 Invalid Request
    let invalid_rpc_req = json!({
        "id": 5,
        "method": "tools/list"
    });
    let resp = mcp_post(
        State(ctx.clone()),
        axum::body::Bytes::from(serde_json::to_vec(&invalid_rpc_req).unwrap()),
    )
    .await;
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let inv_resp: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(inv_resp["id"], 5);
    assert_eq!(inv_resp["error"]["code"], -32600);
    assert_eq!(inv_resp["error"]["message"], "Invalid Request");

    // Test missing or empty method field -> -32600 Invalid Request
    let missing_method_req = json!({
        "jsonrpc": "2.0",
        "id": 6
    });
    let resp = mcp_post(
        State(ctx.clone()),
        axum::body::Bytes::from(serde_json::to_vec(&missing_method_req).unwrap()),
    )
    .await;
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let missing_m_resp: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(missing_m_resp["id"], 6);
    assert_eq!(missing_m_resp["error"]["code"], -32600);
    assert_eq!(missing_m_resp["error"]["message"], "Invalid Request");

    let empty_method_req = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": ""
    });
    let resp = mcp_post(
        State(ctx.clone()),
        axum::body::Bytes::from(serde_json::to_vec(&empty_method_req).unwrap()),
    )
    .await;
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let empty_m_resp: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(empty_m_resp["id"], 7);
    assert_eq!(empty_m_resp["error"]["code"], -32600);
    assert_eq!(empty_m_resp["error"]["message"], "Invalid Request");

    // Test malformed JSON -> -32700 Parse error
    let malformed_bytes = axum::body::Bytes::from("{ this is not valid json }");
    let resp = mcp_post(State(ctx.clone()), malformed_bytes).await;
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let parse_err_resp: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parse_err_resp["error"]["code"], -32700);
    assert_eq!(parse_err_resp["error"]["message"], "Parse error");
}

#[tokio::test]
async fn test_swappable_client_observed_by_call_tool() {
    let db = Arc::new(Db::new_in_memory().expect("in-memory db failed"));
    let ctx = AppContext::new(db.clone(), None);

    // Initial state: client is None. Network tool returns clear 'no token' error.
    let res_none = call_tool(&ctx, "refresh", &json!({})).await;
    assert!(res_none.is_err());
    let err_msg = res_none.unwrap_err();
    assert!(
        err_msg.contains("No Discord token configured"),
        "expected 'No Discord token configured', got '{}'",
        err_msg
    );

    // Swap in a live client
    let client = Arc::new(DiscordClient::new("dummy_token".to_string(), db.clone()).unwrap());
    ctx.set_client(Some(client)).await;

    // Now call_tool observes the new client and does not return the 'no token' error
    let res_some = call_tool(&ctx, "refresh", &json!({})).await;
    // The request attempts network with the dummy token and may fail, but NOT with 'No Discord token configured'
    if let Err(e) = res_some {
        assert!(
            !e.contains("No Discord token configured"),
            "should not return 'no token' error after setting client, got '{}'",
            e
        );
    }
}

#[test]
fn test_is_allowed_host_or_origin() {
    use discord_context_mcp::mcp::is_allowed_host_or_origin;

    // Accepted cases
    assert!(is_allowed_host_or_origin("127.0.0.1:3774"));
    assert!(is_allowed_host_or_origin("localhost:3774"));
    assert!(is_allowed_host_or_origin("[::1]:3774"));
    assert!(is_allowed_host_or_origin("127.0.0.1"));
    assert!(is_allowed_host_or_origin("localhost"));
    assert!(is_allowed_host_or_origin("::1"));
    assert!(is_allowed_host_or_origin("http://localhost:3774"));
    assert!(is_allowed_host_or_origin("http://127.0.0.1:3774"));
    assert!(is_allowed_host_or_origin("http://[::1]:3774"));
    assert!(is_allowed_host_or_origin("https://localhost/path"));
    assert!(is_allowed_host_or_origin("localhost."));
    assert!(is_allowed_host_or_origin("localhost.:3774"));

    // Rejected cases (DNS rebinding / prefix bypass attempts)
    assert!(!is_allowed_host_or_origin("localhost:"));
    assert!(!is_allowed_host_or_origin("127.0.0.1:"));
    assert!(!is_allowed_host_or_origin("[::1]:"));
    assert!(!is_allowed_host_or_origin("localhost.evil.com"));
    assert!(!is_allowed_host_or_origin("127.0.0.1.evil.com"));
    assert!(!is_allowed_host_or_origin("http://localhost.evil.com"));
    assert!(!is_allowed_host_or_origin("http://127.0.0.1.evil.com:3774"));
    assert!(!is_allowed_host_or_origin(
        "http://localhost.attacker.com:3774"
    ));
    assert!(!is_allowed_host_or_origin("attacker-localhost:3774"));
    assert!(!is_allowed_host_or_origin("evil-127.0.0.1"));
    assert!(!is_allowed_host_or_origin("example.com"));
    assert!(!is_allowed_host_or_origin("[::2]:3774"));
    assert!(!is_allowed_host_or_origin("[2001:db8::1]:3774"));
}

#[tokio::test]
async fn test_validate_local_headers_middleware() {
    use axum::middleware;
    use axum::routing::get;
    use axum::Router;
    use discord_context_mcp::mcp::validate_local_headers;

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .layer(middleware::from_fn(validate_local_headers));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let url = format!("http://{}/health", addr);

    // Accepted request with valid host
    let resp = client
        .get(&url)
        .header("host", format!("127.0.0.1:{}", addr.port()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    // Rejected request with malicious host (DNS rebinding)
    let resp = client
        .get(&url)
        .header("host", "localhost.evil.com")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::FORBIDDEN);

    // Rejected request with malicious 127.0.0.1 prefix
    let resp = client
        .get(&url)
        .header("host", "127.0.0.1.evil.com")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::FORBIDDEN);

    // Accepted request with Host localhost (no Origin)
    let resp = client
        .get(&url)
        .header("host", "localhost")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    // Rejected request with valid host but malicious origin
    let resp = client
        .get(&url)
        .header("host", format!("127.0.0.1:{}", addr.port()))
        .header("origin", "http://localhost.evil.com")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::FORBIDDEN);

    // Accepted request with valid host and valid origin
    let resp = client
        .get(&url)
        .header("host", format!("127.0.0.1:{}", addr.port()))
        .header("origin", format!("http://127.0.0.1:{}", addr.port()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
}

#[tokio::test]
async fn test_search_messages_unknown_filter_not_dropped() {
    use discord_context_mcp::models::{DiscordChannel, DiscordGuild, DiscordMessage, DiscordUser};

    let db = Arc::new(Db::new_in_memory().expect("in-memory db failed"));
    let user = DiscordUser {
        id: "u123".to_string(),
        username: "alice".to_string(),
        global_name: Some("Alice".to_string()),
        discriminator: None,
        avatar: None,
    };
    db.upsert_user(&user).unwrap();

    let guild = DiscordGuild {
        id: "g123".to_string(),
        name: "Rust Guild".to_string(),
        icon: None,
    };
    db.upsert_guild(&guild).unwrap();

    let ch = DiscordChannel {
        id: "c123".to_string(),
        guild_id: Some("g123".to_string()),
        name: "general".to_string(),
        kind: 0,
        topic: None,
        parent_id: None,
    };
    db.upsert_channel(&ch).unwrap();

    let msg = DiscordMessage {
        id: "m123".to_string(),
        channel_id: "c123".to_string(),
        author: user.clone(),
        content: "hello world from alice".to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        edited_timestamp: None,
        attachments: vec![],
        referenced_message: None,
    };
    db.upsert_message(&msg).unwrap();

    let ctx = AppContext::new(db, None);

    // 1. Search with matching text without filter -> finds the message
    let res = call_tool(
        &ctx,
        "search_messages",
        &json!({
            "text": "hello"
        }),
    )
    .await;
    assert!(res.is_ok());
    let formatted = res.unwrap()["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        formatted.contains("hello world from alice"),
        "expected match"
    );

    // 2. Search with an unknown person filter -> must return error, NOT drop filter and return all messages
    let res_unknown = call_tool(
        &ctx,
        "search_messages",
        &json!({
            "text": "hello",
            "person": "nonexistent_bob"
        }),
    )
    .await;
    assert!(
        res_unknown.is_err(),
        "search_messages with unknown person must return Err, got: {:?}",
        res_unknown
    );
    let err_msg = res_unknown.unwrap_err();
    assert!(
        err_msg.contains("User not found"),
        "expected 'User not found' error message, got '{}'",
        err_msg
    );
}

#[tokio::test]
async fn test_search_messages_blank_text_rejected() {
    let db = Arc::new(Db::new_in_memory().expect("in-memory db failed"));
    let ctx = AppContext::new(db, None);

    let res = call_tool(
        &ctx,
        "search_messages",
        &json!({
            "text": "   "
        }),
    )
    .await;
    assert!(res.is_err());
    let err_msg = res.unwrap_err();
    assert_eq!(err_msg, "Search text must not be empty");

    // Also assert through JSON-RPC tools/call that isError and error text are returned
    let req = json!({
        "jsonrpc": "2.0",
        "id": 101,
        "method": "tools/call",
        "params": {
            "name": "search_messages",
            "arguments": {
                "text": "   "
            }
        }
    });
    let resp = discord_context_mcp::mcp::mcp_post(
        axum::extract::State(ctx),
        axum::body::Bytes::from(serde_json::to_vec(&req).unwrap()),
    )
    .await;
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["id"], 101);
    assert_eq!(body["result"]["isError"], true);
    assert_eq!(
        body["result"]["content"][0]["text"],
        "Error: Search text must not be empty"
    );
}

#[tokio::test]
async fn test_refresh_invalid_scope_rejected() {
    let db = Arc::new(Db::new_in_memory().expect("in-memory db failed"));
    let client = Arc::new(DiscordClient::new("dummy_token".to_string(), db.clone()).unwrap());
    let ctx = AppContext::new(db, Some(client));

    let res = call_tool(
        &ctx,
        "refresh",
        &json!({
            "scope": "invalid_scope"
        }),
    )
    .await;
    assert!(res.is_err());
    let err_msg = res.unwrap_err();
    assert_eq!(err_msg, "Invalid scope; expected all, servers, or dms");

    // Also assert through JSON-RPC tools/call that isError and error text are returned
    let req = json!({
        "jsonrpc": "2.0",
        "id": 102,
        "method": "tools/call",
        "params": {
            "name": "refresh",
            "arguments": {
                "scope": "invalid_scope"
            }
        }
    });
    let resp = discord_context_mcp::mcp::mcp_post(
        axum::extract::State(ctx),
        axum::body::Bytes::from(serde_json::to_vec(&req).unwrap()),
    )
    .await;
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["id"], 102);
    assert_eq!(body["result"]["isError"], true);
    assert_eq!(
        body["result"]["content"][0]["text"],
        "Error: Invalid scope; expected all, servers, or dms"
    );
}

#[tokio::test]
async fn test_search_messages_person_dm_uses_live_rest_when_dm_cached() {
    use axum::routing::get;
    use axum::Json;
    use axum::Router;
    use discord_context_mcp::models::{DiscordMessage, DiscordUser, DmChannel};

    let db = Arc::new(Db::new_in_memory().expect("in-memory db failed"));
    let user = DiscordUser {
        id: "u_dm".to_string(),
        username: "diana".to_string(),
        global_name: Some("Diana".to_string()),
        discriminator: None,
        avatar: None,
    };
    db.upsert_user(&user).unwrap();

    let dm = DmChannel {
        id: "dm1".to_string(),
        recipient: user.clone(),
        kind: 1,
    };
    db.upsert_dm_channel(&dm).unwrap();

    // Seed one cached message so stats.messages > 0 and no sync_recent network call is triggered
    let seed_msg = DiscordMessage {
        id: "m_seed".to_string(),
        channel_id: "dm1".to_string(),
        author: user.clone(),
        content: "seed msg".to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        edited_timestamp: None,
        attachments: vec![],
        referenced_message: None,
    };
    db.upsert_message(&seed_msg).unwrap();

    let mock_router = Router::new().route(
        "/channels/dm1/messages/search",
        get(|| async {
            Json(json!({
                "messages": [[
                    {
                        "id": "m_live",
                        "channel_id": "dm1",
                        "author": {
                            "id": "u_dm",
                            "username": "diana",
                            "global_name": "Diana"
                        },
                        "content": "live dm search hit",
                        "timestamp": "2026-02-01T00:00:00Z",
                        "attachments": []
                    }
                ]]
            }))
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock_router).await.unwrap();
    });

    let mut client = DiscordClient::new("test_token".to_string(), db.clone()).unwrap();
    client.base = format!("http://{}", addr);
    let ctx = AppContext::new(db, Some(Arc::new(client)));

    let res = call_tool(
        &ctx,
        "search_messages",
        &json!({
            "text": "hello",
            "person": "diana"
        }),
    )
    .await;

    assert!(res.is_ok(), "search_messages failed: {:?}", res);
    let text = res.unwrap()["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        text.contains("live dm search hit"),
        "expected live dm search hit in result text: {}",
        text
    );
}

#[tokio::test]
async fn test_search_messages_person_without_dm_falls_back_to_local_fts() {
    use axum::routing::any;
    use axum::Router;
    use discord_context_mcp::models::{DiscordMessage, DiscordUser};

    let db = Arc::new(Db::new_in_memory().expect("in-memory db failed"));
    let user = DiscordUser {
        id: "u_local".to_string(),
        username: "nobody".to_string(),
        global_name: Some("Nobody".to_string()),
        discriminator: None,
        avatar: None,
    };
    db.upsert_user(&user).unwrap();

    let msg = DiscordMessage {
        id: "m_local".to_string(),
        channel_id: "c_unknown".to_string(),
        author: user.clone(),
        content: "local fts hit".to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        edited_timestamp: None,
        attachments: vec![],
        referenced_message: None,
    };
    db.upsert_message(&msg).unwrap();
    // Do NOT seed a DM channel for that user.

    let mock_router = Router::new().fallback(any(|| async {
        (axum::http::StatusCode::NOT_FOUND, "not found")
    }));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock_router).await.unwrap();
    });

    let mut client = DiscordClient::new("test_token".to_string(), db.clone()).unwrap();
    client.base = format!("http://{}", addr);
    let ctx = AppContext::new(db, Some(Arc::new(client)));

    let res = call_tool(
        &ctx,
        "search_messages",
        &json!({
            "text": "local",
            "person": "nobody"
        }),
    )
    .await;

    assert!(res.is_ok(), "search_messages failed: {:?}", res);
    let text = res.unwrap()["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        text.contains("local fts hit"),
        "expected local fts hit in result text: {}",
        text
    );
}
