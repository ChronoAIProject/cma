import test from 'node:test';
import assert from 'node:assert/strict';
import {cases} from './index.mjs';
import {rows, eventJoin, delivered, invoke, acquire, pages, openBrowser} from './helpers.mjs';
import {executeCase, Blocked} from '../../runner.mjs';
import {loadRegistry} from '../../registry.mjs';

const ctx = {assert(condition, code) {assert.ok(condition, code);}};
const event = {eventId: 'synthetic_event', conversationId: 'synthetic_conversation',
  responseId: 'synthetic_response', agentId: 'synthetic_agent', consumerId: 'synthetic_consumer',
  destination: 'synthetic_destination', expectedSender: 'synthetic_sender', expectedContentDigest: 'content_hash'};
const turn = {...event, servingAm: 'a', owningAm: 'b', epoch: 3,
  providerAccepted: true, turnCount: 1, effectCount: 1};
const receipt = {eventId: event.eventId, responseId: event.responseId, destination: event.destination,
  sender: event.expectedSender, contentDigest: event.expectedContentDigest, status: 'delivered', count: 1};

test('author registrations keep frozen IDs, exact steps and explicit dependencies', async () => {
  const registry = await loadRegistry();
  assert.equal(new Set(cases.map(c => c.id)).size, cases.length);
  for (const definition of cases) {
    assert.ok(registry.cases.has(definition.id));
    assert.deepEqual(definition.steps, rows.find(r => r.case_id === definition.id).required_steps);
    assert.ok(definition.fixtures.length > 0);
  }
});

test('every registered case blocks without its real fixture and cannot claim PASS', async () => {
  for (const definition of cases) {
    const result = await executeCase(definition, rows.find(r => r.case_id === definition.id), {
      phase: 'before_restart', iteration: 1, am: 'a', source: '1'.repeat(40),
      image: 'local-test@sha256:' + '2'.repeat(64), fixtures: {},
    });
    assert.equal(result.status, 'BLOCKED', definition.id);
    assert.equal(result.steps.length, 0);
    assert.equal(result.requests.length, 0);
  }
});

test('missing fixture operation stays BLOCKED rather than a TypeError or invented result', async () => {
  await assert.rejects(invoke({}, 'waitForDelivery', {}), error => error instanceof Blocked);
});

test('event evidence refuses wrong identity, owner ingress, stale epoch and duplicate effects', () => {
  eventJoin(ctx, event, turn);
  for (const field of ['eventId', 'conversationId', 'responseId', 'agentId', 'consumerId']) {
    assert.throws(() => eventJoin(ctx, event, {...turn, [field]: 'other'}));
    assert.throws(() => eventJoin(ctx, {...event, [field]: undefined}, turn));
  }
  for (const patch of [{servingAm: 'b'}, {servingAm: 'unknown'}, {epoch: 0}, {epoch: null},
    {providerAccepted: false}, {turnCount: 2}, {effectCount: 0}, {effectCount: 2}]) {
    assert.throws(() => eventJoin(ctx, event, {...turn, ...patch}));
  }
});

test('provider acceptance, wrong destination and unknown delivery never establish a reply', () => {
  delivered(ctx, event, receipt);
  for (const patch of [{status: 'accepted'}, {status: 'unknown'}, {count: 0}, {count: 2},
    {eventId: 'other'}, {responseId: 'other'}, {destination: 'other'}, {sender: 'other'},
    {contentDigest: 'other'}]) assert.throws(() => delivered(ctx, event, {...receipt, ...patch}));
});

test('unsafe fixture rejection still retains cleanup custody', async () => {
  let closed = 0;
  const row = rows[0];
  const definition = {id: row.case_id, fixtures: ['eg'], scope: 'eg', steps: row.required_steps,
    async run(ctx) {await ctx.step('step_1', () => acquire(ctx, 'unsafe'));}};
  const result = await executeCase(definition, row, {phase: 'before_restart', iteration: 1, am: 'a',
    source: '1'.repeat(40), image: 'local-test@sha256:' + '2'.repeat(64),
    fixtures: {eg: {acquire: async () => ({local: false, close: async () => {closed++;}})}}});
  assert.equal(result.status, 'FAIL'); assert.equal(closed, 1);
  assert.equal(result.primary_failure, 'local_synthetic_fixture_required');
  assert.equal(result.cleanup[0].status, 'PASS');
});

test('pagination rejects repeated identity, cyclic cursor and incorrect replica witness', async () => {
  const observationContext = {...ctx, am: 'a', observe: async (_label, operation) => operation()};
  const run = bodies => {
    let call = 0;
    const lease = {replicas: [{slot: 'a', instance: 'one'}], request: async () => ({
      status: 200, body: bodies[Math.min(call++, bodies.length - 1)], instance: 'one', requestId: 'synthetic'})};
    return pages(observationContext, lease, '/v1/channels', 'record_id');
  };
  assert.deepEqual(await run([{items: [{record_id: 'one'}], next_cursor: 'next'},
    {items: [{record_id: 'two'}]}]), [{record_id: 'one'}, {record_id: 'two'}]);
  await assert.rejects(run([{items: [{record_id: 'one'}], next_cursor: 'next'}]), /page_duplicate_identity/);
  await assert.rejects(run([{items: [], next_cursor: 'next'}]), /page_cursor_progress/);
  await assert.rejects(pages(observationContext, {replicas: [{slot: 'a', instance: 'one'}],
    request: async () => ({status: 200, body: {items: []}, instance: 'wrong', requestId: 'synthetic'})},
  '/v1/channels', 'record_id'), /eg_response_replica_attribution/);
});

test('browser setup uses shared origin and retains attribution before closing', async () => {
  const order = [], cleanups = [], page = {getByRole() {}, async goto(url) {order.push(url);}};
  const value = {real_ui: true, secrets_redacted: true, compiled_source: 'a'.repeat(40),
    requests: [{am: 'a', request_id: 'synthetic'}]};
  const context = {...ctx, row: {id: 'EG-001'}, am: 'a',
    fixture: name => {
      assert.equal(name, 'browser');
      return {open: async () => ({page, origin: 'http://127.0.0.1:1234', close: async () => order.push('close')}),
        evidence: async supplied => {assert.equal(supplied, page); order.push('evidence'); return value;}};
    }, defer: (_label, operation) => cleanups.push(operation), observe: async (_label, operation) => operation()};
  await openBrowser(context);
  for (const cleanup of cleanups.reverse()) await cleanup();
  assert.deepEqual(order, ['http://127.0.0.1:1234/event-gateway', 'evidence', 'close']);
});
