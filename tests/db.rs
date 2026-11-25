use discord_context_mcp::db::{open_db, Db, SearchFilters};
use discord_context_mcp::models::{
    Attachment, DiscordChannel, DiscordGuild, DiscordMessage, DiscordUser, DmChannel,
};
use rusqlite::OptionalExtension;
use tempfile::tempdir;

#[test]
fn test_db_crud_and_stats() {
    let tmp = tempdir().unwrap();
    let db = open_db(tmp.path()).unwrap();

    let user1 = DiscordUser {
        id: "u1".to_string(),
        username: "alice".to_string(),
        global_name: Some("Alice In Chains".to_string()),
        discriminator: Some("0001".to_string()),
        avatar: Some("av1".to_string()),
    };

    let user2 = DiscordUser {
        id: "u2".to_string(),
        username: "bob".to_string(),
        global_name: None,
        discriminator: Some("0002".to_string()),
        avatar: None,
    };

    db.upsert_user(&user1).unwrap();
    db.upsert_user(&user2).unwrap();

    let guild1 = DiscordGuild {
        id: "g1".to_string(),
        name: "General Server".to_string(),
        icon: Some("icon1".to_string()),
    };
    db.upsert_guild(&guild1).unwrap();

    let ch1 = DiscordChannel {
        id: "c1".to_string(),
        guild_id: Some("g1".to_string()),
        name: "general".to_string(),
        kind: 0,
        topic: Some("Chat here".to_string()),
        parent_id: None,
    };
    db.upsert_channel(&ch1).unwrap();

    let dm1 = DmChannel {
        id: "dm1".to_string(),
        recipient: user2.clone(),
        kind: 1,
    };
    db.upsert_dm_channel(&dm1).unwrap();

    let msg1 = DiscordMessage {
        id: "m1".to_string(),
        channel_id: "c1".to_string(),
        author: user1.clone(),
        content: "Hello world rust rewrite".to_string(),
        timestamp: "2026-11-22T10:00:00Z".to_string(),
        edited_timestamp: None,
        attachments: vec![Attachment {
            id: "att1".to_string(),
            filename: "file.png".to_string(),
            url: "http://example.com/file.png".to_string(),
            proxy_url: None,
        }],
        referenced_message: None,
        message_reference: None,
    };
    db.upsert_message(&msg1).unwrap();

    let stats = db.stats().unwrap();
    assert_eq!(stats.users, 2);
    assert_eq!(stats.guilds, 1);
    assert_eq!(stats.channels, 1);
    assert_eq!(stats.dm_channels, 1);
    assert_eq!(stats.messages, 1);
}

#[test]
fn test_upsert_guild_null_name_preserves_existing() {
    let tmp = tempdir().unwrap();
    let db = open_db(tmp.path()).unwrap();

    let g_orig = DiscordGuild {
        id: "g_persist".to_string(),
        name: "Awesome Server".to_string(),
        icon: Some("icon.png".to_string()),
    };
    db.upsert_guild(&g_orig).unwrap();

    // Now upsert with empty / null name
    let g_stale = DiscordGuild {
        id: "g_persist".to_string(),
        name: "".to_string(),
        icon: None,
    };
    db.upsert_guild(&g_stale).unwrap();

    let loaded = db
        .find_guild("g_persist")
        .unwrap()
        .expect("guild should exist");
    assert_eq!(loaded.name, "Awesome Server");
    assert_eq!(loaded.icon, Some("icon.png".to_string()));
}

#[test]
fn test_prune_stale_guilds() {
    let tmp = tempdir().unwrap();
    let db = open_db(tmp.path()).unwrap();

    let g1 = DiscordGuild {
        id: "g_keep".to_string(),
        name: "Keep Me".to_string(),
        icon: None,
    };
    let g2 = DiscordGuild {
        id: "g_drop".to_string(),
        name: "Drop Me".to_string(),
        icon: None,
    };
    db.upsert_guild(&g1).unwrap();
    db.upsert_guild(&g2).unwrap();

    let c1 = DiscordChannel {
        id: "c_keep".to_string(),
        guild_id: Some("g_keep".to_string()),
        name: "general".to_string(),
        kind: 0,
        topic: None,
        parent_id: None,
    };
    let c2 = DiscordChannel {
        id: "c_drop".to_string(),
        guild_id: Some("g_drop".to_string()),
        name: "trash".to_string(),
        kind: 0,
        topic: None,
        parent_id: None,
    };
    db.upsert_channel(&c1).unwrap();
    db.upsert_channel(&c2).unwrap();

    let pruned = db.prune_guilds_not_in(&["g_keep".to_string()]).unwrap();
    assert_eq!(pruned, 2); // 1 guild + 1 channel

    let guilds = db.list_guilds(None).unwrap();
    assert_eq!(guilds.len(), 1);
    assert_eq!(guilds[0].id, "g_keep");

    let channels_drop = db.list_channels("g_drop", None).unwrap();
    assert_eq!(channels_drop.len(), 0);

    let channels_keep = db.list_channels("g_keep", None).unwrap();
    assert_eq!(channels_keep.len(), 1);
}

#[test]
fn test_get_messages_ordering_and_pagination() {
    let tmp = tempdir().unwrap();
    let db = open_db(tmp.path()).unwrap();

    let author = DiscordUser {
        id: "u_msg".to_string(),
        username: "tester".to_string(),
        global_name: None,
        discriminator: None,
        avatar: None,
    };

    for i in 1..=5 {
        let msg = DiscordMessage {
            id: format!("m{}", i),
            channel_id: "c_test".to_string(),
            author: author.clone(),
            content: format!("Message number {}", i),
            timestamp: format!("2026-11-22T10:0{}:00Z", i),
            edited_timestamp: None,
            attachments: vec![],
            referenced_message: None,
        message_reference: None,
        };
        db.upsert_message(&msg).unwrap();
    }

    // Default get_messages returns oldest to newest
    let msgs = db.get_messages("c_test", Some(10), None, None).unwrap();
    assert_eq!(msgs.len(), 5);
    assert_eq!(msgs[0].id, "m1");
    assert_eq!(msgs[4].id, "m5");

    // Before m4 should give m1, m2, m3
    let msgs_before = db
        .get_messages("c_test", Some(10), Some("m4"), None)
        .unwrap();
    assert_eq!(msgs_before.len(), 3);
    assert_eq!(msgs_before[0].id, "m1");
    assert_eq!(msgs_before[2].id, "m3");

    // After m2 should give m3, m4, m5
    let msgs_after = db
        .get_messages("c_test", Some(10), None, Some("m2"))
        .unwrap();
    assert_eq!(msgs_after.len(), 3);
    assert_eq!(msgs_after[0].id, "m3");
    assert_eq!(msgs_after[2].id, "m5");
}

#[test]
fn test_search_messages_and_filters() {
    let tmp = tempdir().unwrap();
    let db = open_db(tmp.path()).unwrap();

    let user = DiscordUser {
        id: "u_search".to_string(),
        username: "searcher".to_string(),
        global_name: Some("Search Master".to_string()),
        discriminator: None,
        avatar: None,
    };

    let msg1 = DiscordMessage {
        id: "sm1".to_string(),
        channel_id: "c_s1".to_string(),
        author: user.clone(),
        content: "Apples and oranges are delicious fruits".to_string(),
        timestamp: "2026-11-22T10:01:00Z".to_string(),
        edited_timestamp: None,
        attachments: vec![],
        referenced_message: None,
        message_reference: None,
    };

    let msg2 = DiscordMessage {
        id: "sm2".to_string(),
        channel_id: "c_s2".to_string(),
        author: user.clone(),
        content: "Apples are red and sweet".to_string(),
        timestamp: "2026-11-22T10:02:00Z".to_string(),
        edited_timestamp: None,
        attachments: vec![],
        referenced_message: None,
        message_reference: None,
    };

    db.upsert_message(&msg1).unwrap();
    db.upsert_message(&msg2).unwrap();

    // Search "apples" across channels
    let res = db
        .search_messages("apples", &SearchFilters::default())
        .unwrap();
    assert_eq!(res.len(), 2);
    // Newest first
    assert_eq!(res[0].id, "sm2");
    assert_eq!(res[1].id, "sm1");

    // Search "oranges"
    let res_orange = db
        .search_messages("oranges", &SearchFilters::default())
        .unwrap();
    assert_eq!(res_orange.len(), 1);
    assert_eq!(res_orange[0].id, "sm1");

    // Filter by channel
    let filters = SearchFilters {
        channel_id: Some("c_s1".to_string()),
        ..Default::default()
    };
    let res_filtered = db.search_messages("apples", &filters).unwrap();
    assert_eq!(res_filtered.len(), 1);
    assert_eq!(res_filtered[0].id, "sm1");
}

#[test]
fn test_search_messages_like_path() {
    let tmp = tempdir().unwrap();
    let db = open_db(tmp.path()).unwrap();

    // Force fts_mode to 'like'
    {
        let conn = rusqlite::Connection::open(tmp.path().join("cache.db")).unwrap();
        conn.execute("UPDATE meta SET value = 'like' WHERE key = 'fts_mode'", [])
            .unwrap();
    }

    let user = DiscordUser {
        id: "u_like".to_string(),
        username: "liketester".to_string(),
        global_name: None,
        discriminator: None,
        avatar: None,
    };

    let msg = DiscordMessage {
        id: "m_like_1".to_string(),
        channel_id: "c_like".to_string(),
        author: user,
        content: "Testing like fallback search capability".to_string(),
        timestamp: "2026-11-22T10:00:00Z".to_string(),
        edited_timestamp: None,
        attachments: vec![],
        referenced_message: None,
        message_reference: None,
    };
    db.upsert_message(&msg).unwrap();

    let res = db
        .search_messages("fallback", &SearchFilters::default())
        .unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].id, "m_like_1");

    // Test search with literal backslash in LIKE fallback
    let user_bs = DiscordUser {
        id: "u_bs".to_string(),
        username: "bs_user".to_string(),
        global_name: None,
        discriminator: None,
        avatar: None,
    };
    let msg_bs = DiscordMessage {
        id: "m_bs_1".to_string(),
        channel_id: "c_like".to_string(),
        author: user_bs,
        content: r"C:\Windows\System32\cmd.exe path".to_string(),
        timestamp: "2026-11-22T10:05:00Z".to_string(),
        edited_timestamp: None,
        attachments: vec![],
        referenced_message: None,
        message_reference: None,
    };
    db.upsert_message(&msg_bs).unwrap();

    let res_bs = db
        .search_messages(r"Windows\System32", &SearchFilters::default())
        .unwrap();
    assert_eq!(res_bs.len(), 1);
    assert_eq!(res_bs[0].id, "m_bs_1");
}

#[test]
fn test_find_and_list_helpers() {
    let tmp = tempdir().unwrap();
    let db = open_db(tmp.path()).unwrap();

    let u = DiscordUser {
        id: "12345".to_string(),
        username: "charlie".to_string(),
        global_name: Some("Charlie Brown".to_string()),
        discriminator: None,
        avatar: None,
    };
    db.upsert_user(&u).unwrap();

    let g = DiscordGuild {
        id: "g99".to_string(),
        name: "Peanuts Club".to_string(),
        icon: None,
    };
    db.upsert_guild(&g).unwrap();

    let ch = DiscordChannel {
        id: "c99".to_string(),
        guild_id: Some("g99".to_string()),
        name: "snoopy-lounge".to_string(),
        kind: 0,
        topic: None,
        parent_id: None,
    };
    db.upsert_channel(&ch).unwrap();

    let dm = DmChannel {
        id: "dm99".to_string(),
        recipient: u.clone(),
        kind: 1,
    };
    db.upsert_dm_channel(&dm).unwrap();

    // find_user
    assert_eq!(db.find_user("12345").unwrap().unwrap().username, "charlie");
    assert_eq!(db.find_user("charlie").unwrap().unwrap().id, "12345");
    assert_eq!(db.find_user("Brown").unwrap().unwrap().id, "12345");

    // find_guild
    assert_eq!(db.find_guild("g99").unwrap().unwrap().name, "Peanuts Club");
    assert_eq!(db.find_guild("peanuts").unwrap().unwrap().id, "g99");

    // find_channel
    assert_eq!(
        db.find_channel("g99", "c99").unwrap().unwrap().name,
        "snoopy-lounge"
    );
    assert_eq!(
        db.find_channel("g99", "#snoopy-lounge")
            .unwrap()
            .unwrap()
            .id,
        "c99"
    );
    assert_eq!(db.find_channel("g99", "snoopy").unwrap().unwrap().id, "c99");

    // Test literal backslash in find_guild, find_channel, find_user
    let u_slash = DiscordUser {
        id: "u_slash".to_string(),
        username: r"foo\bar".to_string(),
        global_name: Some(r"Slash\User".to_string()),
        discriminator: None,
        avatar: None,
    };
    db.upsert_user(&u_slash).unwrap();

    let g_slash = DiscordGuild {
        id: "g_slash".to_string(),
        name: r"Guild\Slash".to_string(),
        icon: None,
    };
    db.upsert_guild(&g_slash).unwrap();

    let ch_slash = DiscordChannel {
        id: "ch_slash".to_string(),
        guild_id: Some("g_slash".to_string()),
        name: r"chan\slash".to_string(),
        kind: 0,
        topic: None,
        parent_id: None,
    };
    db.upsert_channel(&ch_slash).unwrap();

    assert_eq!(
        db.find_guild(r"Guild\Slash").unwrap().unwrap().id,
        "g_slash"
    );
    assert_eq!(db.find_guild(r"ild\Sla").unwrap().unwrap().id, "g_slash");
    assert_eq!(db.find_user(r"foo\bar").unwrap().unwrap().id, "u_slash");
    assert_eq!(db.find_user(r"oo\ba").unwrap().unwrap().id, "u_slash");
    assert_eq!(
        db.find_channel("g_slash", r"chan\slash")
            .unwrap()
            .unwrap()
            .id,
        "ch_slash"
    );
    assert_eq!(
        db.find_channel("g_slash", r"an\sla").unwrap().unwrap().id,
        "ch_slash"
    );

    // list_dm_people
    let dms = db.list_dm_people(Some("Charlie")).unwrap();
    assert_eq!(dms.len(), 1);
    assert_eq!(dms[0].recipient.username, "charlie");
}

#[test]
fn test_get_messages_invalid_anchor_returns_error() {
    let tmp = tempdir().unwrap();
    let db = open_db(tmp.path()).unwrap();

    let res = db.get_messages("c_any", Some(10), Some("non_existent_msg_id"), None);
    assert!(res.is_err());
    assert!(res.unwrap_err().to_string().contains("not found in cache"));

    let res_after = db.get_messages("c_any", Some(10), None, Some("non_existent_msg_id"));
    assert!(res_after.is_err());
    assert!(res_after
        .unwrap_err()
        .to_string()
        .contains("not found in cache"));
}

#[test]
fn test_fts5_order_word_not_mangled() {
    use discord_context_mcp::db::escape_fts5_query;

    let escaped = escape_fts5_query("order status");
    assert_eq!(escaped, "\"order\" AND \"status\"");

    let tmp = tempdir().unwrap();
    let db = open_db(tmp.path()).unwrap();

    let user = DiscordUser {
        id: "u_order".to_string(),
        username: "shopper".to_string(),
        global_name: None,
        discriminator: None,
        avatar: None,
    };

    let msg = DiscordMessage {
        id: "m_order_1".to_string(),
        channel_id: "c_order".to_string(),
        author: user,
        content: "Your ORDER has been processed successfully".to_string(),
        timestamp: "2026-11-22T10:00:00Z".to_string(),
        edited_timestamp: None,
        attachments: vec![],
        referenced_message: None,
        message_reference: None,
    };
    db.upsert_message(&msg).unwrap();

    let res = db
        .search_messages("order", &SearchFilters::default())
        .unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].id, "m_order_1");
}

#[test]
fn test_partial_message_update_and_created_at_preservation() {
    use discord_context_mcp::models::PartialDiscordMessage;

    let tmp = tempdir().unwrap();
    let db = open_db(tmp.path()).unwrap();

    let author = DiscordUser {
        id: "u_msg_update".to_string(),
        username: "editor".to_string(),
        global_name: None,
        discriminator: None,
        avatar: None,
    };

    let orig_msg = DiscordMessage {
        id: "m_update_1".to_string(),
        channel_id: "c_update".to_string(),
        author: author.clone(),
        content: "Original text".to_string(),
        timestamp: "2026-11-22T12:00:00Z".to_string(),
        edited_timestamp: None,
        attachments: vec![],
        referenced_message: None,
        message_reference: None,
    };
    db.upsert_message(&orig_msg).unwrap();

    let messages = db.get_messages("c_update", None, None, None).unwrap();
    assert_eq!(messages.len(), 1);
    let orig_created_at = messages[0].created_at_ms;

    // Partial update from Gateway (only content and edited_timestamp, no author/timestamp)
    let partial = PartialDiscordMessage {
        id: "m_update_1".to_string(),
        channel_id: None,
        author: None,
        content: Some("Edited text via partial update".to_string()),
        timestamp: None,
        edited_timestamp: Some("2026-11-22T12:05:00Z".to_string()),
        attachments: None,
        referenced_message: None,
        message_reference: None,
    };
    db.update_partial_message(&partial).unwrap();

    let updated = db.get_messages("c_update", None, None, None).unwrap();
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].content, "Edited text via partial update");
    assert_eq!(
        updated[0].edited_timestamp,
        Some("2026-11-22T12:05:00Z".to_string())
    );
    assert_eq!(updated[0].created_at_ms, orig_created_at);
    assert_eq!(updated[0].author_username, Some("editor".to_string()));

    // Test upsert_message ON CONFLICT preserves original created_at_ms
    let duplicate_upsert = DiscordMessage {
        id: "m_update_1".to_string(),
        channel_id: "c_update".to_string(),
        author: author.clone(),
        content: "Duplicate upsert with synthetic timestamp".to_string(),
        timestamp: "2026-11-22T13:00:00Z".to_string(),
        edited_timestamp: Some("2026-11-22T13:00:00Z".to_string()),
        attachments: vec![],
        referenced_message: None,
        message_reference: None,
    };
    db.upsert_message(&duplicate_upsert).unwrap();

    let after_dup = db.get_messages("c_update", None, None, None).unwrap();
    assert_eq!(after_dup.len(), 1);
    assert_eq!(after_dup[0].created_at_ms, orig_created_at);
}

#[test]
fn test_dm_channels_migration_and_kind_persistence() {
    let tmp = tempdir().unwrap();
    let db_path = tmp.path().join("cache.db");

    // Create legacy database without 'kind' column in dm_channels
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE users (
                 id TEXT PRIMARY KEY,
                 username TEXT,
                 global_name TEXT,
                 discriminator TEXT,
                 avatar TEXT,
                 updated_at INTEGER
             );
             CREATE TABLE dm_channels (
                 id TEXT PRIMARY KEY,
                 user_id TEXT,
                 name TEXT,
                 updated_at INTEGER
             );
             INSERT INTO users VALUES ('u_legacy', 'legacy_user', 'Legacy', '0001', NULL, 1000);
             INSERT INTO dm_channels VALUES ('dm_legacy', 'u_legacy', 'Legacy', 1000);",
        )
        .unwrap();
    }

    // open_db should migrate and add `kind` with DEFAULT 0
    let db = open_db(tmp.path()).unwrap();

    // Verify existing dm has kind == 0, so find_dm_channels_exact (WHERE kind = 1) does NOT match it
    let found = db.find_dm_channels_exact("legacy_user").unwrap();
    assert_eq!(found.len(), 0);

    // Verify list_dm_people returns it with kind == 0
    let people = db.list_dm_people(None).unwrap();
    assert_eq!(people.len(), 1);
    assert_eq!(people[0].kind, 0);

    // Upserting with kind = 1 updates kind on conflict
    let dm_updated = DmChannel {
        id: "dm_legacy".to_string(),
        recipient: DiscordUser {
            id: "u_legacy".to_string(),
            username: "legacy_user".to_string(),
            global_name: Some("Legacy".to_string()),
            discriminator: Some("0001".to_string()),
            avatar: None,
        },
        kind: 1,
    };
    db.upsert_dm_channel(&dm_updated).unwrap();

    // Now find_dm_channels_exact matches it
    let found_now = db.find_dm_channels_exact("legacy_user").unwrap();
    assert_eq!(found_now.len(), 1);
    assert_eq!(found_now[0].kind, 1);
}

#[test]
fn test_get_message_by_id_and_find_exact_helpers() {
    let tmp = tempdir().unwrap();
    let db = open_db(tmp.path()).unwrap();

    let user = DiscordUser {
        id: "u_exact".to_string(),
        username: "exact_user".to_string(),
        global_name: Some("Exact Display".to_string()),
        discriminator: None,
        avatar: None,
    };
    db.upsert_user(&user).unwrap();

    let guild1 = DiscordGuild {
        id: "g_exact1".to_string(),
        name: "Test Guild".to_string(),
        icon: None,
    };
    let guild2 = DiscordGuild {
        id: "g_exact2".to_string(),
        name: "Test Guild".to_string(), // duplicate name for >1 test
        icon: None,
    };
    db.upsert_guild(&guild1).unwrap();
    db.upsert_guild(&guild2).unwrap();

    let ch1 = DiscordChannel {
        id: "c_exact1".to_string(),
        guild_id: Some("g_exact1".to_string()),
        name: "general".to_string(),
        kind: 0,
        topic: None,
        parent_id: None,
    };
    let ch2 = DiscordChannel {
        id: "c_exact2".to_string(),
        guild_id: Some("g_exact1".to_string()),
        name: "general".to_string(), // duplicate name in same guild
        kind: 0,
        topic: None,
        parent_id: None,
    };
    db.upsert_channel(&ch1).unwrap();
    db.upsert_channel(&ch2).unwrap();

    let dm1 = DmChannel {
        id: "dm_exact1".to_string(),
        recipient: user.clone(),
        kind: 1,
    };
    let dm_group = DmChannel {
        id: "dm_group".to_string(),
        recipient: user.clone(),
        kind: 3, // Group DM
    };
    db.upsert_dm_channel(&dm1).unwrap();
    db.upsert_dm_channel(&dm_group).unwrap();

    let msg = DiscordMessage {
        id: "m_exact_123".to_string(),
        channel_id: "c_exact1".to_string(),
        author: user.clone(),
        content: "Exact message content".to_string(),
        timestamp: "2026-11-22T10:00:00Z".to_string(),
        edited_timestamp: None,
        attachments: vec![],
        referenced_message: None,
        message_reference: None,
    };
    db.upsert_message(&msg).unwrap();

    // get_message_by_id
    assert!(db.get_message_by_id("non_existent").unwrap().is_none());
    let fetched = db.get_message_by_id("m_exact_123").unwrap().unwrap();
    assert_eq!(fetched.id, "m_exact_123");
    assert_eq!(fetched.content, "Exact message content");
    assert_eq!(fetched.author_id, "u_exact");

    // find_guilds_exact
    assert_eq!(db.find_guilds_exact("").unwrap().len(), 0);
    assert_eq!(db.find_guilds_exact("   ").unwrap().len(), 0);
    assert_eq!(db.find_guilds_exact("g_exact1").unwrap().len(), 1);
    assert_eq!(db.find_guilds_exact("test guild").unwrap().len(), 2); // case-insensitive, 2 matches
    assert_eq!(db.find_guilds_exact("no_match").unwrap().len(), 0);

    // find_channels_exact
    assert_eq!(db.find_channels_exact("g_exact1", "").unwrap().len(), 0);
    assert_eq!(
        db.find_channels_exact("g_exact1", "#general")
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        db.find_channels_exact("g_exact1", "c_exact1")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        db.find_channels_exact("g_exact1", "non_existent")
            .unwrap()
            .len(),
        0
    );

    // find_dm_channels_exact
    assert_eq!(db.find_dm_channels_exact("").unwrap().len(), 0);
    assert_eq!(db.find_dm_channels_exact("exact_user").unwrap().len(), 1);
    assert_eq!(
        db.find_dm_channels_exact("exact display").unwrap().len(),
        1
    );
    assert_eq!(db.find_dm_channels_exact("dm_exact1").unwrap().len(), 1);
    // Group DM (kind == 3) must NOT be returned
    assert_eq!(db.find_dm_channels_exact("dm_group").unwrap().len(), 0);
}

#[test]
fn test_reply_to_fallback_and_tie_break_pagination() {
    use discord_context_mcp::models::{MessageReference, PartialDiscordMessage};

    let tmp = tempdir().unwrap();
    let db = open_db(tmp.path()).unwrap();

    let author = DiscordUser {
        id: "u_reply".to_string(),
        username: "reply_user".to_string(),
        global_name: None,
        discriminator: None,
        avatar: None,
    };

    // Upsert with message_reference fallback
    let msg_with_ref = DiscordMessage {
        id: "m_reply_1".to_string(),
        channel_id: "c_reply".to_string(),
        author: author.clone(),
        content: "Message replying via reference".to_string(),
        timestamp: "2026-11-22T10:00:00Z".to_string(),
        edited_timestamp: None,
        attachments: vec![],
        referenced_message: None,
        message_reference: Some(MessageReference {
            message_id: Some("orig_msg_999".to_string()),
            channel_id: Some("c_reply".to_string()),
            guild_id: None,
        }),
    };
    db.upsert_message(&msg_with_ref).unwrap();

    let fetched = db.get_message_by_id("m_reply_1").unwrap().unwrap();
    assert_eq!(fetched.reply_to, Some("orig_msg_999".to_string()));

    // Partial update with message_reference fallback
    let partial = PartialDiscordMessage {
        id: "m_reply_1".to_string(),
        channel_id: None,
        author: None,
        content: Some("Updated reply content".to_string()),
        timestamp: None,
        edited_timestamp: None,
        attachments: None,
        referenced_message: None,
        message_reference: Some(MessageReference {
            message_id: Some("new_ref_888".to_string()),
            channel_id: None,
            guild_id: None,
        }),
    };
    db.update_partial_message(&partial).unwrap();
    let fetched_updated = db.get_message_by_id("m_reply_1").unwrap().unwrap();
    assert_eq!(fetched_updated.reply_to, Some("new_ref_888".to_string()));

    // Test same-millisecond tie-break pagination
    // 3 messages with the EXACT same timestamp but distinct snowflake IDs
    let fixed_ts = "2026-11-22T10:00:00Z";
    let m1 = DiscordMessage {
        id: "100000000000000001".to_string(),
        channel_id: "c_tie".to_string(),
        author: author.clone(),
        content: "First".to_string(),
        timestamp: fixed_ts.to_string(),
        edited_timestamp: None,
        attachments: vec![],
        referenced_message: None,
        message_reference: None,
    };
    let m2 = DiscordMessage {
        id: "100000000000000002".to_string(),
        channel_id: "c_tie".to_string(),
        author: author.clone(),
        content: "Second".to_string(),
        timestamp: fixed_ts.to_string(),
        edited_timestamp: None,
        attachments: vec![],
        referenced_message: None,
        message_reference: None,
    };
    let m3 = DiscordMessage {
        id: "100000000000000003".to_string(),
        channel_id: "c_tie".to_string(),
        author: author.clone(),
        content: "Third".to_string(),
        timestamp: fixed_ts.to_string(),
        edited_timestamp: None,
        attachments: vec![],
        referenced_message: None,
        message_reference: None,
    };
    db.upsert_message(&m1).unwrap();
    db.upsert_message(&m2).unwrap();
    db.upsert_message(&m3).unwrap();

    // Query 'before' m3 -> should tie-break on ID and return m1, m2
    let tie_before = db
        .get_messages("c_tie", Some(10), Some("100000000000000003"), None)
        .unwrap();
    assert_eq!(tie_before.len(), 2);
    assert_eq!(tie_before[0].id, "100000000000000001");
    assert_eq!(tie_before[1].id, "100000000000000002");

    // Uncached valid snowflake before (e.g. 999999999999999999 >= 4_194_304)
    let uncached_before = db
        .get_messages("c_tie", Some(10), Some("999999999999999999"), None)
        .unwrap();
    assert_eq!(uncached_before.len(), 3);
}

fn seed_account_data(db: &Db, tag: &str) {
    let user = DiscordUser {
        id: format!("u_{}", tag),
        username: format!("user_{}", tag),
        global_name: None,
        discriminator: None,
        avatar: None,
    };
    db.upsert_user(&user).unwrap();

    let guild = DiscordGuild {
        id: format!("g_{}", tag),
        name: format!("Guild {}", tag),
        icon: None,
    };
    db.upsert_guild(&guild).unwrap();

    let channel = DiscordChannel {
        id: format!("c_{}", tag),
        guild_id: Some(guild.id.clone()),
        name: "general".to_string(),
        kind: 0,
        topic: None,
        parent_id: None,
    };
    db.upsert_channel(&channel).unwrap();

    let dm = DmChannel {
        id: format!("dm_{}", tag),
        recipient: user.clone(),
        kind: 1,
    };
    db.upsert_dm_channel(&dm).unwrap();

    let msg = DiscordMessage {
        id: format!("m_{}", tag),
        channel_id: channel.id,
        author: user,
        content: format!("message from {}", tag),
        timestamp: "2026-11-22T10:00:00Z".to_string(),
        edited_timestamp: None,
        attachments: vec![],
        referenced_message: None,
        message_reference: None,
    };
    db.upsert_message(&msg).unwrap();
}

fn read_meta(db_path: &std::path::Path, key: &str) -> Option<String> {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .optional()
        .unwrap()
}

fn fts_row_count(db_path: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.query_row("SELECT COUNT(*) FROM messages_fts", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn test_ensure_account_clears_legacy_cache_and_records_account() {
    let tmp = tempdir().unwrap();
    let db_path = tmp.path().join("cache.db");
    let db = open_db(tmp.path()).unwrap();
    seed_account_data(&db, "old");

    let fts_mode_before = db.stats().unwrap().fts_mode;
    assert!(read_meta(&db_path, "account_user_id").is_none());

    assert!(db.ensure_account("user_1").unwrap());

    let stats = db.stats().unwrap();
    assert_eq!(stats.users, 0);
    assert_eq!(stats.guilds, 0);
    assert_eq!(stats.channels, 0);
    assert_eq!(stats.dm_channels, 0);
    assert_eq!(stats.messages, 0);
    assert_eq!(stats.fts_mode, fts_mode_before);
    assert_eq!(read_meta(&db_path, "account_user_id"), Some("user_1".to_string()));
    assert_eq!(fts_row_count(&db_path), 0);
}

#[test]
fn test_ensure_account_same_account_preserves_rows() {
    let tmp = tempdir().unwrap();
    let db_path = tmp.path().join("cache.db");
    let db = open_db(tmp.path()).unwrap();
    seed_account_data(&db, "old");
    assert!(db.ensure_account("user_1").unwrap());

    seed_account_data(&db, "user_1");
    let before = db.stats().unwrap();
    assert_eq!(before.messages, 1);

    assert!(!db.ensure_account("user_1").unwrap());

    assert_eq!(db.stats().unwrap(), before);
    assert!(db.get_message_by_id("m_user_1").unwrap().is_some());
    assert_eq!(read_meta(&db_path, "account_user_id"), Some("user_1".to_string()));
}

#[test]
fn test_ensure_account_switch_clears_previous_account() {
    let tmp = tempdir().unwrap();
    let db_path = tmp.path().join("cache.db");
    let db = open_db(tmp.path()).unwrap();

    seed_account_data(&db, "old");
    assert!(db.ensure_account("user_1").unwrap());
    seed_account_data(&db, "user_1");
    let fts_mode_before = db.stats().unwrap().fts_mode;
    assert_eq!(db.stats().unwrap().messages, 1);
    assert_eq!(fts_row_count(&db_path), 1);

    assert!(db.ensure_account("user_2").unwrap());

    let stats = db.stats().unwrap();
    assert_eq!(stats.users, 0);
    assert_eq!(stats.guilds, 0);
    assert_eq!(stats.channels, 0);
    assert_eq!(stats.dm_channels, 0);
    assert_eq!(stats.messages, 0);
    assert_eq!(stats.fts_mode, fts_mode_before);
    assert_eq!(read_meta(&db_path, "account_user_id"), Some("user_2".to_string()));
    assert_eq!(fts_row_count(&db_path), 0);

    // A repeated call for the same new account is a no-op.
    assert!(!db.ensure_account("user_2").unwrap());
}
