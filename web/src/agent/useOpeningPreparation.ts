import { useCallback, useEffect, useRef } from 'react';
import type { CmaWebClient } from '../api/client';
import type { AgentOpeningPreparation } from './transport/preparation';
import type { AgentLoadLineage } from './loadAgent';
import { prepareOpeningReads } from './transport/openingReads';
import { readCooldown } from './readCooldown';

/** Only classified personal routes admit protected reads while selection finishes. */
export function useOpeningPreparation(
  client: CmaWebClient,
  agentId: string | undefined,
  owner: string,
  authorized: boolean,
  refused: boolean,
  lineage?: AgentLoadLineage,
) {
  const transport = client.agentTransport;
  const active = useRef<AgentOpeningPreparation | undefined>(undefined);
  useEffect(() => {
    if (!transport || !agentId || refused) return;
    // A remounted route must not bypass the controller's retained server floor.
    // Its existing retry owner starts the normal load once that deadline expires.
    if (readCooldown(client, owner, agentId) > Date.now()) return;
    const preparation = transport.prepareOpening(agentId, owner);
    active.current = preparation;
    return () => {
      preparation.release();
      if (active.current === preparation) active.current = undefined;
    };
  }, [client, transport, agentId, owner, refused]);
  // Register before the controller's load effect so only a classified personal
  // view consumes this selection. authorizeLoad still makes a fresh request.
  useEffect(() => {
    if (!authorized || refused || !agentId || !active.current) return;
    active.current.handoff();
    const reads = prepareOpeningReads(client, agentId, active.current, lineage);
    return () => reads.release();
  }, [client, transport, agentId, owner, authorized, refused, lineage]);
}

interface NavigationPreparation {
  readonly abort: AbortController;
  preparation?: AgentOpeningPreparation;
  reads?: { readonly release: () => void };
  released: boolean;
}

/**
 * Prepare a personal Agent from the navigation intent without caching access.
 * The access read is deliberately the first operation: a navigation row can
 * represent a shared or revoked Agent, so hover must never admit protected
 * transport reads until the server classifies this exact identity.
 */
export function useNavigationOpeningPreparation(
  client: CmaWebClient,
  owner: string,
  enabled = true,
) {
  const active = useRef(new Map<string, NavigationPreparation>());
  const releaseTimers = useRef(new Map<string, ReturnType<typeof setTimeout>>());
  const release = useCallback((agentId: string, entry: NavigationPreparation) => {
    if (entry.released) return;
    entry.released = true;
    entry.abort.abort();
    entry.reads?.release();
    entry.preparation?.release();
    if (active.current.get(agentId) === entry) active.current.delete(agentId);
  }, []);
  const prepare = useCallback(
    (agentId: string) => {
      if (!enabled) return () => undefined;
      const transport = client.agentTransport;
      const resources = client.resourceViews;
      if (transport === undefined || resources === undefined) return () => undefined;
      const previous = active.current.get(agentId);
      if (previous !== undefined) return () => release(agentId, previous);
      const entry: NavigationPreparation = {
        abort: new AbortController(),
        released: false,
      };
      active.current.set(agentId, entry);
      void resources.agents
        .readAccessible(agentId, { signal: entry.abort.signal })
        .then((detail) => {
          entry.abort.signal.throwIfAborted();
          if (
            detail.id !== agentId ||
            detail.access === 'shared' ||
            detail.sandbox.status === 'retired'
          )
            return;
          // Navigation is speculative. The route later takes an opening lease;
          // its dedicated navigation lease lets pointer cancellation leave the
          // route-owned selection alive when activation already began.
          entry.preparation = transport.prepareNavigation(agentId, owner);
          entry.reads = prepareOpeningReads(client, agentId, entry.preparation, {
            agentId: detail.id,
            workspaceId: detail.workspace.id,
            sandboxId: detail.sandbox.id,
          });
        })
        .catch((error: unknown) => {
          if (!entry.abort.signal.aborted)
            console.debug('[agent.navigation] opening preparation unavailable', {
              reason: error instanceof Error ? error.name : 'unknown',
            });
        });
      return () => release(agentId, entry);
    },
    [client, enabled, owner, release],
  );
  useEffect(() => {
    const entries = active.current;
    if (!enabled) {
      for (const [agentId, entry] of entries) release(agentId, entry);
    }
    return () => {
      for (const [agentId, entry] of entries) release(agentId, entry);
    };
  }, [enabled, release]);
  const onIntent = useCallback(
    (agentId: string, activeIntent: boolean) => {
      if (activeIntent) {
        const timer = releaseTimers.current.get(agentId);
        if (timer !== undefined) clearTimeout(timer);
        if (!active.current.has(agentId)) prepare(agentId);
        return;
      }
      const timer = setTimeout(() => {
        releaseTimers.current.delete(agentId);
        const entry = active.current.get(agentId);
        if (entry !== undefined) release(agentId, entry);
      }, 250);
      const previous = releaseTimers.current.get(agentId);
      if (previous !== undefined) clearTimeout(previous);
      releaseTimers.current.set(agentId, timer);
    },
    [prepare, release],
  );
  useEffect(
    () => () => {
      for (const timer of releaseTimers.current.values()) clearTimeout(timer);
      releaseTimers.current.clear();
    },
    [],
  );
  return onIntent;
}
