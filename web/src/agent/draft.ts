import { useCallback, useEffect, useState, type Dispatch, type SetStateAction } from 'react';
import type { CmaWebClient } from '../api/client';
import type { DraftAttachment } from './attachments';
import {
  draftReadError,
  draftWriteError,
  emptyDraft,
  readAgentDraft,
  removeAgentDraft,
  saveAgentDraft,
  subscribeDrafts,
  type DraftValue,
} from './draftStorage';
import { requireDraftIdentity, retainAgentDraftOwner } from './draftIdentity';
import { watchDraftSubmission } from './draftDelivery';

export interface AgentDraft {
  readonly content: string;
  readonly setContent: Dispatch<SetStateAction<string>>;
  readonly attachments: readonly DraftAttachment[];
  readonly setAttachments: Dispatch<SetStateAction<readonly DraftAttachment[]>>;
  readonly error?: string;
  readonly recoveryBlocked?: boolean;
  readonly retryStorage?: () => void;
  readonly discardSaved?: () => void;
  readonly beginSubmission?: (message: string) => { accepted: () => void; settled: () => void };
}
interface Entry {
  readonly value: DraftValue;
  readonly error?: string;
  readonly unread?: boolean;
}
function restore(owner: string, agentId?: string): Entry {
  if (!owner || !agentId) return { value: emptyDraft };
  try {
    requireDraftIdentity();
    return { value: readAgentDraft(owner, agentId) };
  } catch {
    return { value: emptyDraft, error: draftReadError, unread: true };
  }
}
function persist(owner: string, agentId: string | undefined, value: DraftValue): Entry {
  if (!owner || !agentId) return { value };
  try {
    requireDraftIdentity();
    saveAgentDraft(owner, agentId, value);
    return { value };
  } catch {
    return { value, error: draftWriteError };
  }
}

/** Browser identity is stable across clients; callbacks still belong to one visit. */
export function useAgentDraft(
  client: CmaWebClient,
  viewerId: string,
  agentId: string | undefined,
): AgentDraft {
  const [stored, setStored] = useState(() => {
    retainAgentDraftOwner(viewerId);
    return {
      scope: { client, viewerId, agentId },
      entry: restore(viewerId, agentId),
      cache: new Map<string, Entry>(),
    };
  });
  const { scope } = stored;
  const current =
    scope.client === client && scope.viewerId === viewerId && scope.agentId === agentId;
  if (!current) {
    const sameOwner = scope.viewerId === viewerId;
    if (!sameOwner) retainAgentDraftOwner(viewerId);
    const cache = sameOwner ? new Map(stored.cache) : new Map<string, Entry>();
    if (sameOwner && scope.agentId) cache.set(scope.agentId, stored.entry);
    const entry = (agentId ? cache.get(agentId) : undefined) ?? restore(viewerId, agentId);
    cache.delete(agentId ?? '');
    setStored({ scope: { client, viewerId, agentId }, entry, cache });
  }
  useEffect(
    () =>
      subscribeDrafts((owner, acceptedAgent, revision, error) => {
        if (owner !== scope.viewerId) return;
        setStored((previous) => {
          if (previous.scope !== scope) return previous;
          if (acceptedAgent === undefined)
            return {
              ...previous,
              scope: { ...scope },
              entry: { value: emptyDraft },
              cache: new Map(),
            };
          const cache = new Map(previous.cache);
          if (cache.get(acceptedAgent)?.value.revision === revision) cache.delete(acceptedAgent);
          return {
            ...previous,
            cache,
            entry:
              acceptedAgent === scope.agentId && previous.entry.value.revision === revision
                ? { value: emptyDraft, error }
                : previous.entry,
          };
        });
      }),
    [scope],
  );
  const update = useCallback(
    (change: (value: DraftValue) => DraftValue) => {
      const revision = crypto.randomUUID();
      setStored((previous) => {
        if (previous.scope !== scope) return previous;
        const value = { ...change(previous.entry.value), revision, commandKey: undefined };
        // A damaged record is preserved until the owner explicitly discards or recovers it.
        const entry = previous.entry.unread
          ? { ...previous.entry, value }
          : persist(scope.viewerId, scope.agentId, value);
        return { ...previous, entry };
      });
    },
    [scope],
  );
  const setContent = useCallback<Dispatch<SetStateAction<string>>>(
    (change) =>
      update((value) => ({
        ...value,
        content: typeof change === 'function' ? change(value.content) : change,
      })),
    [update],
  );
  const setAttachments = useCallback<Dispatch<SetStateAction<readonly DraftAttachment[]>>>(
    (change) =>
      update((value) => ({
        ...value,
        attachments: typeof change === 'function' ? change(value.attachments) : change,
      })),
    [update],
  );
  const retryStorage = () =>
    setStored((previous) => {
      if (previous.scope !== scope) return previous;
      if (previous.entry.unread) {
        const recovered = restore(viewerId, agentId);
        if (previous.entry.value.revision !== '')
          return {
            ...previous,
            entry: {
              ...previous.entry,
              error:
                recovered.error ??
                'Copy your current text, then discard the saved draft to save this version. The saved draft has been left intact.',
            },
          };
        return { ...previous, entry: recovered };
      }
      return { ...previous, entry: persist(viewerId, agentId, previous.entry.value) };
    });
  const discardSaved = () =>
    setStored((previous) => {
      if (previous.scope !== scope || !agentId) return previous;
      try {
        requireDraftIdentity();
        removeAgentDraft(viewerId, agentId);
        return { ...previous, entry: persist(viewerId, agentId, previous.entry.value) };
      } catch {
        return { ...previous, entry: { ...previous.entry, error: draftWriteError } };
      }
    });
  const beginSubmission = (message: string) => {
    const snapshot = stored.entry.value;
    const settled = watchDraftSubmission({
      owner: viewerId,
      agentId: agentId ?? '',
      message,
      bind: (key, storage) => {
        try {
          requireDraftIdentity();
          const saved = readAgentDraft(viewerId, agentId!, storage);
          if (saved.revision === snapshot.revision)
            saveAgentDraft(viewerId, agentId!, { ...saved, commandKey: key }, storage);
        } catch {
          setStored((previous) =>
            previous.scope === scope
              ? { ...previous, entry: { ...previous.entry, error: draftWriteError } }
              : previous,
          );
        }
      },
    });
    return {
      settled,
      accepted: () => {
        settled();
        setStored((previous) => {
          if (previous.scope !== scope || previous.entry.value.revision !== snapshot.revision)
            return previous;
          return { ...previous, entry: persist(viewerId, agentId, emptyDraft) };
        });
      },
    };
  };
  const entry = current ? stored.entry : { value: emptyDraft };
  return {
    ...entry.value,
    error: entry.error,
    recoveryBlocked: entry.unread,
    setContent,
    setAttachments,
    retryStorage,
    discardSaved,
    beginSubmission,
  };
}
