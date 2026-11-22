use discord_context_mcp::db::{open_db, SearchFilters};
use discord_context_mcp::models::{
    Attachment, DiscordChannel, DiscordGuild, DiscordMessage, DiscordUser, DmChannel,
};
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
    };
    db.upsert_message(&msg).unwrap();

    let res = db
        .search_messages("fallback", &SearchFilters::default())
        .unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].id, "m_like_1");
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

    // list_dm_people
    let dms = db.list_dm_people(Some("Charlie")).unwrap();
    assert_eq!(dms.len(), 1);
    assert_eq!(dms[0].recipient.username, "charlie");
}
