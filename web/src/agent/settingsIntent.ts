import type { AgentSettingsView, UpdateAgentPreferences } from '../api/client';

export interface SettingsIntent<Body> {
  readonly key: string;
  readonly body: Body;
  readonly startedAt: number;
  readonly retryAt?: number;
}

export interface SettingsIntentJournal<Body> {
  read(): SettingsIntent<Body> | undefined;
  reserve(intent: SettingsIntent<Body>): void;
  defer(key: string, retryAt: number): void;
  complete(key: string): void;
}

export type SettingsEditorKind = 'settings' | 'rename' | 'prompt';

const retentionMs = 24 * 60 * 60 * 1000;
const maxJournalLength = 200_000;
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

export const settingsJournalError =
  'The saved update could not be recovered from this tab. Reload to check the current settings before retrying.';

export function settingsIntentJournal<Body>(
  storage: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>,
  scope: string,
  requireIdentity: () => void,
  parseBody: (value: unknown) => Body | undefined,
): SettingsIntentJournal<Body> {
  const read = () => {
    requireIdentity();
    const raw = storage.getItem(scope);
    if (raw === null) return undefined;
    if (raw.length > maxJournalLength) throw new Error(settingsJournalError);
    const value = record(JSON.parse(raw));
    const body = parseBody(value?.body);
    const { key, startedAt, retryAt } = value ?? {};
    if (
      typeof key !== 'string' ||
      !uuid.test(key) ||
      typeof startedAt !== 'number' ||
      !Number.isSafeInteger(startedAt) ||
      startedAt < 0 ||
      (retryAt !== undefined &&
        (typeof retryAt !== 'number' || !Number.isSafeInteger(retryAt) || retryAt < startedAt)) ||
      body === undefined
    )
      throw new Error(settingsJournalError);
    return { key, startedAt, body, ...(retryAt === undefined ? {} : { retryAt }) };
  };
  return {
    read,
    reserve(intent) {
      const current = read();
      const now = Date.now();
      if (
        intent.startedAt > now ||
        now - intent.startedAt >= retentionMs ||
        !parseBody(intent.body) ||
        (current &&
          (current.key !== intent.key ||
            current.startedAt !== intent.startedAt ||
            JSON.stringify(current.body) !== JSON.stringify(intent.body)))
      )
        throw new Error(settingsJournalError);
      if (!current) storage.setItem(scope, JSON.stringify(intent));
    },
    defer(key, retryAt) {
      const current = read();
      if (current?.key === key) {
        storage.setItem(scope, JSON.stringify({ ...current, retryAt }));
      }
    },
    complete(key) {
      if (read()?.key === key) storage.removeItem(scope);
    },
  };
}

function record(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

export function parseSettingsIntentBody(value: unknown): AgentSettingsView | undefined {
  const fields = record(value);
  if (!fields || Object.keys(fields).length !== 3) return undefined;
  const { model, reasoning_effort, permission_mode } = fields;
  if (typeof model !== 'string' || !model.trim() || model.length > 256) return undefined;
  if (
    reasoning_effort !== 'minimal' &&
    reasoning_effort !== 'low' &&
    reasoning_effort !== 'medium' &&
    reasoning_effort !== 'high' &&
    reasoning_effort !== 'x_high'
  )
    return undefined;
  if (permission_mode !== 'guarded' && permission_mode !== 'autonomous') return undefined;
  return { model, reasoning_effort, permission_mode };
}

export function parsePreferencesIntentBody(
  value: unknown,
  kind: 'rename' | 'prompt',
): UpdateAgentPreferences | undefined {
  const fields = record(value);
  if (!fields || Object.keys(fields).length !== 2) return undefined;
  const { expected_revision, title, system_prompt } = fields;
  if (
    typeof expected_revision !== 'number' ||
    !Number.isSafeInteger(expected_revision) ||
    expected_revision < 0
  )
    return undefined;
  if (kind === 'rename' && typeof title === 'string' && title.trim() && [...title].length <= 64)
    return { expected_revision, title };
  if (
    kind === 'prompt' &&
    typeof system_prompt === 'string' &&
    new TextEncoder().encode(system_prompt).length <= 32768
  )
    return { expected_revision, system_prompt };
  return undefined;
}
