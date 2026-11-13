import { raceWithAbortSignal } from '@chronoai/cma-sdk';
import type { CmaWebClient } from '../../api/client';
import type { AgentLoadLineage } from '../loadAgent';
import type { AgentOpeningPreparation } from './preparation';
import { readAgentProjections, type AgentProjectionReads } from './projections';

export interface AuthorizedOpeningReads {
  readonly owner: string;
  readonly assertCurrent: () => void;
  readonly reads: AgentProjectionReads;
}

interface Opening {
  readonly agent: string;
  readonly lineage?: AgentLoadLineage;
  readonly request: AbortController;
  readonly result: Promise<AuthorizedOpeningReads>;
  readonly expiry: ReturnType<typeof setTimeout>;
}

const openings = new WeakMap<CmaWebClient, Opening>();
const counts = { started: 0, consumed: 0, cancelled: 0 };

/** Counts only: no account, Agent, credential or transcript data is exposed. */
export const openingReadDiagnostics = () => ({ ...counts });

/** One route-owned read batch; neither hover selection nor reusable authority. */
export function prepareOpeningReads(
  client: CmaWebClient,
  agent: string,
  preparation: AgentOpeningPreparation,
  lineage?: AgentLoadLineage,
) {
  const previous = openings.get(client);
  if (previous) {
    clearTimeout(previous.expiry);
    previous.request.abort();
  }
  const request = new AbortController();
  const result = (async () => {
    const admission = await preparation.authorizeRead(request.signal);
    request.signal.throwIfAborted();
    admission.assertCurrent();
    counts.started += 1;
    return {
      owner: admission.owner,
      assertCurrent: () => {
        request.signal.throwIfAborted();
        admission.assertCurrent();
      },
      reads: readAgentProjections(client, agent, request.signal, lineage),
    };
  })();
  // Preserve the rejection for the load, even if navigation abandons it first.
  void result.catch(() => undefined);
  const release = () => {
    if (openings.get(client) !== value) return;
    openings.delete(client);
    clearTimeout(value.expiry);
    counts.cancelled += 1;
    request.abort();
  };
  const value: Opening = { agent, lineage, request, result, expiry: setTimeout(release, 5000) };
  openings.set(client, value);
  return { release };
}

/** Consume once in the matching load; the loader then owns abort and disposal. */
export function takeOpeningReads(
  client: CmaWebClient,
  agent: string,
  signal: AbortSignal,
  lineage?: AgentLoadLineage,
) {
  const value = openings.get(client);
  if (!value) return undefined;
  openings.delete(client);
  clearTimeout(value.expiry);
  if (
    value.agent !== agent ||
    value.lineage?.agentId !== lineage?.agentId ||
    value.lineage?.workspaceId !== lineage?.workspaceId ||
    value.lineage?.sandboxId !== lineage?.sandboxId ||
    signal.aborted ||
    value.request.signal.aborted
  ) {
    value.request.abort();
    return undefined;
  }
  counts.consumed += 1;
  const abort = () => value.request.abort(signal.reason);
  signal.addEventListener('abort', abort, { once: true });
  return {
    read: () => raceWithAbortSignal(signal, () => value.result),
    dispose: () => {
      signal.removeEventListener('abort', abort);
      value.request.abort();
    },
  };
}
