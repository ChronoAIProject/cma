import test from 'node:test';
import assert from 'node:assert/strict';
import {cases} from './index.mjs';
import {rows, eventJoin, delivered, invoke} from './helpers.mjs';
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
