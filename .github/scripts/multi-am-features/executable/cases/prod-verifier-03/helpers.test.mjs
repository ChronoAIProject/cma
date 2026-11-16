import test from 'node:test';
import assert from 'node:assert/strict';
import {cases} from './index.mjs';
import {rows, eventJoin, retainTurn, delivered, invoke, acquire, pages, openBrowser, same, digest} from './helpers.mjs';
import {executeCase, Blocked} from '../../runner.mjs';
import {loadRegistry} from '../../registry.mjs';

const ctx = {assert(condition, code) {assert.ok(condition, code);}};
const event = {eventId: 'synthetic_event', conversationId: 'synthetic_conversation',
  responseId: 'synthetic_response', agentId: 'synthetic_agent', consumerId: 'synthetic_consumer',
  destination: 'synthetic_destination', expectedSender: 'synthetic_sender', expectedContentDigest: 'content_hash'};
const serving = {schema: 'c275-serving-v1', am: 'a', origin: 'http://127.0.0.1:10001',
  pod_uid: 'pod-a', instance_id: 'instance-a', container_id: 'container-a', ready: true,
  observed_at: '2026-11-16T10:00:00.000Z'};
const servingAfter = {...serving, observed_at: '2026-11-16T10:00:00.003Z'};
const turn = {...event, servingAm: 'a', owningAm: 'b', epoch: 3,
  providerAccepted: true, turnCount: 1, effectCount: 1, amRequests: [{am: 'a',
    request_id: 'actual_request', elapsed_ms: 5, status: 202, agent_id_sha256: digest(event.agentId),
    serving, serving_after: servingAfter, started_at: '2026-11-16T10:00:00.001Z',
    ended_at: '2026-11-16T10:00:00.002Z',
    attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'},
    owner_proof: {verdict: 'PASS', kind: 'non_owner', agent_sha256: digest(event.agentId),
      request_id: 'actual_request', ingress: 'a', owner: 'b', owner_epoch: 3,
      owner_instance_sha256: digest('owner_process'), ingress_instance_sha256: digest('instance-a')}}]};
const receipt = {eventId: event.eventId, responseId: event.responseId, destination: event.destination,
  sender: event.expectedSender, contentDigest: event.expectedContentDigest, status: 'delivered', count: 1};

test('receipt equality ignores JSON object field order and screenshot digest hashes bytes', () => {
  assert.equal(same({id: 'one', version: 3}, {version: 3, id: 'one'}), true);
  assert.equal(same({version: 3}, {version: '3'}), false);
  assert.equal(same([1, 2], [2, 1]), false);
  assert.equal(digest(Buffer.from('abc')), 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad');
});

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
    {providerAccepted: false}, {turnCount: 2}, {effectCount: 0}, {effectCount: 2},
    {amRequests: undefined}, {amRequests: []}]) {
    assert.throws(() => eventJoin(ctx, event, {...turn, ...patch}));
  }
  for (const patch of [{request_id: ''}, {am: 'b'}, {elapsed_ms: undefined}, {agent_id_sha256: digest('wrong')},
    {serving: undefined}, {serving_after: undefined}, {started_at: undefined},
    {ended_at: undefined}, {attribution: {verdict: 'UNPROVED'}},
    {serving_after: {...servingAfter, pod_uid: 'different-pod'}}]) {
    assert.throws(() => eventJoin(ctx, event, {...turn, amRequests: [{...turn.amRequests[0], ...patch}]}));
  }
  for (const patch of [{owner_epoch: 2}, {request_id: 'other'}, {agent_sha256: digest('wrong')},
    {ingress: 'b'}, {verdict: 'RUNNING'}, {kind: 'explicit_ingress'}, {ingress_instance_sha256: digest('owner_process')}]) {
    assert.throws(() => eventJoin(ctx, event, {...turn, amRequests: [{...turn.amRequests[0],
      owner_proof: {...turn.amRequests[0].owner_proof, ...patch}}]}));
  }
});

test('EG AM evidence preserves the complete serving receipt projection', async () => {
  let retained;
  const context = {...ctx, observe: async (_label, operation) => {
    retained = await operation(); return retained;
  }};
  await retainTurn(context, event, turn);
  assert.deepEqual(retained.requests[0].serving, serving);
  assert.deepEqual(retained.requests[0].serving_after, servingAfter);
  assert.equal(retained.requests[0].started_at, turn.amRequests[0].started_at);
  assert.equal(retained.requests[0].ended_at, turn.amRequests[0].ended_at);
  assert.deepEqual(retained.requests[0].attribution, turn.amRequests[0].attribution);
  assert.equal(retained.requests[0].request_id, 'actual_request');
  assert.equal(retained.eg_receipts.length, 0);
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

test('browser setup uses shared origin and retains attribution after closing', async () => {
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
  assert.deepEqual(order, ['http://127.0.0.1:1234/event-gateway', 'close', 'evidence']);
});

test('delayed drain errors and simultaneous close/evidence errors cannot leave EG UI PASS', async () => {
  for (const primary of [false, true]) {
    const order = [];
    const row = {case_id: 'C275:EG-001', id: 'EG-001', family: 'eg', required_steps: ['expected_result']};
    const page = {getByRole() {}, async goto() {}};
    const browser = {open: async () => ({page, origin: 'http://127.0.0.1:1234', async close() {
      await new Promise(resolve => setImmediate(resolve)); order.push('drained'); throw Error('close_failed');
    }}), async evidence() {order.push('evidence'); throw Error('late_mutation_attribution_failed');}};
    const definition = {id: row.case_id, scope: 'browser', fixtures: ['browser'], steps: row.required_steps,
      run: context => context.step('expected_result', async () => {
        await openBrowser(context); context.assert(!primary, 'original_ui_failure');
      })};
    const result = await executeCase(definition, row, {phase: 'steady', iteration: 1, am: 'a',
      source: 'a'.repeat(40), image: 'fixture@sha256:' + 'b'.repeat(64), fixtures: {browser}});
    assert.equal(result.status, 'FAIL'); assert.deepEqual(order, ['drained', 'evidence']);
    assert.deepEqual(result.cleanup[0].error_codes,
      ['eg_browser_finalization_failed', 'close_failed', 'late_mutation_attribution_failed']);
    if (primary) assert.equal(result.primary_failure, 'original_ui_failure');
  }
});
