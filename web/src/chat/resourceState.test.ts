import { describe, expect, it } from 'vitest';
import type { AguiEvent, JsonPatch } from '../agui';
import { PATHS } from '../sdk/wire';
import { EMPTY, reduce } from './conversation';

function delta(seq: number, patch: JsonPatch): AguiEvent {
  return {
    type: 'STATE_DELTA',
    seq,
    threadId: 'sesn_1',
    runId: 'run_1',
    patch: [patch],
  };
}

const READING = {
  cpuMillisUsed: null,
  cpuMillisLimit: 1000,
  memoryBytesUsed: 892_211_200,
  memoryBytesLimit: null,
  memoryBytesPeak: 930_086_912,
  stallPermille: 12,
  ceilingHits: 0,
  allocationFailures: 0,
  kills: 0,
  receivedBytes: 18_442,
  sentBytes: 9_120,
  receivedBytesPerSecond: null,
  sentBytesPerSecond: null,
  intervalMs: null,
} as const;

describe('a whole resource replacement', () => {
  it('carries all fourteen figures and tells null from a real zero', () => {
    const state = reduce(EMPTY, delta(0, { op: 'replace', path: PATHS.resources, value: READING }));

    expect(Object.keys(state.resources ?? {})).toHaveLength(14);
    expect(state.resources).toMatchObject({
      cpuMillisUsed: undefined,
      memoryBytesLimit: undefined,
      ceilingHits: 0,
      allocationFailures: 0,
      kills: 0,
      memoryBytesUsed: 892_211_200,
    });
  });

  it('replaces rather than merging two readings', () => {
    const first = reduce(EMPTY, delta(0, { op: 'replace', path: PATHS.resources, value: READING }));
    const second = reduce(
      first,
      delta(0, {
        op: 'replace',
        path: PATHS.resources,
        value: { memoryBytesUsed: 12_345 },
      }),
    );

    expect(second.resources?.memoryBytesUsed).toBe(12_345);
    expect(second.resources?.memoryBytesPeak).toBeUndefined();
    expect(second.resources?.ceilingHits).toBeUndefined();
  });
});

describe('durable memory-pressure state', () => {
  const opened = delta(8, {
    op: 'replace',
    path: PATHS.memoryPressure,
    value: {
      usedBytes: 912_680_550,
      limitBytes: 1_073_741_824,
      occupancyPermille: 850,
      since: '2026-08-23T04:12:00Z',
    },
  });

  it('folds an open episode and remains idempotent', () => {
    const once = reduce(EMPTY, opened);
    expect(reduce(once, opened)).toEqual(once);
    expect(once.memoryPressure).toEqual({
      usedBytes: 912_680_550,
      limitBytes: 1_073_741_824,
      occupancyPermille: 850,
      since: '2026-08-23T04:12:00Z',
    });
  });

  it.each([
    { op: 'replace', path: PATHS.memoryPressure, value: null },
    { op: 'replace', path: PATHS.memoryPressure },
  ] satisfies readonly JsonPatch[])('clears on null or a missing value', (patch) => {
    const state = reduce(reduce(EMPTY, opened), delta(9, patch));
    expect(state.memoryPressure).toBeUndefined();
  });
});
