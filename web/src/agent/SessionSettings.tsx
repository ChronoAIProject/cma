import { useEffect, useRef, useState } from 'react';
import { useCmaClient } from '../api/context';
import type { AgentSettingsView } from '../api/client';
import { Button } from '../ui/Button';
import { SettingsFields } from './SettingsFields';
import { AgentSettingsError } from '../api/rest/agentSettings';

export function SessionSettings({
  agentId,
  active,
  busy,
}: {
  readonly agentId: string;
  readonly active: boolean;
  readonly busy: boolean;
}) {
  const client = useCmaClient();
  const [value, setValue] = useState<AgentSettingsView>();
  const [saved, setSaved] = useState<AgentSettingsView>();
  const [error, setError] = useState<string>();
  const [pending, setPending] = useState(false);
  const [confirmed, setConfirmed] = useState(false);
  const [reload, setReload] = useState(0);
  const [intent, setIntent] = useState<{ key: string; body: AgentSettingsView } | undefined>(
    undefined,
  );
  const mutation = useRef<AbortController | undefined>(undefined);
  useEffect(() => () => mutation.current?.abort(), []);
  useEffect(() => {
    if (!active || !client.readAgentSettings) return;
    const request = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = () =>
      void client.readAgentSettings!(agentId, { signal: request.signal })
        .then((settings) => {
          if (request.signal.aborted) return;
          setSaved(settings);
          setValue(settings);
          setError(undefined);
          if (settings.pending === false)
            setIntent((previous) =>
              previous &&
              previous.body.model === settings.model &&
              previous.body.reasoning_effort === settings.reasoning_effort &&
              previous.body.permission_mode === settings.permission_mode
                ? undefined
                : previous,
            );
          if (settings.pending) timer = setTimeout(poll, 2000);
        })
        .catch((cause: unknown) => {
          if (!request.signal.aborted)
            setError(
              cause instanceof Error ? cause.message : 'Agent settings could not be loaded.',
            );
        });
    poll();
    return () => {
      request.abort();
      clearTimeout(timer);
    };
  }, [active, agentId, client, reload]);
  const update = async () => {
    if (mutation.current || pending || busy || !value || !client.updateAgentSettings) return;
    const request = new AbortController();
    mutation.current = request;
    const retained = intent ?? {
      key: crypto.randomUUID(),
      body: {
        model: value.model.trim(),
        reasoning_effort: value.reasoning_effort,
        permission_mode: value.permission_mode,
      },
    };
    setIntent(retained);
    setPending(true);
    setError(undefined);
    setConfirmed(false);
    try {
      const result = await client.updateAgentSettings(agentId, retained.body, {
        idempotencyKey: retained.key,
        signal: request.signal,
      });
      if (request.signal.aborted) return;
      if (!result.pending) setIntent(undefined);
      setSaved(result);
      setValue(result);
      setConfirmed(!result.pending);
      if (result.pending) setReload((count) => count + 1);
    } catch (cause) {
      if (cause instanceof AgentSettingsError && [400, 409, 422].includes(cause.status))
        setIntent(undefined);
      if (!request.signal.aborted)
        setError(
          cause instanceof Error
            ? cause.message
            : 'The settings update could not be confirmed. Retry the same request.',
        );
    } finally {
      if (!request.signal.aborted) setPending(false);
      mutation.current = undefined;
    }
  };
  if (!client.readAgentSettings || !client.updateAgentSettings)
    return <p>Agent settings are unavailable from this connection.</p>;
  return (
    <div className="agent-settings">
      {value ? (
        <SettingsFields
          value={value}
          disabled={pending || intent !== undefined || value.pending === true}
          onChange={(next) => {
            setValue(next);
            setConfirmed(false);
          }}
        />
      ) : (
        <p role="status">Loading Agent settings…</p>
      )}
      {value?.pending ? (
        <p role="status">Waiting for the runtime to confirm these settings.</p>
      ) : null}
      {busy ? <p role="status">Settings can be changed after the current turn finishes.</p> : null}
      {error ? <p role="alert">{error}</p> : null}
      {confirmed ? <p role="status">Settings applied to this Agent.</p> : null}
      {value ? (
        <Button
          disabled={
            pending ||
            busy ||
            !value.model.trim() ||
            (!intent && JSON.stringify(value) === JSON.stringify(saved))
          }
          onClick={() => void update()}
        >
          {pending ? 'Applying settings…' : intent ? 'Retry settings update' : 'Apply settings'}
        </Button>
      ) : error ? (
        <Button onClick={() => setReload((count) => count + 1)}>Retry settings</Button>
      ) : null}
    </div>
  );
}
