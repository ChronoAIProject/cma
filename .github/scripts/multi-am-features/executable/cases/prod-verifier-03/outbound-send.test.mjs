import test from 'node:test';
import assert from 'node:assert/strict';
import {Blocked} from '../../runner.mjs';
import {validateOutboundSendFixture, assertCommittedReceipt, assertCancelledReceipt, outboundSendCase} from './outbound-send.mjs';

const body = {source_id: 'relay_telegram', route_id: '11111111-1111-4111-8111-111111111111', message: {text: 'send'}};
const expected = {ownerSubject: 'owner', prepared: {send: {delivery_id: 'send-id'}, cancel: {delivery_id: 'cancel-id'}, uncertain: {delivery_id: 'unknown-id'}},
  sendEffects: [], cancelEffects: [], uncertainEffects: []};
const lease = {channelId: 'channel', outboundBody: body, expected,
  keys: {send: 'send-key', cancel: 'cancel-key', uncertain: 'unknown-key'}, request: async () => ({}),
  dropOutboundSendResponse: async () => ({}), outboundStatus: async () => ({}), captureEffects: async () => [], drain: async () => ({})};
const ctx = {assert: (value, code) => assert.ok(value, code)};
const committed = {delivery_id: 'send-id', route_id: body.route_id, status: 'committed', delivery: {outcome: 'confirmed'}};
const cancelled = {delivery_id: 'cancel-id', route_id: body.route_id, status: 'cancelled', delivery: null};

test('outbound send fixture binds three distinct prepared operations and exact stable keys', () => {
  assert.equal(validateOutboundSendFixture(lease).prepared.send.delivery_id, 'send-id');
  for (const patch of [{expected: {...expected, prepared: {...expected.prepared, cancel: {delivery_id: 'send-id'}}}},
    {keys: {...lease.keys, send: ''}}, {expected: {...expected, ownerSubject: undefined}},
    {dropOutboundSendResponse: undefined}, {expected: {...expected, sendEffects: undefined}}])
    assert.throws(() => validateOutboundSendFixture({...lease, ...patch}), Blocked);
});

test('send and cancel receipts retain the prepared identity and exact destination', () => {
  assertCommittedReceipt(ctx, committed, 'send-id', body);
  assertCancelledReceipt(ctx, cancelled, 'cancel-id', body);
  assert.throws(() => assertCommittedReceipt(ctx, {...committed, delivery_id: 'other'}, 'send-id', body), /outbound_/);
  assert.throws(() => assertCommittedReceipt(ctx, {...committed, route_id: 'other'}, 'send-id', body), /outbound_/);
  assert.throws(() => assertCommittedReceipt(ctx, {...committed, delivery: {outcome: 'unknown'}}, 'send-id', body), /outbound_/);
  assert.throws(() => assertCancelledReceipt(ctx, {...cancelled, status: 'committed'}, 'cancel-id', body), /outbound_/);
  assert.throws(() => assertCancelledReceipt(ctx, {...cancelled, delivery: {outcome: 'confirmed'}}, 'cancel-id', body), /outbound_/);
  assert.deepEqual(outboundSendCase.steps, ['step_1', 'step_2', 'expected_result']);
  assert.deepEqual(outboundSendCase.fixtures, ['browser', 'user-fixtures', 'eg_browser']);
});
