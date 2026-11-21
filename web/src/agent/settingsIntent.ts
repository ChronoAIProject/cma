import type { AgentSettingsView, UpdateAgentPreferences } from '../api/client';

export interface SettingsIntentIdentity {
  readonly key: string;
  readonly startedAt: number;
  readonly retryAt?: number;
}

export interface SettingsIntent<Body> extends SettingsIntentIdentity {
  readonly body: Body;
}

export interface OpaqueSettingsIntent<Body> extends SettingsIntentIdentity {
  recover(candidate: Body): Promise<Body | undefined>;
}

export interface SettingsIntentJournal<Body> {
  read(): SettingsIntent<Body> | OpaqueSettingsIntent<Body> | undefined;
  reserve(intent: SettingsIntent<Body>, signal?: AbortSignal): void | Promise<void>;
  defer(key: string, retryAt: number): void;
  complete(key: string): void;
}

export type RetainedSettingsEditor = 'settings' | 'rename';
type IntentStorage = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>;

const retentionMs = 24 * 60 * 60 * 1000;
const maxJournalLength = 200_000;
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

export const settingsJournalError =
  'The saved update could not be recovered from this tab. Reload to check the current settings before retrying.';

export function settingsIntentJournal<Body>(
  storage: IntentStorage,
  scope: string,
  requireIdentity: () => void,
  parseBody: (value: unknown) => Body | undefined,
) {
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
    reserve(intent: SettingsIntent<Body>) {
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
    defer(key: string, retryAt: number) {
      const current = read();
      if (current?.key === key) {
        storage.setItem(scope, JSON.stringify({ ...current, retryAt }));
      }
    },
    complete(key: string) {
      if (read()?.key === key) storage.removeItem(scope);
    },
  };
}

function parsePromptFingerprint(value: unknown) {
  const fields = record(value);
  if (
    !fields ||
    Object.keys(fields).length !== 2 ||
    typeof fields.expected_revision !== 'number' ||
    !Number.isSafeInteger(fields.expected_revision) ||
    fields.expected_revision < 0 ||
    typeof fields.fingerprint !== 'string' ||
    !/^[a-f0-9]{64}$/.test(fields.fingerprint)
  )
    return undefined;
  return { expected_revision: fields.expected_revision, fingerprint: fields.fingerprint };
}

async function promptFingerprint(
  key: string,
  proposal: { expected_revision: number; system_prompt: string },
) {
  const bytes = new TextEncoder().encode(
    JSON.stringify([
      'cma:prompt-recovery:v1',
      key,
      proposal.expected_revision,
      proposal.system_prompt,
    ]),
  );
  const digest = await crypto.subtle.digest('SHA-256', bytes);
  return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, '0')).join('');
}

/** A prompt can be replayed only from memory or from an exact re-entry. */
export function promptIntentJournal(
  storage: IntentStorage,
  scope: string,
  requireIdentity: () => void,
): SettingsIntentJournal<UpdateAgentPreferences> {
  const journal = settingsIntentJournal(storage, scope, requireIdentity, parsePromptFingerprint);
  return {
    ...journal,
    read() {
      const saved = journal.read();
      if (!saved) return undefined;
      const { body, ...identity } = saved;
      return {
        ...identity,
        async recover(candidate) {
          const original = parsePreferencesIntentBody(
            { expected_revision: body.expected_revision, system_prompt: candidate.system_prompt },
            'prompt',
          );
          if (
            original?.system_prompt === undefined ||
            (await promptFingerprint(identity.key, {
              expected_revision: original.expected_revision,
              system_prompt: original.system_prompt,
            })) !== body.fingerprint
          )
            return undefined;
          requireIdentity();
          return original;
        },
      };
    },
    async reserve({ body, key, startedAt }, signal) {
      const candidate = parsePreferencesIntentBody(body, 'prompt');
      if (candidate?.system_prompt === undefined) throw new Error(settingsJournalError);
      const expected_revision = candidate.expected_revision;
      const fingerprint = await promptFingerprint(key, {
        expected_revision,
        system_prompt: candidate.system_prompt,
      });
      signal?.throwIfAborted();
      journal.reserve({ key, startedAt, body: { expected_revision, fingerprint } });
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
