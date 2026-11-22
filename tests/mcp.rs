use std::sync::Arc;

use discord_context_mcp::db::{Db, MessageRow};
use discord_context_mcp::discord::DiscordClient;
use discord_context_mcp::mcp::{
    call_tool, format_items, format_messages, tool_definitions, AppContext,
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
    let ctx = AppContext { db, client };

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
    use axum::Json;
    use discord_context_mcp::mcp::mcp_post;

    let db = Arc::new(Db::new_in_memory().expect("in-memory db failed"));
    let client = Arc::new(DiscordClient::new("dummy_token".to_string(), db.clone()).unwrap());
    let ctx = AppContext { db, client };

    // Test initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {}
    });
    let Json(init_resp) = mcp_post(State(ctx.clone()), Json(init_req)).await;
    assert_eq!(init_resp["id"], 1);
    assert_eq!(init_resp["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init_resp["result"]["serverInfo"]["name"], "discord-context");

    // Test tools/list
    let list_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });
    let Json(list_resp) = mcp_post(State(ctx.clone()), Json(list_req)).await;
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
    let Json(call_resp) = mcp_post(State(ctx.clone()), Json(call_req)).await;
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
    let Json(bad_resp) = mcp_post(State(ctx.clone()), Json(bad_req)).await;
    assert_eq!(bad_resp["id"], 4);
    assert_eq!(bad_resp["error"]["code"], -32601);
}
