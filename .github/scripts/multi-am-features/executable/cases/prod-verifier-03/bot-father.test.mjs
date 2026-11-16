import test from 'node:test';
import assert from 'node:assert/strict';
import {armFatherPublication, assertFatherReceipt, assertFatherConfiguration, fatherProfileBody, fatherConsumerBody} from './bot-father.mjs';
import {digest} from './helpers.mjs';

const draft = {v: 1, name: 'Reviewed synthetic bot', instructions: 'Reviewed synthetic instructions.'};
const owner = 'synthetic-owner', ctx = {assert: (condition, code) => assert.ok(condition, code)};
const created = {id: 'observed-profile', owner, visibility: 'private', draft_version: 2, name: draft.name,
  draft: fatherProfileBody(draft).content};
const published = {profile_id: created.id, revision: 1, digest: 'a'.repeat(64),
  published_at: '2026-11-16T10:00:00.000Z', content: fatherProfileBody(draft).content};

async function interception(options = {}) {
  const calls = [], deferred = [], controller = new AbortController(); let handler;
  const saved = {version: 1, owner: options.wrongOwner ? 'other-owner' : owner, draft,
    createKey: 'synthetic-create-key', publishKey: 'synthetic-publish-key'};
  const page = {url: () => 'http://127.0.0.1:1234/bots/new', async route(matches, dispatch) {
    assert.ok(matches(new URL('http://127.0.0.1:1234/api/v1/agent-profiles')));
    assert.ok(matches(new URL('http://127.0.0.1:1234/api/agent-manager/v1/agent-profiles/observed-profile/revisions')));
    assert.equal(matches(new URL('http://127.0.0.1:4321/api/v1/agent-profiles')), false);
    assert.equal(matches(new URL('http://127.0.0.1:1234/api/v1/agent-profiles/other/retire')), false);
    handler = dispatch;
  }, async unroute() {calls.push('unroute'); if (options.unrouteFailure) throw Error('unroute_failed');}, async evaluate(_read, subject) {
    assert.equal(subject, owner); calls.push('retained'); return {profile: structuredClone(saved), consumer: null};
  }};
  const lease = {reviewed: draft, expected: {ownerSubject: owner}, async retainProfileIntent(intent) {
    calls.push('custody:' + intent.kind);
    assert.equal(intent.key, intent.kind === 'create' ? saved.createKey : saved.publishKey);
    if (options.custodyFailure) throw Error('profile_custody_failed');
    if (options.cancelBeforeForward) controller.abort(Error('case_budget_exhausted'));
    return {retained: true, key_sha256: digest(intent.key), body_sha256: digest(intent.body)};
  }};
  const operation = await armFatherPublication({...ctx, signal: controller.signal, remainingMs: () => 500,
    defer: (_label, work) => deferred.push(work)}, page, lease);
  const dispatch = (kind, method = 'POST') => {
    const path = kind === 'create' ? '/api/v1/agent-profiles' : '/api/agent-manager/v1/agent-profiles/observed-profile/revisions';
    const requestBody = kind === 'create' ? fatherProfileBody(draft) : {draft_version: created.draft_version};
    if (options.wrongBody) requestBody.name = 'changed';
    return handler({request: () => ({method: () => method, url: () => 'http://127.0.0.1:1234' + path,
      postDataJSON: () => requestBody, allHeaders: async () => ({'idempotency-key': options.wrongKey ? 'unretained'
        : kind === 'create' ? saved.createKey : saved.publishKey})}),
    async fallback() {calls.push('fallback');}, async fetch(config) {
      assert.deepEqual(config, {maxRetries: 0, maxRedirects: 0, timeout: 500});
      calls.push('fetch:' + kind); if (options.waitFetch) await options.waitFetch;
      if (options.fetchFailure) throw Error('profile_transport_unknown');
      const body = structuredClone(kind === 'create' ? created : published);
      if (options.wrongReturnedOwner && kind === 'create') body.owner = 'other-owner';
      return {status: () => 201, json: async () => body, headers: () => ({'x-request-id': 'actual-' + kind}),
        async dispose() {calls.push('dispose:' + kind);}};
    }, async fulfill({response}) {
      calls.push('fulfill:' + kind); const body = await response.json();
      if (kind === 'create') saved.document = {id: body.id, draftVersion: body.draft_version};
      else saved.published = {revision: body.revision, digest: body.digest, at: body.published_at};
    }, async abort() {calls.push('abort'); if (options.abortFailure) throw Error('route_abort_failed');}});
  };
  return {calls, saved, dispatch, controller, operation, cleanup: () => deferred[0]()};
}

test('Bot Father retains separate create/publish keys before forwarding the exact reviewed profile once', async () => {
  const f = await interception(); await f.dispatch('create'); await f.dispatch('publish');
  const result = await f.operation.result();
  assert.deepEqual(result.created.body, created); assert.deepEqual(result.published.body, published);
  assertFatherReceipt(ctx, f.saved, owner, draft, created, published);
  assert.deepEqual(f.operation.attempts(), {create: 1, publish: 1});
  assert.deepEqual(f.calls, ['retained', 'custody:create', 'fetch:create', 'fulfill:create', 'dispose:create',
    'retained', 'custody:publish', 'fetch:publish', 'fulfill:publish', 'dispose:publish']);
  await f.cleanup();
});

test('Bot Father refuses changed identity, configuration, key or missing custody before any mutation', async () => {
  for (const options of [{wrongOwner: true}, {wrongBody: true}, {wrongKey: true}, {custodyFailure: true}, {cancelBeforeForward: true}]) {
    const f = await interception(options); await f.dispatch('create'); await assert.rejects(f.operation.result());
    assert.ok(!f.calls.some(call => call.startsWith('fetch:')));
    await assert.rejects(f.cleanup());
  }
});

test('Bot Father refuses premature publication, duplicate creation and a returned foreign profile', async () => {
  const early = await interception(); await early.dispatch('publish'); await assert.rejects(early.operation.result());
  assert.ok(!early.calls.includes('fetch:publish')); await assert.rejects(early.cleanup());
  const duplicate = await interception(); await duplicate.dispatch('create'); await duplicate.dispatch('create');
  await assert.rejects(duplicate.operation.result(), /father_duplicate_create/);
  assert.equal(duplicate.calls.filter(call => call === 'fetch:create').length, 1); await assert.rejects(duplicate.cleanup());
  const foreign = await interception({wrongReturnedOwner: true}); await foreign.dispatch('create');
  await assert.rejects(foreign.operation.result(), /father_private_owned_profile/);
  assert.ok(!foreign.calls.includes('fulfill:create')); await assert.rejects(foreign.cleanup());
});

test('Bot Father preserves unknown transport and simultaneous fault failures without retrying', async () => {
  const f = await interception({fetchFailure: true, abortFailure: true}); await f.dispatch('create');
  await assert.rejects(f.operation.result(), error => {
    assert.deepEqual(error.errors.map(item => item.message), ['profile_transport_unknown', 'route_abort_failed']); return true;
  });
  assert.equal(f.calls.filter(call => call === 'fetch:create').length, 1); await assert.rejects(f.cleanup());
});

test('Bot Father cancellation settles a no-submit case and cleanup joins an in-flight profile POST', async () => {
  const idle = await interception(); idle.controller.abort(Error('case_cancelled'));
  await assert.rejects(idle.operation.result(), /case_cancelled/); await idle.cleanup();
  let finish; const waitFetch = new Promise(resolve => {finish = resolve;});
  const f = await interception({waitFetch}), work = f.dispatch('create');
  while (!f.calls.includes('fetch:create')) await new Promise(resolve => setImmediate(resolve));
  let closed = false; const closing = f.cleanup().then(() => {closed = true;});
  await new Promise(resolve => setImmediate(resolve)); assert.equal(closed, false);
  finish(); await work; await closing; assert.ok(closed);
});

test('Bot Father identity/configuration receipts reject cross-owner or changed revision reuse', () => {
  const receipt = {version: 1, owner, draft, createKey: 'create-key', publishKey: 'publish-key',
    document: {id: created.id, draftVersion: created.draft_version},
    published: {revision: published.revision, digest: published.digest, at: published.published_at}};
  assertFatherReceipt(ctx, receipt, owner, draft, created, published);
  for (const change of [v => {v.owner = 'other-owner';}, v => {v.draft.instructions = 'changed';},
    v => {v.publishKey = v.createKey;}, v => {v.document.id = 'other-profile';}, v => {v.published.revision++;}]) {
    const value = structuredClone(receipt); change(value);
    assert.throws(() => assertFatherReceipt(ctx, value, owner, draft, created, published), /father_/);
  }
  const body = fatherConsumerBody(draft, 'synthetic-connection');
  assert.equal(body.agent.system_prompt, draft.instructions);
  assert.equal(body.agent.reasoning_effort, 'medium'); assert.equal(body.agent.permission_mode, 'guarded');
  assert.equal(body.connection_ref, 'synthetic-connection');
  assert.equal('profile_digest' in body, false); assert.equal('profile' in body, false);
});

test('Bot Father distinguishes an unset profile model from the Consumer deployment default', () => {
  const content = fatherProfileBody(draft).content;
  assertFatherConfiguration(ctx, content, draft);
  assertFatherConfiguration(ctx, {...content, model: 'qualified-default'}, draft, 'qualified-default');
  assert.throws(() => assertFatherConfiguration(ctx, content, draft, 'qualified-default'), /exact_published_configuration/);
  assert.throws(() => assertFatherConfiguration(ctx, {...content, model: 'other-default'}, draft, 'qualified-default'),
    /exact_published_configuration/);
});

test('Bot Father cleanup joins the forwarded mutation even when route removal fails', async () => {
  let finish; const waitFetch = new Promise(resolve => {finish = resolve;});
  const f = await interception({waitFetch, unrouteFailure: true}), work = f.dispatch('create');
  while (!f.calls.includes('fetch:create')) await new Promise(resolve => setImmediate(resolve));
  let finished = false;
  const cleanup = f.cleanup().catch(error => {finished = true; return error;});
  await new Promise(resolve => setImmediate(resolve)); assert.equal(finished, false);
  finish(); await work;
  const error = await cleanup;
  assert.deepEqual(error.errors.map(item => item.message), ['unroute_failed']);
  assert.ok(f.calls.includes('fulfill:create')); assert.ok(f.calls.includes('dispose:create'));
});
