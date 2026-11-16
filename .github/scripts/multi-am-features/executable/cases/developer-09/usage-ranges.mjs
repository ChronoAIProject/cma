import {define, take, open, goto, pick, captureUsage, committed, completeReport, equal} from './helpers.mjs';
const ranges = [[10, '24h', 'Last 24 hours'], [11, '7d', 'Last 7 days'],
  [12, '30d', 'Last 30 days'], [13, '1h', 'Last hour'], [14, 'custom', 'Custom (up to 30 days)']];
export const cases = ranges.map(([number, range, label]) => define(number,
  ['browser', 'admin-fixtures'], async ctx => {
    const f = await take(ctx, 'usage'); const session = await open(ctx);
    const capture = captureUsage(session); ctx.defer('close_usage_capture', () => capture.close());
    let report, start;
    await ctx.step('step_1', async () => {
      await goto(session, '/operations'); await pick(session.page, 'Usage time range', label);
      if (range === 'custom') {
        await session.page.getByLabel('Usage range from', {exact: true}).fill(f.custom.from_local);
        await session.page.getByLabel('Usage range to', {exact: true}).fill(f.custom.to_local);
        ctx.assert(Date.parse(f.custom.to) - Date.parse(f.custom.from) > 0
          && Date.parse(f.custom.to) - Date.parse(f.custom.from) <= 30 * 86400000, 'custom_fixture_range');
      }
      ctx.assert((await session.page.getByRole('button', {name: 'Usage time range', exact: true}).innerText()).includes(label), 'range_selection');
    });
    await ctx.step('step_2', async () => {
      start = performance.now();
      await session.page.getByRole('button', {name: 'Apply usage filters', exact: true}).click();
      report = await committed(ctx, session, range, capture);
      ctx.assert(completeReport(report, f.ranges[range], f.principal), 'authorized_range_totals');
      const displayed = await session.page.locator('.usage-resource-card > dd').allTextContents();
      const values = ['cpu_core_hours', 'memory_gib_hours', 'disk_gib_hours'].map(k => Number(report.summary.allocated_period[k]));
      ctx.assert(displayed.length === 3 && displayed.every((v, i) => Math.abs(Number(v.replaceAll(',', '')) - values[i]) <= 0.001), 'rendered_totals_mismatch');
    });
    await ctx.step('step_3', async () => {
      const requests = await capture.flush();
      const correlated = requests.filter(x => x.body?.report_id === report.report_id || x.body?.report?.report_id === report.report_id);
      ctx.assert(correlated.length > 0 && correlated.every(x => x.request_id), 'report_request_join');
      const key = `${ctx.row.case_id}:${ctx.phase}`;
      const samples = ctx.runState[key] ??= [];
      ctx.assert(!samples.some(s => s.iteration === ctx.iteration), 'duplicate_usage_iteration');
      const prior = samples.some(s => s.report_id === report.report_id);
      const sample = {iteration: ctx.iteration, report_id: report.report_id, elapsed_ms: performance.now() - start,
        cache_evidence: prior ? 'observed_reused_report' : 'no_previous_report_observed', requests: correlated.map(x => x.request_id)};
      samples.push(sample);
      await ctx.observe('usage_sample', async () => sample);
      ctx.assert(samples.length === ctx.iteration, 'usage_repetition_gap');
      // The parent supplies N=3 before restart and N=1 after; no hidden extra workload.
      if (ctx.iteration === 3) ctx.assert(samples.length === 3, 'usage_three_samples');
    });
    await ctx.step('expected_result', async () => {
      ctx.assert(completeReport(report, f.ranges[range], f.principal), 'final_report_not_complete');
      ctx.assert(equal(report.query.filters?.statuses ?? [], []), 'unexpected_status_scope');
      if (range === 'custom') ctx.assert(report.query.from === Math.floor(Date.parse(f.custom.from) / 1000)
        && report.query.to === Math.floor(Date.parse(f.custom.to) / 1000), 'custom_query_scope');
      else ctx.assert(report.query.to - report.query.from === {'1h': 3600, '24h': 86400, '7d': 604800, '30d': 2592000}[range], 'preset_query_scope');
      ctx.assert(await session.page.locator('[data-usage-state="preview"], [data-usage-state="error"]').count() === 0, 'preview_or_error_claimed_complete');
    });
  }));
