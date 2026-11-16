import test from 'node:test';
import assert from 'node:assert/strict';
import {createUserLimitFactory} from './user-limit-setup.mjs';
import {ownedIntent, perUserLimit} from './enforcement.mjs';

// Stateful public-API/parent contract model, not a live quota verdict.
function model(options = {}) {
  const requests = [], cleanups = [], intents = [], resources = [], refusals = [], observations = [];
  const rows = new Map(); let releases = 0, posts = 0, revision = 1;
  let settings = {retention: {agent_log_days: 30}, limits: {max_sandboxes_per_user: 5},
    inactivity: {auto_pause_enabled: false}, triggers: {enabled: false},
    model_budget: {enabled: false, owners: []}};
  const original = structuredClone(settings);
  const config = {actors: {user: {subject: 'local-quota-owner'}, admin: {subject: 'local-admin'}}};
  const lease = {parent_cleanup_barrier: true,
    async retainIntent(intent) {
      intents.push(structuredClone(intent));
      if (options.intentFailure) throw Error('parent_intent_failed');
    },
    async retainResource(resource) {
      resources.push(structuredClone(resource));
      if (options.resourceFailure) throw Error('parent_resource_failed');
    },
    async retainRefusal(refusal) {
      refusals.push(structuredClone(refusal));
      if (options.refusalFailure) throw Error('parent_refusal_failed');
    },
    async release() {releases++; return {released: !options.releaseFailure};},
  };
  const userResources = {scope: 'LOCAL_DISPOSABLE_TWO_AM', database_disposable: true,
    single_matrix_writer: true, sandbox_profile: {id: 'sbp-local', revision: 2}, timeout_ms: 2000,
    reserve: async () => lease,
    finality: async ({sandboxId}) => ({sandbox_id: sandboxId, children_remaining: 0,
      pending_operations: 0, cleanup_complete: !options.incompleteFinality}),
  };
  const ctx = {am: 'a', row: {case_id: perUserLimit.id}, phase: 'before_restart', iteration: 1,
    assert: (ok, code) => assert.ok(ok, code), step: async (_name, run) => run(),
    defer: (name, run) => cleanups.push({name, run}),
    observe: async (label, run) => {const value = await run(); observations.push({label, value}); return value;},
    request: async input => {
      requests.push(structuredClone(input));
      const {method, path, body, key, actor, am} = input;
      const response = (status, result) => ({status, body: structuredClone(result),
        timing: {am, request_id: 'request-' + requests.length, serving: {pod_uid: 'local-pod-' + am}}});
      if (path === '/api/v1/admin/settings') {
        if (method !== 'GET') {
          assert.equal(body.expected_revision, revision++);
          settings = method === 'PUT' ? structuredClone(body.settings) : {...settings, limits: body.limits};
        }
        return response(200, {revision, settings});
      }
      assert.equal(actor, 'user');
      if (method === 'GET' && path.includes('?')) return response(200, {
        items: [...rows.values()].map(row => options.inventoryMismatch
          ? {...row, id: 'unrelated-' + row.id} : row), ...(options.next ? {next: 'another-page'} : {}),
      });
      if (method === 'POST') {
        posts++;
        assert.ok(intents.some(i => i.key === key && i.am === am), 'durable intent before POST');
        if (rows.size >= settings.limits.max_sandboxes_per_user || options.refuse) {
          const receipt = response(429, {code: options.refusalCode ?? 'sandbox_limit_reached'});
          if (options.wrongRefusalIngress) receipt.timing.am = 'wrong';
          return receipt;
        }
        const row = {id: 'sandbox-' + posts, owner: config.actors.user.subject, name: body.name,
          profile: body.sandbox_profile, status: 'active'};
        rows.set(row.id, row);
        const receipt = response(201, row);
        if (options.unknownCreate) throw Error('create_ack_unknown');
        if (options.attributionFailure) throw Object.assign(Error('request_attribution_missing'), {response: receipt});
        if (options.receiptOwnerMismatch) receipt.body.owner = 'foreign-owner';
        return receipt;
      }
      const row = rows.get(path.split('/').at(-1)); assert.ok(row);
      if (method === 'DELETE') {
        row.status = 'retired';
        if (options.unknownDelete) throw Error('retirement_ack_unknown');
        return response(202, row);
      }
      return response(200, row);
    }};
  const bindings = {userResources};
  const factory = createUserLimitFactory(bindings);
  return {ctx, config, bindings, lease, rows, requests, cleanups, observations, intents, resources, refusals,
    original, get settings() {return settings;}, get releases() {return releases;}, get posts() {return posts;},
    async setup() {return factory(ctx, {config});},
    async fullCase() {
      ctx.fixture = () => ({take: async () => ({...await factory(ctx, {config}),
        scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, exclusiveSettings: true})});
      return perUserLimit.run(ctx);
    },
    async cleanup() {
      const errors = [];
      for (const {run} of cleanups.toReversed()) {try {await run();} catch (e) {errors.push(e);}}
      return errors;
    },
  };
}

const flattened = error => [error.message, ...(error.errors ?? []).flatMap(flattened)].join(' ');
const created = async (h, f, index = 0) => ownedIntent(h.ctx, f, f.intents[index], {am: index % 2 ? 'b' : 'a'});

test('quota recipe runs the real case against the contract model and cleans only its two resources', async () => {
  const h = model(); await h.fullCase();
  assert.equal(h.posts, 3); assert.equal(h.resources.length, 2); assert.equal(h.refusals.length, 1);
  assert.deepEqual(h.intents.map(i => i.am), ['a', 'b', 'a']);
  assert.equal(h.refusals[0].response.timing.serving.pod_uid, 'local-pod-a');
  assert.deepEqual(h.settings, h.original);
  assert.deepEqual(await h.cleanup(), []);
  assert.equal(h.releases, 1);
  assert.equal(h.requests.filter(r => r.method === 'DELETE').length, 2);
  assert.ok([...h.rows.values()].every(r => r.status === 'retired'));
  assert.equal(h.observations.filter(o => o.label === 'admin_quota_native_finality').length, 2);
});

test('setup only reads public state and pins three different intents', async () => {
  const h = model(), f = await h.setup();
  assert.ok(h.requests.every(r => r.method === 'GET'));
  assert.deepEqual(h.requests.filter(r => r.actor === 'user').map(r => r.am), ['a', 'b']);
  assert.equal(new Set(f.intents.map(i => i.key)).size, 3);
  assert.equal(new Set(f.intents.map(i => i.body.name)).size, 3);
  assert.ok(f.intents.every(i => i.body.name.length <= 64 && i.actor === 'user'
    && i.body.sandbox_profile.id === 'sbp-local' && i.body.sandbox_profile.revision === 2));
  await f.cleanup(); await f.cleanup(); assert.equal(h.releases, 1);
});

for (const field of ['database_disposable', 'single_matrix_writer', 'finality']) {
  test('missing parent resource prerequisite blocks before requests: ' + field, async () => {
    const h = model(); delete h.bindings.userResources[field];
    await assert.rejects(h.setup(), /user_resource_custody_required/);
    assert.equal(h.requests.length, 0);
  });
}

test('missing exact owner never borrows admin identity', async () => {
  const h = model(); delete h.config.actors.user;
  await assert.rejects(h.setup(), /admin_quota_exact_roles_required/);
  assert.equal(h.requests.length, 0);
});

test('active background policy and non-writable retained policies are refused', async () => {
  const h = model(); h.settings.inactivity.auto_pause_enabled = true;
  await assert.rejects(h.setup(), /admin_quota_background_policies_active/);
  h.settings.inactivity.auto_pause_enabled = false; h.settings.model_budget.enabled = true;
  await assert.rejects(h.setup(), /admin_settings_spend_gate_closed/);
  assert.ok(h.requests.every(r => r.method === 'GET'));
});

test('an already identical limit blocks instead of claiming a changed settings write', async () => {
  const h = model(); h.settings.limits.max_sandboxes_per_user = 2;
  await assert.rejects(h.setup(), /admin_quota_requires_changed_limit/);
  assert.ok(h.requests.every(r => r.method === 'GET'));
});

test('quota inventory must contain the exact accepted identities on both AMs', async () => {
  const h = model({inventoryMismatch: true});
  await assert.rejects(h.fullCase(), /admin_limit_durable_resource_count/);
  assert.deepEqual(await h.cleanup(), []); assert.equal(h.releases, 1);
});

for (const retired of [false, true]) test('existing inventory is never hidden: retired=' + retired, async () => {
  const h = model(); h.rows.set('existing', {id: 'existing', status: retired ? 'retired' : 'active'});
  await assert.rejects(h.setup(), /admin_quota_owner_not_empty/);
  assert.ok(h.requests.every(r => r.method === 'GET' && !r.path.includes('exclude_retired')));
});

test('an empty first page with another cursor is not an empty owner', async () => {
  const h = model({next: true}); await assert.rejects(h.setup(), /admin_quota_owner_not_empty/);
});

test('incomplete parent refusal contract blocks, but its empty reservation is released', async () => {
  const h = model(); delete h.lease.retainRefusal;
  await assert.rejects(h.setup(), /admin_quota_parent_custody_required/);
  assert.deepEqual(await h.cleanup(), []); assert.equal(h.releases, 1); assert.equal(h.posts, 0);
});

test('failed durable intent never dispatches and retains unresolved parent custody', async () => {
  const h = model({intentFailure: true}), f = await h.setup();
  await assert.rejects(created(h, f), /parent_intent_failed/);
  await assert.rejects(f.cleanup(), e => /admin_quota_create_unresolved/.test(flattened(e)));
  assert.equal(h.posts, 0); assert.equal(h.releases, 0);
});

test('accepted attribution failure is cleaned without replaying creation', async () => {
  const h = model({attributionFailure: true}), f = await h.setup();
  await assert.rejects(created(h, f), /request_attribution_missing/);
  await f.cleanup(); assert.equal(h.posts, 1); assert.equal(h.resources.length, 1); assert.equal(h.releases, 1);
});

for (const kind of ['resourceFailure', 'receiptOwnerMismatch']) {
  test('accepted identity stays in local custody despite receipt failure: ' + kind, async () => {
    const h = model({[kind]: true}), f = await h.setup();
    await assert.rejects(created(h, f));
    await assert.rejects(f.cleanup(), e => /admin_quota_create_unresolved/.test(flattened(e)));
    assert.equal(h.requests.filter(r => r.method === 'DELETE').length, 1);
    assert.equal(h.posts, 1); assert.equal(h.releases, 0);
  });
}

for (const options of [{unknownCreate: true}, {refuse: true, refusalCode: 'different_refusal'},
  {refuse: true, wrongRefusalIngress: true}, {refuse: true, refusalFailure: true}]) {
  test('uncertain creation/refusal never becomes cleanup success: ' + JSON.stringify(options), async () => {
    const h = model(options), f = await h.setup();
    await assert.rejects(created(h, f));
    await assert.rejects(f.cleanup(), e => /admin_quota_create_unresolved/.test(flattened(e)));
    assert.equal(h.posts, 1); assert.equal(h.releases, 0);
    assert.equal(h.requests.filter(r => r.method === 'DELETE').length, 0);
  });
}

test('unknown retirement acknowledgement remains failed and every other root is still attempted once', async () => {
  const h = model({unknownDelete: true}), f = await h.setup();
  await created(h, f, 0); await created(h, f, 1);
  const results = await Promise.allSettled([f.cleanup(), f.cleanup()]);
  assert.ok(results.every(r => r.status === 'rejected' && /retirement_ack_unknown/.test(flattened(r.reason))));
  assert.equal(results[0].reason, results[1].reason);
  await h.cleanup(); assert.equal(h.requests.filter(r => r.method === 'DELETE').length, 2);
  assert.equal(h.releases, 0);
});

for (const field of ['owner', 'name', 'profile', 'id']) test('foreign cleanup identity is protected: ' + field, async () => {
  const h = model(), f = await h.setup(); await created(h, f);
  h.rows.get('sandbox-1')[field] = 'changed';
  await assert.rejects(f.cleanup(), e => /admin_quota_cleanup_identity/.test(flattened(e)));
  assert.equal(h.requests.filter(r => r.method === 'DELETE').length, 0); assert.equal(h.releases, 0);
});

for (const options of [{incompleteFinality: true}, {releaseFailure: true}]) {
  test('real native finality and parent release must both succeed: ' + JSON.stringify(options), async () => {
    const h = model(options), f = await h.setup(); await created(h, f);
    await assert.rejects(f.cleanup()); await assert.rejects(f.cleanup());
    assert.equal(h.requests.filter(r => r.method === 'DELETE').length, 1);
    assert.equal(h.releases, options.releaseFailure ? 1 : 0);
  });
}

test('changed intent, replay and post-cleanup use never dispatch again', async () => {
  const h = model(), f = await h.setup();
  await assert.rejects(ownedIntent(h.ctx, f, {...f.intents[0], body: {}}), /admin_quota_intent_replayed_or_changed/);
  await created(h, f);
  await assert.rejects(created(h, f), /admin_quota_intent_replayed_or_changed/);
  await f.cleanup();
  await assert.rejects(created(h, f, 1), /admin_quota_intent_replayed_or_changed/);
  assert.equal(h.posts, 1);
});

test('duplicate accepted identity retains uncertainty and never deletes it twice', async () => {
  const h = model(), f = await h.setup(); const response = await created(h, f);
  const second = await f.custody(f.intents[1], {am: 'b', actor: 'user'});
  await assert.rejects(second.capture(response), /admin_quota_duplicate_resource/);
  await assert.rejects(second.capture(response), /admin_quota_response_replayed/);
  await assert.rejects(f.cleanup(), e => /admin_quota_create_unresolved/.test(flattened(e)));
  assert.equal(h.requests.filter(r => r.method === 'DELETE').length, 1); assert.equal(h.releases, 0);
});
