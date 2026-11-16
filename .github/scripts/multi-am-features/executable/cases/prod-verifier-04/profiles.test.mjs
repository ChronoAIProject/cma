import test from 'node:test';
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {executeCase} from '../../runner.mjs';
import {profileCases} from './profiles.mjs';
import {rows, equal} from './common.mjs';

// A deliberately small transport model for testing the verifier, not CMA.
// Product verdicts require the actual two-manager run.
function transportModel(fault) {
  const profiles = new Map(), workers = new Map(), receipts = new Map(), calls = [];
  let sequence = 0, ingress;
  const copy = structuredClone;
  const result = (status, body) => ({status, body: copy(body), timing: {am: ingress, request_id: 'test-' + sequence++, body_ms: 1}});
  const deny = code => result(404, {code});
  const transport = {async request(r) {
    ingress = r.am;
    calls.push(copy(r));
    if (fault === 'lost_create' && r.path === '/api/v1/agent-profiles' && r.method === 'POST') throw Error('transport_unknown');
    const url = new URL(r.path, 'http://localhost');
    const parts = url.pathname.split('/').filter(Boolean).slice(2);
    const [collection, id, action, revision] = parts;
    if (r.actor === 'profile-denied' && !(collection === 'marketplace' && r.method === 'GET')) {
      return result(403, {code: collection === 'event-worker-profiles' ? 'event_worker_profile_forbidden' : 'forbidden'});
    }
    const receiptKey = r.actor + ':' + r.key;
    const fingerprint = JSON.stringify([r.method, r.path, r.body]);
    if (r.key && receipts.has(receiptKey)) {
      const receipt = receipts.get(receiptKey);
      if (receipt.fingerprint !== fingerprint) return result(409, {code: 'idempotency_key_reused'});
      return result(receipt.response.status, receipt.response.body);
    }
    const respond = (status, body) => {
      const response = result(status, body);
      if (r.key && status < 300) receipts.set(receiptKey, {fingerprint, response: copy(response)});
      return response;
    };
    if (collection === 'profile-authoring') {
      if (id === 'bounds') return result(200, {min_cpu_millis: 100, min_memory_mib: 128});
      if (id === 'services') return result(200, []);
      return result(200, {...r.body, default_branch: 'main', commit_sha: 'e'.repeat(40)});
    }
    if (collection === 'marketplace') {
      const candidates = [...profiles.values()].filter(p => p.kind + '-profiles' === id && p.doc.visibility === 'public' && !p.doc.retired_at);
      if (!action) return result(200, {items: candidates.map(p => ({id: p.doc.id})), next: null});
      const p = candidates.find(p => p.doc.id === action);
      if (!p) return deny('profile_not_found');
      if (revision === 'fork') {
        const doc = {...copy(p.doc), id: 'profile-' + sequence++, visibility: 'private', draft_version: 1,
          draft: copy(p.revisions[r.body.revision - 1].content), forked_from: {id: p.doc.id, revision: r.body.revision}};
        profiles.set(doc.id, {kind: p.kind, doc, revisions: []});
        return respond(201, doc);
      }
      return result(200, {summary: {id: p.doc.id}, revision: p.revisions.at(-1)});
    }
    if (collection === 'event-worker-profiles') {
      if (!id && r.method === 'GET') return result(200, {items: [...workers.values()].map(p => p.doc), next_after: null});
      if (!id) {
        const doc = {id: 'ewp-' + sequence++, owner: r.actor, source: copy(r.body), revision: 1, retired_at: null};
        workers.set(doc.id, {doc, revisions: [copy(doc)]});
        return respond(201, doc);
      }
      const p = workers.get(id);
      if (r.actor !== p.doc.owner) return deny('event_worker_profile_not_found');
      if (r.method === 'GET') return result(200, action ? p.revisions[Number(revision) - 1] : p.doc);
      if (r.method === 'PUT') {
        if (r.body.expected_revision !== p.doc.revision) return result(409, {code: 'event_worker_profile_conflict'});
        p.doc = {...p.doc, source: copy(r.body.source), revision: p.doc.revision + 1};
      } else p.doc = {...p.doc, revision: p.doc.revision + 1, retired_at: '2026-11-16T00:00:00Z'};
      p.revisions.push(copy(p.doc));
      return result(200, p.doc);
    }
    const kind = collection.replace('-profiles', '');
    if (!id && r.method === 'GET') return result(200, {items: [...profiles.values()]
      .filter(p => p.kind === kind && (p.doc.owner === r.actor || fault === 'list_leak')).map(p => p.doc), next: null});
    if (!id) {
      const doc = {id: 'profile-' + sequence++, owner: r.actor, draft: copy(r.body.content), name: r.body.name,
        description: r.body.description, visibility: 'private', draft_version: 1};
      profiles.set(doc.id, {kind, doc, revisions: []});
      return respond(201, fault === 'wrong_create_content' ? {...doc, draft: {}} : doc);
    }
    const p = profiles.get(id);
    if (r.actor !== p.doc.owner) return deny('profile_not_found');
    if (action === 'revisions') {
      if (r.method === 'POST') {
        const published = {profile_id: id, revision: p.revisions.length + 1, content: copy(p.doc.draft),
          digest: createHash('sha256').update(JSON.stringify(p.doc.draft)).digest('hex')};
        p.revisions.push(published);
        return respond(201, published);
      }
      if (revision) {
        const value = copy(p.revisions[Number(revision) - 1]);
        if (fault === 'mutable_revision' && p.doc.draft_version > 1) value.content = copy(p.doc.draft);
        return result(200, value);
      }
      return result(200, {items: p.revisions, next: null});
    }
    if (action === 'retire') {
      if (fault === 'cleanup_failure') return result(503, {code: 'unavailable'});
      p.doc.retired_at = '2026-11-16T00:00:00Z';
    } else if (action === 'visibility') p.doc.visibility = r.body.visibility;
    else if (r.method === 'PUT') {
      if (r.body.draft_version !== p.doc.draft_version) return result(409, {code: 'profile_version_conflict'});
      p.doc = {...p.doc, name: r.body.name, draft: copy(r.body.content), draft_version: p.doc.draft_version + 1};
    }
    return respond(200, p.doc);
  }};
  return {transport, profiles, workers, calls};
}

async function run(definition, fault) {
  const model = transportModel(fault);
  const result = await executeCase(definition, rows.find(r => r.case_id === definition.id), {
    phase: 'before_restart', iteration: 1, am: 'a', image: 'local@sha256:' + 'a'.repeat(64), source: 'b'.repeat(40),
    transport: model.transport, fixtures: {'api-fixtures': {async take(name) {
      assert.equal(name, 'profile-authoring');
      return {repositoryRequest: {url: 'https://github.com/example/synthetic'}, repositoryCommit: 'e'.repeat(40), services: [], cleanup() {}};
    }}},
  });
  return {...model, result};
}

test('post-restart invocation validates retained original receipts and revisions', async () => {
  const model = transportModel();
  const definition = profileCases[0], runState = {};
  const environment = {iteration:1,am:'a',image:'local@sha256:'+'a'.repeat(64),source:'b'.repeat(40),
    transport:model.transport,runState};
  const before = await executeCase(definition,rows.find(r=>r.case_id===definition.id),{...environment,phase:'before_restart'});
  assert.equal(before.status,'PASS');
  const retainedIds = runState.profileHistory.map(value=>value.id);
  const start = model.calls.length;
  const after = await executeCase(definition,rows.find(r=>r.case_id===definition.id),{...environment,phase:'after_restart'});
  assert.equal(after.status,'PASS',after.error_code);
  assert.ok(retainedIds.every(id=>model.calls.slice(start).some(call=>call.method==='GET' && call.path.includes('/'+id+'/revisions/'))));
  model.profiles.get(retainedIds[0]).revisions[0].content.system_prompt = 'corrupted-after-restart';
  const corrupt = await executeCase(definition,rows.find(r=>r.case_id===definition.id),{...environment,phase:'after_restart'});
  assert.equal(corrupt.primary_failure,'original_profile_revision_survives_restart');
});

test('38 profile cases assert their exact operation and retire every acquired resource', async () => {
  assert.equal(profileCases.length, 38);
  for (const definition of profileCases) {
    const {result, profiles, workers, calls} = await run(definition);
    assert.equal(result.status, 'PASS', definition.id + ': ' + result.error_code);
    assert.equal(result.steps.length, 4);
    assert.ok(result.steps.every(step => step.assertions > 0));
    assert.ok([...profiles.values(), ...workers.values()].every(p => p.doc.retired_at));
    const row = rows.find(r => r.case_id === definition.id);
    const [method, template] = row.entry_point.split(' ');
    const pattern = new RegExp('^' + template.replaceAll(/\{[^}]+\}/g, '[^/?]+') + '(?:\\?|$)');
    assert.ok(calls.some(call => call.method === method && pattern.test(call.path)), definition.id);
    assert.ok(calls.some(call => call.am === 'b'));
  }
});

for (const [fault, code] of [['wrong_create_content', 'profile_create_content'],
  ['list_leak', 'profile_owner_list_isolation'], ['mutable_revision', 'published_revision_immutable_across_am']]) {
  test('verifier rejects ' + fault + ' and still performs exact cleanup', async () => {
    const {result, profiles} = await run(profileCases[0], fault);
    assert.equal(result.status, 'FAIL');
    assert.equal(result.primary_failure, code);
    assert.ok([...profiles.values()].every(p => p.doc.retired_at));
  });
}
test('an uncertain create is not retried or silently forgotten by cleanup', async () => {
  const {result, calls} = await run(profileCases[0], 'lost_create');
  assert.equal(result.primary_failure, 'transport_unknown');
  assert.ok(result.cleanup.some(value => value.error_code === 'profile_create_custody_unknown'));
  assert.equal(calls.filter(r => r.method === 'POST' && r.path === '/api/v1/agent-profiles').length, 1);
});
test('retirement failure preserves primary and cleanup failures', async () => {
  const {result} = await run(profileCases[0], 'cleanup_failure');
  assert.equal(result.status, 'FAIL');
  assert.equal(result.primary_failure, 'request_status_503');
  assert.ok(result.cleanup.some(value => value.status === 'FAIL'));
});
test('fixture-requiring cases are blocked before making application requests', async () => {
  const definition = profileCases.find(c => c.id.endsWith('047'));
  const result = await executeCase(definition, rows.find(r => r.case_id === definition.id), {
    phase: 'before_restart', iteration: 1, am: 'a', image: 'local@sha256:' + 'a'.repeat(64), source: 'b'.repeat(40),
  });
  assert.equal(result.status, 'BLOCKED');
  assert.deepEqual(result.requests, []);
});
