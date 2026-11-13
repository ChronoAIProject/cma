import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { Cma, type UsageMetricQuery } from '@chronoai/cma-sdk';
import { defaultSelection, metricQuery, type Range, type Selection } from './model';
import { reportFixture } from './testFixtures';
import { useUsageReport } from './useReport';
import type { LatestMetricsClient } from './latest';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});
const owner = { principal: 'realm:alice', cacheKey: 'sdk-test-epoch' };
const ranges: Range[] = ['24h', '7d', '30d', 'custom', '1h'];
const from = '2026-11-01T00:00:00Z',
  to = '2026-11-02T00:00:00Z';
function snapshot(admin: boolean, selection: Selection) {
  const report = reportFixture(admin);
  const query = metricQuery(selection, admin);
  return {
    ...report,
    scope: { principal: owner.principal, admin },
    query: {
      ...report.query,
      ...query,
      filters: {
        ...report.query.filters,
        ...Object.fromEntries(
          Object.entries(query.filters ?? {}).filter(([, value]) => value !== undefined),
        ),
      },
    },
  };
}
function client(admin: boolean, fetch: typeof globalThis.fetch) {
  const sdk = new Cma({ baseUrl: 'https://cma.invalid', fetch });
  return (admin ? sdk.adminUsageMetrics : sdk.usageMetrics) as LatestMetricsClient;
}

it.each([false, true].flatMap((admin) => ranges.map((range) => ({ admin, range }))))(
  'uses the real SDK for admin=$admin range=$range across merge order',
  async ({ admin, range }) => {
    const selection = { ...defaultSelection, range, from, to };
    const report = snapshot(admin, selection);
    const transport = vi.fn<typeof fetch>(async (input, init): Promise<Response> => {
      const url = new URL(String(input));
      expect(url.pathname).toBe(
        `/api/v1/${admin ? 'admin/' : ''}usage/metrics/${url.pathname.endsWith('/view') ? 'view' : 'reports'}`,
      );
      if (url.pathname.endsWith('/view')) {
        expect(init?.method ?? 'GET').toBe('GET');
        expect(url.searchParams.get('window')).toBe(range);
        expect(url.searchParams.has('from')).toBe(range === 'custom');
        expect(url.searchParams.has('to')).toBe(range === 'custom');
        expect(url.searchParams.get('refresh')).toBe('true');
        return Response.json({
          schema_version: 1,
          window: range,
          requested_range: { from: report.query.from, to: report.query.to },
          report,
          generated_at: report.as_of,
          stale: false,
          refresh: { state: 'idle', report_id: null, retry_after_seconds: 0 },
        });
      }
      expect(init?.method).toBe('POST');
      const query = JSON.parse(String(init?.body)) as UsageMetricQuery;
      expect(query.from).toBe(report.query.from);
      expect(query.to).toBe(report.query.to);
      return Response.json(report);
    });
    const api = client(admin, transport);
    const hook = renderHook(() => useUsageReport(api, selection, admin, true, owner));
    await waitFor(() => expect(hook.result.current.report?.report_id).toBe(report.report_id));
    expect(transport).toHaveBeenCalledTimes(1);
    const requestedView = new URL(String(transport.mock.calls[0]![0])).pathname.endsWith('/view');
    expect(requestedView).toBe(Boolean(api.latest) && range !== '1h');
  },
);

it.each([
  {
    status: 501,
    body: { status: 501, title: 'Disabled', code: 'report_view_disabled' },
    fallback: true,
  },
  { status: 404, body: { status: 404, title: 'Not found' }, fallback: true },
  { status: 404, body: {}, fallback: false },
  { status: 404, body: { status: 404, title: 'Refused', code: 'not_found' }, fallback: false },
  { status: 404, body: { status: 404, title: 'Refused', resource: 'selected' }, fallback: false },
  { status: 401, body: { status: 401, title: 'Refused' }, fallback: false },
  { status: 403, body: { status: 403, title: 'Refused' }, fallback: false },
  { status: 503, body: { status: 503, title: 'Unavailable' }, fallback: false },
  { status: 200, body: { report: {} }, fallback: false },
])(
  'keeps SDK $status refusal/fallback boundaries (fallback=$fallback)',
  async ({ status, body, fallback }) => {
    const report = snapshot(true, defaultSelection);
    const transport = vi.fn<typeof fetch>(async (input): Promise<Response> => {
      const url = new URL(String(input));
      // The current legacy SDK has no latest method: still verify that refusals
      // propagate. The same test is also run with the frozen retained-view SDK.
      if (transport.mock.calls.length > 1 && url.pathname.endsWith('/reports'))
        return Response.json(report);
      return Response.json(body, { status });
    });
    const api = client(true, transport);
    const hook = renderHook(() => useUsageReport(api, defaultSelection, true, true, owner));
    const shouldFallback = Boolean(api.latest) && fallback;
    await waitFor(() => {
      if (shouldFallback) expect(hook.result.current.report?.report_id).toBe(report.report_id);
      else expect(hook.result.current.error).toBeDefined();
    });
    expect(transport).toHaveBeenCalledTimes(shouldFallback ? 2 : 1);
    if (!shouldFallback) expect(hook.result.current.report).toBeUndefined();
  },
);

it('polls pending through the real SDK without overlapping requests', async () => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
  const ready = snapshot(true, defaultSelection);
  const transport = vi.fn<typeof fetch>(async (input): Promise<Response> => {
    const first = transport.mock.calls.length === 1;
    const report = first
      ? { ...ready, status: 'pending', summary: null, report_complete: false }
      : ready;
    if (new URL(String(input)).pathname.endsWith('/view'))
      return Response.json(
        {
          schema_version: 1,
          window: '24h',
          requested_range: { from: ready.query.from, to: ready.query.to },
          report,
          generated_at: first ? null : ready.as_of,
          stale: false,
          refresh: {
            state: first ? 'pending' : 'idle',
            report_id: first ? ready.report_id : null,
            retry_after_seconds: 1,
          },
        },
        { status: first ? 202 : 200 },
      );
    return Response.json(report, { status: first ? 202 : 200 });
  });
  const api = client(true, transport);
  const hook = renderHook(() => useUsageReport(api, defaultSelection, true, true, owner));
  await act(async () => {});
  expect(hook.result.current.pending).toBeDefined();
  act(() => hook.result.current.refresh());
  await act(async () => vi.advanceTimersByTimeAsync(999));
  expect(transport).toHaveBeenCalledTimes(1);
  await act(async () => vi.advanceTimersByTimeAsync(1));
  expect(transport).toHaveBeenCalledTimes(2);
  expect(hook.result.current.report?.report_id).toBe(ready.report_id);
});
