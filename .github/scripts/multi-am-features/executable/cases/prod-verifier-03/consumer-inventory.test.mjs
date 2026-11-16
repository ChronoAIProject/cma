import test from 'node:test';
import assert from 'node:assert/strict';
import {requireInventory, inventoryReceipt, assertInventoryPage} from './consumer-inventory.mjs';
import {openBrowser} from './helpers.mjs';
import {Blocked} from '../../runner.mjs';

const ctx = {assert(condition, code) {assert.ok(condition, code);}};
const inventory = () => ({botId: 'bot-0', expected: {ownerSubject: 'synthetic-owner', gatedSubject: 'synthetic-gated',
  ownedBotIds: Array.from({length: 51}, (_, i) => 'bot-' + i), provisioningBotId: 'bot-1', failedBotId: 'bot-2',
  callbackBotId: 'bot-3', foreignBotId: 'foreign-bot', connections: [{connection_ref: 'ready-connection', label: 'Ready'}],
  legacyBots: [{id: 'legacy-bot', name: 'Legacy', platform: 'telegram'}]}});
const records = () => Array.from({length: 51}, (_, i) => ({bot_id: 'bot-' + i, name: 'Synthetic ' + i,
  owner: 'synthetic-owner', state: 'active', updated_at: '2026-11-16T10:00:00.000Z'}));

test('Consumer inventory blocks an insufficient pagination, metadata or account fixture', () => {
  assert.equal(requireInventory(inventory()).ownedBotIds.length, 51);
  for (const change of [f => {f.expected.ownedBotIds.length = 2;},
    f => {f.expected.ownedBotIds.push(f.expected.ownedBotIds[0]);},
    f => {f.expected.gatedSubject = f.expected.ownerSubject;},
    f => {f.expected.provisioningBotId = f.botId;},
    f => {f.expected.callbackBotId = 'missing';}, f => {f.expected.foreignBotId = f.botId;},
    f => {f.expected.connections = [];}, f => {f.expected.legacyBots = undefined;}]) {
    const value = inventory(); change(value);
    assert.throws(() => requireInventory(value), error => error instanceof Blocked
      && error.message === 'consumer_inventory_authentic_seed_required');
  }
});

test('Consumer pagination joins all 51 owned identities using the real next_after contract', () => {
  const seen = new Set(), cursors = new Set(), items = records(), expected = inventory().expected;
  assertInventoryPage(ctx, {items: items.slice(0, 50), next_after: 'page-2'}, expected, seen, cursors);
  assertInventoryPage(ctx, {items: items.slice(50), next_after: null}, expected, seen, cursors);
  assert.deepEqual([...seen], expected.ownedBotIds);
  assert.deepEqual([...cursors], ['page-2']);
  // The API emits a continuation for a full final page, then an empty terminal
  // page. Empty terminal results must not look like malformed pagination.
  assertInventoryPage(ctx, {items: [], next_after: null}, expected, seen, cursors);
  assert.throws(() => assertInventoryPage(ctx, {items: items.slice(50), next_cursor: null}, expected,
    new Set(), new Set()), /consumer_inventory_cursor_progress/);
});

test('Consumer pagination rejects foreign records, repeated IDs, cursor loops and malformed states', () => {
  const expected = inventory().expected, item = records()[0];
  for (const [items, cursor, seen, cursors, code] of [
    [[{...item, owner: 'foreign-owner'}], null, new Set(), new Set(), 'owner_isolation'],
    [[{...item, bot_id: 'unexpected'}], null, new Set(), new Set(), 'owner_isolation'],
    [[item, item], null, new Set(), new Set(), 'duplicate_bot'],
    [[item], null, new Set([item.bot_id]), new Set(), 'duplicate_bot'],
    [[item], 'page-2', new Set(), new Set(['page-2']), 'cursor_progress'],
    [[{...item, state: 'apparently-ready'}], null, new Set(), new Set(), 'item_shape'],
    [[{...item, updated_at: 'unknown'}], null, new Set(), new Set(), 'item_shape'],
    [[], 'page-2', new Set(), new Set(), 'page_shape'],
  ]) assert.throws(() => assertInventoryPage(ctx, {items, next_after: cursor}, expected, seen, cursors), new RegExp(code));
});

test('Consumer browser reads require cookie authentication and one full actual AM receipt', () => {
  const serving = {schema: 'c275-serving-v1', am: 'b', origin: 'http://127.0.0.1:1234', ready: true,
    pod_uid: 'fixture-pod', instance_id: 'fixture-process', container_id: 'fixture-container',
    observed_at: '2026-11-16T10:00:00.000Z'};
  const receipt = {am: 'b', request_id: 'actual-read', method: 'GET', status: 200, serving,
    serving_after: {...serving, observed_at: '2026-11-16T10:00:00.003Z'},
    started_at: '2026-11-16T10:00:00.001Z', ended_at: '2026-11-16T10:00:00.002Z',
    attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}};
  const evidence = {authentication: 'browser_session', requests: [receipt]};
  assert.equal(inventoryReceipt(ctx, evidence, 'actual-read').am, 'b');
  assert.throws(() => inventoryReceipt(ctx, {...evidence, authentication: 'real_am_bearer'}, 'actual-read'),
    error => error instanceof Blocked);
  for (const requests of [[{...receipt, am: 'a'}], [{...receipt, serving: undefined}], [receipt, receipt],
    [{...receipt, request_id: 'unrelated-read'}], [{...receipt, status: 403}], [{...receipt, method: 'POST'}],
    [{...receipt, serving_after: {...serving, instance_id: 'replacement'}}]])
    assert.throws(() => inventoryReceipt(ctx, {...evidence, requests}, 'actual-read'), /actual_am_receipt/);
  assert.equal(inventoryReceipt(ctx, {...evidence, requests: [{...receipt, status: 403}]}, 'actual-read', 403).status, 403);
});

test('Consumer-only browser entry avoids an unrelated Event Gateway navigation and retains cleanup custody', async () => {
  const navigations = [], cleanups = [], events = [];
  const page = {getByRole() {}, async goto(url) {navigations.push(url);}};
  const browser = {async open(input) {
    assert.deepEqual(input, {actor: 'user', am: 'a', agentId: undefined});
    return {page, origin: 'http://127.0.0.1:1234', close: async () => events.push('close')};
  }, async evidence(input) {
    assert.equal(input, page); events.push('evidence');
    return {real_ui: true, secrets_redacted: true, compiled_source: 'a'.repeat(40),
      requests: [{am: 'a', request_id: 'actual-request'}]};
  }};
  const session = await openBrowser({...ctx, row: {id: 'EG-047'}, am: 'a',
    fixture: name => {assert.equal(name, 'browser'); return browser;},
    defer: (_label, work) => cleanups.push(work), observe: (_label, work) => work()},
  'user', {initialPath: '/bots'});
  assert.equal(session.page, page);
  assert.deepEqual(navigations, ['http://127.0.0.1:1234/bots']);
  assert.equal(cleanups.length, 1); await cleanups[0]();
  assert.deepEqual(events, ['close', 'evidence']);
});
