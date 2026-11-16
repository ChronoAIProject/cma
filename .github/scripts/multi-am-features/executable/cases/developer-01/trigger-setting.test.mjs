import test from 'node:test';
import assert from 'node:assert/strict';
import {triggerSetting} from './triggers.mjs';
import {triggerModel} from './trigger-setting-model.mjs';

const messages = error => [error.message, ...(error.errors ?? []).map(messages)].join(' ');

test('public Launch identity proves disabled admission, paused work and one resumed launch', async () => {
  const h = triggerModel(); await triggerSetting.run(h.ctx);
  assert.deepEqual(await h.cleanup(), []);
  assert.deepEqual(h.settings(), h.original);
  assert.deepEqual(h.captures.map(c => c.response.status), [503, 503, 202]);
  assert.deepEqual(h.reservations.map(r => r.dispatch.am), ['a', 'b', 'a']);
  assert.equal(h.browserCalls.filter(c => c.operation === 'save').length, 3);
  assert.ok(h.browserCalls.filter(c => c.operation === 'restore').every(c => c.closed));
  assert.equal(h.calls.filter(c => c.key === 'start-once').length, 1);
  const completed = h.observations.find(o => o.label === 'admin_trigger_completed_launch').value;
  assert.deepEqual(completed.map(c => c.am), ['a', 'b']);
  assert.ok(completed.every(c => c.launch.id === 'launch-once' && c.launch.state === 'ready'));
  assert.ok(h.observations.some(o => o.label === 'admin_trigger_paused_launch'));
});

for (const [option, code] of [
  ['wrongRefusal', 'admin_trigger_disabled_refusal'], ['disabledAccepted', 'status_202'],
  ['wrongBlock', 'admin_trigger_wrong_block'], ['provisionWhilePaused', 'admin_trigger_paused_resources_changed'],
  ['wrongResource', 'admin_trigger_peer_resources'], ['missingReadyTime', 'admin_trigger_ready_resources'],
  ['wrongLaunchOwner', 'admin_trigger_launch_identity'], ['changedDigest', 'admin_trigger_launch_identity'],
  ['initialLaunch', 'admin_trigger_launch_inventory'], ['extraPage', 'admin_trigger_inventory_incomplete'],
  ['duplicateLaunch', 'admin_trigger_launch_inventory'], ['failedLaunch', 'admin_trigger_launch_failed'],
  ['draftLaunch', 'admin_trigger_initial_launch'], ['failedSave', 'admin_trigger_save_response'],
  ['unjoinedSave', 'admin_trigger_browser_ingress'], ['wrongServing', 'admin_trigger_browser_ingress'],
  ['bearerOnly', 'admin_trigger_cookie_session_required'], ['lateWrite', 'admin_trigger_browser_extra_write'],
  ['captureFailure', 'launch_capture_failed'], ['missingCapture', 'admin_owned_capture_missing'],
]) {
  test('unproven trigger behavior fails: ' + option, async () => {
    const h = triggerModel({[option]: true});
    await assert.rejects(triggerSetting.run(h.ctx), e => messages(e).includes(code));
    await h.cleanup();
    assert.ok(h.calls.filter(c => c.key === 'start-once').length <= 1);
    assert.ok(h.browserCalls.filter(c => c.operation === 'restore').every(c => c.closed));
  });
}

for (const option of ['unknownAck', 'attributionFailure']) {
  test('uncertain launch has original custody without replay: ' + option, async () => {
    const h = triggerModel({[option]: true});
    await assert.rejects(triggerSetting.run(h.ctx), /launch_ack_unknown|launch_attribution_failed/);
    const failures = await h.cleanup();
    assert.equal(h.calls.filter(c => c.key === 'start-once').length, 1);
    if (option === 'unknownAck') {
      assert.equal(h.captures.length, 2);
      assert.ok(failures.some(e => messages(e).includes('launch_unknown_intent_retained')));
    } else {
      assert.equal(h.captures.length, 3);
      assert.equal(h.captures[2].response.body.id, 'launch-once');
      assert.equal(h.captures[2].response.status, 202);
    }
    assert.deepEqual(h.settings(), h.original);
    const closed = h.browserCalls.findIndex(c => c.operation === 'close');
    assert.ok(h.browserCalls.every((c, i) => c.operation !== 'launch_cleanup' || i > closed));
  });
}

test('browser close/evidence and native custody failures are all retained', async () => {
  const h = triggerModel({closeFailure: true, evidenceFailure: true, cleanupFailure: true});
  await assert.rejects(triggerSetting.run(h.ctx), e => messages(e).includes('trigger_close_failed')
    && messages(e).includes('trigger_evidence_failed'));
  const failures = await h.cleanup();
  for (const code of ['trigger_close_failed', 'trigger_evidence_failed', 'launch_native_finality_failed'])
    assert.ok(failures.some(e => messages(e).includes(code)), code);
  assert.deepEqual(h.settings(), h.original);
  assert.equal(h.browserCalls.filter(c => c.operation === 'close').length, 1);
});

test('an unknown restoration acknowledgement is never retried', async () => {
  const h = triggerModel({restoreFailure: true});
  await assert.rejects(triggerSetting.run(h.ctx), /settings_restore_unknown/);
  assert.ok((await h.cleanup()).some(e => messages(e).includes('settings_restore_unknown')));
  assert.equal(h.browserCalls.filter(c => c.operation === 'restore').length, 1);
});

for (const invalid of ['duplicate_keys', 'wrong_actor', 'wrong_path', 'missing_custody']) {
  test('malformed parent intent contract blocks before any request: ' + invalid, async () => {
    const h = triggerModel();
    if (invalid === 'duplicate_keys') h.f.launchIntent.key = h.f.disabledIntents[0].key;
    if (invalid === 'wrong_actor') h.f.launchIntent.actor = 'admin';
    if (invalid === 'wrong_path') h.f.launchIntent.path = '/api/v1/agents';
    if (invalid === 'missing_custody') delete h.f.custody;
    await assert.rejects(triggerSetting.run(h.ctx)); await h.cleanup();
    assert.equal(h.calls.length, 0);
  });
}

test('second ingress joins its real saves and continues one original Launch', async () => {
  const h = triggerModel(); h.ctx.am = 'b'; await triggerSetting.run(h.ctx);
  assert.deepEqual(await h.cleanup(), []);
  assert.deepEqual(h.reservations.map(r => r.dispatch.am), ['a', 'b', 'b']);
  assert.ok(h.receipts.filter(r => r.method === 'PUT').every(r => r.am === 'b' && r.serving_am === 'b'));
});
