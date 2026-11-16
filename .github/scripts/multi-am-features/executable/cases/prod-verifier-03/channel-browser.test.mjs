import test from 'node:test';
import assert from 'node:assert/strict';
import {channelReceipt, openListedChannel} from './channel-browser.mjs';

const ctx = {assert: (condition, code) => assert.ok(condition, code), observe: async (_label, read) => read()};
function receipt(id, method = 'GET', status = 200) {
  const serving = {schema: 'c275-eg-serving-v1', scope: 'eg', replica: 'b', origin: 'http://127.0.0.1:4321',
    ready: true, pod_uid: 'pod-b', instance_id: 'eg-b', container_id: 'container-b', source: 'a'.repeat(40),
    image: 'eg@sha256:' + 'b'.repeat(64), observed_at: '2026-11-16T10:00:00.000Z'};
  return {scope: 'eg', replica: 'b', request_id: id, method, status, serving,
    serving_after: {...serving, observed_at: '2026-11-16T10:00:00.003Z'},
    started_at: '2026-11-16T10:00:00.001Z', ended_at: '2026-11-16T10:00:00.002Z',
    attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}};
}

test('channel POST receipts require the actual EG serving window and cannot inherit an AM label', () => {
  const actual = receipt('create', 'POST', 201);
  assert.equal(channelReceipt(ctx, [actual], 'create', 'POST', 201).replica, 'b');
  for (const records of [[{...actual, am: 'b'}], [{...actual, serving_after: undefined}],
    [{...actual, replica: 'a'}], [{...actual, request_id: 'other'}], [actual, actual]])
    assert.throws(() => channelReceipt(ctx, records, 'create', 'POST', 201), /actual_eg_browser_receipt/);
  assert.throws(() => channelReceipt(ctx, [actual], 'create'), /actual_eg_browser_receipt/);
});

function listFixture(pages, options = {}) {
  const origin = 'http://127.0.0.1:1234', calls = [], receipts = [];
  const expected = {record_id: 'record', channel_id: 'channel', version: 2,
    endpoints: {custom_event_url: '/event', nyxid_callback_url: '/callback'}, definition: {name: 'Synthetic channel'}};
  let cursor = 0, waiter, selected = false;
  const respond = (path, body) => {
    calls.push(path); const id = 'request-' + calls.length;
    const response = {url: () => origin + '/api/event-gateway' + path,
      request: () => ({method: () => 'GET'}), status: () => 200, json: async () => body,
      headers: () => ({'x-request-id': id})};
    assert.ok(waiter, 'response observer must precede action');
    assert.ok(waiter.predicate(response)); receipts.push(receipt(id)); waiter.resolve(response); waiter = undefined;
  };
  const row = {count: async () => 1, getAttribute: async () => selected ? 'true' : 'false',
    getByRole: () => ({click: async () => {selected = true;
      if (options.unaddressed) respond('/v1/channel-records/record', expected);
    }})};
  const page = {waitForResponse: predicate => new Promise(resolve => {assert.equal(waiter, undefined); waiter = {predicate, resolve};}),
    getByText: () => ({}), getByRole: (role, args) => {
      if (role === 'tree') return {getByRole: () => ({filter: () => row})};
      if (role === 'heading') return {waitFor: async () => {}, isVisible: async () => selected};
      assert.equal(args.name, 'Load more channels');
      return {click: async () => {const after = pages[cursor].next_cursor; cursor++;
        respond('/v1/channels?after=' + encodeURIComponent(after), pages[cursor]);}};
    }};
  const session = {origin, page, egEvidence: async () => receipts, networkIdle: async () => {}};
  return {expected, calls, session, run: () => openListedChannel(ctx, session, expected,
    async () => respond('/v1/channels', pages[0]))};
}

test('addressed channel list selection joins its real list receipt without waiting for a nonexistent detail GET', async () => {
  const f = listFixture([]); const target = f.expected;
  const direct = listFixture([{items: [target], next_cursor: null}]); const result = await direct.run();
  assert.deepEqual(result.body, target); assert.equal(result.receipt.replica, 'b');
  assert.deepEqual(direct.calls, ['/v1/channels']);
});

test('channel list pagination uses actual response cursors and an unaddressed record performs recovery GET', async () => {
  const target = listFixture([]).expected;
  const f = listFixture([{items: [{record_id: 'other'}], next_cursor: 'cursor / next'},
    {items: [{...target, channel_id: null, endpoints: null}], next_cursor: null}], {unaddressed: true});
  const result = await f.run(); assert.deepEqual(result.body, target);
  assert.deepEqual(f.calls, ['/v1/channels', '/v1/channels?after=cursor%20%2F%20next', '/v1/channel-records/record']);
});

test('channel list rejects duplicate records, missing targets and wrong selected version', async () => {
  const target = listFixture([]).expected;
  for (const pages of [[{items: [target, target], next_cursor: null}], [{items: [], next_cursor: null}],
    [{items: [{...target, version: 3}], next_cursor: null}],
    [{items: [{record_id: 'other'}], next_cursor: 'repeat'}, {items: [], next_cursor: 'repeat'}]]) {
    const f = listFixture(pages); await assert.rejects(f.run(), /channel_browser_/);
  }
});
