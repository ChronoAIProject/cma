import { useEffect, useRef, useState } from 'react';
import { useCmaClient } from '../api/context';
import type { AgentPreferencesView, UpdateAgentPreferences } from '../api/client';
import { AgentPreferencesError } from '../api/rest/agentPreferences';
import { Modal } from '../ui/Modal';
import { Input } from '../ui/Input';
import { Button } from '../ui/Button';
import { Alert } from '../ui/Alert';
import './AgentPreferencesDialog.css';

/** Preference mutations never reload or remount the conversation. */
export function AgentPreferencesDialog({
  agentId,
  mode,
  currentTitle,
  active,
  onTitleUpdated,
  onClose,
}: {
  readonly agentId: string;
  readonly mode: 'rename' | 'prompt';
  readonly currentTitle: string;
  readonly active: boolean;
  readonly onTitleUpdated?: (title: string) => void;
  readonly onClose: () => void;
}) {
  const client = useCmaClient();
  const [preferences, setPreferences] = useState<AgentPreferencesView>();
  const [value, setValue] = useState('');
  const [error, setError] = useState<string>();
  const [notice, setNotice] = useState<string>();
  const [reload, setReload] = useState(0);
  const [busy, setBusy] = useState(false);
  const [conflict, setConflict] = useState(false);
  const [intent, setIntent] = useState<{ key: string; body: UpdateAgentPreferences }>();
  const mutation = useRef<AbortController | undefined>(undefined);
  useEffect(() => () => mutation.current?.abort(), []);
  useEffect(() => {
    const read = new AbortController();
    void client.readAgentPreferences?.(agentId, { signal: read.signal }).then(
      (next) => {
        if (read.signal.aborted) return;
        setPreferences(next);
        setValue(mode === 'rename' ? (next.title ?? currentTitle) : next.system_prompt);
        setError(undefined);
        setConflict(false);
      },
      (cause) => {
        if (!read.signal.aborted)
          setError(cause instanceof Error ? cause.message : 'Preferences could not be loaded.');
      },
    );
    return () => read.abort();
  }, [client, agentId, mode, currentTitle, reload]);
  const promptUnavailable =
    mode === 'prompt' && (!preferences?.prompt_supported || active || preferences.prompt_pending);
  const invalid =
    mode === 'rename'
      ? !value.trim() || Array.from(value.trim()).length > 64
      : new TextEncoder().encode(value).length > 32768;
  const save = async () => {
    if (
      mutation.current ||
      !preferences ||
      !client.updateAgentPreferences ||
      invalid ||
      conflict ||
      promptUnavailable
    )
      return;
    const retained = intent ?? {
      key: crypto.randomUUID(),
      body: {
        expected_revision: preferences.revision,
        ...(mode === 'rename' ? { title: value.trim() } : { system_prompt: value }),
      },
    };
    const request = new AbortController();
    mutation.current = request;
    setBusy(true);
    setIntent(retained);
    setError(undefined);
    try {
      const next = await client.updateAgentPreferences(agentId, retained.body, {
        signal: request.signal,
        idempotencyKey: retained.key,
      });
      if (request.signal.aborted) return;
      setPreferences(next);
      setIntent(undefined);
      if (mode === 'rename') {
        onTitleUpdated?.(next.title ?? value.trim());
        onClose();
      } else if (next.prompt_pending)
        setNotice('Saved. The Agent is applying the updated system prompt.');
      else onClose();
    } catch (cause) {
      if (!request.signal.aborted) {
        if (cause instanceof AgentPreferencesError && cause.status >= 400 && cause.status < 500) {
          setIntent(undefined);
          if (cause.status === 409) setConflict(true);
        }
        setError(
          cause instanceof Error
            ? cause.message
            : 'The change could not be confirmed. Retry the same request.',
        );
      }
    } finally {
      mutation.current = undefined;
      if (!request.signal.aborted) setBusy(false);
    }
  };
  return (
    <Modal
      open
      title={mode === 'rename' ? 'Rename Agent' : 'Update system prompt'}
      dismissible={!busy}
      onClose={onClose}
      footer={
        <>
          <Button disabled={busy} onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            disabled={!preferences || busy || invalid || conflict || promptUnavailable}
            onClick={() => void save()}
          >
            {busy ? 'Saving…' : intent ? 'Retry change' : 'Save'}
          </Button>
        </>
      }
    >
      <div className="agent-preferences">
        {!preferences && !error ? <p role="status">Loading preferences…</p> : null}
        {preferences ? (
          <label>
            <span>{mode === 'rename' ? 'Agent name' : 'System prompt'}</span>
            {mode === 'rename' ? (
              <Input
                aria-label="Agent name"
                value={value}
                disabled={busy || intent !== undefined}
                onChange={(event) => setValue(event.target.value)}
              />
            ) : (
              <textarea
                aria-label="Agent system prompt"
                value={value}
                disabled={busy || intent !== undefined || promptUnavailable}
                onChange={(event) => setValue(event.target.value)}
                spellCheck={false}
              />
            )}
          </label>
        ) : null}
        {preferences && invalid ? (
          <p role="alert">
            {mode === 'rename'
              ? 'Choose a name of 1–64 characters.'
              : 'Keep the system prompt within 32 KiB.'}
          </p>
        ) : null}
        {mode === 'prompt' && preferences ? (
          <p>
            {!preferences.prompt_supported
              ? 'This runtime does not support system prompt updates yet.'
              : active
                ? 'Wait for the current turn to finish before changing the system prompt.'
                : preferences.prompt_pending
                  ? 'A system prompt update is still being applied.'
                  : 'Your updated instructions apply to subsequent turns in this Agent.'}
          </p>
        ) : null}
        {notice ? <p role="status">{notice}</p> : null}
        {error ? (
          <Alert
            tone="error"
            actions={
              !intent ? (
                <Button size="sm" disabled={busy} onClick={() => setReload((value) => value + 1)}>
                  Reload preferences
                </Button>
              ) : undefined
            }
          >
            {error}
          </Alert>
        ) : null}
      </div>
    </Modal>
  );
}
