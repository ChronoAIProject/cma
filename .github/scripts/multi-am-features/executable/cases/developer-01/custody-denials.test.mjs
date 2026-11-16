import test from 'node:test';
import assert from 'node:assert/strict';
import {noCustodyEscalation} from './cross-owner.mjs';

function fixture(options = {}) {
  const calls = [], observations = [], cleanups = [];
  const deniedOperations = [
    {kind: 'sharing', path: '/api/v1/resource-shares/agent/owned-agent',
      method: 'POST', body: {target_org_id: 'local-org', mode: 'view'},
      denials: [{status: 403, code: 'resource_forbidden'}]},
    {kind: 'service', path: '/api/v1/sandboxes/owned-sandbox/service-access',
      method: 'POST', body: {services: ['local-service']},
      denials: [{status: 403, code: 'forbidden'}, {status: 404, code: 'sandbox_not_found'}]},
    // Contract-only bridge placeholder. It is never a live documented endpoint
    // or an executable factory; the real parent must bind current management.
    {kind: 'bridge', path: '/fixture-only-management', method: 'POST', body: {},
      denials: [{status: 403, code: 'fixture_bridge_forbidden'}]},
    {kind: 'profile', path: '/api/v1/agent-profiles/owned-profile', method: 'PUT', body: {},
      denials: [{status: 403, code: 'forbidden'}, {status: 404, code: 'profile_not_found'}]},
  ].map((operation, i) => ({...operation, key: 'operation-' + i}));
  const setup = {scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, agentId: 'owned-agent',
    deniedOperations, readPublicState: async () => ({shares: [], service_scope: [], revision: 1}),
    cleanup: async () => {}};
  const ctx = {am: 'a', fixture: () => ({take: async () => setup}),
    assert: (value, code) => assert.ok(value, code), step: async (_label, run) => run(),
    defer: (_label, run) => cleanups.push(run),
    observe: async (label, run) => {const value = await run(); observations.push({label, value}); return value;},
    request: async args => {
      calls.push(args);
      if (args.path.includes('/resource-access/')) return {status: 200, body: {
        capabilities: args.actor === 'creator' ? ['manage_sharing', 'manage_bridge'] : ['view_details']}};
      if (args.actor === 'creator') return {status: 200, body: {items: []}};
      const operation = deniedOperations.find(op => op.path === args.path);
      assert.ok(operation);
      if (options.response) return options.response;
      const refusal = options.hiddenProfile && operation.kind === 'profile'
        ? operation.denials[1] : operation.denials[0];
      return {status: refusal.status, body: {code: refusal.code}};
    }};
  return {ctx, setup, calls, observations, cleanups};
}

test('typed denials retain the complete read/control boundary and owner readback', async () => {
  const f = fixture({hiddenProfile: true}); await noCustodyEscalation.run(f.ctx);
  assert.equal(f.calls.filter(r => ['read_operator', 'control_operator'].includes(r.actor)
    && !r.path.includes('/resource-access/')).length, 8);
  assert.deepEqual(f.observations.map(o => o.label), ['admin_custody_before', 'admin_custody_after']);
  assert.ok(f.calls.some(r => r.actor === 'creator' && r.path.startsWith('/api/v1/resource-shares/')));
});

for (const response of [{status: 404, body: 'Not Found'}, {status: 404, body: {}},
  {status: 404, body: {code: 'route_not_found'}}, {status: 403, body: {code: 'unrelated_refusal'}},
  {status: 404, body: {code: 'resource_forbidden'}}]) {
  test('unclassified or mismatched denial cannot prove custody: ' + JSON.stringify(response), async () => {
    const f = fixture({response});
    await assert.rejects(noCustodyEscalation.run(f.ctx), /admin_operator_custody_denial_unclassified/);
    assert.equal(f.calls.filter(r => r.path === f.setup.deniedOperations[0].path).length, 1);
  });
}

for (const denials of [undefined, [], [{status: 200, code: 'resource_forbidden'}],
  [{status: 403, code: ''}], [{status: 403, code: 'contains spaces'}]]) {
  test('an absent or malformed denial contract blocks before requests: ' + JSON.stringify(denials), async () => {
    const f = fixture(); f.setup.deniedOperations[0].denials = denials;
    await assert.rejects(noCustodyEscalation.run(f.ctx), /admin_custody_denial_contract_missing/);
    assert.equal(f.calls.length, 0);
  });
}
