import test from 'node:test';
import assert from 'node:assert/strict';
import {createDefaultsMarketplaceFactory} from './defaults-marketplace-setup.mjs';
import {defaultsMarketplace} from './settings-lifecycle.mjs';
import {defaultsModel} from './defaults-marketplace-model.mjs';
import {createAdminFixtures} from './fixture-provider.mjs';

const setup = async h => h.bind(await createDefaultsMarketplaceFactory(h.bindings)(h.ctx, {config: h.config}));
const messages = error => [error.message, ...(error.errors ?? []).map(messages)].join(' ');

for (const marketplaceEnabled of [false, true]) {
  test('ADM-050 creates with all three defaults and enforces the changed catalog policy: ' + marketplaceEnabled, async () => {
    const h = defaultsModel({marketplaceEnabled}), f = await setup(h);
    assert.equal(h.profiles.size, 3); assert.equal(h.root, undefined);
    assert.equal(new Set(Object.values(f.defaults).map(ref => ref.id)).size, 3);
    await defaultsMarketplace.run(h.ctx); assert.deepEqual(await h.cleanup(), []);
    assert.deepEqual(h.settings, h.original); assert.equal(h.root.status, 'retired'); assert.equal(h.released, 1);
    assert.ok([...h.profiles.values()].every(p => p.retired_at));
    assert.equal(h.calls.filter(c => c.path === '/api/v1/conversations').length, 1);
    assert.equal(h.calls.find(c => c.path === '/api/v1/agents/agent-fixture').agentId, 'agent-fixture');
    assert.equal(h.intents.length, 1); assert.equal(h.retained.length, 1);
  });
}

test('ADM-050 rejects explicit profile overrides before any case mutation', async () => {
  const h = defaultsModel(), f = await setup(h);
  f.creation.body = {metadata: {agent_profile: f.defaults.agent_profile.id}};
  await assert.rejects(defaultsMarketplace.run(h.ctx), /admin_default_driven_creation_required/);
  assert.ok(!h.calls.some(c => c.path === '/api/v1/conversations' || c.method === 'PATCH'));
  assert.deepEqual(await h.cleanup(), []);
});

test('an erroneously accepted unavailable revision fails but original defaults still restore', async () => {
  const h = defaultsModel({acceptUnavailable: true, allowMissingAttribution: true}); await setup(h);
  await assert.rejects(defaultsMarketplace.run(h.ctx), /status_200/);
  assert.deepEqual(await h.cleanup(), []); assert.deepEqual(h.settings, h.original);
  assert.equal(h.root.status, 'retired'); assert.ok([...h.profiles.values()].every(p => p.retired_at));
});

for (const option of ['unknownCreate', 'attributionFailure', 'retainFails']) {
  test('original conversation response or uncertainty stays in custody without replay: ' + option, async () => {
    const h = defaultsModel({[option]: true}); await setup(h);
    await assert.rejects(defaultsMarketplace.run(h.ctx));
    const cleanup = await h.cleanup();
    assert.equal(h.calls.filter(c => c.path === '/api/v1/conversations').length, 1);
    assert.deepEqual(h.settings, h.original);
    if (option === 'attributionFailure') {
      assert.deepEqual(cleanup, []); assert.equal(h.root.status, 'retired');
    } else {
      assert.ok(cleanup.some(e => messages(e).includes('create_unknown'))); assert.equal(h.released, 0);
    }
  });
}

for (const [option, code] of [['unknownRootRetire', 'root_retire_unknown'], ['nativePending', 'native_unresolved'],
  ['foreignRoot', 'cleanup_identity'], ['unknownProfileRetire', 'profile_retire_unknown']]) {
  test('cleanup retains exact failure without repeated mutation: ' + option, async () => {
    const h = defaultsModel({[option]: true}); await setup(h); await defaultsMarketplace.run(h.ctx);
    const first = await h.cleanup(); assert.ok(first.some(e => messages(e).includes(code)));
    const mutations = h.calls.filter(c => c.method !== 'GET').length;
    const second = await h.cleanup(); assert.ok(second.some(e => messages(e).includes(code)));
    assert.equal(h.calls.filter(c => c.method !== 'GET').length, mutations);
    if (option === 'foreignRoot') assert.ok(!h.calls.some(c => c.method === 'DELETE'));
  });
}

for (const option of ['nonemptyOwner', 'unknownProfileCreate', 'badRevisionContent']) {
  test('actual setup prerequisite or response prevents synthetic readiness: ' + option, async () => {
    const h = defaultsModel({[option]: true}); await assert.rejects(setup(h));
    await h.cleanup(); assert.equal(h.root, undefined);
    assert.ok(!h.calls.some(c => c.path === '/api/v1/conversations'));
  });
}

test('capacity and role bindings are validated before mutations', async () => {
  for (const change of [h => {delete h.bindings.userResources.finality;},
    h => {delete h.bindings.defaultsCapacity;}, h => {h.bindings.defaultsCapacity.cpu_millis = 999999;},
    h => {h.config.actors.user.subject = h.config.actors.admin.subject;}]) {
    const h = defaultsModel(); change(h); await assert.rejects(setup(h));
    assert.ok(!h.calls.some(c => c.method !== 'GET'));
  }
});

for (const failCleanup of [false, true]) {
  test('all profile cleanup stays inside the parent settings lease: ' + failCleanup, async () => {
    const h = defaultsModel({unknownProfileRetire: failCleanup});
    const image = 'localhost/api@sha256:' + '1'.repeat(64);
    let held = false;
    const provider = createAdminFixtures({
      config: {...h.config, scope: 'LOCAL_DISPOSABLE_TWO_AM', database_disposable: true,
        restart_authority: 'PARENT_ONLY', expected_api_image: image, expected_api_source: '2'.repeat(40),
        am_origins: {a: 'http://127.0.0.1:19001', b: 'http://127.0.0.1:19002'}},
      adapter: {topology: async () => ({shared_database: true, coordination_enabled: true,
        ams: Object.fromEntries(['a', 'b'].map(am => [am, {ready: true, image, pod_uid: 'pod-' + am, instance_id: am}]))})},
      withExclusiveSettings: async callback => {held = true; try {await callback();} finally {held = false;}},
      factories: {defaults_marketplace: createDefaultsMarketplaceFactory(h.bindings)},
    });
    h.ctx.fixture = () => provider;
    const real = h.ctx.request;
    h.ctx.request = async args => {
      if (args.path.endsWith('/retire')) assert.equal(held, true, 'profile_retirement_after_lease_release');
      return real(args);
    };
    await defaultsMarketplace.run(h.ctx);
    const errors = await h.cleanup();
    assert.equal(held, false);
    assert.ok([...h.profiles.values()].every(p => p.retired_at));
    if (failCleanup) assert.ok(errors.some(e => messages(e).includes('profile_retire_unknown')));
    else assert.deepEqual(errors, []);
  });
}

test('custody rejects a modified exported create intent before retaining or dispatching', async () => {
  const h = defaultsModel(), f = await setup(h);
  f.creation.key = 'changed-key';
  await assert.rejects(f.custody(f.creation, {actor: 'user', am: 'b'}), /intent_replayed/);
  assert.equal(h.intents.length, 0); assert.deepEqual(await h.cleanup(), []);
});
