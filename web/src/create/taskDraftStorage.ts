import type { AgentLifecycleCreateInput } from '../api/client';
import {
  compileTaskTemplate,
  legacyTemplate,
  parseTaskTemplate,
  type TaskTemplate,
} from './taskTemplateModel';
import { requireTaskStorage } from './taskIdentity';

export interface TaskDraft {
  readonly revision?: string;
  readonly text: string;
  readonly template?: TaskTemplate;
  readonly templates?: readonly TaskTemplate[];
  readonly repositoryUrl: string;
  readonly repositoryRef: string;
  readonly machineIncluded: boolean;
}
export interface SavedTaskDraft {
  readonly id: string;
  readonly draft: TaskDraft;
}
interface DraftRecord {
  readonly draft: TaskDraft;
  readonly legacyObserved: string | null;
  readonly recoverable: readonly SavedTaskDraft[];
}
export const emptyTaskDraft: TaskDraft = {
  text: '',
  repositoryUrl: '',
  repositoryRef: '',
  machineIncluded: false,
};
export const draftReadError =
  'Your saved task could not be read. Copy your text and restore browser session storage, then reload before starting a task.';

export function legacyTaskDraftStorageKey(
  owner?: string,
  workspaceId?: string,
  sandboxId?: string,
) {
  return `cma:task-draft:${JSON.stringify([location.origin, owner, workspaceId ?? null, sandboxId ?? null])}`;
}
export function taskDraftStorageKey(owner?: string, workspaceId?: string, sandboxId?: string) {
  return `cma:task-draft:v3:${JSON.stringify([location.origin, owner, workspaceId ?? null, sandboxId ?? null])}`;
}
function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Error('Invalid task draft');
  return value as Record<string, unknown>;
}
function validRevision(value: unknown): value is string {
  return typeof value === 'string' && /^[a-zA-Z0-9-]{1,200}$/.test(value);
}
function parseDraft(raw: unknown, legacy = false): TaskDraft {
  const value = object(raw);
  if (
    typeof value.text !== 'string' ||
    (value.revision !== undefined && !validRevision(value.revision)) ||
    (value.repositoryUrl !== undefined && typeof value.repositoryUrl !== 'string') ||
    (value.repositoryRef !== undefined && typeof value.repositoryRef !== 'string') ||
    (value.desktop !== undefined && typeof value.desktop !== 'boolean') ||
    (value.machineIncluded !== undefined && typeof value.machineIncluded !== 'boolean')
  )
    throw new Error('Invalid task draft');
  const template = legacy
    ? legacyTemplate(value.text)
    : value.template === undefined
      ? undefined
      : parseTaskTemplate(value.template);
  if (value.templates !== undefined && !Array.isArray(value.templates))
    throw new Error('Invalid template drafts');
  const templates = (value.templates as unknown[] | undefined)?.map(parseTaskTemplate);
  if (
    templates &&
    (templates.length > 5 || new Set(templates.map(({ id }) => id)).size !== templates.length)
  )
    throw new Error('Invalid template drafts');
  return {
    text: legacy && template ? '' : value.text,
    revision: value.revision as string | undefined,
    template,
    ...(templates ? { templates } : {}),
    repositoryUrl: (value.repositoryUrl as string | undefined) ?? '',
    repositoryRef: (value.repositoryRef as string | undefined) ?? '',
    machineIncluded: (value.machineIncluded as boolean | undefined) ?? false,
  };
}
function parseLegacy(raw: string): TaskDraft {
  const value = object(JSON.parse(raw));
  if (value.version !== 1 && value.version !== 2) throw new Error('Unknown task draft');
  return parseDraft(value, value.version === 1);
}
function parseRecord(raw: string): DraftRecord {
  const value = object(JSON.parse(raw));
  if (
    value.version !== 3 ||
    (value.legacyObserved !== null && typeof value.legacyObserved !== 'string') ||
    !Array.isArray(value.recoverable)
  )
    throw new Error('Unknown task draft');
  if (value.legacyObserved !== null) parseLegacy(value.legacyObserved);
  const recoverable = value.recoverable.map((entry) => {
    const saved = object(entry);
    if (!validRevision(saved.id)) throw new Error('Invalid saved draft');
    return { id: saved.id, draft: parseDraft(saved.draft) };
  });
  if (new Set(recoverable.map(({ id }) => id)).size !== recoverable.length)
    throw new Error('Duplicate saved drafts');
  return { draft: parseDraft(value), legacyObserved: value.legacyObserved, recoverable };
}
function content(draft: TaskDraft) {
  const templateContent = (template: TaskTemplate) => [
    template.id,
    template.version,
    Object.entries(template.fields).sort(([left], [right]) => left.localeCompare(right)),
  ];
  return JSON.stringify([
    draft.text,
    draft.template && templateContent(draft.template),
    [...(draft.templates ?? [])]
      .sort((left, right) => left.id.localeCompare(right.id))
      .map(templateContent),
    draft.repositoryUrl,
    draft.repositoryRef,
    draft.machineIncluded,
  ]);
}
function hasContent(draft: TaskDraft) {
  return Boolean(
    draft.text ||
    draft.repositoryUrl ||
    draft.repositoryRef ||
    draft.machineIncluded ||
    [draft.template, ...(draft.templates ?? [])].some(
      (template) => template && Object.values(template.fields).some((value) => value.length),
    ),
  );
}
function sameDraft(left: TaskDraft, right: TaskDraft) {
  return left.revision === right.revision && content(left) === content(right);
}
function keepRecovery(record: DraftRecord, draft: TaskDraft): DraftRecord {
  if (
    !hasContent(draft) ||
    sameDraft(draft, record.draft) ||
    record.recoverable.some((saved) => sameDraft(saved.draft, draft))
  )
    return record;
  return { ...record, recoverable: [...record.recoverable, { id: crypto.randomUUID(), draft }] };
}
function observeLegacy(record: DraftRecord, raw: string | null): DraftRecord {
  if (raw === record.legacyObserved) return record;
  const draft = raw === null ? undefined : parseLegacy(raw);
  const previous = record.legacyObserved === null ? undefined : parseLegacy(record.legacyObserved);
  const next =
    draft && (!previous || !sameDraft(draft, previous)) ? keepRecovery(record, draft) : record;
  return { ...next, legacyObserved: raw };
}

export function keepTaskTemplate(draft: TaskDraft, template: TaskTemplate): TaskDraft {
  return {
    ...draft,
    template,
    templates: [...(draft.templates ?? []).filter(({ id }) => id !== template.id), template],
  };
}
export function remainingTaskDraft(draft: TaskDraft): TaskDraft {
  const templates = draft.templates?.filter(
    (template) =>
      template.id !== draft.template?.id &&
      Object.values(template.fields).some((value) => value.length),
  );
  const text = draft.template ? draft.text : '';
  return text || templates?.length
    ? { ...draft, text, template: undefined, templates }
    : emptyTaskDraft;
}
export function matchesTaskDraft(draft: TaskDraft, body: AgentLifecycleCreateInput) {
  return (
    (draft.template ? compileTaskTemplate(draft.template).text : draft.text) ===
      body.initial_message &&
    JSON.stringify(draft.template) === JSON.stringify(body.template) &&
    draft.repositoryUrl === (body.repository?.url ?? '') &&
    draft.repositoryRef === (body.repository?.ref ?? '') &&
    draft.machineIncluded === (body.machine_included === true)
  );
}

export function openTaskDraftStore(owner?: string, workspaceId?: string, sandboxId?: string) {
  const key = taskDraftStorageKey(owner, workspaceId, sandboxId);
  const legacyKey = legacyTaskDraftStorageKey(owner, workspaceId, sandboxId);
  let expected: string | null;
  let record: DraftRecord;
  let dirty = false;
  const read = () => {
    requireTaskStorage();
    if (!owner) throw new Error('Missing draft owner');
    expected = sessionStorage.getItem(key);
    record =
      expected === null
        ? { draft: emptyTaskDraft, legacyObserved: null, recoverable: [] }
        : parseRecord(expected);
    try {
      const raw = sessionStorage.getItem(legacyKey);
      if (expected === null && raw !== null) {
        record = { draft: parseLegacy(raw), legacyObserved: raw, recoverable: [] };
        dirty = true;
      } else {
        const next = observeLegacy(record, raw);
        dirty = next !== record;
        record = next;
      }
      return { ...record, error: undefined };
    } catch {
      return { ...record, error: draftReadError };
    }
  };
  const write = (next: DraftRecord, restoredId?: string) => {
    requireTaskStorage();
    if (!owner) throw new Error('Missing draft owner');
    if (sessionStorage.getItem(key) !== expected)
      throw new Error('Another saved draft changed. Copy your text and reload before saving.');
    // Merge every observed predecessor value before the single owned-record write.
    record = observeLegacy(record, sessionStorage.getItem(legacyKey));
    next = { ...next, legacyObserved: record.legacyObserved, recoverable: [...next.recoverable] };
    for (const saved of record.recoverable) {
      if (saved.id !== restoredId && !next.recoverable.some((value) => value.id === saved.id))
        next = keepRecovery(next, saved.draft);
    }
    const raw = JSON.stringify({
      version: 3,
      ...next.draft,
      legacyObserved: next.legacyObserved,
      recoverable: next.recoverable,
    });
    sessionStorage.setItem(key, raw);
    expected = raw;
    record = next;
    dirty = false;
    return record;
  };
  return {
    key,
    read,
    get needsSave() {
      return dirty;
    },
    save: (draft: TaskDraft) => write({ ...record, draft }),
    importText: (text: string, preserve: boolean) => {
      if (sessionStorage.getItem(legacyKey) !== record.legacyObserved)
        throw new Error('Another saved draft changed. Review it before importing this request.');
      const next: DraftRecord = {
        ...record,
        draft: { ...emptyTaskDraft, text, revision: crypto.randomUUID() },
        recoverable: preserve
          ? [...record.recoverable, { id: crypto.randomUUID(), draft: record.draft }]
          : record.recoverable,
      };
      const saved = write(next);
      if (sessionStorage.getItem(key) !== expected)
        throw new Error('The imported request could not be verified in browser storage.');
      return saved;
    },
    restore: (id: string, current: TaskDraft) => {
      const saved = record.recoverable.find((value) => value.id === id);
      if (!saved) throw new Error('Saved draft is unavailable');
      const next = keepRecovery(
        {
          ...record,
          draft: { ...saved.draft, revision: crypto.randomUUID() },
          recoverable: record.recoverable.filter((value) => value.id !== id),
        },
        current,
      );
      return write(next, id);
    },
  };
}
