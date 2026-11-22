/**
 * @fileoverview Offline unit tests for discord-context-mcp.
 */

import os from 'node:os';
import path from 'node:path';
import fs from 'node:fs';
import assert from 'node:assert/strict';

const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'discord-mcp-test-'));
process.env.DISCORD_CONTEXT_OFFLINE = '1';
process.env.DISCORD_CONTEXT_DATA_DIR = tempDir;
process.env.DISCORD_CONTEXT_GATEWAY = '0';

async function run() {
  const {
    openDb,
    upsertUser,
    upsertGuild,
    upsertChannel,
    upsertDmChannel,
    upsertMessage,
    pruneGuildsNotIn,
    getMessages,
    searchMessages,
    listGuilds,
    listChannels,
    listDmPeople,
    findUser,
    stats
  } = await import('../src/db.js');
  const { formatMessages } = await import('../src/format.js');

  const db = openDb(tempDir);

  try {
    // 2 users (one named 'Theo')
    const user1 = { id: '1001', username: 'theo_dev', global_name: 'Theo' };
    const user2 = { id: '1002', username: 'bob_builder', global_name: 'Bob' };
    upsertUser(db, user1);
    upsertUser(db, user2);

    // 1 guild 'Test Server'
    const guild1 = { id: '2001', name: 'Test Server' };
    upsertGuild(db, guild1);

    // 2 channels ('general' type 0, 'random' type 0)
    const chan1 = { id: '3001', guild_id: '2001', name: 'general', type: 0 };
    const chan2 = { id: '3002', guild_id: '2001', name: 'random', type: 0 };
    upsertChannel(db, chan1);
    upsertChannel(db, chan2);

    // 1 dm channel linked to Theo
    const dmChanId = '4001';
    upsertDmChannel(db, dmChanId, user1.id, 'Theo');

    // ~5 messages including one with content 'implement the feature' and an attachment
    const now = Date.now();
    const msgs = [
      { id: '5001', channel_id: chan1.id, author_id: user2.id, content: 'hello world', created_at_ms: now - 5000 },
      { id: '5002', channel_id: chan1.id, author_id: user1.id, content: 'hey Bob', created_at_ms: now - 4000 },
      {
        id: '5003',
        channel_id: dmChanId,
        author_id: user1.id,
        content: 'implement the feature',
        attachments_json: [{ filename: 'spec.pdf', url: 'https://cdn.discordapp.com/attachments/spec.pdf' }],
        created_at_ms: now - 3000
      },
      { id: '5004', channel_id: dmChanId, author_id: user2.id, content: 'on it right now', created_at_ms: now - 2000 },
      { id: '5005', channel_id: chan2.id, author_id: user2.id, content: 'random chit chat', created_at_ms: now - 1000 }
    ];

    for (const m of msgs) {
      upsertMessage(db, m);
    }

    // 1. stats counts
    const s = stats(db);
    assert.equal(s.users, 2, 'Stats: users count should be 2');
    assert.equal(s.guilds, 1, 'Stats: guilds count should be 1');
    assert.equal(s.channels, 2, 'Stats: channels count should be 2');
    assert.equal(s.dm_channels, 1, 'Stats: dm_channels count should be 1');
    assert.equal(s.messages, 5, 'Stats: messages count should be 5');
    console.log('PASS: stats counts');

    // 2. listGuilds('Test') finds it
    const guilds = listGuilds(db, 'Test');
    assert.equal(guilds.length, 1);
    assert.equal(guilds[0].name, 'Test Server');
    console.log('PASS: listGuilds finds Test');

    // 3. listChannels finds 'general'
    const channels = listChannels(db, guild1.id, 'general');
    assert.equal(channels.length, 1);
    assert.equal(channels[0].name, 'general');
    console.log('PASS: listChannels finds general');

    // 4. listDmPeople finds Theo
    const dmPeople = listDmPeople(db, 'Theo');
    assert.equal(dmPeople.length, 1);
    assert.equal(dmPeople[0].userId, '1001');
    console.log('PASS: listDmPeople finds Theo');

    // 5. findUser('Theo') works
    const foundTheo = findUser(db, 'Theo');
    assert.ok(foundTheo, 'findUser should find Theo');
    assert.equal(foundTheo.id, '1001');
    console.log('PASS: findUser Theo');

    // 6. getMessages returns 5 oldest->newest (seed 5 in one channel or fetch all)
    // Note: prompt says "getMessages returns 5 oldest->newest"
    // Let's seed a channel with 5 messages or test getMessages on a channel with 5 messages
    // Let's also add 3 more messages to dmChanId or chan1 so it has 5
    const dmMsgExtra1 = { id: '5006', channel_id: dmChanId, author_id: user1.id, content: 'more dm 1', created_at_ms: now - 900 };
    const dmMsgExtra2 = { id: '5007', channel_id: dmChanId, author_id: user1.id, content: 'more dm 2', created_at_ms: now - 800 };
    const dmMsgExtra3 = { id: '5008', channel_id: dmChanId, author_id: user1.id, content: 'more dm 3', created_at_ms: now - 700 };
    upsertMessage(db, dmMsgExtra1);
    upsertMessage(db, dmMsgExtra2);
    upsertMessage(db, dmMsgExtra3);

    const retrieved5 = getMessages(db, { channelId: dmChanId, limit: 5 });
    assert.equal(retrieved5.length, 5, 'getMessages should return 5 messages');
    for (let i = 0; i < retrieved5.length - 1; i++) {
      assert.ok(retrieved5[i].created_at_ms <= retrieved5[i + 1].created_at_ms, 'Messages must be sorted oldest to newest');
    }
    console.log('PASS: getMessages returns 5 oldest->newest');

    // 7. searchMessages('implement') returns the right message (this catches the param-order bug)
    const searchRes = searchMessages(db, 'implement');
    assert.equal(searchRes.length, 1, 'Search for implement should return 1 message');
    assert.equal(searchRes[0].id, '5003');
    assert.equal(searchRes[0].content, 'implement the feature');

    // Test with channelId/guildId/userId filters too
    const searchFiltered = searchMessages(db, 'implement', { channelId: dmChanId, userId: user1.id });
    assert.equal(searchFiltered.length, 1);
    assert.equal(searchFiltered[0].id, '5003');
    console.log('PASS: searchMessages returns matching message (param-order verification)');

    // 8. formatMessages output contains author name and content
    const formatted = formatMessages(searchRes, { title: 'Test Search' });
    assert.ok(formatted.includes('Theo'), 'Formatted messages should contain author name Theo');
    assert.ok(formatted.includes('implement the feature'), 'Formatted messages should contain message content');
    assert.ok(formatted.includes('spec.pdf'), 'Formatted messages should contain attachment filename');
    console.log('PASS: formatMessages contains author name and content');

    // 9. upsertGuild with null/empty name does not overwrite existing name
    upsertGuild(db, { id: '2001', name: null });
    let gCheck = db.prepare('SELECT name FROM guilds WHERE id = ?').get('2001');
    assert.equal(gCheck.name, 'Test Server', 'upsertGuild with null name should preserve existing name');

    upsertGuild(db, { id: '2001', name: '' });
    gCheck = db.prepare('SELECT name FROM guilds WHERE id = ?').get('2001');
    assert.equal(gCheck.name, 'Test Server', 'upsertGuild with empty string name should preserve existing name');
    console.log('PASS: upsertGuild null name does not overwrite');

    // 10. pruneGuildsNotIn removes stale guild and its channels
    const staleGuild = { id: '9999', name: 'Stale Guild' };
    upsertGuild(db, staleGuild);
    upsertChannel(db, { id: '99991', guild_id: '9999', name: 'stale-chan', type: 0 });
    assert.ok(db.prepare('SELECT id FROM guilds WHERE id = ?').get('9999'));
    assert.ok(db.prepare('SELECT id FROM channels WHERE guild_id = ?').get('9999'));

    const removedCount = pruneGuildsNotIn(db, ['2001']);
    assert.equal(removedCount, 1, 'pruneGuildsNotIn should report 1 guild removed');
    assert.equal(db.prepare('SELECT id FROM guilds WHERE id = ?').get('9999'), undefined, 'stale guild should be removed');
    assert.equal(db.prepare('SELECT id FROM channels WHERE guild_id = ?').get('9999'), undefined, 'stale guild channels should be removed');
    assert.ok(db.prepare('SELECT id FROM guilds WHERE id = ?').get('2001'), 'active guild should remain');
    console.log('PASS: pruneGuildsNotIn removes stale guild and channels');

    console.log('ALL TESTS PASSED');
  } finally {
    try {
      db.close();
    } catch {
      // ignore
    }
    try {
      fs.rmSync(tempDir, { recursive: true, force: true });
    } catch {
      // ignore
    }
  }
}

run().catch((err) => {
  console.error('TEST FAILED:', err);
  process.exit(1);
});
