import test from 'node:test';
import assert from 'node:assert/strict';
import {retentionCase} from './settings-browser.mjs';
import {retentionModel} from './retention-model.mjs';
const run = h => retentionCase.run(h.ctx);
const messages = e => [e.message, ...(e.errors ?? []).map(messages)].join(' ');

test('retention restoration follows browser drain', async () => {
  const h = retentionModel(); await run(h); await h.cleanup();
  assert.ok(h.actions.filter(a => a.op === 'restore').every(a => a.closed));
});

test('invalid form dispatch is refused even when the server rejects it without changing state', async () => {
  const h = retentionModel({invalidDispatch: true});
  await assert.rejects(run(h), e => messages(e).includes('admin_retention_extra_write'));
  await h.cleanup();
});

test('one exact UI save, typed negative API requests and both ingress reads retain full proof', async () => {
  const h = retentionModel(); await run(h); assert.deepEqual(await h.cleanup(), []);
  assert.deepEqual(h.settings(), h.original);
  const proof = h.observations.filter(o => o.label === 'admin_retention_browser').at(-1).value;
  const writes = proof.requests.filter(r => r.method === 'PUT'); assert.equal(writes.length, 1);
  for (const key of ['serving', 'serving_after', 'started_at', 'ended_at', 'attribution']) assert.ok(writes[0][key]);
  const observed = h.observations.filter(o => o.label === 'admin_retention_browser_writes').at(-1).value;
  assert.equal(observed[0].body.expected_revision, 1);
  assert.equal(observed[0].body.settings.retention.agent_log_days, 30);
  assert.equal(observed[0].response.request_id, writes[0].request_id);
  assert.deepEqual(h.calls.filter(c => c.method === 'PATCH').map(c => c.body.retention.agent_log_days),
    [-1, 1.5, 4294967296, 'not-a-number']);
  assert.deepEqual(new Set(h.calls.filter(c => c.method === 'GET').map(c => c.am)), new Set(['a', 'b']));
  assert.equal(h.actions.filter(a => a.op === 'restore').length, 1);
  assert.equal(h.page.listenerCount('request'), 0); assert.equal(h.page.listenerCount('response'), 0);
});

test('inclusive unsigned boundaries are checked in the DOM but never saved', async () => {
  const h = retentionModel(); await run(h); assert.deepEqual(await h.cleanup(), []);
  assert.deepEqual(h.actions.filter(a => a.op === 'fill').map(a => a.value), ['0', '4294967295', '30', '-1', '1.5', '4294967296', '']);
  assert.deepEqual(h.actions.filter(a => a.op === 'click').map(a => a.valid), [true, false, false, false, false]);
});

for (const originalDays of [0, 30, 4294967295]) test('original retention restores exactly: ' + originalDays, async () => {
  const h = retentionModel({originalDays}); h.ctx.am = 'b';
  await run(h); assert.deepEqual(await h.cleanup(), []); assert.deepEqual(h.settings(), h.original);
  assert.ok(h.receipts.filter(r => r.method === 'PUT').every(r => r.am === 'b' && r.serving_am === 'b'));
  assert.equal(h.observations.find(o => o.label === 'admin_retention_save').value.body.settings.retention.agent_log_days,
    originalDays === 30 ? 31 : 30);
});

for (const [option, code] of [
  ['wrongBody', 'admin_retention_save_body'], ['failedSave', 'admin_retention_save_response'],
  ['stalePeer', 'admin_retention_peer_policy'], ['unjoined', 'admin_retention_ingress'],
  ['wrongServing', 'admin_retention_ingress'], ['bodyFailure', 'body_failed'],
  ['invalidFormValid', 'admin_retention_invalid_form'], ['boundaryInvalid', 'admin_retention_unsigned_boundary'],
  ['erasedOther', 'admin_retention_saved_policy'], ['wrongRefusal', 'admin_retention_invalid_refusal'],
  ['invalidEffect', 'admin_retention_invalid_api_effect'], ['lateWrite', 'admin_retention_extra_write'],
  ['bearer', 'admin_retention_cookie_session'],
]) test('unproven retention fails: ' + option, async () => {
  const h = retentionModel({[option]: true});
  await assert.rejects(run(h), e => messages(e).includes(code)); await h.cleanup();
  assert.ok(h.actions.filter(a => a.op === 'restore').every(a => a.closed));
  assert.equal(h.page.listenerCount('request'), 0); assert.equal(h.page.listenerCount('response'), 0);
});

for (const fieldContract of [{type: 'text'}, {min: '1'}, {max: '4294967296'}, {step: 'any'}, {required: false}]) {
  test('incorrect compiled unsigned field contract fails: ' + JSON.stringify(fieldContract), async () => {
    const h = retentionModel({fieldContract});
    await assert.rejects(run(h), /admin_retention_number_contract/); assert.deepEqual(await h.cleanup(), []);
    assert.equal(h.actions.filter(a => a.op === 'click').length, 0);
  });
}

for (const originalDays of [-1, 1.5, 4294967296, '90']) test('invalid original retention blocks: ' + originalDays, async () => {
  const h = retentionModel({originalDays}); await assert.rejects(run(h), /admin_retention_original_unsigned/);
  assert.deepEqual(await h.cleanup(), []); assert.equal(h.actions.filter(a => a.op === 'click').length, 0);
});

for (const status of [200, 202]) test('source-defined report prefetch remains separate from settings writes: ' + status, async () => {
  const h = retentionModel({prefetch: {status, body: {report_id: 'background-report', status: status === 200 ? 'ready' : 'pending'}}});
  await run(h); assert.deepEqual(await h.cleanup(), []);
  const rows = h.observations.filter(o => o.label === 'admin_retention_browser_writes').at(-1).value;
  assert.equal(rows.length, 2); assert.equal(rows[0].path, '/api/v1/admin/usage/metrics/reports');
  assert.equal(rows[1].path, '/api/v1/admin/settings'); assert.equal(rows[0].response.status, status);
});

for (const prefetch of [
  {status: 503}, {status: 200, body: {report_id: 'x', status: 'pending'}},
  {body: {report_id: '', status: 'pending'}}, {body: {report_id: 'x', status: 'failed'}},
]) test('invalid report admission does not pass: ' + JSON.stringify(prefetch), async () => {
  const h = retentionModel({prefetch});
  await assert.rejects(run(h), e => messages(e).includes('admin_retention_report_prefetch')); await h.cleanup();
  assert.equal(h.actions.filter(a => a.op === 'click').length, 0);
});

test('a different browser mutation cannot borrow report-prefetch permission', async () => {
  const h = retentionModel(), goto = h.page.goto;
  h.page.goto = async () => {await goto(); h.emit('/api/v1/admin/sandboxes', 'POST', {}, 200, {});};
  await assert.rejects(run(h), e => messages(e).includes('admin_retention_unexpected_mutation')); await h.cleanup();
});

test('unresolved browser mutation prevents pass after drain and still captures adapter evidence', async () => {
  const h = retentionModel(), goto = h.page.goto;
  h.page.goto = async () => {await goto(); h.page.emit('request', {method: () => 'POST',
    url: () => 'http://127.0.0.1:19900/api/v1/admin/usage/metrics/reports', postDataJSON: () => ({})});};
  await assert.rejects(run(h), e => messages(e).includes('admin_retention_mutation_unresolved')); await h.cleanup();
  assert.ok(h.observations.some(o => o.label === 'admin_retention_browser'));
});

test('in-flight report prefetch may finish during close but must then have an exact receipt', async () => {
  const h = retentionModel(), goto = h.page.goto, adapter = h.ctx.fixture('browser'), open = adapter.open;
  const request = {method: () => 'POST', url: () => 'http://127.0.0.1:19900/api/v1/admin/usage/metrics/reports',
    postDataJSON: () => ({from: 1, to: 2})};
  h.page.goto = async () => {await goto(); h.page.emit('request', request);};
  adapter.open = async () => {
    const session = await open(); let closed = false;
    return {...session, close: async () => {
      if (closed) return; closed = true;
      await session.close();
      h.receipts.push({method: 'POST', status: 202, request_id: 'late-report', am: 'a', serving_am: 'a'});
      h.page.emit('response', {request: () => request, status: () => 202, headerValue: async () => 'late-report',
        json: async () => ({report_id: 'report-created-before-close', status: 'pending'})});
    }};
  };
  await run(h); assert.deepEqual(await h.cleanup(), []);
  assert.deepEqual(h.settings(), h.original);
  assert.equal(h.observations.filter(o => o.label === 'admin_retention_browser_writes').at(-1).value[0].response.request_id,
    'late-report');
});

test('an uncaptured adapter mutation cannot escape proof validation', async () => {
  const h = retentionModel(), goto = h.page.goto;
  h.page.goto = async () => {await goto(); h.receipts.push({method: 'PUT', request_id: 'uncaptured'});};
  await assert.rejects(run(h), e => messages(e).includes('admin_retention_uncaptured_mutation')); await h.cleanup();
});

test('duplicate ingress receipts are rejected', async () => {
  const h = retentionModel({prefetch: true}), goto = h.page.goto;
  h.page.goto = async () => {await goto(); h.receipts.push(structuredClone(h.receipts.at(-1)));};
  await assert.rejects(run(h), e => messages(e).includes('admin_retention_ingress')); await h.cleanup();
});

test('erroneously accepted invalid PATCH fails while retaining restoration custody', async () => {
  const h = retentionModel({invalidAccepted: true}); await assert.rejects(run(h), /status_200/);
  assert.deepEqual(await h.cleanup(), []); assert.deepEqual(h.settings(), h.original);
  assert.equal(h.calls.filter(c => c.method === 'PATCH').length, 1);
  assert.equal(h.actions.filter(a => a.op === 'restore').length, 1);
});

test('unknown UI save is not replayed and owned readback restores once after drain', async () => {
  const h = retentionModel({unknownSave: true}); await assert.rejects(run(h), /save_unknown/);
  assert.deepEqual(await h.cleanup(), []); assert.deepEqual(h.settings(), h.original);
  assert.equal(h.receipts.filter(r => r.method === 'PUT').length, 1);
  assert.equal(h.actions.filter(a => a.op === 'restore').length, 1);
  assert.ok(h.actions.filter(a => a.op === 'restore').every(a => a.closed));
});

test('unknown restoration is memoized and never replayed by deferred hooks', async () => {
  const h = retentionModel({restoreFailure: true});
  await assert.rejects(run(h), e => messages(e).includes('restore_unknown'));
  assert.ok((await h.cleanup()).some(e => messages(e).includes('restore_unknown')));
  assert.equal(h.actions.filter(a => a.op === 'restore').length, 1);
});

test('close, evidence and restore failures remain independent without step-only cleanup assertions', async () => {
  const h = retentionModel({closeFailure: true, evidenceFailure: true, restoreFailure: true});
  await assert.rejects(run(h), e => messages(e).includes('close_failed'));
  const errors = await h.cleanup();
  for (const code of ['close_failed', 'evidence_failed', 'restore_unknown']) assert.ok(errors.some(e => messages(e).includes(code)), code);
  assert.ok(!errors.some(e => messages(e).includes('assertion_outside_step')));
  assert.equal(h.actions.filter(a => a.op === 'close').length, 1);
  assert.equal(h.actions.filter(a => a.op === 'restore').length, 1);
  assert.equal(h.page.listenerCount('request'), 0); assert.equal(h.page.listenerCount('response'), 0);
});
