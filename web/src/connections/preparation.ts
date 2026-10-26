import { connectionClient, type ConnectionReport } from './client';

export const PREPARED_SEND_WINDOW_MS = 1000;

export interface AgentConnectionObservation {
  readonly report: ConnectionReport;
  readonly observedAt: number;
  readonly prepared: boolean;
}

/** A caller must still guard its exact authorized view before consuming this once. */
export function createPreparedConnectionHandoff(observation: AgentConnectionObservation) {
  let consumed = false;
  return () => {
    if (consumed) return undefined;
    consumed = true;
    if (!observation.prepared || !observation.report.ready || document.hidden) return undefined;
    const age = performance.now() - observation.observedAt;
    return age >= 0 && age <= PREPARED_SEND_WINDOW_MS ? observation.report : undefined;
  };
}

interface Preparation {
  readonly agentId: string;
  readonly owner: string;
  readonly request: AbortController;
  readonly result: Promise<AgentConnectionObservation>;
  readonly reusable: () => boolean;
  readonly stopWatching: () => void;
  transferred: boolean;
}

const preparations = new WeakMap<object, Preparation>();

async function check(agentId: string, signal: AbortSignal, prepared: boolean) {
  const report = await connectionClient.check({ type: 'agent', id: agentId }, undefined, signal);
  signal.throwIfAborted();
  return { report, observedAt: performance.now(), prepared };
}

/** The server authorizes this exact owner target before reading its required services. */
export function prepareAgentConnections(context: object, agentId: string, owner: string) {
  preparations.get(context)?.request.abort();
  const request = new AbortController();
  let reusable = !document.hidden;
  let expiry: ReturnType<typeof setTimeout> | undefined;
  const stopWatching = () => {
    document.removeEventListener('visibilitychange', discard);
    window.removeEventListener('blur', discard);
    request.signal.removeEventListener('abort', discard);
    clearTimeout(expiry);
  };
  const discard = () => {
    reusable = false;
    stopWatching();
  };
  document.addEventListener('visibilitychange', discard);
  window.addEventListener('blur', discard);
  request.signal.addEventListener('abort', discard, { once: true });
  const value: Preparation = {
    agentId,
    owner,
    request,
    result: check(agentId, request.signal, true).then(
      (observation) => {
        if (reusable) expiry = setTimeout(discard, PREPARED_SEND_WINDOW_MS);
        else stopWatching();
        return observation;
      },
      (error: unknown) => {
        discard();
        throw error;
      },
    ),
    reusable: () => reusable,
    stopWatching,
    transferred: false,
  };
  preparations.set(context, value);
  void value.result.catch(() => undefined);
  return {
    handoff() {
      if (preparations.get(context) === value && !request.signal.aborted) value.transferred = true;
    },
    release() {
      if (value.transferred) return;
      request.abort();
      if (preparations.get(context) === value) preparations.delete(context);
    },
  };
}

/** Consume only the matching creation handoff after the personal view is authorized. */
export async function readAgentConnections(
  context: object | undefined,
  agentId: string,
  owner: string | undefined,
  signal: AbortSignal,
): Promise<AgentConnectionObservation> {
  signal.throwIfAborted();
  const value = context === undefined ? undefined : preparations.get(context);
  if (value === undefined || !value.transferred) return check(agentId, signal, false);
  preparations.delete(context!);
  if (value.agentId !== agentId || value.owner !== owner) {
    value.request.abort();
    return check(agentId, signal, false);
  }
  const abort = () => value.request.abort();
  signal.addEventListener('abort', abort, { once: true });
  try {
    const observation = await value.result;
    signal.throwIfAborted();
    value.request.signal.throwIfAborted();
    return { ...observation, prepared: value.reusable() && !document.hidden };
  } finally {
    value.stopWatching();
    signal.removeEventListener('abort', abort);
  }
}
