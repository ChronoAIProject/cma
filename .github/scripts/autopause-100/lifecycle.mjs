import assert from 'node:assert/strict';
import {performance} from 'node:perf_hooks';
import {poll, awaitInitialIdle} from '../multi-am-features/cma/core.mjs';

export function isNativeSuspended(snapshot) {
  return snapshot.operating_mode === 'Suspended'
    && Number.isSafeInteger(snapshot.generation) && snapshot.generation > 0
    && Array.isArray(snapshot.pods) && snapshot.pods.length === 0
    && Array.isArray(snapshot.conditions)
    && snapshot.conditions.some(condition => condition.type === 'Suspended'
      && condition.status === 'True' && condition.observedGeneration === snapshot.generation);
}

export async function awaitCreatedIdle(read, {timeout = 120000, interval = 2000} = {}) {
  const deadline = performance.now() + timeout;
  // A successful lifecycle admission precedes the Agent worker's Ready publish.
  // Do not feed that legitimate starting state to the stricter active-turn check.
  await poll(read, value => {
    assert(value && typeof value.status === 'string', 'created_agent_status_missing');
    assert(!['failed', 'deleted', 'waiting_for_connection'].includes(value.status), 'created_agent_failed');
    return ['ready', 'working'].includes(value.status) && value.sandbox?.status === 'active';
  }, {timeout, interval, code: 'created_agent_ready_timeout'});
  const remaining = deadline - performance.now();
  assert(remaining > 0, 'created_agent_idle_deadline');
  return awaitInitialIdle(read, {timeout: remaining, interval});
}
