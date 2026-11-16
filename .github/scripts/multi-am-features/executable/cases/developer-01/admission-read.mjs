import {define, setup, read, request, P, other, equal, required} from './common.mjs';

export function validateAdmission(value) {
  if (!Number.isSafeInteger(value?.revision) || value.revision < 0
      || typeof value.writes_available !== 'boolean' || !Array.isArray(value.history)
      || value.history.length !== Math.min(value.revision, 20)) return false;
  if (value.revision === 0) return value.last_change === null && value.operator_stop === null;
  if (!equal(value.last_change, value.history[0])
      || !equal(value.operator_stop, value.last_change.new_stop)) return false;
  return value.history.every((entry, i) => entry.revision === value.revision - i
    && typeof entry.actor === 'string' && typeof entry.reason === 'string'
    && entry.reason.trim().length > 0 && Number.isSafeInteger(entry.at)
    && (entry.old_stop === null) !== (entry.new_stop === null)
    && (i === 0 || equal(value.history[i - 1].old_stop, entry.new_stop)));
}

export const admissionRead = define(72, 'global', 1, async ctx => {
  const f = await setup(ctx, 'admission_read');
  required(f.exclusiveAdmission === true && typeof f.setWriteAvailability === 'function',
    'admin_admission_availability_fixture_missing');
  await ctx.step('step_1', async () => {
    const first = await read(ctx, P + '/admission');
    const second = await read(ctx, P + '/admission', other(ctx.am));
    ctx.assert(validateAdmission(first) && equal(first, second), 'admin_admission_snapshot_mismatch');
    required(first.revision >= 21, 'admin_admission_history_boundary_not_seeded');
    ctx.assert(first.history.length === 20, 'admin_admission_history_unbounded');
    await ctx.observe('admin_admission_snapshot', async () => first);
  });
  await ctx.step('expected_result', async () => {
    await f.setWriteAvailability(false);
    for (const am of ['a', 'b']) {
      const current = await read(ctx, P + '/admission', am);
      ctx.assert(validateAdmission(current) && current.writes_available === false,
        'admin_admission_unavailable_not_exposed');
      const refused = await request(ctx, 'POST', P + '/admission/stop', {
        expected_revision: current.revision, reason: 'local unavailable writer check',
      }, {am, allowed: [503]});
      ctx.assert(refused.body?.code === 'admission_control_writes_unavailable', 'admin_admission_wrong_unavailable_code');
    }
  });
});
