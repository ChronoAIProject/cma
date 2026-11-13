import type { ReadOptions } from '../../api/client';
import type { AgentV2Session } from './client';

export interface AgentTransportPreparation {
  /** Transfer this exact preparation to the first navigation load. */
  readonly handoff: () => void;
  /** Ordinary modal cleanup cancels preparation; intended navigation transfers it. */
  readonly release: () => void;
}

interface PreparedTransport {
  readonly agent: string;
  readonly owner: string;
  readonly request: AbortController;
  readonly result: Promise<AgentV2Session | undefined>;
  readonly leases: Set<object>;
  transferred: boolean;
}

/** One exact-Agent handoff per client, with no cached eligibility or account authority. */
export function createTransportPreparation(
  select: (agent: string, read: ReadOptions, owner: string) => Promise<AgentV2Session | undefined>,
  open: (agent: string, read: ReadOptions) => Promise<AgentV2Session | undefined>,
) {
  let prepared: PreparedTransport | undefined;
  const retain = (value: PreparedTransport, opening = false): AgentTransportPreparation => {
    const lease = {};
    value.leases.add(lease);
    return {
      handoff() {
        if (value.leases.has(lease) && prepared === value && !value.request.signal.aborted)
          value.transferred = true;
      },
      release() {
        value.leases.delete(lease);
        // Once consumed, the load owns cancellation. Before consumption, a
        // cancelled route must revoke even a transferred creation selection.
        if (opening ? prepared !== value : value.transferred || value.leases.size > 0) return;
        value.request.abort();
        if (prepared === value) prepared = undefined;
      },
    };
  };
  const create = (agent: string, owner: string) => {
    prepared?.request.abort();
    const request = new AbortController();
    const value: PreparedTransport = {
      agent,
      owner,
      request,
      result: select(agent, { signal: request.signal }, owner),
      leases: new Set(),
      transferred: false,
    };
    prepared = value;
    // Preparation can fail before navigation attaches its consumer.
    void value.result.catch(() => undefined);
    return value;
  };
  return {
    prepare: (agent: string, owner: string) => retain(create(agent, owner)),
    /** Overlap route classification without replacing an exact creation handoff. */
    prepareOpening(agent: string, owner: string): AgentTransportPreparation {
      const value =
        prepared?.agent === agent && prepared.owner === owner && !prepared.request.signal.aborted
          ? prepared
          : create(agent, owner);
      return retain(value, true);
    },
    async openForLoad(agent: string, read: ReadOptions = {}): Promise<AgentV2Session | undefined> {
      read.signal?.throwIfAborted();
      const value = prepared;
      if (value === undefined || !value.transferred) return open(agent, read);
      prepared = undefined;
      if (value.agent !== agent) {
        value.request.abort();
        return open(agent, read);
      }
      const abort = () => value.request.abort();
      read.signal?.addEventListener('abort', abort, { once: true });
      try {
        const session = await value.result;
        read.signal?.throwIfAborted();
        value.request.signal.throwIfAborted();
        session?.discardEntryHandoff();
        // v2 loading always starts with a fresh authorization. AG-UI has no
        // such entry check, so it keeps the ordinary freshly authorized open.
        return session ?? (await open(agent, read));
      } finally {
        read.signal?.removeEventListener('abort', abort);
      }
    },
  };
}
