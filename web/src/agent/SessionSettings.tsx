import type { AgentUsage } from '../api/types';
import { usageDescription } from './usageDescription';
import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useCmaClient } from '../api/context';
import type { AgentModelOption, AgentSettingsView } from '../api/client';
import { Button } from '../ui/Button';
import { useSettingsEditor, type SettingsEditorAdapter } from './useSettingsEditor';
import { unconfirmedSettingsWarning, useUnconfirmedSettings } from './useUnconfirmedSettings';
import { parseSettingsIntentBody } from './settingsIntent';
import { agentTransportJournal } from './transport/journal';
import { Presence } from '../ui/Presence';
import { ModelEffortPicker } from './ModelEffortPicker';
import { PermissionChip } from './PermissionChip';
import { effortLabel, modelLabel, permissionLabel } from './settingsChoices';
import { IconChevronDownOutline14 } from '../ui/icons';
import './ComposerSettings.css';

interface SessionSettingsProps {
  readonly usage?: AgentUsage;
  readonly agentId: string;
  readonly viewerId?: string;
  readonly active: boolean;
  readonly runtimeReady: boolean;
  readonly busy: boolean;
  readonly authorizeUpdate?: (signal: AbortSignal) => Promise<void>;
}

const settingsProposal = (view: AgentSettingsView): AgentSettingsView => ({
  model: view.model,
  reasoning_effort: view.reasoning_effort,
  permission_mode: view.permission_mode,
});
const settingsFields = ['model', 'reasoning_effort', 'permission_mode'] as const;
const settingsFieldLabels = {
  model: 'model',
  reasoning_effort: 'reasoning effort',
  permission_mode: 'permission mode',
};
const sameSettings = (left: AgentSettingsView, right: AgentSettingsView) =>
  settingsFields.every((field) => left[field] === right[field]);

export function SessionSettings(props: SessionSettingsProps) {
  return <SessionSettingsForm key={JSON.stringify([props.viewerId, props.agentId])} {...props} />;
}

function SessionSettingsForm({
  agentId,
  viewerId,
  active,
  runtimeReady,
  busy,
  authorizeUpdate,
  usage,
}: SessionSettingsProps) {
  const client = useCmaClient();
  const [expanded, setExpanded] = useState(false);
  const settingsId = useId();
  const usageId = useId();
  const usageText = usageDescription(usage);
  const authorize = useRef(authorizeUpdate);
  useLayoutEffect(() => {
    authorize.current = authorizeUpdate;
  }, [authorizeUpdate]);
  const adapter = useMemo<
    SettingsEditorAdapter<AgentSettingsView, AgentSettingsView, AgentSettingsView>
  >(
    () => ({
      read: (signal) => client.readAgentSettings!(agentId, { signal }),
      update: async (body, options) => {
        await authorize.current?.(options.signal);
        options.signal.throwIfAborted();
        return client.updateAgentSettings!(agentId, body, options);
      },
      proposal: settingsProposal,
      body: (_view, proposal) => ({ ...settingsProposal(proposal), model: proposal.model.trim() }),
      equal: sameSettings,
      pending: (view) => view.pending === true,
      mergeFields: settingsFields,
      retention: viewerId
        ? {
            journal: () =>
              agentTransportJournal(sessionStorage, viewerId, agentId).settingsIntent(
                'settings',
                parseSettingsIntentBody,
              ),
            proposal: settingsProposal,
          }
        : undefined,
    }),
    [client, agentId, viewerId],
  );
  const editor = useSettingsEditor(adapter, active && !!client.readAgentSettings);
  const { proposal: value, snapshot: saved, intent } = editor;
  const error = editor.failure?.message;
  const pending = editor.saving;
  const confirmed = editor.confirmed;
  useUnconfirmedSettings(intent !== undefined && !viewerId);
  const [modelsReload, setModelsReload] = useState(0);
  const [models, setModels] = useState<readonly AgentModelOption[]>([]);
  const [modelsLoading, setModelsLoading] = useState(true);
  const [modelsError, setModelsError] = useState<string>();
  useEffect(() => {
    // The saved settings already name the current model. Fetch the provider's
    // catalog only when its picker is needed, outside the opening critical path.
    if (!active || !runtimeReady || !expanded) return;
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
  }, [active, runtimeReady, expanded, agentId, busy, client, modelsReload]);
  const unavailableModelChange = Boolean(
    value &&
    saved &&
    value.model !== saved.model &&
    !models.some((model) => model.id === value.model && model.available),
  );
  const draft =
    unavailableModelChange && !intent && value && saved ? { ...value, model: saved.model } : value;
  const changedFields =
    draft && saved ? settingsFields.filter((field) => draft[field] !== saved[field]) : [];
  const changed = changedFields.length > 0;
  const update = async () => {
    if (busy || !runtimeReady || !draft || !editor.canApply) return;
    if (!intent) editor.edit(draft);
    await editor.apply();
  };
  if (!client.readAgentSettings || !client.updateAgentSettings)
    return (
      <>
        <button
          type="button"
          className="composer-chip agent-settings__summary"
          aria-label="Agent settings: unavailable"
          aria-disabled="true"
          aria-describedby={usageId}
          title={`Agent settings are unavailable from this connection.\n${usageText}`}
        >
          Agent settings
        </button>
        <span id={usageId} className="sr-only">
          {usageText}
        </span>
        <div className="agent-settings__status">
          <p>Agent settings are unavailable from this connection.</p>
        </div>
      </>
    );
  const locked = editor.locked;
  const change = (next: AgentSettingsView) => {
    editor.edit(next);
  };
  const summaryModel = saved ? modelLabel(models, saved.model) : '';
  const summary = saved
    ? `${summaryModel} · ${effortLabel(saved.reasoning_effort)} · ${permissionLabel(saved.permission_mode)}`
    : undefined;
  const compactSummary = saved
    ? `${summaryModel.match(/\(([^)]+)\)/)?.[1] ?? summaryModel} · ${effortLabel(saved.reasoning_effort)}`
    : undefined;
  const settingsTooltip = saved
    ? `${summaryModel} · ${effortLabel(saved.reasoning_effort)} reasoning · ${permissionLabel(saved.permission_mode)}`
    : undefined;
  return (
    <>
      <span id={usageId} className="sr-only">
        {usageText}
      </span>
      {draft && saved ? (
        <>
          <button
            type="button"
            className="composer-chip agent-settings__summary"
            aria-label={`Agent settings: ${summary}`}
            aria-expanded={expanded}
            aria-controls={settingsId}
            title={`${settingsTooltip}\n${usageText}`}
            aria-describedby={usageId}
            onClick={() => setExpanded((current) => !current)}
          >
            <span className="composer-chip__label">{compactSummary}</span>
            <IconChevronDownOutline14 />
          </button>
          <div
            id={settingsId}
            role="region"
            aria-label="Agent settings"
            className="agent-settings__panel"
            hidden={!expanded}
          >
            {expanded ? (
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
            ) : null}
          </div>
        </>
      ) : error ? null : (
        <p className="agent-settings__loading" role="status">
          Loading Agent settings…
        </p>
      )}
      <div className="agent-settings__status">
        {changed && !intent && !pending ? (
          <p role="status">
            Unapplied changes to{' '}
            {changedFields.map((field) => settingsFieldLabels[field]).join(', ')}.
          </p>
        ) : null}
        {editor.pending ? (
          <p role="status">Waiting for the runtime to confirm these settings.</p>
        ) : null}
        {busy && changed ? (
          <p role="status">Settings can be changed after the current turn finishes.</p>
        ) : null}
        {intent ? <p role="status">{unconfirmedSettingsWarning(viewerId)}</p> : null}
        {editor.waiting ? <p role="status">Waiting before another request is allowed.</p> : null}
        {error ? <p role="alert">{error}</p> : null}
        {error && draft ? (
          <Button size="sm" disabled={editor.busy || editor.waiting} onClick={editor.refresh}>
            Refresh current settings
          </Button>
        ) : null}
        <Presence>
          {confirmed ? <p role="status">Settings applied to this Agent.</p> : null}
        </Presence>
        {draft && (changed || intent !== undefined || pending) ? (
          <Button
            size="sm"
            placement="agent-settings__apply"
            disabled={
              !editor.canApply ||
              busy ||
              !runtimeReady ||
              !draft.model.trim() ||
              (!intent && !changed)
            }
            onClick={() => void update()}
          >
            {pending ? 'Applying settings…' : intent ? 'Retry settings update' : 'Apply settings'}
          </Button>
        ) : !draft && error ? (
          <Button
            size="sm"
            placement="agent-settings__apply"
            disabled={editor.busy || editor.waiting}
            onClick={editor.refresh}
          >
            Retry settings
          </Button>
        ) : null}
      </div>
    </>
  );
}
