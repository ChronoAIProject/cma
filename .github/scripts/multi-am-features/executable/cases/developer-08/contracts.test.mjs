import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {cases, setups} from './index.mjs';
import {prepareDocuments} from './setups.mjs';
import {hash, loseAcknowledgment} from './helpers.mjs';
import {executeCase, Blocked} from '../../runner.mjs';
import {loadRegistry} from '../../registry.mjs';

const documentId = 'doc_' + 'a'.repeat(32);
function fixtureHarness() {
  const events = [], cleanups = [];
  const identities = {user: {actor: 'user', subject: 'subject-user'},
    recipient: {actor: 'recipient', subject: 'subject-recipient'}, outsider: {actor: 'outsider', subject: 'subject-outsider'}};
  const lease = {parent_cleanup_barrier: true,
    retainIntent: async intent => events.push({type: 'intent', intent}),
    retainDocument: async id => events.push({type: 'document', id}),
    release: async () => {events.push({type: 'release'}); return {registered: true, parent_cleanup_barrier: true};}};
  const dependencies = {identities, disposableDatabase: {scope: 'LOCAL_DISPOSABLE_TWO_AM', database_disposable: true,
    reserve: async () => {events.push({type: 'reserve'}); return lease;}}};
  let serial = 0;
  const ctx = {am: 'a', row: {case_id: cases[0].id},
    defer: (label, run) => {events.push({type: 'defer', label}); cleanups.push(run);},
    observe: async (_label, run) => run(),
    request: async request => {
      events.push({type: 'request', request});
      if (request.method === 'POST' && request.path === '/api/v1/documents') return {status: 201,
        body: {id: 'doc_' + (++serial).toString(16).padStart(32, '0'), owner_subject: identities[request.actor].subject,
          access: 'owner', revision: 1, source_revision: 1, title: request.body.title, sha256: hash(request.body.markdown)}};
      throw Error('unexpected_fixture_request');
    }};
  return {ctx, dependencies, lease, events, cleanups};
}

test('all eight assigned IDs preserve every catalog step; missing fixture never executes', async () => {
  const registry = await loadRegistry();
  const assignment = JSON.parse(readFileSync(new URL('../../assignments/developer-08.json', import.meta.url)));
  assert.deepEqual(cases.map(value => value.id).sort(), [...assignment.case_ids].sort());
  for (const definition of cases) {
    const row = registry.rows.get(definition.id);
    assert.equal(registry.owners.get(definition.id), 'developer-08');
    assert.deepEqual(definition.steps, row.required_steps);
    const result = await executeCase(definition, row, {phase: 'before_restart', iteration: 1, am: 'a',
      image: 'local/api@sha256:' + 'a'.repeat(64), source: 'b'.repeat(40), fixtures: {}});
    assert.equal(result.status, 'BLOCKED'); assert.equal(result.requests.length, 0);
  }
});

test('setup refuses non-disposable or duplicate-subject fixtures before mutations', async () => {
  for (const mutate of [d => delete d.disposableDatabase, d => d.disposableDatabase.database_disposable = false,
    d => d.disposableDatabase.scope = 'PRODUCTION', d => d.identities.recipient = {...d.identities.user},
    d => delete d.identities.outsider]) {
    const f = fixtureHarness(); mutate(f.dependencies);
    await assert.rejects(prepareDocuments(f.ctx, f.dependencies), Blocked);
    assert.equal(f.events.filter(e => e.type === 'request').length, 0);
  }
});

test('cleanup barrier is mandatory before create and malformed custody is refused', async () => {
  const f = fixtureHarness(); f.lease.parent_cleanup_barrier = false;
  await assert.rejects(prepareDocuments(f.ctx, f.dependencies), /parent_cleanup_barrier/);
  assert.equal(f.events.filter(e => e.type === 'request').length, 0);
});

test('original intent is retained before create; accepted attribution failure retains identity without replay', async () => {
  const f = fixtureHarness(); let calls = 0;
  f.ctx.request = async request => {
    calls++; assert.equal(f.events.at(-1).type, 'intent');
    assert.equal(f.events.at(-1).intent.key, request.key);
    const error = Error('request_attribution_missing'); error.response = {status: 201, body: {id: documentId}}; throw error;
  };
  await assert.rejects(prepareDocuments(f.ctx, f.dependencies), /attribution_missing/);
  assert.equal(calls, 1); assert.ok(f.events.some(e => e.type === 'document' && e.id === documentId));
  assert.equal(f.cleanups.length, 1); await f.cleanups[0]();
  assert.equal(f.events.at(-1).type, 'release');
});

test('unacknowledged create does not invent receipt, retry or lose cleanup ownership', async () => {
  const f = fixtureHarness(); let calls = 0;
  f.ctx.request = async () => {calls++; throw Error('transport_lost');};
  await assert.rejects(prepareDocuments(f.ctx, f.dependencies), /transport_lost/);
  assert.equal(calls, 1); assert.equal(f.events.filter(e => e.type === 'intent').length, 1);
  assert.equal(f.events.filter(e => e.type === 'document').length, 0);
  await f.cleanups[0](); assert.equal(f.events.at(-1).type, 'release');
});

test('setup creates authentic API inputs and cleanup failure stays a failure', async () => {
  const f = fixtureHarness(), value = await prepareDocuments(f.ctx, f.dependencies);
  assert.equal(value.documents.length, 2); assert.equal(value.cleanupRegistered, true);
  const writes = f.events.filter(e => e.type === 'request').map(e => e.request);
  assert.ok(writes.every(r => r.method === 'POST' && r.path === '/api/v1/documents' && r.allowed[0] === 201));
  assert.equal(new Set(writes.map(r => r.key)).size, 2);
  f.lease.release = async () => ({registered: false});
  await assert.rejects(value.cleanup(), /cleanup_custody_unconfirmed/);
  f.lease.release = async () => ({registered: true, parent_cleanup_barrier: true});
  await value.cleanup();
});

function faultHarness() {
  const cleanups = [], observations = [], operations = []; let handler;
  const page = {route: async (_pattern, fn) => {handler = fn;}, unroute: async () => operations.push('unroute')};
  const ctx = {defer: (_label, fn) => cleanups.push(fn), assert: (ok, code) => assert.ok(ok, code),
    observe: async (_label, run) => {const value = await run(); observations.push(value); return value;}};
  const request = (key, body) => ({method: () => 'POST', url: () => 'http://127.0.0.1/api/v1/documents',
    headerValue: async () => key, postData: () => body});
  const attempt = async (key, body, status = true) => handler({request: () => request(key, body),
    fetch: async options => {operations.push(['real-fetch', options]); return {ok: () => status,
      json: async () => ({id: documentId}), dispose: async () => operations.push('dispose')};},
    abort: async () => operations.push('abort'), continue: async () => operations.push('continue'),
    fallback: async () => operations.push('fallback')});
  return {ctx, page, attempt, observations, operations, cleanups};
}

test('lost-ack helper forwards one real first request then UI retry, comparing body and key', async () => {
  const f = faultHarness(), accepted = [];
  const fault = await loseAcknowledgment(f.ctx, {page: f.page}, 'POST', '/api/v1/documents', async value => accepted.push(value));
  await f.attempt('original-key', '{"title":"original"}'); await f.attempt('original-key', '{"title":"original"}');
  assert.equal((await fault.verify()).id, documentId); assert.equal(accepted.length, 1);
  assert.equal(f.operations.filter(e => Array.isArray(e) && e[0] === 'real-fetch').length, 1);
  assert.equal(f.operations.filter(e => e === 'continue').length, 1);
  assert.ok(!JSON.stringify(f.observations).includes('original-key'));
  await f.cleanups[0]();
});

test('unknown intent rekey or body substitution cannot pass the retry check', async () => {
  for (const [key, body] of [['changed-key', '{"title":"original"}'], ['original-key', '{"title":"changed"}']]) {
    const f = faultHarness(); const fault = await loseAcknowledgment(f.ctx, {page: f.page}, 'POST', '/api/v1/documents');
    await f.attempt('original-key', '{"title":"original"}'); await f.attempt(key, body);
    await assert.rejects(fault.verify(), /unknown_intent_changed/); await f.cleanups[0]();
  }
});

test('failed upstream cannot be relabeled as deliberately lost successful receipt', async () => {
  const f = faultHarness(), accepted = [];
  const fault = await loseAcknowledgment(f.ctx, {page: f.page}, 'POST', '/api/v1/documents', value => accepted.push(value));
  await f.attempt('key', '{}', false);
  await assert.rejects(fault.verify(), /requires_actual_acceptance/);
  await assert.rejects(f.cleanups[0](), /requires_actual_acceptance/); assert.equal(accepted.length, 0);
});

test('all named setup exports have exact parent primitive requirements', () => {
  const contract = JSON.parse(readFileSync(new URL('./fixtures.json', import.meta.url)));
  assert.deepEqual(Object.keys(setups).sort(), [...contract['user-fixtures'].names].sort());
  assert.equal(contract['user-fixtures'].primitives.disposableDatabase.lease.parent_cleanup_barrier, true);
});
