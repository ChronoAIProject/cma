import test from 'node:test';
import assert from 'node:assert/strict';
import {awaitCreatedIdle, isNativeSuspended} from './lifecycle.mjs';


// Offline implementation of the injected bounded-wait contract. Virtual time
// exercises deadlines deterministically; no runtime or cluster is contacted.
function waitsFixture() {
  let clock = 0;
  const calls = [];
  const waits = {
    now: () => clock,
    async poll(read, accept, {timeout, interval, code}) {
      calls.push({kind: 'poll', timeout});
      const deadline = clock + timeout;
      while (clock < deadline) {
        const value = await read();
        if (accept(value)) return value;
        clock += Math.max(1, interval);
      }
      throw Error(code);
    },
    async awaitInitialIdle(read, options) {
      calls.push({kind: 'idle', timeout: options.timeout});
      return waits.poll(read, value => {
        assert(['ready', 'working'].includes(value?.status), 'open_not_ready');
        const message = value.first_message;
        if (!message) {assert.equal(value.status, 'ready', 'open_working_without_first_message'); return true;}
        assert(['setup', 'task'].includes(message.purpose), 'open_first_message_purpose_unknown');
        assert(['queued', 'dispatch_fenced', 'write_confirmed', 'execution_observed', 'completed'].includes(message.state),
          'open_first_message_failed');
        return value.status === 'ready' && message.state === 'completed';
      }, {...options, code: 'open_initial_message_timeout'});
    },
  };
  return {waits, calls, advance: milliseconds => {clock += milliseconds;}};
}
const createdIdle = (read, options) => awaitCreatedIdle(read, waitsFixture().waits, options);

const suspended = {operating_mode: 'Suspended', generation: 2, pods: [],
  conditions: [{type: 'Suspended', status: 'True', observedGeneration: 2}]};

test('native suspension uses the current controller condition and zero Pods', () => {
  assert.equal(isNativeSuspended(suspended), true);
  assert.equal(Object.hasOwn(suspended, 'replicas'), false);
});

test('missing, stale, running, and terminating native state cannot qualify', () => {
  for (const change of [
    {operating_mode: 'Running'}, {operating_mode: undefined},
    {generation: undefined}, {generation: 0},
    {pods: undefined}, {pods: [{phase: 'Running'}]}, {pods: [{deleting: true}]},
    {conditions: undefined}, {conditions: []},
    {conditions: [{type: 'Ready', status: 'True', observedGeneration: 2}]},
    {conditions: [{type: 'Suspended', status: 'False', observedGeneration: 2}]},
    {conditions: [{type: 'Suspended', status: 'True', observedGeneration: 1}]},
  ]) assert.equal(isNativeSuspended({...suspended, ...change}), false);
});

test('admission waits through starting and completes the original setup turn', async () => {
  const states = [
    {status: 'starting', sandbox: {status: 'active'}},
    {status: 'working', sandbox: {status: 'active'}, first_message: {purpose: 'setup', state: 'queued'}},
    {status: 'working', sandbox: {status: 'active'}, first_message: {purpose: 'setup', state: 'execution_observed'}},
    {status: 'ready', sandbox: {status: 'active'}, first_message: {purpose: 'setup', state: 'completed'}},
  ];
  let reads = 0;
  const ready = await createdIdle(async () => states[reads++], {timeout: 100, interval: 0});
  assert.equal(reads, 4);
  assert.equal(ready.first_message.state, 'completed');
});

test('Agent failure and malformed status are refused before an idle check', async () => {
  for (const status of ['failed', 'deleted', 'waiting_for_connection']) {
    let reads = 0;
    await assert.rejects(createdIdle(async () => {reads += 1; return {status};}), /created_agent_failed/);
    assert.equal(reads, 1);
  }
  await assert.rejects(createdIdle(async () => ({})), /created_agent_status_missing/);
});

test('missing active Sandbox never qualifies as Ready and the wait is bounded', async () => {
  await assert.rejects(createdIdle(async () => ({status: 'ready', sandbox: {status: 'starting'}}),
    {timeout: 10, interval: 0}), /created_agent_ready_timeout/);
});

test('a Ready Agent without an initial message qualifies without inventing a turn', async () => {
  const ready = {status: 'ready', sandbox: {status: 'active'}};
  assert.deepEqual(await createdIdle(async () => ready), ready);
});

test('the injected wait phases share one deadline, with no reset after Ready', async () => {
  const fixture = waitsFixture();
  const ready = {status: 'ready', sandbox: {status: 'active'}};
  let reads = 0;
  await awaitCreatedIdle(async () => {
    reads++;
    if (reads === 1) {fixture.advance(30); return {status: 'starting'};}
    return ready;
  }, fixture.waits, {timeout: 100, interval: 5});
  assert.equal(fixture.calls.find(x => x.kind === 'idle').timeout, 65);
  assert.equal(reads, 3);
});

test('missing wait dependencies and invalid budgets refuse without a read', async () => {
  const read = async () => {throw Error('must_not_read');};
  for (const waits of [undefined, {}, {poll() {}}]) await assert.rejects(awaitCreatedIdle(read, waits), /required/);
  for (const timeout of [0, -1, NaN, Infinity]) {
    await assert.rejects(awaitCreatedIdle(read, waitsFixture().waits, {timeout}), /timeout_invalid/);
  }
  await assert.rejects(awaitCreatedIdle(read, waitsFixture().waits, {interval: -1}), /interval_invalid/);
  assert.equal(isNativeSuspended(null), false);
});

test('exhausted or nonmonotonic clocks and late idle results cannot qualify', async () => {
  const ready = {status: 'ready', sandbox: {status: 'active'}};
  for (const clocks of [[0, 100], [0, -1], [0, 10, 101], [0, 10, -1], [NaN]]) {
    let index = 0;
    const waits = {poll: async () => ready, awaitInitialIdle: async () => ready,
      now: () => clocks[index++]};
    await assert.rejects(awaitCreatedIdle(async () => ready, waits, {timeout: 100}), /deadline|clock_invalid/);
  }
});

test('terminal or refused initial messages never qualify as idle', async () => {
  for (const first_message of [{purpose: 'unknown', state: 'completed'},
    {purpose: 'setup', state: 'failed'}, {purpose: 'task', state: 'refused'}]) {
    await assert.rejects(createdIdle(async () => ({status: 'ready', sandbox: {status: 'active'}, first_message})),
      /open_first_message/);
  }
  await assert.rejects(createdIdle(async () => ({status: 'working', sandbox: {status: 'active'}})),
    /open_working_without_first_message/);
});

test('an injected idle helper cannot substitute an unfinished or inactive result', async () => {
  const ready = {status: 'ready', sandbox: {status: 'active'}};
  for (const result of [{...ready, status: 'working'}, {...ready, sandbox: {status: 'paused'}},
    {...ready, first_message: {purpose: 'setup', state: 'queued'}},
    {...ready, first_message: {purpose: 'unknown', state: 'completed'}}]) {
    await assert.rejects(awaitCreatedIdle(async () => ready,
      {poll: async () => ready, awaitInitialIdle: async () => result}), /not_idle|initial_message_incomplete/);
  }
});
