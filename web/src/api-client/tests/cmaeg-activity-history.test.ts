// @vitest-environment node
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
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
