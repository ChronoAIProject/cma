import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { Cma, type UsageMetricReport } from '@cma/client';
import { defaultSelection, metricQuery } from './model';
import { asOf, reportFixture } from './testFixtures';
import { useUsageReport } from './useReport';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});
const id = 'umr_00000000000000000000000000000001';
const owner = { principal: 'owner-1', cacheKey: 'mixed-replica-owner' };
const flush = () => act(async () => {});
const advance = (ms: number) => act(async () => vi.advanceTimersByTimeAsync(ms));
const missingRoute = () =>
  Response.json(
    {
      type: 'https://spec.chrono-ai.fun/cma/1.0/problems/not-found',
      title: 'not found',
      status: 404,
      detail: 'No REST route matches this request path.',
      code: 'not_found',
    },
    { status: 404, headers: { 'Content-Type': 'application/problem+json' } },
  );

function setup(admin = true, range: '24h' | '1h' = '24h', absence = 501, initialStart?: Response) {
  vi.useFakeTimers();
  vi.setSystemTime(asOf * 1000);
  vi.spyOn(Math, 'random').mockReturnValue(0);
  const selection = { ...defaultSelection, range };
  const query = metricQuery(selection, admin);
  const ready: UsageMetricReport = {
    ...reportFixture(admin, id),
    query: { ...reportFixture(admin).query, ...query, filters: reportFixture(admin).query.filters },
    effective_range: { from: query.from!, to: Math.min(query.to!, asOf - 120) },
  };
  const pending: UsageMetricReport = {
    ...ready,
    status: 'pending',
    summary: null,
    report_complete: false,
    retry_after_seconds: 2,
  };
  const view = (report: UsageMetricReport) => ({
    schema_version: 1,
    window: range,
    requested_range: { from: report.query.from, to: report.query.to },
    report,
    generated_at: report.status === 'pending' ? null : report.as_of,
    stale: false,
    refresh: {
      state: report.status === 'pending' ? 'pending' : 'idle',
      report_id: report.status === 'pending' ? report.report_id : null,
      retry_after_seconds: 2,
    },
  });
  const base = `/api/v1/${admin ? 'admin/' : ''}usage/metrics`;
  const calls: { path: string; method: string; at: number }[] = [];
  const initialRead = vi.fn(async () =>
    Response.json(
      {
        status: absence,
        title: 'Route unavailable',
        ...(absence === 501 ? { code: 'report_view_disabled' } : {}),
      },
      { status: absence },
    ),
  );
  const legacyRead = vi.fn(async () => Response.json(ready));
  const transport = vi.fn<typeof fetch>(async (input, options) => {
    const path = new URL(String(input)).pathname;
    calls.push({ path, method: options?.method ?? 'GET', at: Date.now() });
    if (path === `${base}/initial/reports/${id}`) return initialRead();
    if (path === `${base}/reports/${id}`) return legacyRead();
    if (path === `${base}/initial/view` || path === `${base}/initial/reports`) {
      return (
        initialStart ??
        Response.json(path.endsWith('/view') ? { view: view(pending) } : { report: pending }, {
          status: 202,
          headers: { 'Retry-After': '2' },
        })
      );
    }
    if (path === `${base}/view` || path === `${base}/reports`) {
      const replacement = { ...ready, report_id: 'umr_00000000000000000000000000000002' };
      return Response.json(path.endsWith('/view') ? view(replacement) : replacement);
    }
    throw new Error(`Unexpected usage request: ${path}`);
  });
  const client = new Cma({ baseUrl: 'https://cma.invalid', fetch: transport });
  const api = admin ? client.adminUsageMetrics : client.usageMetrics;
  const hook = renderHook(
    ({ active }) => useUsageReport(api, selection, admin, active, owner, false, false, true),
    { initialProps: { active: true } },
  );
  const admissions = () =>
    calls.filter(({ path }) => path.endsWith('/view') || path.endsWith('/reports'));
  return { hook, ready, pending, base, calls, admissions, initialRead, legacyRead };
}

it.each(['1h', '24h'] as const)(
  'starts one legacy request when the old REST router rejects the %s initial route',
  async (range) => {
    const { hook, base, admissions } = setup(false, range, 501, missingRoute());
    await flush();
    expect(hook.result.current.report?.status).toBe('ready');
    const suffix = range === '1h' ? '/reports' : '/view';
    expect(admissions().map(({ path, method }) => [method, path])).toEqual([
      ['POST', base + '/initial' + suffix],
      [range === '1h' ? 'POST' : 'GET', base + suffix],
    ]);
  },
);

it('reads the admitted ID through the legacy route after a REST router refusal', async () => {
  const { hook, initialRead, legacyRead, admissions } = setup(false, '1h');
  initialRead.mockImplementation(async () => missingRoute());
  await advance(2000);
  expect(hook.result.current.report?.report_id).toBe(id);
  expect(legacyRead).toHaveBeenCalledOnce();
  expect(admissions()).toHaveLength(1);
});

it.each(
  [false, true].flatMap((admin) =>
    (['24h', '1h'] as const).flatMap((range) =>
      [501, 404].map((absence) => ({ admin, range, absence })),
    ),
  ),
)('keeps the admitted ID across a $absence replica for admin=$admin range=$range', async (args) => {
  const { hook, calls, base, admissions } = setup(args.admin, args.range, args.absence);
  await flush();
  expect(hook.result.current.pending?.report_id).toBe(id);
  act(() => hook.result.current.refresh());
  await advance(1999);
  expect(calls).toHaveLength(1);
  await advance(1);
  expect(hook.result.current.report?.report_id).toBe(id);
  expect(hook.result.current.initial).toBeUndefined();
  expect(admissions()).toHaveLength(1);
  expect(calls.slice(1).map(({ method, path }) => [method, path])).toEqual([
    ['POST', `${base}/initial/reports/${id}`],
    ['GET', `${base}/reports/${id}`],
  ]);
});

it('keeps the legacy pending ID and retry floor across hiding and resuming', async () => {
  const { hook, pending, ready, admissions, legacyRead } = setup();
  legacyRead.mockResolvedValueOnce(Response.json(pending)).mockResolvedValue(Response.json(ready));
  await advance(2000);
  expect(legacyRead).toHaveBeenCalledTimes(1);
  expect(hook.result.current.pending?.report_id).toBe(id);
  hook.rerender({ active: false });
  await advance(1000);
  hook.rerender({ active: true });
  await advance(999);
  expect(legacyRead).toHaveBeenCalledTimes(1);
  await advance(1);
  expect(hook.result.current.report?.report_id).toBe(id);
  expect(admissions()).toHaveLength(1);
  expect(legacyRead).toHaveBeenCalledTimes(2);
});

it('drops a late fallback response after unmount without starting another report', async () => {
  const { hook, ready, admissions, legacyRead } = setup();
  let finish!: (response: Response) => void;
  legacyRead.mockImplementation(() => new Promise((resolve) => (finish = resolve)));
  await advance(2000);
  expect(legacyRead).toHaveBeenCalledTimes(1);
  hook.unmount();
  await act(async () => finish(Response.json(ready)));
  await advance(3000);
  expect(hook.result.current.report).toBeUndefined();
  expect(admissions()).toHaveLength(1);
});

it.each([
  [401, 'not_authenticated'],
  [403, 'forbidden'],
  [404, 'not_found'],
  [410, 'report_expired'],
  [501, 'different_feature_disabled'],
  [200, 'malformed_success'],
])('does not fall back from an admitted initial report after %s %s', async (status, code) => {
  const { hook, initialRead, legacyRead, admissions } = setup();
  initialRead.mockResolvedValue(
    Response.json(
      {
        status,
        title: 'Refused',
        code,
        type: `https://spec.chrono-ai.fun/cma/1.0/problems/${code}`,
      },
      { status },
    ),
  );
  await advance(2000);
  expect(hook.result.current.error).toBeDefined();
  expect(hook.result.current.report).toBeUndefined();
  expect(legacyRead).not.toHaveBeenCalled();
  expect(admissions()).toHaveLength(1);
});

it.each(['denied', 'missing', 'expired', 'wrong identity', 'wrong principal'])(
  'does not replace an admitted report when the legacy read is %s',
  async (failure) => {
    const { hook, ready, legacyRead, admissions } = setup();
    const status = failure === 'denied' ? 403 : failure === 'missing' ? 404 : 410;
    legacyRead.mockResolvedValue(
      failure === 'wrong identity' || failure === 'wrong principal'
        ? Response.json({
            ...ready,
            ...(failure === 'wrong identity'
              ? { report_id: 'umr_00000000000000000000000000000002' }
              : { scope: { ...ready.scope, principal: 'another-owner' } }),
          })
        : Response.json({ status, title: 'Unavailable', code: failure }, { status }),
    );
    await advance(2000);
    expect(legacyRead).toHaveBeenCalledTimes(1);
    expect(hook.result.current.error).toBeDefined();
    expect(hook.result.current.report).toBeUndefined();
    expect(admissions()).toHaveLength(1);
  },
);
