import { useEffect, useRef, useState } from 'react';
import type { AgentLifecycleCreateInput } from '../api/client';
import { requireTaskStorage } from './taskIdentity';

interface TaskDraft {
  readonly text: string;
  readonly repositoryUrl: string;
  readonly repositoryRef: string;
  readonly machineIncluded: boolean;
}
const empty: TaskDraft = { text: '', repositoryUrl: '', repositoryRef: '', machineIncluded: false };
const discarded = new Set<(key: string, body: AgentLifecycleCreateInput) => void>();

export function taskDraftStorageKey(owner?: string, workspaceId?: string, sandboxId?: string) {
  return `cma:task-draft:${JSON.stringify([location.origin, owner, workspaceId ?? null, sandboxId ?? null])}`;
}

function parse(raw: string): TaskDraft {
  const value = JSON.parse(raw) as {
    version?: unknown;
    text?: unknown;
    repositoryUrl?: unknown;
    repositoryRef?: unknown;
    desktop?: unknown;
    machineIncluded?: unknown;
  };
  if (
    value.version !== 1 ||
    typeof value.text !== 'string' ||
    (value.repositoryUrl !== undefined && typeof value.repositoryUrl !== 'string') ||
    (value.repositoryRef !== undefined && typeof value.repositoryRef !== 'string') ||
    (value.desktop !== undefined && typeof value.desktop !== 'boolean') ||
    (value.machineIncluded !== undefined && typeof value.machineIncluded !== 'boolean')
  )
    throw new Error('Invalid task draft');
  return {
    text: value.text,
    repositoryUrl: value.repositoryUrl ?? '',
    repositoryRef: value.repositoryRef ?? '',
    machineIncluded: value.machineIncluded ?? false,
  };
}

function matches(draft: TaskDraft, body: AgentLifecycleCreateInput) {
  return (
    draft.text === body.initial_message &&
    draft.repositoryUrl === (body.repository?.url ?? '') &&
    draft.repositoryRef === (body.repository?.ref ?? '') &&
    draft.machineIncluded === (body.machine_included === true)
  );
}

/** Called only when a confirmed successful creation clears its retained intent. */
export function discardSubmittedTaskDraft(owner: string, body: AgentLifecycleCreateInput) {
  if (body.initial_message === undefined) return;
  try {
    requireTaskStorage();
    const key = taskDraftStorageKey(owner, body.workspace_id, body.sandbox_id);
    const raw = sessionStorage.getItem(key);
    if (raw === null || !matches(parse(raw), body)) return;
    sessionStorage.removeItem(key);
    // sessionStorage does not emit a storage event in the document that changed it.
    for (const notify of discarded) notify(key, body);
  } catch {
    // Draft cleanup must never block access to the successfully created Agent.
  }
}

export function useTaskDraft(owner?: string, workspaceId?: string, sandboxId?: string) {
  const key = taskDraftStorageKey(owner, workspaceId, sandboxId);
  const [initial] = useState(() => {
    try {
      requireTaskStorage();
      if (!owner) throw new Error();
      const raw = sessionStorage.getItem(key);
      return { draft: raw === null ? empty : parse(raw) };
    } catch {
      return {
        draft: empty,
        error:
          'Your saved task could not be read. Restore browser session storage before starting a task.',
      };
    }
  });
  const [draft, setDraft] = useState(initial.draft);
  const current = useRef(initial.draft);
  const [error, setError] = useState(initial.error);
  useEffect(() => {
    const notify = (discardedKey: string, body: AgentLifecycleCreateInput) => {
      if (discardedKey !== key || !matches(current.current, body)) return;
      current.current = empty;
      setDraft(empty);
      setError(undefined);
    };
    discarded.add(notify);
    return () => {
      discarded.delete(notify);
    };
  }, [key]);
  const persist = (value: TaskDraft) => {
    try {
      requireTaskStorage();
      if (!owner) throw new Error();
      sessionStorage.setItem(key, JSON.stringify({ version: 1, ...value }));
      setError(undefined);
      return true;
    } catch {
      setError(
        'Your task could not be saved. Copy your text and restore browser session storage before starting a task.',
      );
      return false;
    }
  };
  const change = (value: TaskDraft) => {
    current.current = value;
    setDraft(value);
    persist(value);
  };
  return {
    ...draft,
    changeMachineIncluded: (enabled: boolean) => {
      change({ ...current.current, machineIncluded: enabled });
    },
    changeRepository: (url: string, reference: string) => {
      change({ ...current.current, repositoryUrl: url, repositoryRef: reference });
    },
    error,
    persist: () => persist(current.current),
    change: (text: string) => {
      change({ ...current.current, text });
    },
  };
}
