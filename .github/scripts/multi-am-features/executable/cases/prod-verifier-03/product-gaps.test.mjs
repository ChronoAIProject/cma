import test from 'node:test';
import assert from 'node:assert/strict';
import {executeCase} from '../../runner.mjs';
import {channelCases} from './channel-remaining.mjs';
import {rows} from './helpers.mjs';

for (const [number, scenario, code] of [
  [23, 'producer_edit', 'producer_removal_web_workflow_missing'],
  [26, 'multi_consumer_routing', 'routing_handoff_runtime_and_limits_missing'],
]) test(`EG-${number} cannot pass its missing product clause or mutate before blocking`, async () => {
  const id = 'C275:EG-' + String(number).padStart(3, '0');
  const definition = channelCases.find(value => value.id === id), row = rows.find(value => value.case_id === id);
  let cleaned = 0, dispatched = 0; const acquired = [];
  const image = 'local@sha256:' + 'a'.repeat(64);
  const env = {phase: 'before_restart', iteration: 1, am: 'a', source: 'b'.repeat(40), image,
    fixtures: {browser: {open: async () => {dispatched++; throw Error('unexpected_browser');}},
      'user-fixtures': {take: async name => {
        acquired.push(name);
        return {local: true, synthetic: true,
          replicas: ['a', 'b'].map(slot => ({slot, instance: 'eg-' + slot, image, ready: true})),
          close: async () => {cleaned++;}, request: async () => {dispatched++;},
          platformSend: async () => {dispatched++;}};
      }}}, transport: {request: async () => {dispatched++;}}};
  const result = await executeCase(definition, row, env);
  assert.equal(result.status, 'BLOCKED'); assert.equal(result.error_code, code);
  assert.deepEqual(definition.steps, ['step_1', 'step_2', 'expected_result']);
  assert.deepEqual(acquired, [scenario]); assert.equal(dispatched, 0); assert.equal(cleaned, 1);
  assert.equal(result.requests.length, 0);
  assert.deepEqual(result.cleanup.map(value => value.status), ['PASS']);
});
