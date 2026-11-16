import {define, setup, request, P, other, equal, required, check} from './common.mjs';

const unsigned = value => Number.isSafeInteger(value) && value >= 0;
const actor = value => typeof value === 'string' && value.length > 0;
const reason = value => typeof value === 'string' && value.trim().length > 0
  && new TextEncoder().encode(value).length <= 512 && !/\p{Cc}/u.test(value);
const stop = value => value && unsigned(value.since) && unsigned(value.revision) && value.revision > 0
  && actor(value.actor) && reason(value.reason);

// Capacity counters in the same response are live observations. This row proves
// the separately revisioned operator control; raw responses remain retained.
const durable = value => ({revision: value.revision, operator_stop: value.operator_stop,
  last_change: value.last_change, history: value.history});

export function validateAdmission(value) {
  if (!unsigned(value?.revision)
      || typeof value.writes_available !== 'boolean' || !Array.isArray(value.history)
      || value.history.length !== Math.min(value.revision, 20)) return false;
  if (value.revision === 0) return value.last_change === null && value.operator_stop === null;
  if (!value.last_change || !equal(value.last_change, value.history[0])
      || !equal(value.operator_stop, value.last_change.new_stop)) return false;
  return value.history.every((entry, i) => entry && entry.revision === value.revision - i
    && actor(entry.actor) && reason(entry.reason) && unsigned(entry.at)
    && (entry.old_stop === null) !== (entry.new_stop === null)
    && (entry.new_stop === null || (stop(entry.new_stop) && entry.new_stop.revision === entry.revision
      && entry.new_stop.since === entry.at && entry.new_stop.actor === entry.actor && entry.new_stop.reason === entry.reason))
    && (entry.old_stop === null || (stop(entry.old_stop) && entry.old_stop.revision + 1 === entry.revision))
    && (i === 0 || equal(value.history[i - 1].old_stop, entry.new_stop)));
}

export const admissionRead = define(72, 'global', 1, async ctx => {
  const f = await setup(ctx, 'admission_read');
  required(f.exclusiveAdmission === true && typeof f.setWriteAvailability === 'function',
    'admin_admission_availability_fixture_missing');
  let original, modeIntent = false, finalityJob, finalized = false;
  const snapshot = async (am, label) => {
    const response = await request(ctx, 'GET', P + '/admission', undefined, {am});
    await ctx.observe(label, async () => ({am, response}));
    check(validateAdmission(response.body), 'admin_admission_snapshot_shape');
    return response.body;
  };
  const unchanged = (current, available) => {
    check(current.writes_available === available, 'admin_admission_write_gate');
    check(equal(durable(current), durable(original)), 'admin_admission_durable_changed');
  };
  const pair = async (available, label) => {
    for (const am of [ctx.am, other(ctx.am)]) unchanged(await snapshot(am, label), available);
  };
  const finalize = () => finalityJob ??= (async () => {
    if (!original) return;
    const errors = [];
    // The parent owns the real writer mode. Record the transition before its
    // call; a lost response still requires one restoration attempt, never replay.
    if (modeIntent) try {await f.setWriteAvailability(original.writes_available);} catch (error) {errors.push(error);}
    for (const am of ['a', 'b']) {
      try {unchanged(await snapshot(am, 'admin_admission_final'), original.writes_available);}
      catch (error) {errors.push(error);}
    }
    if (errors.length) throw new AggregateError(errors, 'admin_admission_read_finality');
    finalized = true;
  })();
  ctx.defer('admin_admission_read_finalize', finalize);
  await ctx.step('step_1', async () => {
    const first = await snapshot(ctx.am, 'admin_admission_initial');
    const second = await snapshot(other(ctx.am), 'admin_admission_initial');
    ctx.assert(equal(durable(first), durable(second)) && first.writes_available === second.writes_available,
      'admin_admission_snapshot_mismatch');
    required(first.revision >= 21, 'admin_admission_history_boundary_not_seeded');
    original = structuredClone(first);
    ctx.assert(first.history.length === 20, 'admin_admission_history_unbounded');
  });
  await ctx.step('expected_result', async () => {
    if (original.writes_available) {modeIntent = true; await f.setWriteAvailability(false);}
    await pair(false, 'admin_admission_unavailable');
    for (const am of ['a', 'b']) {
      const refused = await request(ctx, 'POST', P + '/admission/stop', {
        expected_revision: original.revision, reason: 'local unavailable writer check',
      }, {am, allowed: [503]});
      await ctx.observe('admin_admission_refusal', async () => ({am, response: refused}));
      ctx.assert(refused.status === 503 && refused.body?.code === 'admission_control_writes_unavailable',
        'admin_admission_wrong_unavailable_code');
      await pair(false, 'admin_admission_after_refusal');
    }
    await finalize();
    ctx.assert(finalized, 'admin_admission_read_finalized');
  });
});
