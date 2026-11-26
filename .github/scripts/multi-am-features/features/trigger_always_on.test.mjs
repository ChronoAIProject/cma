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
  let createKey, startKey, ownerReceipt, ownerKey;
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
    if (path === '/api/v1/sandboxes/' + sandbox + '/always-on') {
      assert.equal(method, 'POST');
      if (options.body.always_on) {
        assert.deepEqual(options.allowed, [403]);
        assert.equal(options.body.expected_revision, revision);
        calls.push('owner-self-approval-refused');
        return {status: 403, body: {code: 'always_on_approval_required'}};
      }
      if (options.allowed?.includes(409)) {
        assert.equal(options.body.expected_revision, 1);
        assert(alwaysOn);
        calls.push('stale-owner-relinquishment-refused');
        return {status: 409, body: {code: 'sandbox_invalid_state'}};
      }
      if (ownerReceipt) {
        assert.equal(am, 'a');
        assert.equal(options.key, ownerKey);
        assert.equal(options.body.expected_revision, revision - 1);
        calls.push('owner-relinquishment-replay');
        return {status: 200, body: ownerReceipt, timing: {request_id: 'fixture-replay'}};
      }
      assert.equal(am, 'b');
      assert.equal(options.body.expected_revision, revision);
      assert(alwaysOn);
      alwaysOn = false;
      ownerKey = options.key;
      ownerReceipt = {id: sandbox, always_on: alwaysOn, revision: ++revision};
      calls.push('owner-relinquishment');
      return {status: 200, body: ownerReceipt};
    }
    if (path.endsWith('/launches')) {
      if (!createKey) {
        assert.equal(am, 'a');
        createKey = options.key;
        calls.push('create');
      } else if (options.body.start_if_ready) {
        assert.deepEqual(options.allowed, [422]);
        assert.equal(options.key, createKey);
        return {status: 422, body: {code: 'idempotency_key_reused'}};
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
    if (path.endsWith('/provenance')) return {body: {agent_id: agent, creation_origin: 'trigger',
      details: {trigger: {trigger_id: trigger, launch_id: launch}}}};
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
    assert.deepEqual(options.allowed, path.startsWith('/api/v1/admin/') ? [403] : [404]);
    if (path.startsWith('/api/v1/sandboxes/')) calls.push('foreign-relinquishment-refused');
    return {status: options.allowed[0]};
  }};
  const sandboxRoute = '/sandboxes/' + sandbox;
  const relinquishmentPath = '/api/v1' + sandboxRoute + '/always-on';
  const browser = {forIngress: async (am, run) => {
    let href, request, method = 'GET';
    const origin = 'http://127.0.0.1:12345';
    const page = {
      goto: async path => { href = origin + path; },
      url: () => href,
      waitForEvent: async (name, {predicate}) => {
        assert.equal(name, 'requestfinished');
        const request = {method: () => 'GET', url: () => origin + '/api/v1' + sandboxRoute,
          response: async () => ({status: () => 200, headerValue: async () => 'GET-' + am,
            json: async () => ({id: sandbox, revision, always_on: alwaysOn})})};
        assert(predicate(request));
        return request;
      },
      getByText: text => ({waitFor: async () => assert.match(text, alwaysOn ? /administrator approved/ : /Always on is off/)}),
      getByRole: (role, {name}) => {
        assert.equal(role, 'button');
        assert.equal(name, 'Turn off Always on');
        method = 'POST';
        return {
          click: async () => { await request; },
          waitFor: async ({state}) => { assert.equal(state, 'hidden'); assert.equal(alwaysOn, false); },
        };
      },
      waitForResponse: predicate => {
        const body = {always_on: false, expected_revision: revision}, key = 'fixture-browser-intent';
        const responseMethod = method;
        const value = {request: () => ({method: () => responseMethod, postDataJSON: () => body,
          headerValue: async () => key}),
        status: () => 200, url: () => origin + (responseMethod === 'POST' ? relinquishmentPath : '/api/v1' + sandboxRoute),
        headerValue: async () => responseMethod + '-' + am,
        json: async () => responseMethod === 'POST' ? (await request).body : {id: sandbox, revision, always_on: alwaysOn}};
        assert(predicate(value));
        if (responseMethod === 'POST') request = user.request(am, 'POST', relinquishmentPath, {body, key});
        return Promise.resolve(value);
      },
    };
    return run({page, origin});
  }};
  const result = await triggerAlwaysOn({adapter, user, admin, outsider, browser,
    config: {owner_id: subject, public_origin: 'https://fixture.example'}});
  assert.deepEqual(calls, ['enable-triggers', 'create', 'start', 'enable', 'replay-create', 'replay-start',
    'owner-self-approval-refused', 'stale-owner-relinquishment-refused', 'foreign-relinquishment-refused',
    'owner-relinquishment', 'owner-relinquishment-replay', 'replay-create', 'replay-start',
    'stop-agent', 'delete-agent', 'retire-sandbox', 'retire-trigger',
    'restore-triggers']);
  assert.equal(result.owner_relinquishment.ingress, 'b');
  assert.equal(result.owner_relinquishment.revisions_advanced, 1);
  assert.equal(new Set(Object.values(result.owner_relinquishment.request_sha256)).size, 4);
});
