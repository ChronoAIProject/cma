import test from 'node:test';
import assert from 'node:assert/strict';
import {awaitCreatedIdle, isNativeSuspended} from './lifecycle.mjs';

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
  const ready = await awaitCreatedIdle(async () => states[reads++], {timeout: 100, interval: 0});
  assert.equal(reads, 4);
  assert.equal(ready.first_message.state, 'completed');
});

test('Agent failure and malformed status are refused before an idle check', async () => {
  for (const status of ['failed', 'deleted', 'waiting_for_connection']) {
    let reads = 0;
    await assert.rejects(awaitCreatedIdle(async () => {reads += 1; return {status};}), /created_agent_failed/);
    assert.equal(reads, 1);
  }
  await assert.rejects(awaitCreatedIdle(async () => ({})), /created_agent_status_missing/);
});

test('missing active Sandbox never qualifies as Ready and the wait is bounded', async () => {
  await assert.rejects(awaitCreatedIdle(async () => ({status: 'ready', sandbox: {status: 'starting'}}),
    {timeout: 10, interval: 0}), /created_agent_ready_timeout/);
});

test('a Ready Agent without an initial message qualifies without inventing a turn', async () => {
  const ready = {status: 'ready', sandbox: {status: 'active'}};
  assert.deepEqual(await awaitCreatedIdle(async () => ready), ready);
});
