import test from 'node:test';
import assert from 'node:assert/strict';
import {nativePlatformCases} from './native-platform.mjs';
import {Blocked} from '../../runner.mjs';

test('EG-043 cannot pass using generic native/relay resource verdicts', async () => {
  const definition = nativePlatformCases.find(c => c.id === 'C275:EG-043');
  assert.deepEqual(definition.steps, ['step_1', 'step_2', 'expected_result']);
  let fixtureCalls = 0;
  // Inspection has its own real fixture and browser requirements. Even after
  // that step, neither an opaque resource result nor a native sender can PASS.
  await assert.rejects(definition.run({step: async (label, operation) => label === 'step_2' ? operation() : undefined,
    fixture: () => {fixtureCalls++; throw Error('generic_resource_oracle_must_not_run');}}),
  error => error instanceof Blocked && error.message === 'native_lark_resource_public_journeys_incomplete');
  assert.equal(fixtureCalls, 0);
  const dispatched = [];
  await definition.run({step: async label => dispatched.push(label)});
  assert.deepEqual(dispatched, definition.steps);
});
