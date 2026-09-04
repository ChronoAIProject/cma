import { useEffect, useId, useMemo, useState, type ReactNode } from 'react';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import {
  IconChevronDownOutline14,
  IconChevronUpOutline14,
  IconPlusOutline16,
  IconTrashOutline16,
} from '../ui/icons';
import { PlatformSettingsConflict } from './types';
import type { ChatSettings, OperatorStore, StoredChatSettings } from './types';
import './ChatSettingsView.css';

interface ChatSettingsViewProps {
  readonly store: OperatorStore;
}

type Field = 'repository' | 'branch' | 'prefixes' | 'commands' | 'pause' | 'threads' | 'size';

export interface ChatSettingsValidation {
  readonly valid: boolean;
  readonly errors: Partial<Record<Field, string>>;
}

const DEFAULT_CHAT: ChatSettings = {
  enabled: false,
  workspace: {
    repositoryUrl: 'https://github.com/the-omega-institute/trureturing',
    branch: 'dev',
  },
  agent: {
    model: null,
    reasoningEffort: 'high',
    instructions: '',
    recordToolBodies: true,
  },
  messagePrefixes: [],
  inactivity: { pauseAfterSeconds: 900 },
  setup: {
    commands: [
      'set -eu; found=0; for skill in skills/*; do test -f "$skill/SKILL.md" || continue; name=${skill##*/}; case "$name" in \'\'|*[!a-z0-9_-]*) exit 1;; esac; found=1; mkdir -p "$HOME/.agents/skills/$name" "$HOME/.codex/skills/$name"; cp -R "$skill"/. "$HOME/.agents/skills/$name/"; cp -R "$skill"/. "$HOME/.codex/skills/$name/"; done; test "$found" -eq 1',
    ],
  },
  host: { size: null, maxThreads: 10 },
};

const DEFAULT_RETAINED_SIZE = { cpuMillis: 2_000, memoryMib: 4_096 };
const MAX_U32 = 4_294_967_295;
const UTF8 = new TextEncoder();
const LEAN_SETUP_FIXTURE =
  '[ -x .toolchain/bin/lake ] || (curl -sSf https://elan.lean-lang.org/elan-init.sh | ELAN_HOME=$PWD/.toolchain sh -s -- -y --default-toolchain "$(cat lean-toolchain)")';

function byteLength(value: string): number {
  return UTF8.encode(value).byteLength;
}

function copyChat(settings: ChatSettings): ChatSettings {
  return {
    ...settings,
    workspace: { ...settings.workspace },
    agent: { ...settings.agent },
    messagePrefixes: [...settings.messagePrefixes],
    inactivity: { ...settings.inactivity },
    setup: { commands: [...settings.setup.commands] },
    host: {
      ...settings.host,
      size: settings.host.size === null ? null : { ...settings.host.size },
    },
  };
}

function currentChat(stored: StoredChatSettings): ChatSettings {
  return copyChat(stored.chat ?? DEFAULT_CHAT);
}

export function validateChatSettings(settings: ChatSettings): ChatSettingsValidation {
  const errors: Partial<Record<Field, string>> = {};
  const repository = settings.workspace.repositoryUrl;
  try {
    const parsed = new URL(repository);
    const basename = parsed.pathname
      .split('/')
      .filter(Boolean)
      .at(-1)
      ?.replace(/\.git$/u, '');
    if (
      byteLength(repository) === 0 ||
      byteLength(repository) > 512 ||
      parsed.protocol !== 'https:' ||
      parsed.host === '' ||
      parsed.username !== '' ||
      parsed.password !== '' ||
      parsed.search !== '' ||
      parsed.hash !== '' ||
      basename === undefined ||
      !/^(?!\.{1,2}$)[A-Za-z0-9._-]{1,255}$/u.test(basename)
    ) {
      errors.repository = 'Use a credential-free HTTPS repository URL with a safe name.';
    }
  } catch {
    errors.repository = 'Use a credential-free HTTPS repository URL with a safe name.';
  }
  if (
    byteLength(settings.workspace.branch) === 0 ||
    byteLength(settings.workspace.branch) > 200 ||
    /\s/u.test(settings.workspace.branch)
  ) {
    errors.branch = 'Use 1 to 200 UTF-8 bytes with no whitespace.';
  }
  if (
    settings.messagePrefixes.length > 8 ||
    settings.messagePrefixes.some(
      (prefix) => prefix.trim() === '' || byteLength(prefix) > 256 || /[\r\n]/u.test(prefix),
    )
  ) {
    errors.prefixes = 'Use at most 8 non-empty single-line entries, 256 UTF-8 bytes each.';
  }
  if (
    settings.setup.commands.length > 16 ||
    settings.setup.commands.some((command) => byteLength(command) > 4_096)
  ) {
    errors.commands = 'Use at most 16 commands, 4096 characters each.';
  }
  if (
    !Number.isInteger(settings.inactivity.pauseAfterSeconds) ||
    settings.inactivity.pauseAfterSeconds < 60 ||
    settings.inactivity.pauseAfterSeconds > 86_400
  ) {
    errors.pause = 'Use a duration from 1 to 1440 minutes, in whole seconds.';
  }
  if (
    !Number.isInteger(settings.host.maxThreads) ||
    settings.host.maxThreads < 1 ||
    settings.host.maxThreads > 50
  ) {
    errors.threads = 'Use a whole number from 1 to 50.';
  }
  if (
    settings.host.size !== null &&
    (!Number.isSafeInteger(settings.host.size.cpuMillis) ||
      settings.host.size.cpuMillis < 1 ||
      settings.host.size.cpuMillis > MAX_U32 ||
      !Number.isSafeInteger(settings.host.size.memoryMib) ||
      settings.host.size.memoryMib < 1 ||
      settings.host.size.memoryMib > MAX_U32)
  ) {
    errors.size = 'Use whole numbers from 1 to 4,294,967,295.';
  }
  return { valid: Object.keys(errors).length === 0, errors };
}

export function ChatSettingsView({ store }: ChatSettingsViewProps) {
  const id = useId();
  const [stored, setStored] = useState<StoredChatSettings | null>(null);
  const [draft, setDraft] = useState<ChatSettings | null>(null);
  const [retainedSize, setRetainedSize] = useState(DEFAULT_RETAINED_SIZE);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [conflict, setConflict] = useState(false);
  const [saved, setSaved] = useState(false);
  const [readAttempt, setReadAttempt] = useState(0);

  useEffect(() => {
    let live = true;
    store
      .readChatSettings()
      .then((value) => {
        if (!live) return;
        const settings = currentChat(value);
        setStored(value);
        setDraft(settings);
        if (settings.host.size !== null) setRetainedSize({ ...settings.host.size });
      })
      .catch((cause: unknown) => {
        if (live) setFailure(cause instanceof Error ? cause.message : String(cause));
      });
    return () => {
      live = false;
    };
  }, [readAttempt, store]);

  const validation = useMemo(
    () => (draft === null ? { valid: false, errors: {} } : validateChatSettings(draft)),
    [draft],
  );
  const changed =
    stored !== null &&
    draft !== null &&
    JSON.stringify(currentChat(stored)) !== JSON.stringify(draft);

  function update(change: (settings: ChatSettings) => ChatSettings): void {
    setDraft((value) => (value === null ? value : change(value)));
    setFailure(null);
    setSaved(false);
  }

  async function save(): Promise<void> {
    if (stored === null || draft === null || !validation.valid) return;
    setBusy(true);
    setFailure(null);
    setConflict(false);
    setSaved(false);
    try {
      const next = await store.changeChatSettings(stored.revision, draft);
      setStored(next);
      setDraft(currentChat(next));
      setSaved(true);
    } catch (cause) {
      if (cause instanceof PlatformSettingsConflict) {
        try {
          const latest = await store.readChatSettings();
          setStored(latest);
          setConflict(true);
        } catch (readCause) {
          setFailure(readCause instanceof Error ? readCause.message : String(readCause));
        }
      } else {
        setFailure(cause instanceof Error ? cause.message : String(cause));
      }
    } finally {
      setBusy(false);
    }
  }

  if (draft === null || stored === null) {
    return failure === null ? (
      <p className="operator__waiting">Reading chat settings...</p>
    ) : (
      <div className="notice notice--fail chat-settings__read-error" role="alert">
        <div>
          <strong className="notice__title">Chat settings could not be read</strong>
          <p className="notice__detail">{failure}</p>
        </div>
        <Button
          size="sm"
          variant="outline"
          onClick={() => {
            setFailure(null);
            setReadAttempt((attempt) => attempt + 1);
          }}
        >
          Try again
        </Button>
      </div>
    );
  }

  return (
    <form
      className="chat-settings"
      aria-label="Chat settings"
      onSubmit={(event) => {
        event.preventDefault();
        void save();
      }}
    >
      <div className="chat-settings__head">
        <div>
          <h2 className="chat-settings__title">Chat settings</h2>
          <p className="chat-settings__revision">Revision {String(stored.revision)}</p>
        </div>
      </div>

      {failure === null ? null : (
        <p className="chat-settings__notice chat-settings__notice--failure" role="alert">
          {failure}
        </p>
      )}
      {conflict ? (
        <div className="notice notice--quiet chat-settings__notice" role="alert">
          <div>
            <strong className="notice__title">Settings changed elsewhere</strong>
            <p className="notice__detail chat-settings__notice-detail">
              Chat settings changed after this draft was opened. Your draft is unchanged. Revision{' '}
              {String(stored.revision)} is current.
            </p>
          </div>
          <div className="chat-settings__notice-actions">
            <Button
              size="sm"
              variant="outline"
              onClick={() => {
                const latest = currentChat(stored);
                setDraft(latest);
                if (latest.host.size !== null) setRetainedSize({ ...latest.host.size });
                setConflict(false);
              }}
            >
              Load current values
            </Button>
            <Button size="sm" onClick={() => setConflict(false)}>
              Review retained draft
            </Button>
          </div>
        </div>
      ) : null}
      {saved ? (
        <p className="chat-settings__saved" role="status">
          Saved revision {String(stored.revision)}.
        </p>
      ) : null}

      <fieldset className="chat-settings__controls" disabled={busy}>
        <SettingsSection id={`${id}-workspace`} title="Workspace">
          <Field label="Repository URL" error={validation.errors.repository}>
            <Input
              aria-invalid={validation.errors.repository !== undefined}
              value={draft.workspace.repositoryUrl}
              onChange={(event) =>
                update((settings) => ({
                  ...settings,
                  workspace: { ...settings.workspace, repositoryUrl: event.target.value },
                }))
              }
            />
          </Field>
          <Field label="Branch" error={validation.errors.branch}>
            <Input
              aria-invalid={validation.errors.branch !== undefined}
              value={draft.workspace.branch}
              onChange={(event) =>
                update((settings) => ({
                  ...settings,
                  workspace: { ...settings.workspace, branch: event.target.value },
                }))
              }
            />
          </Field>
        </SettingsSection>

        <SettingsSection id={`${id}-agent`} title="Agent">
          <Field label="Model">
            <Input
              value={draft.agent.model ?? ''}
              placeholder="Deployment default"
              onChange={(event) =>
                update((settings) => ({
                  ...settings,
                  agent: { ...settings.agent, model: event.target.value || null },
                }))
              }
            />
          </Field>
          <label className="chat-settings__field">
            <span className="chat-settings__label">Thinking effort</span>
            <select
              className="chat-settings__select"
              value={draft.agent.reasoningEffort}
              onChange={(event) =>
                update((settings) => ({
                  ...settings,
                  agent: {
                    ...settings.agent,
                    reasoningEffort: event.target.value as ChatSettings['agent']['reasoningEffort'],
                  },
                }))
              }
            >
              {['minimal', 'low', 'medium', 'high', 'x_high'].map((effort) => (
                <option key={effort} value={effort}>
                  {effort.replace('_', ' ')}
                </option>
              ))}
            </select>
          </label>
          <label className="chat-settings__field chat-settings__field--wide">
            <span className="chat-settings__label">Instructions (agent.md)</span>
            <textarea
              className="chat-settings__textarea"
              rows={7}
              value={draft.agent.instructions}
              onChange={(event) =>
                update((settings) => ({
                  ...settings,
                  agent: { ...settings.agent, instructions: event.target.value },
                }))
              }
            />
          </label>
          <label className="chat-settings__check chat-settings__field--wide">
            <input
              type="checkbox"
              checked={draft.agent.recordToolBodies}
              onChange={(event) =>
                update((settings) => ({
                  ...settings,
                  agent: { ...settings.agent, recordToolBodies: event.target.checked },
                }))
              }
            />
            Record bounded tool bodies
          </label>
          <p className="chat-settings__fixed chat-settings__field--wide">
            Chat sessions run with full access (unrestricted).
          </p>
        </SettingsSection>

        <SettingsSection id={`${id}-messages`} title="Messages">
          <OrderedEditor
            label="Prepended texts"
            values={draft.messagePrefixes}
            maximum={8}
            error={validation.errors.prefixes}
            onChange={(messagePrefixes) => update((settings) => ({ ...settings, messagePrefixes }))}
          />
        </SettingsSection>

        <SettingsSection id={`${id}-setup`} title="Setup">
          <p className="chat-settings__caption chat-settings__field--wide">
            Run inside the prepared repository and again after every revive. Commands run as uid
            10001 without root access. Keep them idempotent and prefer workspace-local installation
            so toolchains survive pause checkpoints.
          </p>
          <p className="chat-settings__fixture chat-settings__field--wide">
            <span>Lean fixture</span>
            <code>{LEAN_SETUP_FIXTURE}</code>
          </p>
          <OrderedEditor
            label="Software-installation commands"
            values={draft.setup.commands}
            maximum={16}
            multiline
            error={validation.errors.commands}
            onChange={(commands) => update((settings) => ({ ...settings, setup: { commands } }))}
          />
        </SettingsSection>

        <SettingsSection id={`${id}-lifetime`} title="Lifetime">
          <Field label="Auto-pause after (minutes)" error={validation.errors.pause}>
            <Input
              type="number"
              min={1}
              max={1_440}
              step="any"
              aria-invalid={validation.errors.pause !== undefined}
              value={draft.inactivity.pauseAfterSeconds / 60}
              onChange={(event) =>
                update((settings) => ({
                  ...settings,
                  inactivity: { pauseAfterSeconds: Number(event.target.value) * 60 },
                }))
              }
            />
          </Field>
          <Field label="Max chat windows per user" error={validation.errors.threads}>
            <Input
              type="number"
              min={1}
              max={50}
              aria-invalid={validation.errors.threads !== undefined}
              value={draft.host.maxThreads}
              onChange={(event) =>
                update((settings) => ({
                  ...settings,
                  host: { ...settings.host, maxThreads: Number(event.target.value) },
                }))
              }
            />
          </Field>
        </SettingsSection>

        <SettingsSection id={`${id}-host-resources`} title="Host resources">
          <label className="chat-settings__check chat-settings__field--wide">
            <input
              type="checkbox"
              checked={draft.host.size === null}
              onChange={(event) =>
                update((settings) => ({
                  ...settings,
                  host: {
                    ...settings.host,
                    size: event.target.checked ? null : { ...retainedSize },
                  },
                }))
              }
            />
            Use deployment default
          </label>
          <Field label="CPU (millicores)" error={validation.errors.size}>
            <Input
              type="number"
              min={1}
              max={MAX_U32}
              aria-invalid={validation.errors.size !== undefined}
              disabled={draft.host.size === null}
              placeholder={draft.host.size === null ? 'Deployment default' : undefined}
              value={draft.host.size?.cpuMillis ?? ''}
              onChange={(event) => {
                const cpuMillis = Number(event.target.value);
                setRetainedSize((size) => ({ ...size, cpuMillis }));
                update((settings) => ({
                  ...settings,
                  host: {
                    ...settings.host,
                    size: settings.host.size === null ? null : { ...settings.host.size, cpuMillis },
                  },
                }));
              }}
            />
          </Field>
          <Field label="Memory (MiB)" error={validation.errors.size}>
            <Input
              type="number"
              min={1}
              max={MAX_U32}
              aria-invalid={validation.errors.size !== undefined}
              disabled={draft.host.size === null}
              placeholder={draft.host.size === null ? 'Deployment default' : undefined}
              value={draft.host.size?.memoryMib ?? ''}
              onChange={(event) => {
                const memoryMib = Number(event.target.value);
                setRetainedSize((size) => ({ ...size, memoryMib }));
                update((settings) => ({
                  ...settings,
                  host: {
                    ...settings.host,
                    size: settings.host.size === null ? null : { ...settings.host.size, memoryMib },
                  },
                }));
              }}
            />
          </Field>
        </SettingsSection>

        <SettingsSection id={`${id}-enabled`} title="Enabled">
          <label className="chat-settings__check chat-settings__field--wide">
            <input
              type="checkbox"
              role="switch"
              checked={draft.enabled}
              onChange={(event) =>
                update((settings) => ({ ...settings, enabled: event.target.checked }))
              }
            />
            Accept new chats
          </label>
        </SettingsSection>
      </fieldset>

      <div className="chat-settings__actions">
        <Button variant="primary" type="submit" disabled={busy || !changed || !validation.valid}>
          {busy ? 'Saving...' : 'Save chat settings'}
        </Button>
      </div>
    </form>
  );
}

function SettingsSection({
  id,
  title,
  children,
}: {
  readonly id: string;
  readonly title: string;
  readonly children: ReactNode;
}) {
  return (
    <section className="chat-settings__section" aria-labelledby={id}>
      <h3 id={id} className="chat-settings__heading">
        {title}
      </h3>
      <div className="chat-settings__fields">{children}</div>
    </section>
  );
}

function Field({
  label,
  error,
  children,
}: {
  readonly label: string;
  readonly error?: string;
  readonly children: ReactNode;
}) {
  return (
    <div className="chat-settings__field">
      <label className="chat-settings__field-control">
        <span className="chat-settings__label">{label}</span>
        {children}
      </label>
      {error === undefined ? null : <span className="chat-settings__field-error">{error}</span>}
    </div>
  );
}

function OrderedEditor({
  label,
  values,
  maximum,
  multiline = false,
  error,
  onChange,
}: {
  readonly label: string;
  readonly values: readonly string[];
  readonly maximum: number;
  readonly multiline?: boolean;
  readonly error?: string;
  readonly onChange: (values: string[]) => void;
}) {
  function move(index: number, offset: -1 | 1): void {
    const target = index + offset;
    if (target < 0 || target >= values.length) return;
    const next = [...values];
    [next[index], next[target]] = [next[target] ?? '', next[index] ?? ''];
    onChange(next);
  }

  return (
    <div className="chat-settings__ordered chat-settings__field--wide">
      <span className="chat-settings__label">{label}</span>
      {values.map((value, index) => (
        <div className="chat-settings__ordered-row" key={index}>
          {multiline ? (
            <textarea
              className="chat-settings__textarea"
              rows={2}
              aria-label={`${label} ${String(index + 1)}`}
              aria-invalid={error !== undefined}
              value={value}
              onChange={(event) => {
                const next = [...values];
                next[index] = event.target.value;
                onChange(next);
              }}
            />
          ) : (
            <Input
              aria-label={`${label} ${String(index + 1)}`}
              aria-invalid={error !== undefined}
              value={value}
              onChange={(event) => {
                const next = [...values];
                next[index] = event.target.value;
                onChange(next);
              }}
            />
          )}
          <div className="chat-settings__ordered-actions">
            <Button
              size="sm"
              placement="chat-settings__icon"
              icon={<IconChevronUpOutline14 />}
              aria-label={`Move ${label.toLowerCase()} ${String(index + 1)} up`}
              title="Move up"
              disabled={index === 0}
              onClick={() => move(index, -1)}
            />
            <Button
              size="sm"
              placement="chat-settings__icon"
              icon={<IconChevronDownOutline14 />}
              aria-label={`Move ${label.toLowerCase()} ${String(index + 1)} down`}
              title="Move down"
              disabled={index === values.length - 1}
              onClick={() => move(index, 1)}
            />
            <Button
              size="sm"
              placement="chat-settings__icon"
              icon={<IconTrashOutline16 />}
              aria-label={`Remove ${label.toLowerCase()} ${String(index + 1)}`}
              title="Remove"
              onClick={() => onChange(values.filter((_, item) => item !== index))}
            />
          </div>
        </div>
      ))}
      <Button
        size="sm"
        variant="outline"
        icon={<IconPlusOutline16 />}
        disabled={values.length >= maximum}
        onClick={() => onChange([...values, ''])}
      >
        Add {label.toLowerCase()}
      </Button>
      {error === undefined ? null : <span className="chat-settings__field-error">{error}</span>}
    </div>
  );
}
