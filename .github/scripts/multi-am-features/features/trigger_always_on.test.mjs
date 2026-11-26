import assert from 'node:assert/strict';
import test from 'node:test';
import {triggerAlwaysOn} from './trigger_always_on.mjs';

test('Trigger policy toggles replay the original visitor receipts and retire the exact resources', async () => {
  const agent = 'fixture-agent', sandbox = 'fixture-sandbox', subject = 'fixture-owner';
  const trigger = 'fixture-trigger', launch = 'fixture-launch', calls = [];
  const initial = {id: launch, trigger_id: trigger, revision_digest: 'fixture-digest',
    resolution: {agent: {state: 'pending'}}, trust: {confirmation_digest: 'fixture-trust'}};
  const admitted = {...initial, state: 'ready', resolution: {
    agent: {state: 'created', id: agent}, sandbox: {state: 'created', id: sandbox},
  }};
  let alwaysOn = false, revision = 1, agentState = 'ready', sandboxState = 'active';
  let triggers = {enabled: false, orchestration_paused: false};
  let createKey, startKey;
  const adapter = {ownerOf: async input => {
    assert.equal(input.agentId, agent);
    return {am: 'a', agent_id: agent, epoch: 3, pod_uid: 'manager-a'};
  }};
  const admin = {request: async (am, method, path, options) => {
    if (path === '/api/v1/admin/settings') {
      if (method === 'PATCH') {
        assert.equal(options.body.expected_revision, 1);
        triggers = options.body.triggers;
        calls.push(triggers.enabled ? 'enable-triggers' : 'restore-triggers');
      }
      return {body: {revision: 1, settings: {triggers}}};
    }
    if (path === '/api/v1/triggers') return {body: {id: trigger, draft_version: 1}};
    if (path.endsWith('/publish')) return {body: {public_id: 'fixture-public'}};
    if (path.endsWith('/retire')) {
      calls.push('retire-trigger');
      return {status: 200, body: {id: trigger, retired_at: 'fixture-time'}};
    }
    if (method === 'GET') return {body: {sandbox: {id: sandbox, revision, profile: {always_on: alwaysOn}}}};
    assert.match(path, /fixture-sandbox\/always-on$/);
    if (options.allowed.includes(409)) {
      assert.equal(options.body.expected_revision, 1);
      assert(alwaysOn);
      return {status: 409};
    }
    assert.equal(am, 'b');
    assert.equal(options.body.expected_revision, revision++);
    alwaysOn = options.body.always_on;
    calls.push(alwaysOn ? 'enable' : 'revoke');
    return {body: {id: sandbox, always_on: alwaysOn}};
  }};
  const user = {request: async (am, method, path, options) => {
    if (path.endsWith('/launches')) {
      if (!createKey) {
        assert.equal(am, 'a');
        createKey = options.key;
        calls.push('create');
      } else if (options.body.start_if_ready) {
        assert.deepEqual(options.allowed, [409]);
        assert.equal(options.key, createKey);
        return {status: 409};
      } else {
        assert.equal(options.key, createKey);
        calls.push('replay-create');
      }
      return {status: 201, body: initial};
    }
    if (path.endsWith('/start')) {
      assert.equal(am, 'b');
      assert.deepEqual(options.body, {trust_confirmed_digest: 'fixture-trust'});
      if (startKey) assert.equal(options.key, startKey);
      calls.push(startKey ? 'replay-start' : 'start');
      startKey = options.key;
      return {status: 202, body: admitted};
    }
    if (path.endsWith(launch)) return {body: admitted};
    if (path.startsWith('/api/v1/agents?')) return {body: {items: []}};
    if (path.endsWith('/stop')) {
      agentState = 'stopped'; calls.push('stop-agent');
      return {status: 202};
    }
    if (path.endsWith(agent)) {
      if (method === 'DELETE') { agentState = 'deleted'; calls.push('delete-agent'); }
      return {body: {id: agent, status: agentState, sandbox: {id: sandbox}}};
    }
    assert(path.endsWith(sandbox));
    if (method === 'DELETE') { sandboxState = 'retired'; calls.push('retire-sandbox'); }
    return {body: {id: sandbox, status: sandboxState, owner: subject}};
  }};
  const outsider = {request: async (_, method, path, options) => {
    assert.equal(method, 'POST');
    assert.deepEqual(options.allowed, path.endsWith('/start') ? [404] : [403]);
    return {status: options.allowed[0]};
  }};
  await triggerAlwaysOn({adapter, user, admin, outsider,
    config: {owner_id: subject, public_origin: 'https://fixture.example'}});
  assert.deepEqual(calls, ['enable-triggers', 'create', 'start', 'enable', 'replay-create', 'replay-start',
    'revoke', 'replay-create', 'replay-start', 'stop-agent', 'delete-agent', 'retire-sandbox', 'retire-trigger',
    'restore-triggers']);
});
