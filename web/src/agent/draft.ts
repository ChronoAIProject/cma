import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
} from 'react';
import type { CmaWebClient } from '../api/client';
import type { DraftAttachment } from './attachments';
import {
  draftReadError,
  draftWriteError,
  emptyDraft,
  importEmptyAgentDraft,
  readAgentDraft,
  removeAgentDraft,
  saveAgentDraft,
  subscribeDrafts,
  type DraftValue,
} from './draftStorage';
import { requireDraftIdentity, retainAgentDraftOwner } from './draftIdentity';
import { watchDraftSubmission } from './draftDelivery';

export interface AgentDraft {
  readonly importText?: (
    source: { readonly owner: string; readonly agentId: string; readonly text: string },
    current: () => boolean,
  ) => { readonly verify: () => boolean } | undefined;
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
  const active = useRef<typeof stored | undefined>(undefined);
  const editing = useRef(false);
  const submissions = useRef(new Set<{ scope: typeof scope }>());
  useLayoutEffect(() => {
    active.current = stored;
    editing.current = false;
    return () => {
      active.current = undefined;
    };
  }, [stored]);
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
      if (active.current?.scope !== scope) return;
      editing.current = true;
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
    const submission = { scope };
    submissions.current.add(submission);
    const snapshot = stored.entry.value;
    const stopWatching = watchDraftSubmission({
      owner: viewerId,
      agentId: agentId ?? '',
      message,
      bind: (key, storage) => {
        const current = active.current;
        if (!agentId || (current && current.scope.viewerId !== viewerId)) return 'superseded';
        const mounted = current?.scope.agentId === agentId;
        const entry = mounted ? current.entry : current?.cache.get(agentId);
        if ((mounted && editing.current) || (entry && entry.value.revision !== snapshot.revision))
          return 'superseded';
        const updateEntry = (update: (value: Entry) => Entry) => {
          if (!current) return;
          setStored((previous) => {
            if (previous.scope.viewerId !== viewerId) return previous;
            if (previous.scope.agentId === agentId) {
              if (previous.entry.value.revision !== snapshot.revision) return previous;
              return { ...previous, entry: update(previous.entry) };
            }
            const cached = previous.cache.get(agentId);
            if (cached?.value.revision !== snapshot.revision) return previous;
            return { ...previous, cache: new Map(previous.cache).set(agentId, update(cached)) };
          });
        };
        try {
          requireDraftIdentity();
          const saved = readAgentDraft(viewerId, agentId, storage);
          if (entry?.unread) throw new Error(draftWriteError);
          if (saved.revision !== snapshot.revision) {
            if (entry) throw new Error(draftWriteError);
            return 'superseded';
          }
          saveAgentDraft(viewerId, agentId, { ...saved, commandKey: key }, storage);
          updateEntry((entry) => ({
            ...entry,
            value: { ...entry.value, commandKey: key },
            error: entry.error === draftWriteError ? undefined : entry.error,
          }));
          return 'bound';
        } catch {
          updateEntry((entry) => (entry.unread ? entry : { ...entry, error: draftWriteError }));
          return 'unavailable';
        }
      },
    });
    const settled = () => {
      submissions.current.delete(submission);
      stopWatching();
    };
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
    importText: ({ owner, agentId: targetAgent, text }, permitted) => {
      const entry = active.current?.scope === scope ? active.current.entry : undefined;
      const canImport = () =>
        active.current?.scope === scope &&
        !editing.current &&
        ![...submissions.current].some((submission) => submission.scope === scope) &&
        owner === scope.viewerId &&
        targetAgent === scope.agentId &&
        active.current.entry === entry &&
        entry !== undefined &&
        !entry.error &&
        !entry.unread &&
        !entry.value.content &&
        entry.value.attachments.length === 0 &&
        entry.value.commandKey === undefined &&
        permitted();
      if (!scope.agentId || !canImport()) return;
      try {
        requireDraftIdentity();
        const imported = importEmptyAgentDraft(scope.viewerId, scope.agentId, text, canImport);
        if (!imported) return;
        const next = { ...active.current!, entry: { value: imported } };
        active.current = next;
        setStored(next);
        return {
          verify: () => {
            if (active.current?.scope !== scope || editing.current || !permitted()) return false;
            const saved = readAgentDraft(scope.viewerId, scope.agentId!);
            return (
              active.current.entry.value.revision === imported.revision &&
              saved.revision === imported.revision &&
              saved.content === text &&
              saved.attachments.length === 0 &&
              saved.commandKey === undefined
            );
          },
        };
      } catch {
        return undefined;
      }
    },
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
