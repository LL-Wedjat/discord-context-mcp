use discord_context_mcp::db::Db;
use discord_context_mcp::discord::{
    base64_encode, compute_bucket_wait_ms, make_super_properties, normalize_route_key, Bucket,
    DiscordClient, SyncReport,
};
use std::sync::Arc;

#[test]
fn test_compute_bucket_wait_ms() {
    let now = 10000i64;

    // Remaining > 0: no wait
    let mut b1 = Bucket {
        remaining: 5,
        reset_at_ms: now + 5000,
    };
    assert_eq!(compute_bucket_wait_ms(&mut b1, now), 0);
    assert_eq!(b1.remaining, 5);

    // Remaining <= 0, reset_at_ms in future: wait delta, remaining reset to 1
    let mut b2 = Bucket {
        remaining: 0,
        reset_at_ms: now + 3500,
    };
    assert_eq!(compute_bucket_wait_ms(&mut b2, now), 3500);
    assert_eq!(b2.remaining, 1);
    assert_eq!(b2.reset_at_ms, 0);

    // Remaining <= 0, reset_at_ms in past: no wait
    let mut b3 = Bucket {
        remaining: 0,
        reset_at_ms: now - 1000,
    };
    assert_eq!(compute_bucket_wait_ms(&mut b3, now), 0);
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
