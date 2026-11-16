import {randomUUID} from 'node:crypto';
import {setTimeout as delay} from 'node:timers/promises';
import {check, required, read, request, P, other, equal} from './common.mjs';
import {validateAdmission} from './admission-read.mjs';

// Admission changes commit their own revision and audit atomically. They make
// real configure records without changing the platform settings revision, and
// need no Sandbox/runtime allocation. The caller holds the parent's global lease.
export async function admissionAuditCohort(ctx, actor) {
  const path = P + '/admission', original = await read(ctx, path);
  required(validateAdmission(original) && original.writes_available
    && original.operator_stop === null, 'admin_audit_admission_open_writer_required');
  let pair, used = false, cleanupJob;
  const changes = [];
  const ownedStop = current => current.revision === pair.revision + 1
    && current.operator_stop?.reason === pair.reason && current.operator_stop.actor === actor
    && current.operator_stop.revision === current.revision;
  const resumed = current => current.revision === pair.revision + 2 && current.operator_stop === null
    && current.last_change?.reason === pair.reason && current.last_change.actor === actor
    && current.last_change.old_stop?.reason === pair.reason
    && current.last_change.old_stop.actor === actor;
  const resume = async current => {
    check(!pair.resumeAttempted, 'admin_audit_unknown_resume_requires_parent_cleanup');
    pair.resumeAttempted = true;
    return (await request(ctx, 'POST', path + '/resume', {
      expected_revision: current.revision, reason: pair.reason,
    }, {am: other(ctx.am)})).body;
  };
  const cleanup = () => cleanupJob ??= (async () => {
    if (!pair) return;
    const current = await read(ctx, path, other(ctx.am));
    check(validateAdmission(current), 'admin_audit_admission_cleanup_shape');
    if (resumed(current) || (current.revision === pair.revision && current.operator_stop === null)) return;
    check(ownedStop(current), 'admin_audit_admission_foreign_change');
    // A lost stop ACK can be read back and compensated once. A lost resume ACK
    // must be read back; it never authorizes replay of that mutation.
    check(resumed(await resume(current)), 'admin_audit_admission_restore_failed');
    check(resumed(await read(ctx, path)), 'admin_audit_admission_restore_unconfirmed');
  })();
  ctx.defer('admin_audit_admission_restore', cleanup);
  return {cleanup, async displace(after) {
    check(!used && !cleanupJob, 'admin_audit_cohort_replayed'); used = true;
    const timestamp = Date.parse(after);
    check(Number.isFinite(timestamp), 'admin_audit_timestamp_invalid');
    // Admission audit timestamps have second precision. Let the first new
    // record sort strictly after the original settings audit, then prove that
    // ordering from the actual API records rather than from elapsed time.
    const wait = Math.floor(timestamp / 1000) * 1000 + 1000 - Date.now();
    required(wait <= 2000, 'admin_audit_server_clock_unbound');
    if (wait > 0) await delay(wait, undefined, {signal: ctx.signal});
    let expected = original.revision;
    for (let index = 0; index < 25; index++) {
      const before = await read(ctx, path);
      check(validateAdmission(before) && before.revision === expected && before.operator_stop === null,
        'admin_audit_admission_changed');
      pair = {revision: expected, reason: 'm6-settings-audit-' + randomUUID(), resumeAttempted: false};
      const stopped = (await request(ctx, 'POST', path + '/stop', {
        expected_revision: expected, reason: pair.reason,
      })).body;
      check(ownedStop(stopped) && stopped.last_change?.old_stop === null
        && equal(stopped.last_change?.new_stop, stopped.operator_stop)
        && stopped.last_change.actor === actor && stopped.last_change.reason === pair.reason,
      'admin_audit_stop_receipt');
      const opened = await resume(stopped);
      check(resumed(opened) && equal(opened.last_change.old_stop, stopped.operator_stop),
        'admin_audit_resume_receipt');
      changes.push(stopped.last_change, opened.last_change);
      expected = opened.revision;
    }
    const final = await read(ctx, path, other(ctx.am));
    check(validateAdmission(final) && resumed(final), 'admin_audit_admission_finality');
    return structuredClone(changes);
  }};
}
