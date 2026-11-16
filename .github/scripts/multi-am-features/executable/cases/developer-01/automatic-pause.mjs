import {setup, settingsSession, read, P, other, required, check, equal, sameSettings, id} from './common.mjs';
import {lifecycleSnapshot} from './idle-preview.mjs';
import {automaticPauseUI} from './automatic-pause-ui.mjs';
import {automaticBudget, waitAutomaticGrace, automaticPoll, realClock} from './automatic-pause-time.mjs';

async function busyWork(ctx, fixture, sandbox) {
  const value = await ctx.observe('admin_auto_pause_busy_work', () => fixture.readBusyWork(ctx, sandbox));
  check(value && ['sandbox_id', 'agent_id', 'runtime_id'].every(k => typeof value[k] === 'string' && value[k].trim())
    && Number.isSafeInteger(value.turn_sequence) && value.turn_sequence > 0
    && Number.isSafeInteger(value.sequence) && value.sequence >= 0
    && Number.isSafeInteger(value.restarts) && value.restarts >= 0, 'admin_auto_pause_busy_shape');
  check(value.sandbox_id === sandbox.id, 'admin_auto_pause_busy_identity');
  return structuredClone(value);
}

export async function automaticPauseScenario(ctx, clock = realClock) {
  automaticBudget(ctx);
  const f = await setup(ctx, 'automatic_pause');
  required(f.exclusiveSettings === true && Array.isArray(f.cohort) && f.cohort.length >= 3
    && f.cohort.every(s => typeof s?.id === 'string' && ['idle', 'busy', 'always_on'].includes(s.kind))
    && new Set(f.cohort.map(s => s.id)).size === f.cohort.length
    && ['idle', 'busy', 'always_on'].every(kind => f.cohort.some(s => s.kind === kind))
    && typeof f.readBusyWork === 'function', 'admin_auto_pause_cohort');
  for (const sandbox of f.cohort) id(sandbox.id);
  const state = await settingsSession(ctx, f);
  required(state.original.settings.inactivity?.auto_pause_enabled === false, 'admin_auto_pause_initially_disabled');
  const ui = await automaticPauseUI(ctx, state), initialWork = new Map();
  let before, terminal = false, finalityJob;
  const finalize = () => finalityJob ??= (async () => {
    const errors = [];
    try {await ui.close();} catch (error) {errors.push(error);}
    if (before) for (const am of ['a', 'b']) {
      try {
        const final = await ctx.observe('admin_auto_pause_final_lifecycle', async () =>
          ({am, states: await lifecycleSnapshot(ctx, f.cohort, am)}));
        for (const [i, sandbox] of f.cohort.entries()) {
          if (sandbox.kind !== 'idle') check(equal(final.states[i], before[i]), 'admin_auto_pause_protected_changed');
          else if (terminal) check(final.states[i].status === 'paused' && !final.states[i].lifecycle.operation,
            'admin_auto_pause_incomplete_lifecycle');
        }
      } catch (error) {errors.push(error);}
    }
    for (const sandbox of f.cohort.filter(s => initialWork.has(s.id))) {
      try {
        const previous = initialWork.get(sandbox.id), value = await busyWork(ctx, f, sandbox);
        check(['sandbox_id', 'agent_id', 'turn_sequence', 'runtime_id'].every(k => value[k] === previous[k]),
          'admin_auto_pause_busy_identity');
        check(value.restarts === 0 && (terminal ? value.sequence > previous.sequence : value.sequence >= previous.sequence),
          'admin_auto_pause_busy_interrupted');
      } catch (error) {errors.push(error);}
    }
    try {await state.restore();} catch (error) {errors.push(error);}
    if (errors.length) throw new AggregateError(errors, 'admin_auto_pause_finality');
  })();
  ctx.defer('admin_auto_pause_finalize', finalize);
  await ctx.step('step_1', async () => {
    before = await ctx.observe('admin_auto_pause_initial_lifecycle', () => lifecycleSnapshot(ctx, f.cohort));
    for (const [i, sandbox] of f.cohort.entries()) {
      ctx.assert(before[i].status === 'active' && !before[i].lifecycle.operation, 'admin_auto_pause_initial_lifecycle');
      ctx.assert(sandbox.kind === 'always_on' ? before[i].always_on === true : before[i].always_on !== true,
        'admin_auto_pause_initial_policy');
      if (sandbox.kind === 'busy') {
        const work = await busyWork(ctx, f, sandbox);
        ctx.assert(work.restarts === 0, 'admin_auto_pause_busy_interrupted'); initialWork.set(sandbox.id, work);
      }
    }
    await ui.invalidBounds();
    await ui.change(false, 86400);
    await ui.change(false, 60);
    ctx.assert(equal(before, await lifecycleSnapshot(ctx, f.cohort, other(ctx.am))),
      'admin_auto_pause_disabled_effect');
    automaticBudget(ctx);
    const enabled = await ui.change(true, 60, true);
    await ui.close();
    await waitAutomaticGrace(ctx, enabled, clock);
    await automaticPoll(ctx, async () => {
      const states = [];
      for (const am of ['a', 'b']) for (const sandbox of f.cohort) {
        const value = (await read(ctx, P + '/sandboxes/' + id(sandbox.id), am)).sandbox;
        check(value?.id === sandbox.id, 'admin_auto_pause_poll_identity');
        if (sandbox.kind !== 'idle') check(value.status === 'active', 'admin_auto_pause_protected_status');
        states.push({am, id: sandbox.id, kind: sandbox.kind, status: value.status});
      }
      return ctx.observe('admin_auto_pause_lifecycle_poll', async () => states);
    }, states => states.every(s => s.status === (s.kind === 'idle' ? 'paused' : 'active')), clock);
    terminal = true;
  });
  await ctx.step('step_2', async () => {
    await finalize();
    for (const am of ['a', 'b']) ctx.assert(sameSettings((await read(ctx, P + '/settings', am)).settings,
      state.original.settings), 'admin_auto_pause_restore');
  });
  await ctx.step('expected_result', async () => {
    ctx.assert(terminal, 'admin_auto_pause_completion_unproven');
  });
}
