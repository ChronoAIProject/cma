import {define, setup, browser, evidence, navigateSettings, read, P, other, id, equal, required} from './common.mjs';

export async function lifecycleSnapshot(ctx, cohort, am = ctx.am) {
  return Promise.all(cohort.map(async sandbox => {
    const detail = await read(ctx, P + '/sandboxes/' + id(sandbox.id), am);
    const events = await read(ctx, P + '/sandboxes/' + id(sandbox.id) + '/events?limit=100', am);
    return {id: sandbox.id, status: detail.sandbox.status,
      lifecycle_operation: detail.sandbox.lifecycle_operation ?? null,
      events: events.map(event => ({seq: event.seq, kind: event.kind, event: event.event}))};
  }));
}

export const idlePreview = define(48, 'browser', 2, async ctx => {
  const f = await setup(ctx, 'idle_preview');
  required(f.exclusiveSettings === true && f.cohort?.length >= 3
    && f.cohort.some(s => s.kind === 'idle') && f.cohort.some(s => s.kind === 'busy')
    && f.cohort.some(s => s.kind === 'always_on'), 'admin_idle_cohort_missing');
  const session = await browser(ctx);
  let before, count;
  await ctx.step('step_1', async () => {
    before = await lifecycleSnapshot(ctx, f.cohort);
    const preview = await read(ctx, P + '/auto-pause/preview');
    count = f.cohort.filter(s => s.kind === 'idle').length;
    ctx.assert(preview.candidates === count, 'admin_preview_eligibility_count');
    const peer = await read(ctx, P + '/auto-pause/preview', other(ctx.am));
    ctx.assert(peer.candidates === count, 'admin_preview_peer_count');
  });
  await ctx.step('step_2', async () => {
    await navigateSettings(session);
    await session.page.getByRole('button', {name: 'Preview idle Sandboxes', exact: true}).click();
    await session.page.getByText(count + ' Sandboxes are candidates.', {exact: true}).waitFor();
    ctx.assert(await session.page.getByRole('button', {name: 'Pause idle now', exact: true}).isEnabled(),
      'admin_preview_not_separate_from_pause');
  });
  await ctx.step('expected_result', async () => {
    const after = await lifecycleSnapshot(ctx, f.cohort, other(ctx.am));
    ctx.assert(equal(before, after), 'admin_preview_caused_lifecycle_effect');
    const capture = await evidence(ctx, session);
    ctx.assert(!capture.requests.some(r => r.method === 'POST'),
      'admin_preview_dispatched_pause');
  });
}, ['admin-fixtures', 'browser']);
