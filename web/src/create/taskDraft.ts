import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { AgentLifecycleCreateInput } from '../api/client';
import {
  compileTaskTemplate,
  emptyTemplate,
  type TaskTemplate,
  type TaskTemplateId,
} from './taskTemplateModel';
import {
  draftReadError,
  emptyTaskDraft,
  keepTaskTemplate,
  matchesTaskDraft,
  openTaskDraftStore,
  remainingTaskDraft,
  taskDraftStorageKey,
  type TaskDraft,
} from './taskDraftStorage';
export { taskDraftStorageKey, legacyTaskDraftStorageKey } from './taskDraftStorage';

const saveError =
  'Your task could not be saved. Copy your text and restore browser session storage before starting a task.';
const discarded = new Set<
  (key: string, body: AgentLifecycleCreateInput, revision: string | undefined) => void
>();
const mountedDrafts = new Map<string, number>();
export function taskDraftIsMounted(key: string) {
  return (mountedDrafts.get(key) ?? 0) > 0;
}

export function discardSubmittedTaskDraft(
  owner: string,
  body: AgentLifecycleCreateInput,
  revision?: string,
  reason: 'completed' | 'discarded' = 'completed',
) {
  if (body.initial_message === undefined || (reason === 'completed' && revision === undefined))
    return;
  try {
    const store = openTaskDraftStore(owner, body.workspace_id, body.sandbox_id);
    const saved = store.read();
    if (saved.error) throw new Error(saved.error);
    if (saved.draft.revision !== revision || !matchesTaskDraft(saved.draft, body)) return;
    const remaining = remainingTaskDraft(saved.draft);
    store.save(
      reason === 'discarded'
        ? { ...remaining, repositoryUrl: '', repositoryRef: '', machineIncluded: false }
        : remaining,
    );
    for (const notify of discarded) notify(store.key, body, saved.draft.revision);
  } catch (error) {
    if (reason === 'discarded') throw error;
    // Draft cleanup must never block access to the successfully created Agent.
  }
}

export function useTaskDraft(
  owner?: string,
  workspaceId?: string,
  sandboxId?: string,
  allowV2 = false,
) {
  const key = taskDraftStorageKey(owner, workspaceId, sandboxId);
  useLayoutEffect(() => {
    mountedDrafts.set(key, (mountedDrafts.get(key) ?? 0) + 1);
    return () => {
      const count = (mountedDrafts.get(key) ?? 1) - 1;
      if (count) mountedDrafts.set(key, count);
      else mountedDrafts.delete(key);
    };
  }, [key]);
  const [initial] = useState(() => {
    const store = openTaskDraftStore(owner, workspaceId, sandboxId);
    try {
      return { store, ...store.read() };
    } catch {
      return { store, draft: emptyTaskDraft, recoverable: [], error: draftReadError };
    }
  });
  const [draft, setDraft] = useState(initial.draft);
  const current = useRef(initial.draft);
  const [error, setError] = useState(initial.error);
  const [recoverable, setRecoverable] = useState(initial.recoverable);
  useEffect(() => {
    if (initial.error || !initial.store.needsSave) return;
    try {
      const saved = initial.store.save(current.current);
      setRecoverable(saved.recoverable);
    } catch {
      setError(saveError);
    }
  }, [initial]);
  useEffect(() => {
    const notify = (
      discardedKey: string,
      body: AgentLifecycleCreateInput,
      revision: string | undefined,
    ) => {
      if (
        discardedKey !== key ||
        current.current.revision !== revision ||
        !matchesTaskDraft(current.current, body)
      )
        return;
      const saved = initial.store.read();
      current.current = saved.draft;
      setDraft(saved.draft);
      setRecoverable(saved.recoverable);
      setError(saved.error);
    };
    discarded.add(notify);
    return () => {
      discarded.delete(notify);
    };
  }, [initial, key]);
  const persist = (value: TaskDraft) => {
    if (initial.error) {
      setError(initial.error);
      return undefined;
    }
    try {
      const saved = initial.store.save(value);
      current.current = saved.draft;
      setDraft(saved.draft);
      setRecoverable(saved.recoverable);
      setError(undefined);
      return value.revision;
    } catch (reason) {
      setError(
        reason instanceof Error && reason.message.startsWith('Another saved draft')
          ? reason.message
          : saveError,
      );
      return undefined;
    }
  };
  const change = (value: TaskDraft) => {
    const edited = { ...value, revision: crypto.randomUUID() };
    current.current = edited;
    setDraft(edited);
    persist(edited);
  };
  const switchDraft = (value: TaskDraft) =>
    Boolean(persist({ ...value, revision: crypto.randomUUID() }));
  return {
    ...draft,
    recoverable,
    restoreSavedDraft: (id: string) => {
      if (initial.error) return false;
      try {
        const saved = initial.store.restore(id, current.current);
        current.current = saved.draft;
        setDraft(saved.draft);
        setRecoverable(saved.recoverable);
        setError(undefined);
        return true;
      } catch (reason) {
        setError(
          reason instanceof Error && reason.message.startsWith('Another saved draft')
            ? reason.message
            : saveError,
        );
        return false;
      }
    },
    selectTemplate: (id: TaskTemplateId) => {
      const draft = current.current;
      const retained = draft.template ? keepTaskTemplate(draft, draft.template) : draft;
      return switchDraft(
        keepTaskTemplate(
          retained,
          retained.templates?.find((template) => template.id === id) ??
            emptyTemplate(id, allowV2 ? 2 : 1),
        ),
      );
    },
    changeTemplate: (template: TaskTemplate) => change(keepTaskTemplate(current.current, template)),
    clearTemplate: () => {
      const template = current.current.template;
      const compiled = template ? compileTaskTemplate(template) : undefined;
      return switchDraft({
        ...(template ? keepTaskTemplate(current.current, template) : current.current),
        template: undefined,
        text: compiled && !compiled.issue ? compiled.text : current.current.text,
      });
    },
    changeMachineIncluded: (enabled: boolean) => {
      change({ ...current.current, machineIncluded: enabled });
    },
    changeRepository: (url: string, reference: string) => {
      change({ ...current.current, repositoryUrl: url, repositoryRef: reference });
    },
    error,
    persist: (template = current.current.template) => {
      const currentDraft = current.current;
      const submittedDraft = template ? keepTaskTemplate(currentDraft, template) : currentDraft;
      return persist({ ...submittedDraft, revision: crypto.randomUUID() });
    },
    change: (text: string) => {
      change({ ...current.current, text });
    },
  };
}
