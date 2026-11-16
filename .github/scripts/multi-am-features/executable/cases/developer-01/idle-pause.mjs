import {setup, browser, check, navigateSettings, request, read, P, other, id, equal, required, apiPath} from './common.mjs';
import {operationsPrefetch} from './operations-prefetch.mjs';
import {lifecycleSnapshot} from './idle-preview.mjs';

const PAUSE = P + '/auto-pause/pause-idle-now';
const CONFIRM = 'Pause every currently idle Sandbox now? Active work and Always on Sandboxes are rechecked.';
const reads = new Set(['GET', 'HEAD', 'OPTIONS']);

async function busyWork(ctx, fixture, sandbox) {
  const work = await ctx.observe('admin_pause_busy_work', () => fixture.readBusyWork(ctx, sandbox));
  check(work && ['sandbox_id', 'agent_id', 'runtime_id'].every(k => typeof work[k] === 'string' && work[k].trim())
    && Number.isSafeInteger(work.turn_sequence) && work.turn_sequence > 0
    && Number.isSafeInteger(work.sequence) && work.sequence >= 0
    && Number.isSafeInteger(work.restarts) && work.restarts >= 0, 'admin_pause_busy_shape');
  check(work.sandbox_id === sandbox.id, 'admin_pause_busy_identity');
  return structuredClone(work);
}

export async function pauseIdleScenario(ctx, poll) {
  const f = await setup(ctx, 'idle_pause');
  required(f.exclusiveSettings === true && Array.isArray(f.cohort) && f.cohort.length >= 3
    && f.cohort.every(s => typeof s?.id === 'string' && ['idle', 'busy', 'always_on'].includes(s.kind))
    && new Set(f.cohort.map(s => s.id)).size === f.cohort.length
    && ['idle', 'busy', 'always_on'].every(kind => f.cohort.some(s => s.kind === kind))
    && typeof f.readBusyWork === 'function', 'admin_pause_cohort_missing');
  for (const sandbox of f.cohort) id(sandbox.id);
  const session = await browser(ctx), reports = operationsPrefetch(ctx, session), initialWork = new Map();
  let before, ack, intent = false, terminal = false, closeJob, finalityJob;
  const capture = async (final = false) => {
    const value = await reports.capture('admin_browser_evidence', final);
    required(value.authentication === 'browser_session', 'admin_pause_cookie_session_required');
    const writes = value.requests.filter(r => !reads.has(r.method));
    check(writes.length <= (intent ? 1 : 0), 'admin_pause_browser_extra_write');
    if (ack) check(writes.length === 1 && writes[0].method === 'POST' && writes[0].status === 200
      && writes[0].request_id === ack.request_id && writes[0].am === ctx.am && writes[0].serving_am === ctx.am,
    'admin_pause_browser_ingress');
    return value;
  };
  const close = () => closeJob ??= (async () => {
    const errors = [];
    try {await session.close();} catch (error) {errors.push(error);}
    try {await capture(true);} catch (error) {errors.push(error);}
    if (errors.length) throw new AggregateError(errors, 'admin_pause_browser_finality_failed');
  })();
  // Cleanup can be reached after an unknown acknowledgement. Never click again;
  // drain first and independently retain lifecycle and ongoing-work failures.
  const finalize = () => finalityJob ??= (async () => {
    const errors = [];
    try {await close();} catch (error) {errors.push(error);}
    if (before) for (const am of ['a', 'b']) {
      try {
        const final = await ctx.observe('admin_pause_final_lifecycle', async () =>
          ({am, states: await lifecycleSnapshot(ctx, f.cohort, am)}));
        for (const [i, sandbox] of f.cohort.entries()) {
          if (sandbox.kind !== 'idle') check(equal(final.states[i], before[i]),
            'admin_pause_protected_lifecycle_changed');
          else if (terminal) check(final.states[i].status === 'paused' && !final.states[i].lifecycle.operation,
            'admin_pause_incomplete_lifecycle');
        }
      } catch (error) {errors.push(error);}
    }
    for (const sandbox of f.cohort.filter(s => initialWork.has(s.id))) {
      try {
        const previous = initialWork.get(sandbox.id), work = await busyWork(ctx, f, sandbox);
        check(['sandbox_id', 'agent_id', 'turn_sequence', 'runtime_id'].every(k => work[k] === previous[k]),
          'admin_pause_busy_identity');
        check(work.restarts === 0 && (terminal ? work.sequence > previous.sequence : work.sequence >= previous.sequence),
          'admin_pause_busy_turn_interrupted');
      } catch (error) {errors.push(error);}
    }
    if (errors.length) throw new AggregateError(errors, 'admin_pause_finality_failed');
  })();
  ctx.defer('admin_pause_finality', finalize);
  await ctx.step('step_1', async () => {
    before = await ctx.observe('admin_pause_initial_lifecycle', () => lifecycleSnapshot(ctx, f.cohort));
    for (const [i, sandbox] of f.cohort.entries()) {
      ctx.assert(before[i].status === 'active' && !before[i].lifecycle.operation, 'admin_pause_initial_lifecycle');
      ctx.assert(sandbox.kind === 'always_on' ? before[i].always_on === true : before[i].always_on !== true,
        'admin_pause_initial_policy');
      if (sandbox.kind === 'busy') {
        const work = await busyWork(ctx, f, sandbox);
        ctx.assert(work.restarts === 0, 'admin_pause_busy_turn_interrupted'); initialWork.set(sandbox.id, work);
      }
    }
  });
  await ctx.step('step_2', async () => {
    const refusal = await request(ctx, 'POST', PAUSE, {confirmed: false}, {allowed: [400]});
    ctx.assert(refusal.status === 400 && refusal.body?.code === 'invalid_admin_request', 'admin_pause_unconfirmed_refusal');
    const afterRefusal = await ctx.observe('admin_pause_unconfirmed_lifecycle', () => lifecycleSnapshot(ctx, f.cohort, other(ctx.am)));
    ctx.assert(equal(before, afterRefusal), 'admin_pause_unconfirmed_effect');
    await navigateSettings(session);
    await capture();
    intent = true;
    const [response] = await Promise.all([
      session.page.waitForResponse(r => r.request().method() === 'POST' && apiPath(r.url()) === PAUSE),
      session.page.waitForEvent('dialog').then(async dialog => {
        const valid = dialog.type() === 'confirm' && dialog.message() === CONFIRM;
        if (!valid) await dialog.dismiss();
        ctx.assert(valid, 'admin_pause_confirmation_dialog');
        await dialog.accept();
      }),
      session.page.getByRole('button', {name: 'Pause idle now', exact: true}).click(),
    ]);
    ack = await ctx.observe('admin_pause_ack', async () => ({status: response.status(),
      request_id: await response.headerValue('x-request-id'), body: await response.json()}));
    ctx.assert(ack.status === 200 && ack.request_id?.trim(), 'admin_pause_ack_response');
    ctx.assert(equal(response.request().postDataJSON(), {confirmed: true}), 'admin_pause_confirmation_body');
    ctx.assert(['scheduled', 'already_satisfied', 'skipped'].every(k => Number.isSafeInteger(ack.body?.[k])
      && ack.body[k] >= 0), 'admin_pause_ack_shape');
    const idle = f.cohort.filter(s => s.kind === 'idle').length;
    ctx.assert(ack.body.scheduled === idle && ack.body.already_satisfied === 0
      && ack.body.skipped === f.cohort.length - idle, 'admin_pause_scheduled_wrong_cohort');
    await session.page.getByText(idle + ' Sandboxes scheduled to pause.', {exact: true}).waitFor();
    await capture();
  });
  await ctx.step('step_3', async () => {
    await close();
    const result = await poll(async () => {
      const states = [];
      for (const am of ['a', 'b']) for (const sandbox of f.cohort) {
        const state = (await read(ctx, P + '/sandboxes/' + id(sandbox.id), am)).sandbox;
        ctx.assert(state?.id === sandbox.id, 'admin_pause_terminal_identity');
        if (sandbox.kind !== 'idle') ctx.assert(state.status === 'active', 'admin_pause_protected_work_changed');
        states.push({am, id: sandbox.id, kind: sandbox.kind, status: state.status});
      }
      return ctx.observe('admin_pause_lifecycle_poll', async () => states);
    }, states => states.every(s => s.status === (s.kind === 'idle' ? 'paused' : 'active')), 'admin_pause_lifecycle_timeout');
    terminal = result.every(s => s.status === (s.kind === 'idle' ? 'paused' : 'active'));
    ctx.assert(terminal, 'admin_pause_lifecycle_observed');
  });
  await ctx.step('expected_result', async () => {await finalize(); ctx.assert(terminal, 'admin_pause_finalized');});
}
