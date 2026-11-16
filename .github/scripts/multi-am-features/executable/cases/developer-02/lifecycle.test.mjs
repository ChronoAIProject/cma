import test from 'node:test';
import assert from 'node:assert/strict';
import {stopEligible} from './lifecycle.mjs';

test('Stop uses the serialized working status for an active turn', () => {
  assert.equal(stopEligible('working'), true);
  for (const status of ['starting', 'ready', 'waiting_approval', 'paused', 'stopped', 'deleted', 'failed', 'running'])
    assert.equal(stopEligible(status), false, status);
});
