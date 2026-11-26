// @vitest-environment node
import { describe, expect, it, vi } from 'vitest';
import {
  createClient,
  isUsageViewUnavailable,
  type UsageInitialCreate,
  type UsageInitialProjection,
} from '../index.js';
import { ApiError, ProtocolError, RequestAborted, ValidationError } from '../errors.js';
import { loadCorpus } from './public-contract/context.js';
const corpus = loadCorpus();
const ready = corpus.find((entry) => entry.id === 'initial-owner-create-ready')!;
const pending = corpus.find((entry) => entry.id === 'initial-owner-read-pending')!;
const input = ready.request.body as UsageInitialCreate;
const id = 'umr_00000000000000000000000000000001';
function set(value: unknown, path: string, replacement: unknown, remove = false) {
  const keys = path.split('/');
  let current = value as Record<string, unknown>;
  for (const key of keys.slice(0, -1)) current = current[key] as Record<string, unknown>;
  if (remove) delete current[keys.at(-1)!];
  else current[keys.at(-1)!] = replacement;
}
function setup(value: unknown, status = 200) {
  const fetch = vi.fn(
    async () =>
      new Response(JSON.stringify(value), {
        status,
        headers: { 'content-type': 'application/json' },
      }),
  );
  const observer = vi.fn();
  const client = createClient({
    baseUrl: 'https://example.test',
    fetch,
    responseObserver: observer,
  });
  return { client, fetch, observer };
}

describe('bounded initial report client', () => {
  it('returns complete parsed data and preserves the response observer', async () => {
    const { client, fetch, observer } = setup(ready.response.body);
    const value = await client.usageMetrics.createInitial(input);
    expect(value).toEqual(ready.response.body);
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(observer).toHaveBeenCalledWith(expect.objectContaining({ status: 200 }));
  });
  it.each([
    ['report/schema_version', 2],
    ['report/report_id', 'other'],
    ['report/scope/admin', true],
    ['report/status', 'pending'],
    ['report/report_complete', false],
    ['report/summary', null],
    ['report/query/from', 900],
    ['report/query/step_seconds', 0],
    ['report/query/filters/owner', 'foreign'],
    ['report/effective_range/to', 1059],
    ['report/expires_at', 1000],
    ['report/unknown', true],
    ['initial/projection/first_page/limit', 49],
    ['initial/projection/first_page/cursor', 'next'],
    ['initial/rows/report_id', 'other'],
    ['initial/rows/as_of', 1179],
    ['initial/rows/items', []],
    ['initial/rows/total_count', 2],
    ['initial/rows/next', 'unrequested-page'],
    ['initial/rows/items/0/owner/subject', 'foreign'],
    ['initial/rows/items/0/unknown', true],
    ['initial/charts', []],
    ['initial/charts/0/report_id', 'other'],
    ['initial/charts/0/metric', 'disk'],
    ['initial/charts/0/unit', 'bytes'],
    ['initial/charts/0/source', 'other'],
    ['initial/charts/0/lines/0/id', 'owner'],
    ['initial/charts/0/lines/0/points', []],
    ['initial/charts/0/lines/0/points/0/to', 1019],
    ['initial/charts/0/lines/0/points/0/unknown', true],
    ['unknown', true],
  ])('rejects mismatched or malformed response %s', async (path, replacement) => {
    const value = structuredClone(ready.response.body);
    set(value, path as string, replacement);
    const { client, fetch } = setup(value);
    await expect(client.usageMetrics.createInitial(input)).rejects.toBeInstanceOf(ProtocolError);
    expect(fetch).toHaveBeenCalledTimes(1);
  });
  it('rejects missing children and a success status inconsistent with pending', async () => {
    const value = structuredClone(ready.response.body);
    set(value, 'initial', undefined, true);
    await expect(setup(value).client.usageMetrics.createInitial(input)).rejects.toBeInstanceOf(
      ProtocolError,
    );
    await expect(
      setup(ready.response.body, 202).client.usageMetrics.createInitial(input),
    ).rejects.toBeInstanceOf(ProtocolError);
    await expect(
      setup(pending.response.body, 200).client.usageMetrics.initialReport(id, input.projection),
    ).rejects.toBeInstanceOf(ProtocolError);
    const result = await setup(pending.response.body, 202).client.usageMetrics.initialReport(
      id,
      input.projection,
    );
    expect(result.report.report_id).toBe(id);
    expect(result.initial).toBeUndefined();
  });
  it('binds reads to the supplied report ID', async () => {
    const { client } = setup(ready.response.body);
    await expect(
      client.usageMetrics.initialReport('umr_00000000000000000000000000000002', input.projection),
    ).rejects.toBeInstanceOf(ProtocolError);
  });
  it.each([
    { first_page: { limit: 0 } },
    { first_page: { limit: 51 } },
    { first_page: { cursor: 'next' } },
    { total_metrics: ['cpu', 'cpu'] },
    { total_metrics: ['unknown'] },
    { group_by: 'owner' },
    { first_page: { sort: 'owner' } },
  ])('rejects unsupported local projection before fetch: %j', async (projection) => {
    const { client, fetch } = setup(ready.response.body);
    await expect(
      client.usageMetrics.createInitial({
        ...input,
        projection: projection as UsageInitialProjection,
      }),
    ).rejects.toBeInstanceOf(ValidationError);
    expect(fetch).not.toHaveBeenCalled();
  });
  it('captures the requested selection before waiting for transport', async () => {
    const captured = structuredClone(input);
    const { client } = setup(ready.response.body);
    const result = client.usageMetrics.createInitial(captured);
    set(captured, 'query/from', 123);
    set(captured, 'projection/total_metrics', []);
    await expect(result).resolves.toEqual(ready.response.body);
  });
  it.each([
    [401, 'not_authenticated'],
    [403, 'forbidden'],
    [404, 'not_found'],
    [503, 'usage_unavailable'],
    [501, 'report_view_disabled'],
  ])('preserves typed %s without automatic fallback', async (status, code) => {
    const { client, fetch } = setup(
      { type: 'about:blank', title: 'refused', status, code },
      status as number,
    );
    const error = await client.usageMetrics.createInitial(input).catch((error) => error);
    expect(error).toBeInstanceOf(ApiError);
    expect(isUsageViewUnavailable(error)).toBe(code === 'report_view_disabled');
    expect(fetch).toHaveBeenCalledTimes(1);
  });
  it('honors cancellation before any request', async () => {
    const { client, fetch } = setup(ready.response.body);
    const controller = new AbortController();
    controller.abort();
    await expect(
      client.usageMetrics.createInitial(input, { signal: controller.signal }),
    ).rejects.toBeInstanceOf(RequestAborted);
    expect(fetch).not.toHaveBeenCalled();
  });
});
