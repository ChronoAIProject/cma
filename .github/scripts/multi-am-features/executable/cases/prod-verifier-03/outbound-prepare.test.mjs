import test from 'node:test';
import assert from 'node:assert/strict';
import {Blocked} from '../../runner.mjs';
import {validateOutboundFixture, assertPreparedReceipt, outboundPrepareCase} from './outbound-prepare.mjs';

const body = {source_id: 'relay_telegram', route_id: '11111111-1111-4111-8111-111111111111',
  message: {text: 'prepared text'}};
const expected = {sourceId: body.source_id, routeId: body.route_id, destination: body.route_id};
const lease = {channelId: 'channel', outboundBody: body, expected, keys: {prepare: 'prepare-key'},
  request: async () => ({}), captureEffects: async () => []};
const ctx = {assert: (value, code) => assert.ok(value, code)};
const receipt = {delivery_id: 'delivery', source_id: body.source_id, route_id: body.route_id,
  status: 'prepared', expires_at: 200, retained_until: 300, rendering: {}, delivery: null};

test('outbound fixture requires a saved UUID route, exact source and bounded public media', () => {
  assert.deepEqual(validateOutboundFixture(lease).body, body);
  for (const patch of [
    {outboundBody: {...body, route_id: 'route'}},
    {outboundBody: {...body, source_id: 'foreign'}},
    {outboundBody: {...body, message: {text: 'x', native_attachments: [{kind: 'image', source: {type: 'url', url: 'http://bad'}}]}}},
    {outboundBody: {...body, message: {native_attachments: Array.from({length: 5}, () => ({kind: 'image', source: {type: 'url', url: 'https://media.example/x'}}))}}},
    {keys: {prepare: ''}}, {captureEffects: undefined}, {expected: {...expected, destination: 'other'}},
  ]) assert.throws(() => validateOutboundFixture({...lease, ...patch}), Blocked);
});

test('prepared receipt is durable and cannot be relabeled as delivered or changed destination', () => {
  assertPreparedReceipt(ctx, receipt, body, lease.keys.prepare, expected);
  for (const patch of [{status: 'committed'}, {delivery: {outcome: 'confirmed'}},
    {route_id: 'other'}, {source_id: 'other'}, {expires_at: 'bad'}, {retained_until: 100}])
    assert.throws(() => assertPreparedReceipt(ctx, {...receipt, ...patch}, body, lease.keys.prepare, expected), /outbound_/);
  assert.equal(outboundPrepareCase.steps.join(','), 'step_1,step_2,expected_result');
  assert.deepEqual(outboundPrepareCase.fixtures, ['browser', 'user-fixtures', 'eg_browser']);
});

test('old direct API-only prepare cannot pass through the frozen case', async () => {
  const cleanups = [];
  const context = {row: {case_id: outboundPrepareCase.id}, phase: 'steady', iteration: 1, am: 'a',
    defer: (_label, operation) => cleanups.push(operation)};
  const direct = {...lease, close: async () => {}};
  await assert.rejects(import('./setup.mjs').then(({parentEgSetup}) => parentEgSetup(context,
    {eg: {acquire: async () => direct}}, 'outbound_prepare')), error => error instanceof Blocked);
  for (const cleanup of cleanups) await cleanup();
});
