import { useEffect, useRef, useState } from 'react';
import { useCmaClient } from '../api/context';
import type { AgentModelOption, AgentSettingsView } from '../api/client';
import { Button } from '../ui/Button';
import { AgentSettingsError } from '../api/rest/agentSettings';
import { Presence } from '../ui/Presence';
import { ModelEffortPicker } from './ModelEffortPicker';
import { PermissionChip } from './PermissionChip';
import './ComposerSettings.css';

/**
 * The Agent's model, reasoning effort and permission mode, in the composer's
 * control row — `docs/Web_Style.md` §13.
 *
 * It renders loose pieces rather than one box: the permission chip, the model
 * and effort chip and a status line each take their own place in the
 * composer's grid. Choosing changes a draft only; nothing reaches the Agent
 * until Apply, and an uncertain update is retried with its original key.
 */
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
  const [modelsReload, setModelsReload] = useState(0);
  const [models, setModels] = useState<readonly AgentModelOption[]>([]);
  const [modelsLoading, setModelsLoading] = useState(true);
  const [modelsError, setModelsError] = useState<string>();
  useEffect(() => {
    if (!active) return;
    const request = new AbortController();
    void (async () => {
      setModelsLoading(true);
      setModelsError(undefined);
      try {
        if (!client.readAgentModels)
          throw new Error('The model catalog is unavailable from this connection.');
        const result = await client.readAgentModels(agentId, { signal: request.signal });
        if (!request.signal.aborted) setModels(result);
      } catch (cause) {
        if (!request.signal.aborted) {
          setModels([]);
          setModelsError(
            cause instanceof Error ? cause.message : 'Available models could not be loaded.',
          );
        }
      } finally {
        if (!request.signal.aborted) setModelsLoading(false);
      }
    })();
    return () => request.abort();
  }, [active, agentId, busy, client, modelsReload]);
  const [intent, setIntent] = useState<{ key: string; body: AgentSettingsView } | undefined>(
    undefined,
  );
  const mutation = useRef<AbortController | undefined>(undefined);
  const unavailableModelChange = Boolean(
    value &&
    saved &&
    value.model !== saved.model &&
    !models.some((model) => model.id === value.model && model.available),
  );
  const draft =
    unavailableModelChange && !intent && value && saved ? { ...value, model: saved.model } : value;
  const changed = Boolean(draft && JSON.stringify(draft) !== JSON.stringify(saved));
  useEffect(() => {
    if (!confirmed) return;
    const timer = setTimeout(() => setConfirmed(false), 3000);
    return () => clearTimeout(timer);
  }, [confirmed]);
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
    if (mutation.current || pending || busy || !draft || !client.updateAgentSettings) return;
    const request = new AbortController();
    mutation.current = request;
    const retained = intent ?? {
      key: crypto.randomUUID(),
      body: {
        model: draft.model.trim(),
        reasoning_effort: draft.reasoning_effort,
        permission_mode: draft.permission_mode,
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
    return (
      <div className="agent-settings__status">
        <p>Agent settings are unavailable from this connection.</p>
      </div>
    );
  const locked = pending || intent !== undefined || draft?.pending === true;
  const change = (next: AgentSettingsView) => {
    setValue(next);
    setConfirmed(false);
  };
  return (
    <>
      {draft ? (
        <>
          <PermissionChip
            value={draft.permission_mode}
            disabled={locked}
            onChange={(permission_mode) => change({ ...draft, permission_mode })}
          />
          <ModelEffortPicker
            value={draft}
            saved={saved}
            models={models}
            modelsLoading={modelsLoading}
            modelsError={modelsError}
            onRetryModels={() => setModelsReload((count) => count + 1)}
            disabled={locked}
            onChange={change}
          />
        </>
      ) : error ? null : (
        <p className="agent-settings__loading" role="status">
          Loading Agent settings…
        </p>
      )}
      {/* Present only when there is something to say; an empty line takes no room. */}
      <div className="agent-settings__status">
        {value?.pending ? (
          <p role="status">Waiting for the runtime to confirm these settings.</p>
        ) : null}
        {busy && changed ? (
          <p role="status">Settings can be changed after the current turn finishes.</p>
        ) : null}
        {error ? <p role="alert">{error}</p> : null}
        <Presence>
          {confirmed ? <p role="status">Settings applied to this Agent.</p> : null}
        </Presence>
        {draft && (changed || intent !== undefined || pending) ? (
          <Button
            size="sm"
            placement="agent-settings__apply"
            disabled={pending || busy || !draft.model.trim() || (!intent && !changed)}
            onClick={() => void update()}
          >
            {pending ? 'Applying settings…' : intent ? 'Retry settings update' : 'Apply settings'}
          </Button>
        ) : !draft && error ? (
          <Button
            size="sm"
            placement="agent-settings__apply"
            onClick={() => setReload((count) => count + 1)}
          >
            Retry settings
          </Button>
        ) : null}
      </div>
    </>
  );
}
