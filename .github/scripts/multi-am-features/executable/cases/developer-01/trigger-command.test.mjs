import test from 'node:test';
import assert from 'node:assert/strict';
import {triggerPause, triggerResume} from './triggers.mjs';
import {commandModel} from './trigger-command-model.mjs';

const messages = error => [error.message, ...(error.errors ?? []).map(messages)].join(' ');
for (const [action, scenario] of [['pause', triggerPause], ['resume', triggerResume]]) {
  test('dedicated ' + action + ' joins its public Launch, command receipt and exact audit', async () => {
    const h = commandModel(action); await scenario.run(h.ctx);
    assert.deepEqual(await h.cleanup(), []); assert.deepEqual(h.settings(), h.original);
    assert.equal(h.calls.filter(c => c.path === '/api/v1/admin/triggers/' + action).length, 1);
    assert.equal(h.calls.filter(c => c.key === 'launch-once').length, 1);
    assert.equal(h.operations.filter(c => c.kind === 'resource_cleanup').length, 1);
    assert.ok(h.operations.findIndex(c => c.kind === 'resource_cleanup') < h.operations.findIndex(c => c.kind === 'restore'));
    const command = h.observations.find(o => o.label === 'admin_trigger_command_receipt').value;
    assert.equal(command.status, 200); assert.ok(command.timing.request_id);
    for (const key of ['serving', 'serving_after', 'started_at', 'ended_at', 'attribution']) assert.ok(command.timing[key]);
    const audits = h.observations.find(o => o.label === 'admin_trigger_command_audit_join').value;
    assert.equal(audits.length, 2); assert.deepEqual(audits.map(a => a.am), ['a', 'b']);
    assert.ok(audits.every(a => a.record.id === 'command-audit' && a.record.detail.revision === String(command.body.revision)));
  });
}

for (const [action, scenario] of [['pause', triggerPause], ['resume', triggerResume]]) {
  for (const [option, code] of [
    ['staleRevision', 'admin_trigger_command_receipt_policy'], ['wrongReceiptFlag', 'admin_trigger_command_receipt_policy'],
    ['unattributedCommand', 'admin_trigger_command_ingress'], ['wrongCommandAM', 'admin_trigger_command_ingress'],
    ['peerDrift', 'admin_trigger_command_peer'], ['failedCommand', 'status_503'],
    ['wrongBlock', 'admin_trigger_wrong_block'], ['pausedProvisioning', 'admin_trigger_paused_resources_changed'],
    ['differentLaunch', 'admin_trigger_launch_identity'], ['duplicateLaunch', 'admin_trigger_launch_inventory'],
    ['missingAudit', 'admin_trigger_command_audit'], ['duplicateAudit', 'admin_trigger_command_audit'],
    ['wrongAuditActor', 'admin_trigger_audit_filter'], ['wrongAuditStatus', 'admin_trigger_command_audit'],
    ['wrongAuditRevision', 'admin_trigger_command_audit'], ['wrongAuditTarget', 'admin_trigger_command_audit'],
    ['failedAudit', 'admin_trigger_command_audit'], ['splitAudit', 'admin_trigger_audit_peer_identity'],
    ['reusedAuditId', 'admin_trigger_command_audit'],
  ]) {
    test(action + ' cannot pass with invalid command/Launch/audit proof: ' + option, async () => {
      const h = commandModel(action, {[option]: true});
      await assert.rejects(scenario.run(h.ctx), e => messages(e).includes(code));
      await h.cleanup();
      assert.ok(h.operations.filter(c => c.kind === 'command').length <= 1);
      assert.ok(h.operations.filter(c => c.kind === 'create').length <= 1);
    });
  }
  for (const option of ['unknownCommand', 'commandAttributionFailure', 'unknownLaunch', 'launchAttributionFailure']) {
    test(action + ' preserves unknown/accepted outcomes without replay: ' + option, async () => {
      const h = commandModel(action, {[option]: true});
      await assert.rejects(scenario.run(h.ctx), /command_ack_unknown|command_attribution_failed|command_launch_unknown|command_launch_attribution_failed/);
      const failures = await h.cleanup();
      assert.ok(h.operations.filter(c => c.kind === 'command').length <= 1);
      assert.ok(h.operations.filter(c => c.kind === 'create').length <= 1);
      assert.deepEqual(h.settings(), h.original);
      if (option === 'unknownLaunch') assert.ok(failures.some(e => messages(e).includes('command_launch_unknown_custody')));
      if (option === 'launchAttributionFailure') {
        assert.equal(h.captures.length, 1); assert.equal(h.captures[0].body.id, 'launch-once');
      }
      if (option === 'commandAttributionFailure') {
        const captured = h.observations.find(o => o.label === 'admin_trigger_uncertain_command_receipt');
        assert.equal(captured.value.status, 200); assert.ok(captured.value.timing.request_id);
      }
      if (action === 'pause' && ['unknownCommand', 'commandAttributionFailure'].includes(option))
        assert.equal(h.operations.filter(c => c.kind === 'create').length, 0);
    });
  }
  test(action + ' reads complete audit pages through each AM', async () => {
    const h = commandModel(action, {auditPageCount: 53}); await scenario.run(h.ctx);
    assert.deepEqual(await h.cleanup(), []);
    for (const am of ['a', 'b']) assert.ok(h.calls.some(c => c.am === am && c.path.includes('cursor=50')));
  });
  test(action + ' preserves independent cleanup and restoration failures without retry', async () => {
    const h = commandModel(action, {cleanupFailure: true, restoreFailure: true});
    await assert.rejects(scenario.run(h.ctx), /command_native_finality_failed/);
    const failures = await h.cleanup();
    for (const code of ['command_native_finality_failed', 'command_restore_unknown'])
      assert.ok(failures.some(e => messages(e).includes(code)));
    assert.equal(h.operations.filter(c => c.kind === 'resource_cleanup').length, 1);
    assert.equal(h.operations.filter(c => c.kind === 'restore').length, 1);
  });
}

test('pause must observe cancellation before restoring a policy that can resume work', async () => {
  const h = commandModel('pause', {noCancel: true});
  await assert.rejects(triggerPause.run(h.ctx), /admin_trigger_command_cleanup_state/);
  await h.cleanup();
});
for (const option of ['differentResources', 'failedLaunch']) {
  test('resume rejects divergent or failed completed resources: ' + option, async () => {
    const h = commandModel('resume', {[option]: true});
    await assert.rejects(triggerResume.run(h.ctx), /admin_trigger_peer_resources|admin_trigger_launch_failed/);
    await h.cleanup();
  });
}
for (const [option, code] of [['repeatedCursor', 'admin_trigger_audit_row'], ['auditPageCount', 'admin_trigger_audit_bound']]) {
  test('an incomplete audit cannot prove a unique command: ' + option, async () => {
    const h = commandModel('pause', {[option]: option === 'auditPageCount' ? 1001 : true});
    await assert.rejects(triggerPause.run(h.ctx), new RegExp(code));
    await h.cleanup();
    assert.equal(h.operations.filter(c => c.kind === 'command').length, 0);
  });
}

test('accepted command and evidence failure retain both causes', async () => {
  const h = commandModel('pause', {commandAttributionFailure: true}), observe = h.ctx.observe;
  h.ctx.observe = (label, run) => {
    if (label === 'admin_trigger_uncertain_command_receipt') throw Error('command_capture_failed');
    return observe(label, run);
  };
  await assert.rejects(triggerPause.run(h.ctx), e => messages(e).includes('command_attribution_failed')
    && messages(e).includes('command_capture_failed'));
  await h.cleanup(); assert.deepEqual(h.settings(), h.original);
  assert.equal(h.operations.filter(c => c.kind === 'command').length, 1);
});

for (const field of ['adminSubject', 'launchIntent', 'custody']) {
  test('missing actual parent binding blocks before requests: ' + field, async () => {
    const h = commandModel('pause'); delete h.f[field];
    await assert.rejects(triggerPause.run(h.ctx), /admin_trigger_command_fixture_missing/);
    assert.equal(h.calls.length, 0); await h.cleanup();
  });
}

test('the other scheduled ingress retains the actual command and Launch receipts', async () => {
  const h = commandModel('resume'); h.ctx.am = 'b'; await triggerResume.run(h.ctx);
  assert.deepEqual(await h.cleanup(), []);
  assert.equal(h.observations.find(o => o.label === 'admin_trigger_command_receipt').value.timing.am, 'b');
  assert.equal(h.operations.find(o => o.kind === 'custody').dispatch.am, 'b');
});
