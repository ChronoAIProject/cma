import { useEffect, useRef, useState } from 'react';
import type { AgentLifecycleCreateInput } from '../api/client';
import {
  compileTaskTemplate,
  emptyTemplate,
  legacyTemplate,
  parseTaskTemplate,
  type TaskTemplate,
  type TaskTemplateId,
} from './taskTemplateModel';
import { requireTaskStorage } from './taskIdentity';

interface TaskDraft {
  readonly revision?: string;
  readonly text: string;
  readonly template?: TaskTemplate;
  readonly repositoryUrl: string;
  readonly repositoryRef: string;
  readonly machineIncluded: boolean;
}
const empty: TaskDraft = { text: '', repositoryUrl: '', repositoryRef: '', machineIncluded: false };
const discarded = new Set<
  (key: string, body: AgentLifecycleCreateInput, revision: string) => void
>();

export function taskDraftStorageKey(owner?: string, workspaceId?: string, sandboxId?: string) {
  return `cma:task-draft:${JSON.stringify([location.origin, owner, workspaceId ?? null, sandboxId ?? null])}`;
}

function parse(raw: string): TaskDraft {
  const value = JSON.parse(raw) as {
    version?: unknown;
    revision?: unknown;
    text?: unknown;
    template?: unknown;
    repositoryUrl?: unknown;
    repositoryRef?: unknown;
    desktop?: unknown;
    machineIncluded?: unknown;
  };
  if (
    (value.version !== 1 && value.version !== 2) ||
    typeof value.text !== 'string' ||
    (value.revision !== undefined &&
      (typeof value.revision !== 'string' || !/^[a-zA-Z0-9-]{1,200}$/.test(value.revision))) ||
    (value.repositoryUrl !== undefined && typeof value.repositoryUrl !== 'string') ||
    (value.repositoryRef !== undefined && typeof value.repositoryRef !== 'string') ||
    (value.desktop !== undefined && typeof value.desktop !== 'boolean') ||
    (value.machineIncluded !== undefined && typeof value.machineIncluded !== 'boolean')
  )
    throw new Error('Invalid task draft');
  return {
    text: value.text,
    revision: value.revision as string | undefined,
    template:
      value.version === 1
        ? legacyTemplate(value.text)
        : value.template === undefined
          ? undefined
          : parseTaskTemplate(value.template),
    repositoryUrl: value.repositoryUrl ?? '',
    repositoryRef: value.repositoryRef ?? '',
    machineIncluded: value.machineIncluded ?? false,
  };
}

function matches(draft: TaskDraft, body: AgentLifecycleCreateInput) {
  return (
    (draft.template ? compileTaskTemplate(draft.template).text : draft.text) ===
      body.initial_message &&
    JSON.stringify(draft.template) === JSON.stringify(body.template) &&
    draft.repositoryUrl === (body.repository?.url ?? '') &&
    draft.repositoryRef === (body.repository?.ref ?? '') &&
    draft.machineIncluded === (body.machine_included === true)
  );
}

/** Called only when a confirmed successful creation clears its retained intent. */
export function discardSubmittedTaskDraft(
  owner: string,
  body: AgentLifecycleCreateInput,
  revision?: string,
) {
  if (body.initial_message === undefined || revision === undefined) return;
  try {
    requireTaskStorage();
    const key = taskDraftStorageKey(owner, body.workspace_id, body.sandbox_id);
    const raw = sessionStorage.getItem(key);
    if (raw === null) return;
    const draft = parse(raw);
    if (draft.revision !== revision || !matches(draft, body)) return;
    sessionStorage.removeItem(key);
    // sessionStorage does not emit a storage event in the document that changed it.
    for (const notify of discarded) notify(key, body, revision);
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
    const notify = (discardedKey: string, body: AgentLifecycleCreateInput, revision: string) => {
      if (
        discardedKey !== key ||
        current.current.revision !== revision ||
        !matches(current.current, body)
      )
        return;
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
      sessionStorage.setItem(key, JSON.stringify({ version: 2, ...value }));
      current.current = value;
      setDraft(value);
      setError(undefined);
      return value.revision;
    } catch {
      setError(
        'Your task could not be saved. Copy your text and restore browser session storage before starting a task.',
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
  return {
    ...draft,
    selectTemplate: (id: TaskTemplateId) =>
      change({ ...current.current, template: emptyTemplate(id) }),
    changeTemplate: (template: TaskTemplate) => change({ ...current.current, template }),
    clearTemplate: () => {
      const template = current.current.template;
      const compiled = template ? compileTaskTemplate(template) : undefined;
      change({
        ...current.current,
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
    persist: () => persist({ ...current.current, revision: crypto.randomUUID() }),
    change: (text: string) => {
      change({ ...current.current, text });
    },
  };
}
