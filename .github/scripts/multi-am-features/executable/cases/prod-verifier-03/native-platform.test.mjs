import test from 'node:test';
import assert from 'node:assert/strict';
import {nativePlatformCases} from './native-platform.mjs';
import {Blocked} from '../../runner.mjs';

test('EG-043 cannot pass using generic native/relay resource verdicts', async () => {
  const definition = nativePlatformCases.find(c => c.id === 'C275:EG-043');
  assert.deepEqual(definition.steps, ['step_1', 'step_2', 'expected_result']);
  let fixtureCalls = 0;
  // Inspection and native intake have real fixture requirements. Even after
  // those steps, native delivery cannot imply scoped relay resource access.
  await assert.rejects(definition.run({step: async (label, operation) => label === 'expected_result' ? operation() : undefined,
    fixture: () => {fixtureCalls++; throw Error('generic_resource_oracle_must_not_run');}}),
  error => error instanceof Blocked && error.message === 'native_lark_resource_public_journeys_incomplete');
  assert.equal(fixtureCalls, 0);
  const dispatched = [];
  await definition.run({step: async label => dispatched.push(label)});
  assert.deepEqual(dispatched, definition.steps);
});
