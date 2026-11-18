import assert from 'node:assert/strict';
import {performance} from 'node:perf_hooks';

export function isNativeSuspended(snapshot) {
  return snapshot?.operating_mode === 'Suspended'
    && Number.isSafeInteger(snapshot.generation) && snapshot.generation > 0
    && Array.isArray(snapshot.pods) && snapshot.pods.length === 0
    && Array.isArray(snapshot.conditions)
    && snapshot.conditions.some(condition => condition.type === 'Suspended'
      && condition.status === 'True' && condition.observedGeneration === snapshot.generation);
}

export async function awaitCreatedIdle(read, waits, {timeout = 120000, interval = 2000} = {}) {
  assert.equal(typeof read, 'function', 'created_agent_reader_required');
  assert.equal(typeof waits?.poll, 'function', 'bounded_poll_required');
  assert.equal(typeof waits?.awaitInitialIdle, 'function', 'bounded_idle_wait_required');
  assert(Number.isFinite(timeout) && timeout > 0, 'created_agent_timeout_invalid');
  assert(Number.isFinite(interval) && interval >= 0, 'created_agent_interval_invalid');
  const {poll, awaitInitialIdle, now = () => performance.now()} = waits;
  const started = now();
  assert(Number.isFinite(started), 'created_agent_clock_invalid');
  const deadline = started + timeout;
  // A successful lifecycle admission precedes the Agent worker's Ready publish.
  // Do not feed that legitimate starting state to the stricter active-turn check.
  await poll(read, value => {
    assert(value && typeof value.status === 'string', 'created_agent_status_missing');
    assert(!['failed', 'deleted', 'waiting_for_connection'].includes(value.status), 'created_agent_failed');
    return ['ready', 'working'].includes(value.status) && value.sandbox?.status === 'active';
  }, {timeout, interval, code: 'created_agent_ready_timeout'});
  const remaining = deadline - now();
  assert(remaining > 0 && remaining <= timeout, 'created_agent_idle_deadline');
  const ready = await awaitInitialIdle(read, {timeout: remaining, interval});
  const finished = now();
  assert(Number.isFinite(finished) && finished >= started && finished <= deadline,
    'created_agent_idle_deadline');
  assert(ready?.status === 'ready' && ready.sandbox?.status === 'active', 'created_agent_not_idle');
  assert(!ready.first_message || (['setup', 'task'].includes(ready.first_message.purpose)
    && ready.first_message.state === 'completed'), 'created_agent_initial_message_incomplete');
  return ready;
}
