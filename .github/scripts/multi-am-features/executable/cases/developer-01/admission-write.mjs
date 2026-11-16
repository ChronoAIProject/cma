import {define, setup, required, check, request, read, P, other, equal} from './common.mjs';
import {validateAdmission} from './admission-read.mjs';

function admissionCase(number) {
  return define(number, 'global', 3, async ctx => {
    const f = await setup(ctx, 'admission_change');
    required(f.exclusiveAdmission === true && typeof f.faults?.withholdNextSuccessfulResponse === 'function'
      && typeof f.freshCreate?.take === 'function' && typeof f.custody === 'function' && typeof f.sampleExistingWork === 'function',
    'admin_admission_mutation_fixture_missing');
    const reason = `m6-admin-${number}-${ctx.phase}-${ctx.iteration}`;
    const path = P + '/admission';
    let original, stopped, initialWork, restoreJob, resumeUnknown = false;
    const resume = async revision => {
      check(!resumeUnknown, 'admin_admission_resume_outcome_unknown');
      // Record intent before dispatch. A lost/invalid acknowledgement never
      // authorizes another resume, including from the cleanup callback.
      resumeUnknown = true;
      const resumed = (await request(ctx, 'POST', path + '/resume', {
        expected_revision: revision, reason,
      })).body;
      check(resumed.operator_stop === null && resumed.revision === revision + 1,
        'admin_resume_not_committed');
      resumeUnknown = false;
      return resumed;
    };
    // Explicit restoration and deferred cleanup join the same attempt and error.
    const restore = () => restoreJob ??= (async () => {
      const current = await read(ctx, path, other(ctx.am));
      if (current.operator_stop !== null) {
        check(current.operator_stop.reason === reason, 'admin_admission_foreign_stop');
        await resume(current.revision);
      }
      check((await read(ctx, path)).operator_stop === null, 'admin_admission_restore_failed');
    })();
    const stop = async revision => (await request(ctx, 'POST', path + '/stop', {
      expected_revision: revision, reason,
    })).body;
    const fresh = async (am, allowed) => {
      const intent = await f.freshCreate.take(am);
      required(intent.body && intent.key, 'admin_fresh_create_identity_missing');
      // Cleanup reconciliation owns the original intent before dispatch, including unknown ACKs.
      const custody = await f.custody(intent);
      ctx.defer('admin_admission_resource_cleanup', () => custody.cleanup());
      return request(ctx, 'POST', '/api/v1/sandboxes', intent.body,
        {actor: 'user', am, key: intent.key, allowed});
    };
    await ctx.step('step_1', async () => {
      original = await read(ctx, path);
      required(original.operator_stop === null && original.writes_available, 'admin_admission_initial_open_required');
      initialWork = await ctx.observe('admin_existing_work_before', () => f.sampleExistingWork());
      required(initialWork.running === true && initialWork.work_id, 'admin_ongoing_work_fixture_missing');
      ctx.defer('admin_admission_restore', restore);
      ctx.assert(validateAdmission(original), 'admin_admission_initial_shape');
      if (number === 74) stopped = await stop(original.revision);
    });
    await ctx.step('step_2', async () => {
      if (number === 73) stopped = await stop(original.revision);
      else {
        const resumed = await resume(stopped.revision);
        const peer = await read(ctx, path, other(ctx.am));
        ctx.assert(peer.operator_stop === null && peer.revision === resumed.revision, 'admin_resume_peer_mismatch');
        stopped = await stop(resumed.revision);
      }
      const peer = await read(ctx, path, other(ctx.am));
      ctx.assert(equal(peer.operator_stop, stopped.operator_stop) && peer.revision === stopped.revision,
        'admin_stop_peer_mismatch');
      for (const am of ['a', 'b']) {
        const refused = await fresh(am, [503]);
        ctx.assert(refused.body.code === 'admission_paused', 'admin_stopped_fresh_work_admitted');
      }
    });
    await ctx.step('step_3', async () => {
      const repeated = await Promise.all(['a', 'b'].map(am => request(ctx, 'POST', path + '/stop', {
        expected_revision: original.revision, reason: reason + '-repeat',
      }, {am})));
      ctx.assert(repeated.every(r => equal(r.body.operator_stop, stopped.operator_stop)
        && r.body.revision === stopped.revision), 'admin_duplicate_stop_replaced_first');
      await request(ctx, 'POST', path + '/resume', {expected_revision: original.revision, reason},
        {allowed: [409], am: other(ctx.am)});
      const afterStale = await read(ctx, path);
      ctx.assert(equal(afterStale.operator_stop, stopped.operator_stop), 'admin_stale_resume_cleared_stop');
      const fault = await f.faults.withholdNextSuccessfulResponse({method: 'POST', path: path + '/resume'});
      ctx.defer('admin_admission_fault_cleanup', () => fault.cleanup());
      let uncertain = false;
      try {
        await resume(stopped.revision);
      } catch (error) {
        // Only the explicitly armed transport-disconnect receipt establishes expected uncertainty.
        const receipt = await fault.receipt();
        if (receipt.forwarded_writes !== 1 || receipt.response_withheld !== true) throw error;
        uncertain = true;
      }
      ctx.assert(uncertain, 'admin_uncertain_response_fault_not_exercised');
      const reconciled = await read(ctx, path, other(ctx.am));
      ctx.assert(reconciled.operator_stop === null && reconciled.revision === stopped.revision + 1,
        'admin_uncertain_resume_not_reconciled');
      const receipt = await ctx.observe('admin_admission_uncertainty', () => fault.receipt());
      ctx.assert(receipt.forwarded_writes === 1 && receipt.following_requests?.[0]?.method === 'GET',
        'admin_uncertain_resume_replayed');
      await restore();
    });
    await ctx.step('expected_result', async () => {
      for (const am of ['a', 'b']) {
        const created = await fresh(am, [200, 201, 202]);
        ctx.assert(typeof (created.body.id ?? created.body.sandbox?.id) === 'string',
          'admin_resumed_create_missing_identity');
      }
      const currentWork = await ctx.observe('admin_existing_work_after', () => f.sampleExistingWork());
      ctx.assert(currentWork.work_id === initialWork.work_id && currentWork.sequence > initialWork.sequence
        && currentWork.restarts === initialWork.restarts, 'admin_existing_work_interrupted');
      ctx.assert((await read(ctx, path)).operator_stop === null, 'admin_admission_not_restored');
    });
  });
}
export const admissionStop = admissionCase(73);
export const admissionResume = admissionCase(74);
