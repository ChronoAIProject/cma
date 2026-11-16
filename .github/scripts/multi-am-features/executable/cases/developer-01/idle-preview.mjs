import {define, setup, browser, navigateSettings, read, P, other, id, equal, required, check, apiPath} from './common.mjs';
import {operationsPrefetch} from './operations-prefetch.mjs';

const EVENT_LIMIT = 200, MAX_EVENT_PAGES = 20;

async function lifecycleEvents(ctx, sandboxId, am) {
  const events = []; let after = 0;
  for (let page = 0; page < MAX_EVENT_PAGES; page++) {
    const rows = await read(ctx, P + '/sandboxes/' + id(sandboxId) + '/events?'
      + new URLSearchParams({after: String(after), limit: String(EVENT_LIMIT)}), am);
    check(Array.isArray(rows) && rows.length <= EVENT_LIMIT, 'admin_idle_event_page');
    for (const event of rows) {
      check(event?.sandbox_id === sandboxId && Number.isFinite(Date.parse(event.appended_at))
        && typeof event.kind?.type === 'string', 'admin_idle_event_identity');
      check(Number.isSafeInteger(event.seq) && event.seq === after + 1, 'admin_idle_event_sequence');
      events.push(event); after = event.seq;
    }
    if (rows.length < EVENT_LIMIT) return events;
  }
  throw Error('admin_idle_event_bound');
}

export async function lifecycleSnapshot(ctx, cohort, am = ctx.am) {
  const snapshot = [];
  // Bounded sequential reads avoid leaving detached page requests on failure.
  for (const sandbox of cohort) {
    const detail = (await read(ctx, P + '/sandboxes/' + id(sandbox.id), am)).sandbox;
    check(detail?.id === sandbox.id, 'admin_idle_detail_identity');
    check(typeof detail.status === 'string' && detail.lifecycle && typeof detail.lifecycle === 'object'
      && !Array.isArray(detail.lifecycle) && detail.work && typeof detail.work === 'object'
      && !Array.isArray(detail.work), 'admin_idle_detail_shape');
    snapshot.push({id: sandbox.id, status: detail.status,
      lifecycle: structuredClone(detail.lifecycle), work: structuredClone(detail.work),
      always_on: detail.profile?.always_on ?? null, events: await lifecycleEvents(ctx, sandbox.id, am)});
  }
  return snapshot;
}

export const idlePreview = define(48, 'browser', 2, async ctx => {
  const f = await setup(ctx, 'idle_preview');
  required(f.exclusiveSettings === true && Array.isArray(f.cohort) && f.cohort.length >= 3
    && f.cohort.every(s => typeof s?.id === 'string' && ['idle', 'busy', 'always_on'].includes(s.kind))
    && new Set(f.cohort.map(s => s.id)).size === f.cohort.length
    && ['idle', 'busy', 'always_on'].every(kind => f.cohort.some(s => s.kind === kind)), 'admin_idle_cohort_missing');
  const session = await browser(ctx), reports = operationsPrefetch(ctx, session);
  let before, count, finalityJob;
  const readOnly = async (final = false) => {
    const capture = await reports.capture('admin_browser_evidence', final);
    required(capture.authentication === 'browser_session', 'admin_preview_cookie_session_required');
    check(capture.requests.every(r => ['GET', 'HEAD', 'OPTIONS'].includes(r.method)),
      'admin_preview_browser_mutation');
    return capture;
  };
  // Drain before comparing durable state. Always collect evidence and lifecycle
  // reads, even after close failure; retain every failure without another click.
  const finalize = () => finalityJob ??= (async () => {
    const errors = [];
    try {await session.close();} catch (error) {errors.push(error);}
    try {await readOnly(true);} catch (error) {errors.push(error);}
    if (before) {
      try {
        const after = await ctx.observe('admin_idle_lifecycle_after', () => lifecycleSnapshot(ctx, f.cohort, other(ctx.am)));
        check(equal(before, after), 'admin_preview_caused_lifecycle_effect');
      } catch (error) {errors.push(error);}
    }
    if (errors.length) throw new AggregateError(errors, 'admin_idle_preview_finality_failed');
  })();
  ctx.defer('admin_idle_preview_finality', finalize);
  await ctx.step('step_1', async () => {
    before = await ctx.observe('admin_idle_lifecycle_before', () => lifecycleSnapshot(ctx, f.cohort));
    const preview = await read(ctx, P + '/auto-pause/preview');
    count = f.cohort.filter(s => s.kind === 'idle').length;
    ctx.assert(preview.candidates === count, 'admin_preview_eligibility_count');
    const peer = await read(ctx, P + '/auto-pause/preview', other(ctx.am));
    ctx.assert(peer.candidates === count, 'admin_preview_peer_count');
  });
  await ctx.step('step_2', async () => {
    await navigateSettings(session);
    await readOnly();
    const selectedAM = other(ctx.am);
    await session.switchAM(selectedAM);
    const [response] = await Promise.all([
      session.page.waitForResponse(r => r.request().method() === 'GET' && apiPath(r.url()) === P + '/auto-pause/preview'),
      session.page.getByRole('button', {name: 'Preview idle Sandboxes', exact: true}).click(),
    ]);
    const requestId = await response.headerValue('x-request-id');
    ctx.assert(response.status() === 200 && requestId?.trim(), 'admin_preview_browser_response');
    ctx.assert((await response.json()).candidates === count, 'admin_preview_browser_count');
    await session.page.getByText(count + ' Sandboxes are candidates.', {exact: true}).waitFor();
    const capture = await readOnly();
    ctx.assert(capture.requests.some(r => r.request_id === requestId && r.method === 'GET'
      && r.status === 200 && r.am === selectedAM && r.serving_am === selectedAM), 'admin_preview_browser_ingress');
    ctx.assert(await session.page.getByRole('button', {name: 'Pause idle now', exact: true}).isEnabled(),
      'admin_preview_not_separate_from_pause');
  });
  await ctx.step('expected_result', async () => {await finalize(); ctx.assert(before != null, 'admin_preview_finalized');});
}, ['admin-fixtures', 'browser']);
