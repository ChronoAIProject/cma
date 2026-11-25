// @vitest-environment node
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import { ApiError, Cmaeg, RequestAborted } from '../index.js';
import {
  CoverageGapSchema,
  HistoryCoverageSchema,
  HistoryDetailPartSchema,
  HistoryDetailsSchema,
  HistoryItemSchema,
  HistoryPageSchema,
  HistoryRangeSchema,
} from '../cmaeg/types.js';

const examples = JSON.parse(
  readFileSync(
    new URL('../../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json', import.meta.url),
    'utf8',
  ),
) as Record<string, readonly unknown[]>;

describe('retained activity history contracts', () => {
  it.each([
    ['CoverageGap', CoverageGapSchema],
    ['HistoryCoverage', HistoryCoverageSchema],
    ['HistoryDetailPart', HistoryDetailPartSchema],
    ['HistoryDetails', HistoryDetailsSchema],
    ['HistoryItem', HistoryItemSchema],
    ['HistoryPage', HistoryPageSchema],
    ['HistoryRange', HistoryRangeSchema],
  ] as const)('preserves every %s fixture including nullable and omitted fields', (name, parse) => {
    expect(examples[name]?.length).toBeGreaterThan(0);
    for (const example of examples[name] ?? []) expect(parse(example)).toEqual(example);
  });

  it('keeps continuation, terminal and empty pages distinct without claiming full coverage', () => {
    const pages = examples.HistoryPage!.map(HistoryPageSchema);
    expect(pages.map((page) => page.next_cursor)).toEqual(['fixture-next-page', null, undefined]);
    expect(pages.map((page) => page.has_more)).toEqual([true, false, false]);
    expect(pages[2]?.items).toEqual([]);
    expect(pages[1]?.coverage.complete_event_enumeration).toBe(false);
    expect(pages[1]?.coverage.gaps).toEqual([
      'legacy_partial',
      'retention_expired',
      'unreadable_record',
      'capture_gap',
    ]);
  });

  it('retains detail fragment bytes, order and integrity metadata', () => {
    const pages = examples.HistoryDetails!.map(HistoryDetailsSchema);
    const parts = pages.slice(0, 2).flatMap((page) => page.parts);
    expect(parts.map((part) => part.index)).toEqual([0, 1]);
    for (const part of parts)
      expect(createHash('sha256').update(Buffer.from(part.data, 'base64')).digest('hex')).toBe(
        part.sha256,
      );
    const decoded: unknown = JSON.parse(
      Buffer.concat(parts.map((part) => Buffer.from(part.data, 'base64'))).toString('utf8'),
    );
    expect(decoded).toEqual(HistoryItemSchema(examples.HistoryItem![0]).activity);
    expect(pages[2]?.detail_incomplete).toBe(true);
    expect(pages[2]?.detail_truncated).toBe(true);
  });

  it('rejects malformed nested receipts, coverage, cursors and fragment positions', () => {
    const page = HistoryPageSchema(examples.HistoryPage![0]);
    expect(() => HistoryPageSchema({ ...page, next_cursor: 1 })).toThrow('next_cursor');
    expect(() =>
      HistoryPageSchema({ ...page, items: [{ ...page.items[0], activity: {} }] }),
    ).toThrow('activity');
    expect(() =>
      HistoryPageSchema({ ...page, coverage: { ...page.coverage, gaps: ['unknown'] } }),
    ).toThrow('gaps');
    const details = HistoryDetailsSchema(examples.HistoryDetails![0]);
    for (const index of [-1, 0.5, Number.MAX_SAFE_INTEGER + 1]) {
      expect(() =>
        HistoryDetailsSchema({ ...details, parts: [{ ...details.parts[0], index }] }),
      ).toThrow('index');
    }
  });
});

describe('gateway retained Activity reads', () => {
  it('reads 55 unique receipts and their requested details across explicit pages', async () => {
    const template = HistoryPageSchema(examples.HistoryPage![1]);
    const { range, coverage } = template;
    const receipt = HistoryItemSchema(examples.HistoryItem![0]);
    const historyEventId = 'retained/+?event';
    const detailCursor = 'detail/+?start=';
    const detailedReceipt = {
      ...receipt,
      history_event_id: historyEventId,
      detail_cursor: detailCursor,
      detail_incomplete: true,
      detail_truncated: true,
    };
    const items = [
      detailedReceipt,
      ...Array.from({ length: 54 }, (_, index) => ({
        ...receipt,
        history_event_id: `retained-${index}`,
        activity: { ...receipt.activity, event_id: `receipt-${index}` },
        detail_cursor: null,
      })),
    ];
    const firstCursor = 'page/+?one=';
    const secondCursor = 'page/+?two=';
    const first = {
      items: items.slice(0, 20),
      range,
      coverage,
      has_more: true,
      next_cursor: firstCursor,
    };
    const second = { ...first, items: items.slice(20, 40), next_cursor: secondCursor };
    const last = { ...first, items: items.slice(40), has_more: false, next_cursor: null };
    const nextDetailCursor = 'detail/+?next=';
    const firstDetail = {
      ...HistoryDetailsSchema(examples.HistoryDetails![0]),
      detail_incomplete: true,
      detail_truncated: true,
      next_cursor: nextDetailCursor,
    };
    const lastDetail = {
      ...HistoryDetailsSchema(examples.HistoryDetails![1]),
      detail_incomplete: true,
      detail_truncated: true,
    };
    const fetch = vi.fn<typeof globalThis.fetch>();
    for (const page of [first, second, last, firstDetail, lastDetail]) {
      fetch.mockResolvedValueOnce(Response.json(page));
    }
    const gateway = new Cmaeg({
      baseUrl: 'https://gateway.example/prefix',
      credential: () => 'creator',
      fetch,
    });
    const channel = 'channel/+?id';
    const owner = 'org-one';
    const limit = 20;

    expect(fetch).not.toHaveBeenCalled();
    const head = await gateway.getChannelActivityHistory(channel, { owner, ...range, limit });
    expect(head).toEqual(first);
    expect(fetch).toHaveBeenCalledTimes(1);
    const middle = await gateway.getChannelActivityHistory(channel, {
      owner,
      cursor: head.next_cursor!,
    });
    expect(middle).toEqual(second);
    const tail = await gateway.getChannelActivityHistory(channel, {
      owner,
      ...range,
      limit,
      cursor: middle.next_cursor!,
    });
    expect(tail).toEqual(last);
    const identities = [head, middle, tail].flatMap((page) =>
      page.items.map((item) => item.activity.event_id),
    );
    expect(identities).toEqual(items.map((item) => item.activity.event_id));
    expect(new Set(identities).size).toBe(55);
    expect(head.coverage.complete_event_enumeration).toBe(false);
    expect(head.coverage.gaps).toContain('capture_gap');

    const item = head.items[0]!;
    const detail = await gateway.getChannelActivityHistoryDetails(channel, item.history_event_id, {
      owner,
      cursor: item.detail_cursor!,
    });
    expect(detail).toEqual(firstDetail);
    expect(fetch).toHaveBeenCalledTimes(4);
    expect(
      await gateway.getChannelActivityHistoryDetails(channel, item.history_event_id, {
        owner,
        cursor: detail.next_cursor!,
      }),
    ).toEqual(lastDetail);

    const history =
      'https://gateway.example/prefix/v1/channels/channel%2F%2B%3Fid/activity/history';
    const details = `${history}/events/retained%2F%2B%3Fevent/details`;
    const rangeQuery = 'from=2026-10-27T00%3A00%3A00Z&until=2026-10-28T00%3A00%3A00Z&limit=20';
    expect(fetch.mock.calls.map(([url]) => url)).toEqual([
      `${history}?${rangeQuery}`,
      `${history}?cursor=page%2F%2B%3Fone%3D`,
      `${history}?${rangeQuery}&cursor=page%2F%2B%3Ftwo%3D`,
      `${details}?cursor=detail%2F%2B%3Fstart%3D`,
      `${details}?cursor=detail%2F%2B%3Fnext%3D`,
    ]);
    for (const [, init] of fetch.mock.calls) {
      const headers = new Headers(init?.headers);
      expect(init?.method).toBe('GET');
      expect(headers.get('x-cmaeg-owner')).toBe(owner);
      expect(headers.get('authorization')).toBe('Bearer creator');
    }
  });

  it.each([
    [410, 'activity_history_cursor_expired'],
    [503, 'activity_history_disabled'],
  ] as const)('surfaces the %i history problem after one request', async (status, code) => {
    const fetch = vi
      .fn<typeof globalThis.fetch>()
      .mockResolvedValueOnce(
        Response.json(
          { type: `urn:cmaeg:problem:${code}`, title: code, status, code },
          { status, headers: { 'content-type': 'application/problem+json' } },
        ),
      );
    const gateway = new Cmaeg({ baseUrl: 'https://gateway.example', fetch });

    await expect(
      gateway.getChannelActivityHistory('channel', { cursor: 'opaque' }),
    ).rejects.toEqual(expect.objectContaining({ status, code, name: ApiError.name }));
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it('passes cancellation to the detail GET and surfaces an aborted request', async () => {
    const controller = new AbortController();
    const fetch = vi.fn<typeof globalThis.fetch>().mockImplementation((_url, init) => {
      controller.abort();
      expect(init?.signal?.aborted).toBe(true);
      return new Promise(() => {});
    });
    const gateway = new Cmaeg({ baseUrl: 'https://gateway.example', fetch });

    await expect(
      gateway.getChannelActivityHistoryDetails('channel', 'retained-event', {
        cursor: 'opaque',
        signal: controller.signal,
      }),
    ).rejects.toBeInstanceOf(RequestAborted);
    expect(fetch).toHaveBeenCalledTimes(1);
  });
});
