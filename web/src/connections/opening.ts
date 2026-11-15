import { connectionClient, type ConnectionReport } from './client';
import { readWithConnectionCooldown } from './readCooldown';

const completedWindowMs = 1000;

interface Observation {
  readonly report: ConnectionReport;
  readonly observedAt: number;
  readonly prepared: false;
}

interface Opening {
  readonly agent: string;
  readonly owner: string;
  readonly request: AbortController;
  readonly result: Promise<Observation>;
  readonly release: () => void;
  readonly stopWatching: () => void;
  observation?: Observation;
}

const openings = new WeakMap<object, Opening>();

export function discardOpeningConnections(context: object) {
  openings.get(context)?.release();
}

/** Start one independently authenticated read for the selected route, never a hover. */
export function prepareOpeningConnections(context: object, agent: string, owner: string) {
  discardOpeningConnections(context);
  if (document.hidden) return { release() {} };
  const request = new AbortController();
  let expiry: ReturnType<typeof setTimeout> | undefined;
  const stopWatching = () => {
    clearTimeout(expiry);
    document.removeEventListener('visibilitychange', release);
    window.removeEventListener('blur', release);
  };
  const release = () => {
    if (openings.get(context) === value) openings.delete(context);
    stopWatching();
    request.abort();
  };
  const result = readWithConnectionCooldown(context, owner, request.signal, () =>
    connectionClient.check({ type: 'agent', id: agent }, undefined, request.signal),
  ).then((report): Observation => {
    request.signal.throwIfAborted();
    const observation = { report, observedAt: performance.now(), prepared: false as const };
    value.observation = observation;
    // A completed result can only continue this opening, not a later visit.
    if (openings.get(context) === value) expiry = setTimeout(release, completedWindowMs);
    return observation;
  });
  const value: Opening = { agent, owner, request, result, release, stopWatching };
  openings.set(context, value);
  document.addEventListener('visibilitychange', release);
  window.addEventListener('blur', release);
  // Preserve a failure for the authorized consumer without an unhandled rejection.
  void result.catch(() => stopWatching());
  return { release };
}

/** The caller must have current personal access; this transfers no Send permission. */
export function takeOpeningConnections(
  context: object | undefined,
  agent: string,
  owner: string | undefined,
  signal: AbortSignal,
): Promise<Observation> | undefined {
  const value = context === undefined ? undefined : openings.get(context);
  if (!value) return undefined;
  openings.delete(context!);
  const age = value.observation && performance.now() - value.observation.observedAt;
  if (
    signal.aborted ||
    value.request.signal.aborted ||
    document.hidden ||
    value.agent !== agent ||
    value.owner !== owner ||
    (age !== undefined && (age < 0 || age >= completedWindowMs))
  ) {
    value.release();
    return undefined;
  }
  // The authorized view now owns an ordinary fresh read. Preserve its existing
  // visibility behavior; only an unconsumed speculative transfer expires on blur.
  value.stopWatching();
  const abort = () => value.release();
  signal.addEventListener('abort', abort, { once: true });
  return value.result
    .then((observation) => {
      signal.throwIfAborted();
      value.request.signal.throwIfAborted();
      return observation;
    })
    .finally(() => {
      signal.removeEventListener('abort', abort);
      value.stopWatching();
    });
}
