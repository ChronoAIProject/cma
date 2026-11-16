import test from 'node:test';
import assert from 'node:assert/strict';
import { createUserFixtureFactories, supportedSetupNames } from './setups.mjs';
import { Blocked } from '../../runner.mjs';

function environment({ failApproval = false, uncertain = false, failFinality = false } = {}) {
  const calls = [],
    cleanups = [],
    resources = new Map();
  let counter = 0;
  const ctx = { am: 'a', defer: (_label, fn) => cleanups.push(fn), observe: async (_label, fn) => fn() };
  const options = {
    config: {
      scope: 'LOCAL_DISPOSABLE_TWO_AM',
      database_disposable: true,
      single_matrix_writer: true,
      sandbox_profile: { id: 'profile', revision: 1 },
      admin_available: true,
    },
    finality: async ({ sandboxId }) => ({
      sandbox_id: sandboxId,
      children_remaining: failFinality ? 1 : 0,
      cleanup_complete: !failFinality,
    }),
    request: async (r) => {
      calls.push(r);
      let body,
        status = 200;
      if (r.path === '/api/v1/sandboxes' && r.method === 'POST') {
        if (uncertain) throw Error('transport_lost');
        body = { id: `sandbox-${++counter}`, name: r.body.name, status: 'active', revision: 1 };
        resources.set(body.id, body);
        status = 201;
      } else if (r.path.endsWith('/workspaces/inline')) {
        body = {
          id: `workspace-${++counter}`,
          name: r.body.name,
          status: 'ready',
          sandbox_id: r.path.split('/')[4],
        };
        resources.set(body.id, body);
        status = 201;
      } else if (r.path.endsWith('/always-on')) {
        if (failApproval) throw Error('approval_failed');
        assert.equal(r.actor, 'admin');
        body = { ...resources.get(r.path.split('/')[5]), always_on: true };
      } else {
        body = resources.get(r.path.split('/').at(-1));
        assert.ok(body, 'only own known resources may be read or removed');
        if (r.method === 'DELETE') {
          body.status = 'retired';
          status = 202;
        }
      }
      return { status, body: { ...body }, timing: { am: r.am, request_id: 'setup-' + calls.length } };
    },
  };
  return { ctx, options, calls, cleanups };
}
test('real setup exports create and approve owned Sandbox; cleanup is idempotent', async () => {
  const { ctx, options, calls, cleanups } = environment();
  const factories = createUserFixtureFactories(options);
  assert.deepEqual(Object.keys(factories), [...supportedSetupNames]);
  const fixture = await factories['sandbox-always-on'](ctx);
  assert.equal(fixture.sandboxId, 'sandbox-1');
  assert.equal(fixture.run_owned, true);
  assert.equal(calls.filter((r) => r.path.endsWith('/always-on')).length, 1);
  assert.equal((await fixture.cleanup()).complete, true);
  await cleanups[0]();
  assert.equal(calls.filter((r) => r.method === 'DELETE').length, 1);
});
test('missing parent services block before any creation', async () => {
  const { ctx, options, calls } = environment();
  for (const patch of [{ finality: undefined }, { config: { ...options.config, scope: 'PRODUCTION' } }]) {
    await assert.rejects(
      createUserFixtureFactories({ ...options, ...patch })['sandbox-always-on'](ctx),
      Blocked,
    );
  }
  assert.equal(calls.length, 0);
});
test('failed setup keeps cleanup custody after actual creation', async () => {
  const { ctx, options, calls, cleanups } = environment({ failApproval: true });
  await assert.rejects(createUserFixtureFactories(options)['sandbox-always-on'](ctx), /approval_failed/);
  assert.equal(cleanups.length, 1);
  await cleanups[0]();
  assert.equal(calls.filter((r) => r.method === 'DELETE').length, 1);
});
test('uncertain create is never repeated and cannot claim cleanup complete', async () => {
  const { ctx, options, calls, cleanups } = environment({ uncertain: true });
  await assert.rejects(createUserFixtureFactories(options)['workspace-empty'](ctx), /transport_lost/);
  await assert.rejects(cleanups[0](), /uncertain_creation_requires_parent/);
  assert.equal(calls.length, 1);
});
test('terminal API state alone cannot hide live child resources', async () => {
  const { ctx, options } = environment({ failFinality: true });
  const fixture = await createUserFixtureFactories(options)['workspace-empty'](ctx);
  await assert.rejects(fixture.cleanup(), /children_remaining/);
});
test('one failed child check does not abandon the second owned Sandbox', async () => {
  const { ctx, options, calls } = environment({ failFinality: true });
  const fixture = await createUserFixtureFactories(options)['sandbox-delete'](ctx);
  await assert.rejects(fixture.cleanup(), /children_remaining/);
  assert.equal(calls.filter((r) => r.method === 'DELETE').length, 2);
});
test('Workspace creation fixture has independent sibling and checks registered child parent', async () => {
  const { ctx, options, calls } = environment();
  const fixture = await createUserFixtureFactories(options)['workspace-delete'](ctx);
  assert.notEqual(fixture.workspaceId, fixture.siblingWorkspaceId);
  await fixture.registerWorkspace(fixture.workspaceId);
  await fixture.cleanup();
  assert.equal(calls.filter((r) => r.path.endsWith('/workspaces/inline')).length, 2);
});
