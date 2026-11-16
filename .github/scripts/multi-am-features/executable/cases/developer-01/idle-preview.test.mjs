import test from 'node:test';
import assert from 'node:assert/strict';
import {idlePreview, lifecycleSnapshot} from './idle-preview.mjs';
import {previewModel} from './idle-preview-model.mjs';

const messages = error => [error.message, ...(error.errors ?? []).map(messages)].join(' ');

for (const eventCount of [2, 200, 401]) {
  test('preview reads complete lifecycle/event evidence and drains browser before final snapshot: ' + eventCount, async () => {
    const h = previewModel({eventCount}); await idlePreview.run(h.ctx);
    assert.deepEqual(await h.cleanup(), []);
    assert.equal(h.browserCalls.filter(c => c.operation === 'click').length, 1);
    assert.ok(h.browserCalls.some(c => c.operation === 'evidence' && c.closed));
    const before = h.observations.find(o => o.label === 'admin_idle_lifecycle_before').value;
    const after = h.observations.find(o => o.label === 'admin_idle_lifecycle_after').value;
    assert.equal(before[0].events.length, eventCount); assert.deepEqual(before, after);
    assert.equal(before[0].lifecycle.next_authority, 1); assert.equal(before[0].work.step, 'ready');
    assert.ok(h.calls.every(c => c.method === 'GET'));
    assert.deepEqual(new Set(h.calls.map(c => c.am)), new Set(['a', 'b']));
  });
}

for (const option of ['lifecycleChanged', 'operationChanged', 'workChanged', 'lateLifecycle', 'tailMutation']) {
  test('lifecycle evidence cannot hide preview side effects: ' + option, async () => {
    const h = previewModel({[option]: true, eventCount: 401});
    await assert.rejects(idlePreview.run(h.ctx), e => messages(e).includes('admin_preview_caused_lifecycle_effect'));
    const errors = await h.cleanup();
    assert.ok(errors.some(e => messages(e).includes('admin_preview_caused_lifecycle_effect')));
    assert.equal(h.browserCalls.filter(c => c.operation === 'click').length, 1);
  });
}

for (const lateMutation of ['POST', 'PUT', 'DELETE']) {
  test('late browser mutation fails read-only preview after drain: ' + lateMutation, async () => {
    const h = previewModel({lateMutation});
    await assert.rejects(idlePreview.run(h.ctx), e => messages(e).includes('admin_preview_browser_mutation'));
    assert.ok((await h.cleanup()).some(e => messages(e).includes('admin_preview_browser_mutation')));
  });
}

for (const [option, code] of [['failedPreview', 'admin_preview_browser_response'],
  ['wrongBodyCount', 'admin_preview_browser_count'], ['unjoinedResponse', 'admin_preview_browser_ingress'],
  ['wrongServing', 'admin_preview_browser_ingress'], ['eagerMutation', 'admin_preview_browser_mutation'],
  ['bearerOnly', 'admin_preview_cookie_session_required']]) {
  test('an unproven UI preview cannot pass: ' + option, async () => {
    const h = previewModel({[option]: true});
    await assert.rejects(idlePreview.run(h.ctx), e => messages(e).includes(code));
    await h.cleanup();
  });
}

for (const [option, code] of [['wrongDetail', 'admin_idle_detail_identity'],
  ['missingLifecycle', 'admin_idle_detail_shape'], ['wrongEventOwner', 'admin_idle_event_identity'],
  ['gap', 'admin_idle_event_sequence'], ['nonArrayEvents', 'admin_idle_event_page']]) {
  test('malformed or unrelated lifecycle snapshot is rejected: ' + option, async () => {
    const h = previewModel({[option]: true});
    await assert.rejects(lifecycleSnapshot(h.ctx, h.cohort), new RegExp(code));
  });
}

test('a repeated full event page fails instead of looping', async () => {
  const h = previewModel({repeatedPage: true, eventCount: 401});
  await assert.rejects(lifecycleSnapshot(h.ctx, h.cohort), /admin_idle_event_sequence/);
  assert.ok(h.calls.length < 20);
});

test('unbounded event history cannot become a truncated passing snapshot', async () => {
  const h = previewModel({eventCount: 10000});
  await assert.rejects(lifecycleSnapshot(h.ctx, h.cohort), /admin_idle_event_bound/);
  assert.ok(h.calls.length <= 63);
});

test('close, evidence and lifecycle failures preserve all three outcomes', async () => {
  const h = previewModel({closeFailure: true, evidenceFailure: true, lateLifecycle: true});
  await assert.rejects(idlePreview.run(h.ctx), e => ['preview_close_failed', 'preview_evidence_failed',
    'admin_preview_caused_lifecycle_effect'].every(code => messages(e).includes(code)));
  const errors = await h.cleanup();
  for (const code of ['preview_close_failed', 'preview_evidence_failed', 'admin_preview_caused_lifecycle_effect'])
    assert.ok(errors.some(e => messages(e).includes(code)));
});

test('duplicate cohort identities are blocked before any API request', async () => {
  const h = previewModel(); h.cohort[1].id = h.cohort[0].id;
  await assert.rejects(idlePreview.run(h.ctx), /admin_idle_cohort_missing/);
  assert.equal(h.calls.length, 0);
});
