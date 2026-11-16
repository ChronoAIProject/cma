import test from 'node:test';
import assert from 'node:assert/strict';
import {visitorLimits} from './enforcement.mjs';
import {visitorModel} from './visitor-custody-model.mjs';

const attempted = h => h.calls.filter(c => c.method === 'POST');

for (const launchStatus of [201, 202]) {
  test('visitor boundaries capture every original outcome and complete exact cleanup: ' + launchStatus, async () => {
    const h = visitorModel({launchStatus});
    await visitorLimits.run(h.ctx);
    assert.equal(h.captures.length, 5);
    assert.deepEqual(h.reservations.map(r => r.dispatch), ['a', 'b', 'a', 'b', 'a'].map(am => ({actor: 'visitor', am})));
    assert.deepEqual(h.captures.map(c => c.response), h.responses);
    const observations = h.observations.filter(o => o.label.endsWith('_outcomes'));
    assert.deepEqual(observations.flatMap(o => o.value), h.responses);
    assert.ok(observations.every(o => o.value.every(r => r.timing.serving && r.timing.serving_after
      && r.timing.started_at && r.timing.ended_at && r.timing.attribution)));
    assert.deepEqual(await h.cleanup(), []);
    assert.deepEqual(h.settings, h.original); assert.equal(h.outstanding.size, 0);
    assert.deepEqual([...h.retired].sort(), ['launch-result-0', 'launch-result-1', 'sandbox-result-0']);
    assert.equal(attempted(h).length, 5);
  });
}

for (const failAt of [1, 3, 4]) {
  test('late accepted visitor response is captured before attribution failure escapes: ' + failAt, async () => {
    const h = visitorModel({failAt, attributionFailure: true});
    await assert.rejects(visitorLimits.run(h.ctx), /visitor_attribution_failed/);
    assert.equal(attempted(h).length, failAt); assert.equal(h.captures.length, failAt);
    assert.ok(h.captures.at(-1).response.body.id);
    assert.deepEqual(await h.cleanup(), []); assert.deepEqual(h.settings, h.original);
    const n = attempted(h).length; assert.deepEqual(await h.cleanup(), []); assert.equal(attempted(h).length, n);
  });
}

test('primary attribution and failed response-retention remain separate without replay', async () => {
  const h = visitorModel({failAt: 1, attributionFailure: true, captureFailure: true});
  await assert.rejects(visitorLimits.run(h.ctx), error => {
    assert.deepEqual(error.errors?.map(e => e.message), ['visitor_attribution_failed', 'visitor_capture_failed']);
    return true;
  });
  assert.equal(h.captures.length, 1); assert.equal(attempted(h).length, 1);
  assert.ok((await h.cleanup()).some(e => e.message === 'visitor_unknown_intent_retained'));
  assert.deepEqual(h.settings, h.original);
});

test('capture failure after a successful request stops later creates and preserves uncertainty', async () => {
  const h = visitorModel({captureFailure: true});
  await assert.rejects(visitorLimits.run(h.ctx), /visitor_capture_failed/);
  assert.equal(h.captures.length, 1); assert.equal(attempted(h).length, 1);
  assert.ok((await h.cleanup()).some(e => e.message === 'visitor_unknown_intent_retained'));
  assert.deepEqual(h.settings, h.original);
});

test('unknown visitor result stays unresolved while settings restore; neither create nor cleanup replays', async () => {
  const h = visitorModel({failAt: 3, unknownOutcome: true});
  await assert.rejects(visitorLimits.run(h.ctx), /visitor_unknown_ack/);
  assert.equal(h.captures.length, 2); assert.equal(attempted(h).length, 3);
  for (let i = 0; i < 2; i++) assert.ok((await h.cleanup()).some(e => e.message === 'visitor_unknown_intent_retained'));
  assert.deepEqual(h.settings, h.original); assert.equal(attempted(h).length, 3);
});

for (const [option, code] of [
  ['duplicateLaunch', 'admin_visitor_resource_identity'], ['missingIdentity', 'admin_visitor_resource_identity'],
  ['foreignInventory', 'admin_visitor_durable_identity'], ['wrongIngress', 'admin_visitor_ingress'],
  ['wrongRefusalResource', 'admin_visitor_launch_quota_scope'], ['wrongRefusalCode', 'admin_visitor_limit_refusal'],
]) {
  test('incorrect visitor evidence cannot pass the row: ' + option, async () => {
    const h = visitorModel({[option]: true});
    await assert.rejects(visitorLimits.run(h.ctx), new RegExp(code));
    await h.cleanup(); assert.deepEqual(h.settings, h.original);
    assert.ok(attempted(h).length <= 5);
  });
}

test('undocumented success status cannot prove a public visitor create', async () => {
  const h = visitorModel({successStatus: 200});
  await assert.rejects(visitorLimits.run(h.ctx), /status_200/);
  assert.equal(attempted(h).length, 1);
  assert.equal(h.captures.length, 1, 'accepted error response remains available for cleanup');
  await h.cleanup(); assert.deepEqual(h.settings, h.original);
});

test('missing response capture blocks before a visitor dispatch and preserves cleanup', async () => {
  const h = visitorModel({missingCapture: true});
  await assert.rejects(visitorLimits.run(h.ctx), /admin_owned_capture_missing/);
  assert.equal(attempted(h).length, 0);
  await h.cleanup(); assert.deepEqual(h.settings, h.original);
});

for (const [name, mutate] of [
  ['duplicate keys', f => {f.launchIntents[1].key = f.launchIntents[0].key;}],
  ['admin identity', f => {f.sandboxIntents[0].actor = 'admin';}],
  ['wrong route', f => {f.launchIntents[0].path = '/api/v1/agents';}],
  ['non-POST intent', f => {f.sandboxIntents[0].method = 'PUT';}],
  ['absent custody', f => {delete f.custody;}],
  ['absent inventory', f => {delete f.readPublicInventory;}],
]) {
  test('invalid visitor fixture blocks before changing limits: ' + name, async () => {
    const h = visitorModel(); mutate(h.f);
    await assert.rejects(visitorLimits.run(h.ctx), /admin_visitor_.*fixture/);
    assert.ok(h.calls.every(c => c.method === 'GET'));
    await h.cleanup(); assert.deepEqual(h.settings, h.original);
  });
}

test('cleanup failure remains separate from a completed visitor boundary and never repeats creates', async () => {
  const h = visitorModel({cleanupFailure: true});
  await visitorLimits.run(h.ctx);
  for (let i = 0; i < 2; i++) assert.ok((await h.cleanup()).some(e => e.message === 'visitor_native_finality_failed'));
  assert.equal(attempted(h).length, 5); assert.deepEqual(h.settings, h.original);
});

const messages = error => [error.message, ...(error.errors ?? []).map(messages)].join(' ');

test('visitor policy changes use both compiled settings controls and join the actual save ingress', async () => {
  const h = visitorModel(); await visitorLimits.run(h.ctx); await h.cleanup();
  assert.deepEqual(h.uiCalls.filter(c => c.operation === 'fill').map(c => [c.label, c.value]),
    [['Visitor Sandboxes', '1'], ['Visitor launches per day', '2']]);
  assert.equal(h.uiCalls.filter(c => c.operation === 'save').length, 1);
  assert.ok(!h.calls.some(c => c.method === 'PATCH'));
  assert.ok(h.uiCalls.some(c => c.operation === 'evidence' && c.closed));
  assert.ok(h.calls.every(c => !Object.hasOwn(c, 'requireCapture')));
});

for (const [option, code] of [['browserNoChange', 'admin_visitor_policy_not_changed'],
  ['browserUnjoined', 'admin_visitor_browser_save_receipt'], ['browserBearer', 'admin_visitor_cookie_session_required'],
  ['browserSaveUnknown', 'browser_save_unknown']]) {
  test('failed or unproven visitor settings UI cannot proceed to resource creation: ' + option, async () => {
    const h = visitorModel({[option]: true});
    await assert.rejects(visitorLimits.run(h.ctx), new RegExp(code));
    await h.cleanup(); assert.deepEqual(h.settings, h.original);
    assert.equal(attempted(h).length, 0);
    assert.ok(h.uiCalls.filter(c => c.operation === 'save').length <= 1);
  });
}

test('a late browser mutation is detected after drain and cannot undo restored settings silently', async () => {
  const h = visitorModel({browserLateMutation: true});
  await assert.rejects(visitorLimits.run(h.ctx), e => messages(e).includes('admin_visitor_browser_extra_write'));
  assert.ok((await h.cleanup()).some(e => messages(e).includes('admin_visitor_browser_extra_write')));
  assert.deepEqual(h.settings, h.original);
  assert.equal(h.uiCalls.filter(c => c.operation === 'save').length, 1);
});

test('browser close and evidence failures both survive and settings restoration still runs', async () => {
  const h = visitorModel({browserCloseFailure: true, browserEvidenceFailure: true});
  await assert.rejects(visitorLimits.run(h.ctx), e => messages(e).includes('browser_close_failed')
    && messages(e).includes('browser_evidence_failed'));
  const cleanup = await h.cleanup();
  assert.ok(cleanup.some(e => messages(e).includes('browser_close_failed')));
  assert.ok(cleanup.some(e => messages(e).includes('browser_evidence_failed')));
  assert.deepEqual(h.settings, h.original);
});
