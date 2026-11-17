import { raceWithAbortSignal } from '@chronoai/cma-sdk';
import type { CmaWebClient } from '../../api/client';
import type { AgentLoadLineage } from '../loadAgent';
import type { AgentOpeningPreparation } from './preparation';
import { readAgentProjections, type AgentProjectionReads } from './projections';
import { prepareOpeningReplay } from './openingReplay';

export interface AuthorizedOpeningReads {
  readonly owner: string;
  readonly assertCurrent: () => void;
  readonly reads: AgentProjectionReads;
  readonly replay?: ReturnType<typeof prepareOpeningReplay>;
}

interface Opening {
  readonly agent: string;
  readonly lineage?: AgentLoadLineage;
  readonly request: AbortController;
  readonly result: Promise<AuthorizedOpeningReads>;
  readonly expiry: ReturnType<typeof setTimeout>;
  readonly leases: Set<object>;
}

const openings = new WeakMap<CmaWebClient, Opening>();
const counts = { started: 0, consumed: 0, cancelled: 0 };

function sameLineage(left?: AgentLoadLineage, right?: AgentLoadLineage): boolean {
  // The route may first authorize before its lineage state is committed. A
  // classified navigation batch already carries the exact lineage and may be
  // adopted by that intermediate route render; a later mismatching lineage
  // still replaces it below.
  if (right === undefined) return true;
  return (
    left?.agentId === right?.agentId &&
    left?.workspaceId === right?.workspaceId &&
    left?.sandboxId === right?.sandboxId
  );
}

function cancelOpening(client: CmaWebClient, value: Opening): void {
  if (openings.get(client) !== value) return;
  openings.delete(client);
  clearTimeout(value.expiry);
  value.leases.clear();
  counts.cancelled += 1;
  value.request.abort();
}

/** Counts only: no account, Agent, credential or transcript data is exposed. */
export const openingReadDiagnostics = () => ({ ...counts });

/** One exact-Agent read batch shared by speculative navigation and its route. */
export function prepareOpeningReads(
  client: CmaWebClient,
  agent: string,
  preparation: AgentOpeningPreparation,
  lineage?: AgentLoadLineage,
) {
  const previous = openings.get(client);
  if (previous?.agent === agent && sameLineage(previous.lineage, lineage)) {
    const lease = {};
    previous.leases.add(lease);
    return {
      release: () => {
        previous.leases.delete(lease);
        if (previous.leases.size === 0) cancelOpening(client, previous);
      },
    };
  }
  if (previous) cancelOpening(client, previous);
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
      replay:
        admission.selectedEvents === undefined
          ? undefined
          : prepareOpeningReplay(
              admission.selectedEvents,
              agent,
              request.signal,
              admission.assertCurrent,
            ),
    };
  })();
  // Preserve the rejection for the load, even if navigation abandons it first.
  void result.catch(() => undefined);
  const lease = {};
  const value: Opening = {
    agent,
    lineage,
    request,
    result,
    expiry: setTimeout(() => cancelOpening(client, value), 5000),
    leases: new Set([lease]),
  };
  openings.set(client, value);
  return {
    release: () => {
      value.leases.delete(lease);
      if (value.leases.size === 0) cancelOpening(client, value);
    },
  };
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
  value.leases.clear();
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
