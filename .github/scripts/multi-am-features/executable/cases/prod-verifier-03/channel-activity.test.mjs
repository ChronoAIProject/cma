import test from 'node:test';
import assert from 'node:assert/strict';
import {EventEmitter} from 'node:events';
import {Blocked} from '../../runner.mjs';
import {assertActivityPage, expiredActivityCursor, activityProjection, assertAtomicActivity, activityWait,
  observeActivityStream, overrideNextActivityCursor, installActivityDomObserver} from './activity-observer.mjs';
import {validateActivityFixture, assertUnrelatedContext, assertHeldActivity, channelActivityCase} from './channel-activity.mjs';
import {parentEgSetup} from './setup.mjs';

const ctx = {assert: (condition, code) => assert.ok(condition, code)};
const at = n => `2026-11-17T01:00:0${n}.000Z`;
const markers = ['synthetic-unrelated-transcript-one', 'synthetic-unrelated-transcript-two'];
const encode = value => Buffer.from(JSON.stringify(value)).toString('base64url');
const cursor = encode({record: 'record-digest', day: 20774, versions: [1, 0, 0, 0, 0, 0, 0]});
const consumer = {consumer_id: 'consumer', display_name: 'Synthetic consumer', agent_id: 'agent',
  provider: 'cma_codex', conversation_id: 'agent-conversation', state: 'accepted', response_id: 'response',
  dispatched_at: at(0), accepted_at: at(1), produced_at: null, completed_at: null, deliveries: []};
const held = {eventId: 'event-held', conversationId: 'conversation', consumerId: 'consumer',
  agentId: 'agent', agentConversationId: 'agent-conversation'};
const event = {event_id: held.eventId, kind: 'message', source: {type: 'custom'}, actor: {id: 'sender', kind: 'human'},
  conversation: {id: 'conversation', kind: 'direct'}, received_at: at(0), occurred_at: at(0), consumers: [consumer]};
const before = {cursor, retention_days: 7, limit: 50, has_more: false, items: [event,
  {...event, event_id: 'event-retained', consumers: [{...consumer, state: 'completed', produced_at: at(2), completed_at: at(3)}]}]};
const after = structuredClone(before);
after.cursor = encode({record: 'record-digest', day: 20774, versions: [4, 0, 0, 0, 0, 0, 0]});
Object.assign(after.items[0].consumers[0], {state: 'completed', produced_at: at(2), completed_at: at(3), deliveries: [{
  step: 'answer', destination: 'origin', outcome: 'confirmed', observed_at: at(3), confirmed_at: at(2),
  recipients: [{kind: 'custom', id: 'destination', thread_id: 'thread'}]}]});
const ids = before.items.map(item => item.event_id);
const expected = {retainedEventIds: ids, held, unrelated: {agentId: 'other-agent', conversationId: 'other-conversation', markers}};
const copy = value => structuredClone(value);

test('activity snapshots require exact retained identities and reject duplicated or leaking data', () => {
  assertActivityPage(ctx, before, ids, markers);
  const invalid = [page => {page.items.push(copy(page.items[0]));}, page => {page.items[1].event_id = 'foreign';},
    page => {page.items[0].consumers.push(copy(consumer));}, page => {page.items[0].consumers[0].state = 'invented';},
    page => {page.items[0].transcript = markers[0];}, page => {page.items[0].received_at = 'not a timestamp';},
    page => {page.has_more = true;}, page => {delete page.retention_days;}];
  for (const change of invalid) {const value = copy(before); change(value); assert.throws(() => assertActivityPage(ctx, value, ids, markers), /activity_/);}
});

test('expired cursor preserves the actual record and version vector and changes only retention day', () => {
  const original = JSON.parse(Buffer.from(before.cursor, 'base64url'));
  const expired = JSON.parse(Buffer.from(expiredActivityCursor(before), 'base64url'));
  assert.deepEqual(expired, {...original, day: original.day - before.retention_days});
  assert.ok(expired.day < original.day - before.retention_days + 1);
  for (const value of [{...before, cursor: 'not json'}, {...before, cursor: encode({day: 1})},
    {...before, retention_days: 2}, {...before, cursor: encode({...original, day: 1.5})}])
    assert.throws(() => expiredActivityCursor(value), /activity_cursor_/);
});

test('activity detail requires actual acceptance, same provider identity and confirmed recipient delivery', () => {
  assertHeldActivity(ctx, before, held, false); assertHeldActivity(ctx, after, held, true);
  for (const patch of [{state: 'produced'}, {agent_id: 'different'}, {conversation_id: 'different'},
    {completed_at: null}, {deliveries: []}, {deliveries: [{...after.items[0].consumers[0].deliveries[0], outcome: 'buffered'}]}]) {
    const value = copy(after); Object.assign(value.items[0].consumers[0], patch);
    assert.throws(() => assertHeldActivity(ctx, value, held, true), /activity_/);
  }
  assert.throws(() => assertHeldActivity(ctx, after, held, false), /activity_held_transition/);
});

test('UI projection compares Agent details, exact timestamp attributes and delivered recipient metadata', () => {
  const projected = activityProjection(after)[0];
  assert.deepEqual(projected, {eventId: held.eventId, conversation: 'conversation', receivedAt: at(0), eventTime: at(0),
    consumers: [{name: 'Synthetic consumer', state: 'Agent completed', dispatchedAt: at(0), acceptedAt: at(1),
      agent: 'agent', provider: 'cma_codex', conversation: 'agent-conversation', producedAt: at(2),
      deliveries: [{description: 'answer · custom: destination · Thread thread', label: 'Delivery confirmed', time: at(2)}]}]});
  const platform = copy(after); platform.items[0].kind = 'event'; platform.items[0].source_occurred_at = at(1);
  const delivery = platform.items[0].consumers[0].deliveries[0]; delivery.recipients[0].kind = 'whatsapp';
  assert.equal(activityProjection(platform)[0].consumers[0].deliveries[0].label, 'Accepted by WhatsApp');
  delivery.step = 'platform_status'; delivery.outcome = 'read';
  assert.equal(activityProjection(platform)[0].consumers[0].deliveries[0].label, 'Read by recipient');
  assert.equal(activityProjection(platform)[0].eventTime, at(1));
});

test('atomicity rejects partial rows, mixed details, duplicates, regressions and transient transcript leakage', () => {
  const first = {rows: activityProjection(before), leak: false}, last = {rows: activityProjection(after), leak: false};
  const frames = [{page: before}, {page: after}];
  assertAtomicActivity(ctx, [first, last, last], frames);
  const partial = copy(last); partial.rows.pop();
  const mixed = copy(last); mixed.rows[0].consumers[0].state = first.rows[0].consumers[0].state;
  const duplicate = copy(last); duplicate.rows.push(copy(duplicate.rows[0]));
  for (const observations of [[first, partial, last], [first, mixed, last], [first, duplicate, last],
    [last, first], [first, {...last, leak: true}, last], []])
    assert.throws(() => assertAtomicActivity(ctx, observations, frames), /activity_/);
});

test('fixtures need an actually held event and admitted unrelated transcript, not scenario result methods', () => {
  const noop = async () => {};
  const lease = {recordId: 'record', channelId: 'channel', expected, request: noop, releaseCompletion: noop,
    drain: noop, providerContext: noop};
  assert.equal(validateActivityFixture(lease), expected);
  for (const patch of [{recordId: undefined}, {providerContext: undefined}, {expected: {...expected, retainedEventIds: ['single']}},
    {expected: {...expected, unrelated: {...expected.unrelated, agentId: held.agentId}}}])
    assert.throws(() => validateActivityFixture({...lease, ...patch}), Blocked);
  const capture = {agentId: expected.unrelated.agentId, conversationId: expected.unrelated.conversationId,
    requestCount: 1, messages: [{role: 'user', content: markers[0]}, {role: 'assistant', content: [{type: 'output_text', text: markers[1]}]}]};
  assertUnrelatedContext(ctx, expected.unrelated, capture);
  for (const patch of [{requestCount: 0}, {agentId: held.agentId}, {messages: []}, {messages: [capture.messages[0]]}])
    assert.throws(() => assertUnrelatedContext(ctx, expected.unrelated, {...capture, ...patch}), /activity_/);
  assert.equal(channelActivityCase.evidence_scope, 'eg_only');
  assert.deepEqual(channelActivityCase.steps, ['step_1', 'step_2', 'expected_result']);
});

test('old high-level activity fixture stays BLOCKED and retains cleanup custody', async () => {
  const cleanups = []; let closed = 0;
  const method = async () => {throw Error('old_oracle_must_not_run');};
  const lease = {local: true, synthetic: true, replicas: [{ready: true}, {ready: true}],
    recordId: 'record', channelId: 'channel', expected, request: method,
    activitySnapshot: method, activityStream: method, activityDetail: method, close: async () => {closed++;}};
  const context = {row: {case_id: channelActivityCase.id}, phase: 'steady', iteration: 1, am: 'a',
    defer: (_label, action) => cleanups.push(action)};
  await assert.rejects(parentEgSetup(context, {eg: {acquire: async () => lease}}, 'channel_activity'),
    error => error instanceof Blocked && /releasecompletion/.test(error.message));
  await cleanups[0](); assert.equal(closed, 1);
});

async function observerFixture(options = {}) {
  const cleanups = [], calls = [], cdp = new EventEmitter();
  cdp.send = async method => {calls.push(method); if (options.enableFails) throw Error('cdp_enable_failed');};
  cdp.detach = async () => {calls.push('detach'); if (options.detachFails) throw Error('cdp_detach_failed');};
  const context = {...ctx, defer: (_label, action) => cleanups.push(action), remainingMs: () => 20};
  const page = {context: () => ({newCDPSession: async () => cdp})};
  const url = 'http://127.0.0.1:1234/api/event-gateway/v1/channels/channel/activity/stream';
  const start = () => observeActivityStream(context, page, url, value => assertActivityPage(ctx, value, ids, markers));
  const request = (id = 'cdp-1', cursor, target = url) => cdp.emit('Network.requestWillBeSent', {
    requestId: id, request: {method: 'GET', url: target, headers: {Authorization: 'never-retained', ...(cursor ? {'Last-Event-ID': cursor} : {})}}});
  const response = (id = 'cdp-1', patch = {}) => cdp.emit('Network.responseReceived', {requestId: id,
    response: {url, status: 200, mimeType: 'text/event-stream', headers: {'X-Request-Id': 'actual-' + id}, ...patch}});
  const frame = (snapshot = before, id = 'cdp-1', type = 'activity', eventId = snapshot.cursor) =>
    cdp.emit('Network.eventSourceMessageReceived', {requestId: id, eventName: type, eventId, data: JSON.stringify(snapshot)});
  return {start, request, response, frame, cdp, calls, cleanups, url};
}

test('native SSE observation correlates exact requests and headers across real connection events', async () => {
  const f = await observerFixture(), observer = await f.start();
  f.request('unrelated', undefined, f.url + '/other'); f.frame(before, 'unrelated');
  f.request(); f.response(); f.frame(); f.frame(after);
  f.cdp.emit('Network.requestWillBeSentExtraInfo', {requestId: 'cdp-2', headers: {'Last-Event-ID': after.cursor, Cookie: 'never-retained'}});
  f.request('cdp-2'); f.response('cdp-2'); f.frame(after, 'cdp-2');
  const reconnected = await observer.wait(frame => frame.connection.index === 1, 'missing');
  assert.equal(observer.frames.length, 3); assert.equal(observer.connections.length, 2);
  assert.equal(reconnected.connection.requestId, 'actual-cdp-2');
  assert.equal(reconnected.connection.lastEventId, after.cursor);
  assert.ok(!JSON.stringify(observer.connections).includes('never-retained'));
  await observer.close(); await f.cleanups[0]();
  assert.deepEqual(f.calls, ['Network.enable', 'detach']);
  assert.equal(f.cdp.eventNames().length, 0);
});

test('an uncorrelated frame cannot satisfy the wait, and malformed streams poison later good frames', async () => {
  const pending = await observerFixture(), watch = await pending.start();
  pending.request(); pending.frame();
  await assert.rejects(watch.wait(() => true, 'activity_no_handshake_receipt'), /activity_no_handshake_receipt/);
  await watch.close();
  for (const corrupt of [f => f.response('cdp-1', {status: 400}), f => f.response('cdp-1', {mimeType: 'application/json'}),
    f => f.response('cdp-1', {headers: {}}), f => f.frame(before, 'cdp-1', 'activity', 'different-cursor'),
    f => f.frame({...before, items: [event, event]}), f => f.frame({...before, transcript: markers[0]}),
    f => f.frame(before, 'cdp-1', 'invented-reset')]) {
    const f = await observerFixture(), observer = await f.start(); f.request(); f.response(); corrupt(f); f.frame();
    assert.throws(observer.check, /native_stream_observation_failed/);
    await assert.rejects(observer.wait(() => true, 'missing'), /native_stream_observation_failed/);
    await assert.rejects(f.cleanups[0](), /activity_stream_cleanup_failed/);
    assert.equal(f.cdp.eventNames().length, 0);
  }
});

test('CDP acquisition owns detach even when enable fails and retains failed detach on repeated close', async () => {
  const missing = {context: () => ({})};
  await assert.rejects(observeActivityStream(ctx, missing, 'http://127.0.0.1/', () => {}), Blocked);
  const f = await observerFixture({enableFails: true}); await assert.rejects(f.start(), /cdp_enable_failed/);
  await f.cleanups[0](); assert.deepEqual(f.calls, ['Network.enable', 'detach']);
  const broken = await observerFixture({detachFails: true}), observer = await broken.start();
  await assert.rejects(observer.close(), /activity_stream_cleanup_failed/);
  await assert.rejects(broken.cleanups[0](), /activity_stream_cleanup_failed/);
  assert.equal(broken.calls.filter(call => call === 'detach').length, 1);
});

async function overrideFixture(options = {}) {
  let handler; const calls = [], cleanups = [], controller = new AbortController();
  const page = {route: async (_url, fn) => {handler = fn;}, unroute: async () => {calls.push('unroute');}};
  const context = {...ctx, signal: controller.signal, defer: (_label, action) => cleanups.push(action)};
  const override = await overrideNextActivityCursor(context, page, 'http://127.0.0.1/stream', 'actual-expired');
  const dispatch = () => handler({request: () => ({method: () => 'GET', allHeaders: async () => ({
    'Last-Event-ID': 'actual-previous', accept: 'text/event-stream', cookie: 'private-memory-only'})}),
  continue: async input => {calls.push(input); if (options.failDispatch) throw Error('transport_unknown');
    if (options.dispatchGate) await options.dispatchGate;}, abort: async code => {calls.push(code);}});
  return {override, dispatch, calls, cleanups, controller};
}

test('expired-cursor fault changes one outgoing header, never fetches/replaces SSE, and preserves native original', async () => {
  const f = await overrideFixture(); await f.dispatch(); f.override.check('actual-previous');
  assert.deepEqual(f.calls, [{headers: {accept: 'text/event-stream', cookie: 'private-memory-only', 'last-event-id': 'actual-expired'}}]);
  assert.throws(() => f.override.check('wrong-cursor'), /native_original/);
  await f.dispatch(); assert.equal(f.calls.filter(call => typeof call === 'object').length, 1);
  assert.throws(() => f.override.check('actual-previous'), /override_once/);
  await assert.rejects(f.cleanups[0](), /activity_cursor_override_cleanup_failed/);
});

test('cancelled or unknown cursor dispatch is not replayed and cleanup awaits pending request work', async () => {
  const cancelled = await overrideFixture(); cancelled.controller.abort(); await cancelled.dispatch();
  assert.deepEqual(cancelled.calls, ['failed']);
  await assert.rejects(cancelled.cleanups[0](), /activity_cursor_override_cleanup_failed/);
  const unknown = await overrideFixture({failDispatch: true}); await unknown.dispatch();
  assert.equal(unknown.calls.filter(call => typeof call === 'object').length, 1);
  assert.throws(() => unknown.override.check('actual-previous'), /transport_unknown/);
  await assert.rejects(unknown.cleanups[0](), /activity_cursor_override_cleanup_failed/);
  let release; const gate = new Promise(resolve => {release = resolve;});
  const pending = await overrideFixture({dispatchGate: gate}); const running = pending.dispatch();
  let closed = false; const cleanup = pending.cleanups[0]().then(() => {closed = true;});
  await new Promise(resolve => setImmediate(resolve)); assert.equal(closed, false);
  release(); await running; await cleanup; assert.equal(closed, true);
});

test('activity waits honor cancellation and the remaining case budget', async () => {
  await assert.rejects(activityWait({...ctx, signal: AbortSignal.abort()}, () => true, 'timeout'), /activity_cancelled/);
  let polls = 0;
  await assert.rejects(activityWait({...ctx, remainingMs: () => 0}, () => {polls++; return false;}, 'activity_deadline'), /activity_deadline/);
  assert.equal(polls, 1);
});

test('DOM observer captures transient disclosure, stops on disconnect and bounds private observations', () => {
  // The DOM projection itself is verified against compiled UI only in the
  // parent run; this isolated double verifies observer lifecycle and retention.
  let notify, disconnected = false; const original = globalThis.MutationObserver;
  globalThis.MutationObserver = class {
    constructor(fn) {notify = fn;}
    observe() {}
    disconnect() {disconnected = true;}
  };
  const key = '__activity_unit_test', root = {textContent: '', isConnected: true, querySelectorAll: () => []};
  try {
    installActivityDomObserver(root, {key, forbidden: markers});
    root.textContent = markers[0]; notify(); root.textContent = ''; notify();
    assert.deepEqual(globalThis[key].read().snapshots.map(value => value.leak), [false, true, false]);
    for (let i = 0; i < 600; i++) {root.textContent = i % 2 ? '' : markers[0]; notify();}
    assert.equal(globalThis[key].read().overflow, true);
    globalThis[key].close(); assert.equal(disconnected, true);
  } finally {delete globalThis[key]; globalThis.MutationObserver = original;}
});
