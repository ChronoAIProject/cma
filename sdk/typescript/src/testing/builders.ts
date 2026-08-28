import type { ConcurrentInput } from '../agui/operations.js';
import { LIVE_MARKER, PATHS, STATE_DELTA_PATCHES } from '../wire.js';

const CONFORMANCE_ROOT = 'https://spec.chrono-ai.fun/cma/1.0/conf';

export interface SessionPayloadFixture {
  readonly id: string;
  readonly status?: string;
  readonly kernel?: string;
  readonly createdAt?: string;
  readonly grants?: readonly string[];
  readonly concurrentInput?: ConcurrentInput;
  readonly cols?: number;
  readonly rows?: number;
  readonly lastSeq?: number;
}

/** Build one valid REST session payload without leaking wire names into a consumer test. */
export function sessionPayload({
  id,
  status = 'running',
  kernel = 'codex',
  createdAt = '2026-01-01T00:00:00Z',
  grants = [],
  concurrentInput,
  cols,
  rows,
  lastSeq,
}: SessionPayloadFixture): Readonly<Record<string, unknown>> {
  return {
    id,
    status,
    kernel,
    created_at: createdAt,
    conforms_to: ['core', ...grants].map((grant) => `${CONFORMANCE_ROOT}/${grant}`),
    concurrent_input: concurrentInput,
    cols,
    rows,
    last_seq: lastSeq,
  };
}

export interface StateFixturePatch {
  readonly op: 'add' | 'replace' | 'remove';
  readonly path: keyof typeof PATHS;
  readonly value?: unknown;
}

export interface StateFrameFixture {
  readonly threadId: string;
  readonly runId: string;
  readonly seq: number;
  readonly patches: readonly StateFixturePatch[];
  readonly live?: boolean;
}

/** Build and validate one AG-UI state frame from semantic state-path names. */
export function stateFrame({
  threadId,
  runId,
  seq,
  patches,
  live = false,
}: StateFrameFixture): Readonly<Record<string, unknown>> {
  return {
    type: 'STATE_DELTA',
    threadId,
    runId,
    seq,
    ...(live ? { [LIVE_MARKER]: true } : {}),
    [STATE_DELTA_PATCHES]: patches.map((patch) => ({
      op: patch.op,
      path: PATHS[patch.path],
      ...('value' in patch ? { value: patch.value } : {}),
    })),
  };
}
