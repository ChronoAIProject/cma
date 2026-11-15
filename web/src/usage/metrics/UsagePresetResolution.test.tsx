import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { Cma } from '@chronoai/cma-sdk';
import { defaultSelection, metricQuery } from './model';
import { reportFixture } from './testFixtures';
import { useUsageReport } from './useReport';

afterEach(() => vi.useRealTimers());
it.each([0, 1, 2, 3, 4])(
  'accepts a rolling seven-day preset at minute offset %s with the real SDK',
  async (minute) => {
    vi.useFakeTimers();
    vi.setSystemTime(Date.parse('2026-11-14T07:30:00Z') + minute * 60000);
    const selection = { ...defaultSelection, range: '7d' as const };
    const explicit = metricQuery(selection, false);
    expect(explicit.step_seconds).toBe(300);
    const end = Math.floor(Date.now() / 60000) * 60;
    const begin = end - 7 * 86400;
    const buckets = (step: number) => Math.ceil(end / step) - Math.floor(begin / step);
    expect(buckets(300)).toBe(minute === 0 ? 2016 : 2017);
    const transport = vi.fn<typeof fetch>(async (input) => {
      const url = new URL(String(input));
      expect(url.pathname).toBe('/api/v1/usage/metrics/view');
      expect(url.searchParams.get('window')).toBe('7d');
      for (const key of ['from', 'to', 'step_seconds'])
        expect(url.searchParams.has(key)).toBe(false);
      // The public server chooses 600 seconds when the preset has no explicit step.
      const report = {
        ...reportFixture(),
        query: { ...reportFixture().query, from: begin, to: end, step_seconds: 600 },
        effective_range: { from: begin, to: end },
      };
      expect(buckets(600)).toBeLessThanOrEqual(2016);
      return Response.json({
        schema_version: 1,
        window: '7d',
        requested_range: { from: begin, to: end },
        report,
        generated_at: report.as_of,
        stale: false,
        refresh: { state: 'idle', report_id: null, retry_after_seconds: 0 },
      });
    });
    const sdk = new Cma({ baseUrl: 'https://cma.invalid', fetch: transport });
    const hook = renderHook(() => useUsageReport(sdk.usageMetrics, selection, false));
    await act(async () => {});
    expect(transport).toHaveBeenCalledTimes(1);
    expect(hook.result.current.error).toBeUndefined();
    expect(hook.result.current.report?.query.step_seconds).toBe(600);
  },
);
