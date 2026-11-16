import { raceWithAbortSignal } from '@chronoai/cma-sdk';
import type { ReadOptions } from '../../api/client';
import type { AgentV2Session } from './client';

export interface AgentTransportPreparation {
  /** Transfer this exact preparation to the first navigation load. */
  readonly handoff: () => void;
  /** Ordinary modal cleanup cancels preparation; intended navigation transfers it. */
  readonly release: () => void;
}

export interface AgentOpeningPreparation extends AgentTransportPreparation {
  /** Only a classified personal route may admit reads before selection finishes. */
  readonly authorizeRead: (signal: AbortSignal) => Promise<{
    readonly owner: string;
    readonly assertCurrent: () => void;
    readonly selectedEvents?: Promise<AgentV2Session['events'] | undefined>;
  }>;
}

interface PreparedTransport {
  readonly agent: string;
  readonly owner: string;
  readonly request: AbortController;
  readonly result: Promise<AgentV2Session | undefined>;
  readonly identity: Promise<AgentV2Session>;
  readonly leases: Set<object>;
  transferred: boolean;
}

type LeaseKind = 'basic' | 'opening' | 'navigation';

/** One exact-Agent handoff per client, with no cached eligibility or account authority. */
export function createTransportPreparation(
  select: (
    agent: string,
    read: ReadOptions,
    owner: string,
    onIdentity: (session: AgentV2Session) => void,
  ) => Promise<AgentV2Session | undefined>,
  open: (agent: string, read: ReadOptions) => Promise<AgentV2Session | undefined>,
) {
  let prepared: PreparedTransport | undefined;
  const retain = (value: PreparedTransport, kind: LeaseKind = 'basic'): AgentOpeningPreparation => {
    const lease = {};
    value.leases.add(lease);
    return {
      async authorizeRead(signal) {
        const current = AbortSignal.any([signal, value.request.signal]);
        const session = await raceWithAbortSignal(current, () => value.identity);
        // This check starts after personal access, independently of eligibility.
        // It cannot reuse the identity read that admitted transport selection.
        await session.authorize(current);
        current.throwIfAborted();
        const assertCurrent = () => {
          current.throwIfAborted();
          session.journal.read();
        };
        // The selection's final check follows eligibility IO. A retained v2
        // selection has no eligibility IO and this admission is already fresh.
        // Waiting for selection prevents speculative v2 reads on legacy routes.
        const selectedEvents = raceWithAbortSignal(current, () => value.result).then((selected) => {
          assertCurrent();
          return selected?.events;
        });
        // The caller may cancel without ever attaching a replay consumer.
        void selectedEvents.catch(() => undefined);
        return {
          owner: session.owner,
          assertCurrent,
          selectedEvents,
        };
      },
      handoff() {
        if (value.leases.has(lease) && prepared === value && !value.request.signal.aborted)
          value.transferred = true;
      },
      release() {
        value.leases.delete(lease);
        // Once consumed, the load owns cancellation. A speculative navigation
        // must keep its selection alive while the activated route still holds
        // a lease, but a cancelled route retains its original abort semantics.
        if (
          kind === 'navigation'
            ? value.transferred || value.leases.size > 0 || prepared !== value
            : kind === 'opening'
              ? prepared !== value
              : value.transferred || value.leases.size > 0
        )
          return;
        value.request.abort();
        if (prepared === value) prepared = undefined;
      },
    };
  };
  const create = (agent: string, owner: string) => {
    prepared?.request.abort();
    const request = new AbortController();
    let identified!: (session: AgentV2Session) => void;
    let failed!: (error: unknown) => void;
    const identity = new Promise<AgentV2Session>((resolve, reject) => {
      identified = resolve;
      failed = reject;
    });
    // Selection failures remain on result; identity has a separate early reader.
    void identity.catch(() => undefined);
    const value: PreparedTransport = {
      agent,
      owner,
      request,
      result: select(agent, { signal: request.signal }, owner, identified).catch(
        (error: unknown) => {
          failed(error);
          throw error;
        },
      ),
      identity,
      leases: new Set(),
      transferred: false,
    };
    prepared = value;
    // Preparation can fail before navigation attaches its consumer.
    void value.result.catch(() => undefined);
    return value;
  };
  return {
    prepare: (agent: string, owner: string): AgentTransportPreparation =>
      retain(create(agent, owner)),
    /** Overlap a classified navigation with the route's fresh authorization. */
    prepareNavigation(agent: string, owner: string): AgentOpeningPreparation {
      const value =
        prepared?.agent === agent && prepared.owner === owner && !prepared.request.signal.aborted
          ? prepared
          : create(agent, owner);
      return retain(value, 'navigation');
    },
    /** Overlap route classification without replacing an exact creation handoff. */
    prepareOpening(agent: string, owner: string): AgentOpeningPreparation {
      const value =
        prepared?.agent === agent && prepared.owner === owner && !prepared.request.signal.aborted
          ? prepared
          : create(agent, owner);
      return retain(value, 'opening');
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
