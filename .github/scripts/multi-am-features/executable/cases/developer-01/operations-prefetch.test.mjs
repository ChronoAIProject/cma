import test from 'node:test';
import assert from 'node:assert/strict';
import {automaticPauseScenario} from './automatic-pause.mjs';
import {automaticModel} from './automatic-pause-model.mjs';
import {measuredVisibility} from './enforcement.mjs';
import {measuredModel} from './measured-model.mjs';
import {visitorLimits} from './enforcement.mjs';
import {visitorModel} from './visitor-custody-model.mjs';
import {triggerSettingScenario} from './trigger-setting.mjs';
import {ownedIntent} from './enforcement.mjs';
import {triggerModel} from './trigger-setting-model.mjs';
import {idlePreview} from './idle-preview.mjs';
import {previewModel} from './idle-preview-model.mjs';
import {pauseIdleScenario} from './idle-pause.mjs';
import {pauseModel} from './idle-pause-model.mjs';
import {withPrefetch} from './operations-prefetch-model.mjs';

const entries = [
  ['ADM-042', automaticModel, h => automaticPauseScenario(h.ctx, h.clock)],
  ['ADM-043', measuredModel, h => measuredVisibility.run(h.ctx)],
  ['ADM-046', visitorModel, h => visitorLimits.run(h.ctx)],
  ['ADM-047', triggerModel, h => triggerSettingScenario(h.ctx, {ownedIntent,
    poll: async (read, predicate) => {const result = await read(); assert.ok(predicate(result)); return result;}})],
  ['ADM-048', previewModel, h => idlePreview.run(h.ctx)],
  ['ADM-049', pauseModel, h => pauseIdleScenario(h.ctx, async (read, predicate) => {
    const result = await read(); assert.ok(predicate(result)); return result;
  })],
];
for (const [name, model, run] of entries) test(name + ' retains report prefetch without counting it as the feature mutation', async () => {
  const h = model(); withPrefetch(h); await run(h); assert.deepEqual(await h.cleanup(), []);
  assert.ok(h.observations.some(o => o.value?.requests?.some(r => r.request_id?.startsWith('prefetch-'))));
});

const messages = e => [e.message, ...(e.errors ?? []).map(messages)].join(' ');
for (const [name, model, run] of entries) {
  test(name + ' drains late prefetch and retains complete report serving fields', async () => {
    const h = model(), sessions = withPrefetch(h, {late: true});
    await run(h); assert.deepEqual(await h.cleanup(), []);
    const rows = h.observations.flatMap(o => o.value?.requests ?? []).filter(r => r.request_id?.startsWith('prefetch-'));
    assert.ok(rows.length > 0);
    assert.ok(rows.every(r => ['serving', 'serving_after', 'started_at', 'ended_at', 'attribution'].every(k => r[k])));
    assert.ok(sessions.every(s => s.page.listenerCount('request') === 0 && s.page.listenerCount('response') === 0));
  });
  test(name + ' preserves failure for an unrelated mutation', async () => {
    const h = model(); withPrefetch(h, {path: '/api/v1/admin/sandboxes', body: {id: 'unexpected'}});
    await assert.rejects(run(h), e => /extra.*write|browser_mutation/.test(messages(e))); await h.cleanup();
  });
  test(name + ' never passes an unresolved prefetch after final drain', async () => {
    const h = model(), sessions = withPrefetch(h, {unresolved: true});
    await assert.rejects(run(h), e => messages(e).includes('admin_operations_report_unresolved')); await h.cleanup();
    assert.ok(sessions.every(s => s.page.listenerCount('request') === 0 && s.page.listenerCount('response') === 0));
  });
  test(name + ' retains cleanup proof outside an active checklist step', async () => {
    const h = model(), step = h.ctx.step, check = h.ctx.assert;
    let active = false;
    h.ctx.step = async (label, runStep) => step(label, async () => {
      active = true; try {return await runStep();} finally {active = false;}
    });
    h.ctx.assert = (...args) => {assert.ok(active, 'assertion_outside_step'); return check(...args);};
    withPrefetch(h, {unresolved: true});
    await assert.rejects(run(h), e => messages(e).includes('admin_operations_report_unresolved'));
    const errors = await h.cleanup();
    assert.ok(!errors.some(e => messages(e).includes('assertion_outside_step')));
    assert.ok(errors.some(e => messages(e).includes('admin_operations_report_unresolved')));
  });
}

for (const [options, code] of [
  [{wrongAM: true}, 'admin_operations_report_ingress'], [{unjoined: true}, 'admin_operations_report_ingress'],
  [{duplicate: true}, 'admin_operations_report_ingress'], [{bodyFailure: true}, 'report_body_failed'],
  [{requestFailure: true}, 'request_body_failed'], [{status: 503}, 'admin_operations_report_admission'],
  [{status: 200, body: {report_id: 'x', status: 'pending'}}, 'admin_operations_report_admission'],
  [{body: {report_id: '', status: 'pending'}}, 'admin_operations_report_admission'],
  [{body: {report_id: 'x', status: 'failed'}}, 'admin_operations_report_admission'],
  [{method: 'DELETE'}, 'admin_operations_report_method'],
]) test('unproven report prefetch is rejected: ' + JSON.stringify(options), async () => {
  const h = automaticModel(); withPrefetch(h, options);
  await assert.rejects(automaticPauseScenario(h.ctx, h.clock), e => messages(e).includes(code)); await h.cleanup();
  assert.equal(h.actions.filter(a => a.op === 'save').length, 0);
});

test('a ready report prefetch is admitted by its exact 200 response', async () => {
  const h = automaticModel(); withPrefetch(h, {status: 200});
  await automaticPauseScenario(h.ctx, h.clock); assert.deepEqual(await h.cleanup(), []);
});

test('prefetch failure, close failure and unknown restoration remain separate and are never replayed', async () => {
  const h = automaticModel({closeFailure: true, restoreFailure: true}); withPrefetch(h, {late: true, wrongAM: true});
  await assert.rejects(automaticPauseScenario(h.ctx, h.clock), e => messages(e).includes('close_failed'));
  const errors = await h.cleanup();
  for (const code of ['close_failed', 'restore_unknown', 'admin_operations_report_ingress'])
    assert.ok(errors.some(e => messages(e).includes(code)), code);
  assert.equal(h.actions.filter(a => a.op === 'restore').length, 1);
  assert.ok(h.actions.filter(a => a.op === 'restore').every(a => a.closed));
});
