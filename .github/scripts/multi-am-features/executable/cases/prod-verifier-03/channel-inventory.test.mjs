import test from 'node:test';
import assert from 'node:assert/strict';
import {Blocked} from '../../runner.mjs';
import {digest} from './helpers.mjs';
import {setups} from './setup.mjs';
import {channelInventoryCase, validateInventoryFixture, assertInventoryRecord,
  acceptInventoryPage, assertInventoryCounters, readInventoryRows} from './channel-inventory.mjs';

const ctx = {assert: (value, code) => assert.ok(value, code)};
function fixture() {
  const records = Array.from({length: 51}, (_, index) => {
    const record_id = 'record-' + String(index).padStart(2, '0');
    return {record_id, version: 3, provider: {slug: 'cma_codex', level: 'l2'},
      ...(index === 50 ? {} : {channel_id: 'channel-' + index,
        endpoints: {custom_event_url: '/events/' + index, nyxid_callback_url: '/callback/' + index}}),
      definition: {name: 'Channel ' + index, created_at: '2026-11-16T00:00:00Z',
        owner: {subject: 'owner', display_name: 'Synthetic owner'}, sources: []}};
  });
  const channels = records.map(record => ({recordId: record.record_id, name: record.definition.name,
    version: record.version, creator: record.definition.owner.display_name, createdAt: record.definition.created_at,
    definitionDigest: digest(record.definition), addressed: !!record.channel_id, channelId: record.channel_id}));
  return {records, lease: {local: true, synthetic: true, replicas: [{ready: true}, {ready: true}],
    close: async () => {}, request() {}, counters() {}, quiesce() {}, scanCredentialValues() {}, scanBrowserForCredentialValues() {},
    expected: {ownerSubject: 'owner', channels, selectedRecordId: 'record-00', missingRecordId: 'record-50',
      foreignRecordId: 'other-owner-record', nodes: [
        {kind: 'producer', name: 'Producer: Custom events (HTTP events)', facts: {'Access policy': 'hmac'}},
        {kind: 'producer', name: 'Producer: Telegram (NyxID relay)', facts: {'Access policy': 'All senders', Routes: 'synthetic-route'}},
        {kind: 'consumer', name: 'Consumer: Default consumer (cma_codex)',
          facts: {'Consumer ID': 'default', Provider: 'cma_codex', Binding: 'default-binding'}},
        {kind: 'consumer', name: 'Consumer: Reviewer (cma_codex)',
          facts: {'Consumer ID': 'reviewer', Provider: 'cma_codex', Binding: 'reviewer-binding'}},
      ]}}};
}

test('EG-018 keeps its frozen read-only journey and explicitly requires EG-scoped evidence', () => {
  assert.equal(channelInventoryCase.id, 'C275:EG-018');
  assert.deepEqual(channelInventoryCase.steps, ['step_1', 'step_2', 'expected_result']);
  assert.deepEqual(channelInventoryCase.fixtures, ['browser', 'user-fixtures', 'eg_browser']);
  assert.equal(channelInventoryCase.evidence_scope, 'eg_only');
});

test('inventory setup needs real pagination, versioned records, missing address and topology', () => {
  validateInventoryFixture(fixture().lease);
  for (const change of [l => l.expected.channels.pop(), l => l.expected.channels[1].recordId = 'record-00',
    l => l.expected.channels[1].name = 'Channel 0', l => l.expected.channels[0].version = 1,
    l => l.expected.channels[0].createdAt = 'invalid', l => l.expected.foreignRecordId = 'record-00',
    l => l.expected.missingRecordId = 'record-00', l => l.expected.nodes.pop(),
    l => l.expected.nodes[0].facts = {}, l => delete l.expected.nodes[2].facts.Binding,
    l => l.expected.nodes[2].facts['Consumer ID'] = 'another-consumer', l => delete l.scanCredentialValues]) {
    const {lease} = fixture(); change(lease);
    assert.throws(() => validateInventoryFixture(lease), error => error instanceof Blocked);
  }
});

test('invalid inventory setup still releases the parent lease exactly once', async () => {
  const {lease} = fixture(), cleanups = []; let releases = 0;
  lease.close = async () => releases++; delete lease.scanBrowserForCredentialValues;
  await assert.rejects(setups.channel_inventory({row: {}, defer: (_label, fn) => cleanups.push(fn)},
    {eg: {acquire: async () => lease}}), error => error instanceof Blocked);
  await cleanups[0](); await cleanups[0](); assert.equal(releases, 1);
});

test('inventory pagination accepts exactly 50 plus one retained owned record', () => {
  const {lease, records} = fixture(), collected = [], cursors = new Set();
  acceptInventoryPage(ctx, {items: records.slice(0, 50), next_cursor: 'record-49'}, lease.expected, collected, cursors);
  acceptInventoryPage(ctx, {items: records.slice(50), next_cursor: null}, lease.expected, collected, cursors);
  assert.equal(collected.length, 51); assert.deepEqual([...cursors], ['record-49']);
});

test('inventory refuses extra, duplicate, foreign, stale and truncated pages', () => {
  for (const change of [
    page => page.items[0].record_id = 'other-owner-record',
    page => page.items[0].definition.owner.subject = 'foreign-owner',
    page => page.items[1] = structuredClone(page.items[0]),
    page => page.items[0].version--,
    page => page.items[0].definition.name += ' misleading suffix',
    page => page.next_cursor = 'wrong-cursor',
    page => page.next_cursor = null,
    page => page.items.pop(),
  ]) {
    const {lease, records} = fixture();
    const page = {items: structuredClone(records.slice(0, 50)), next_cursor: 'record-49'}; change(page);
    assert.throws(() => acceptInventoryPage(ctx, page, lease.expected, [], new Set()));
  }
  const {lease, records} = fixture(), collected = [], cursors = new Set();
  const first = {items: records.slice(0, 50), next_cursor: 'record-49'};
  acceptInventoryPage(ctx, first, lease.expected, collected, cursors);
  assert.throws(() => acceptInventoryPage(ctx, first, lease.expected, collected, cursors), /owned_unique/);
});

test('record validation rejects changed saved configuration, metadata and invented legacy addresses', () => {
  const {lease, records} = fixture();
  assertInventoryRecord(ctx, records[0], lease.expected.channels[0], 'owner');
  assertInventoryRecord(ctx, records[50], lease.expected.channels[50], 'owner');
  for (const change of [r => r.definition.created_at = '2026-11-17T00:00:00Z',
    r => r.definition.owner.display_name = 'Different creator', r => r.channel_id = 'different-channel',
    r => delete r.endpoints, r => r.definition.sources.push({type: 'custom'})]) {
    const record = structuredClone(records[0]); change(record);
    assert.throws(() => assertInventoryRecord(ctx, record, lease.expected.channels[0], 'owner'));
  }
  assert.throws(() => assertInventoryRecord(ctx, {...records[50], channel_id: 'invented-channel'},
    lease.expected.channels[50], 'owner'), /exact_saved_address/);
  assert.throws(() => assertInventoryRecord(ctx, {...records[50], endpoints: records[0].endpoints},
    lease.expected.channels[50], 'owner'), /exact_saved_address/);
});

test('inventory metadata reads immutable source timestamps rather than rendered locale dates', () => {
  const element = {
    getAttribute: name => name === 'data-node-id' ? 'channel:record-00' : null,
    querySelector: selector => ({
      '.tree-row__name': {textContent: 'Channel 0'},
      '.tree-row__age time': {textContent: 'Nov 16', getAttribute: name => name === 'datetime' ? '2026-11-16T00:00:00Z' : null},
      '.tree-row__age': {getAttribute: name => name === 'title' ? 'Created 2026-11-16T00:00:00Z · By Synthetic owner' : null},
    })[selector],
  };
  assert.deepEqual(readInventoryRows([element]), [{record: 'channel:record-00', name: 'Channel 0',
    createdAt: '2026-11-16T00:00:00Z', metadata: 'Created 2026-11-16T00:00:00Z · By Synthetic owner'}]);
});

test('read-only inventory rejects missing counters and any provider or platform effect', () => {
  const before = {turns: 2, effects: 2, deliveries: 1, ingress: 3};
  assertInventoryCounters(ctx, before, {...before});
  for (const key of Object.keys(before)) {
    assert.throws(() => assertInventoryCounters(ctx, before, {...before, [key]: before[key] + 1}));
    assert.throws(() => assertInventoryCounters(ctx, before, {...before, [key]: undefined}));
  }
});
