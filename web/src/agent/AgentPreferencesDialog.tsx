import { useMemo, useState } from 'react';
import { useCmaClient } from '../api/context';
import type { AgentPreferencesView, UpdateAgentPreferences } from '../api/client';
import { Modal } from '../ui/Modal';
import { Input } from '../ui/Input';
import { Button } from '../ui/Button';
import { Alert } from '../ui/Alert';
import { useSettingsEditor, type SettingsEditorAdapter } from './useSettingsEditor';
import { unconfirmedSettingsWarning, useUnconfirmedSettings } from './useUnconfirmedSettings';
import { parsePreferencesIntentBody } from './settingsIntent';
import { agentTransportJournal } from './transport/journal';
import './AgentPreferencesDialog.css';

interface AgentPreferencesProps {
  readonly agentId: string;
  readonly viewerId?: string;
  readonly mode: 'rename' | 'prompt';
  readonly currentTitle: string;
  readonly active: boolean;
  readonly onTitleUpdated?: (title: string) => void;
  readonly onClose: () => void;
}

/** Preference mutations never reload or remount the conversation. */
export function AgentPreferencesDialog(props: AgentPreferencesProps) {
  return (
    <PreferencesForm key={JSON.stringify([props.viewerId, props.agentId, props.mode])} {...props} />
  );
}

function PreferencesForm({
  agentId,
  viewerId,
  mode,
  currentTitle,
  active,
  onTitleUpdated,
  onClose,
}: AgentPreferencesProps) {
  const client = useCmaClient();
  const [initialTitle] = useState(currentTitle);
  const adapter = useMemo<
    SettingsEditorAdapter<AgentPreferencesView, string, UpdateAgentPreferences>
  >(
    () => ({
      read: (signal) => client.readAgentPreferences!(agentId, { signal }),
      update: (body, options) => client.updateAgentPreferences!(agentId, body, options),
      proposal: (view) => (mode === 'rename' ? (view.title ?? initialTitle) : view.system_prompt),
      body: (view, proposal) => ({
        expected_revision: view.revision,
        ...(mode === 'rename' ? { title: proposal.trim() } : { system_prompt: proposal }),
      }),
      equal: (left, right) => left === right,
      pending: (view) => mode === 'prompt' && view.prompt_pending,
      retention: viewerId
        ? {
            journal: () => {
              const journal = agentTransportJournal(sessionStorage, viewerId, agentId);
              return mode === 'prompt'
                ? journal.promptSettingsIntent()
                : journal.settingsIntent('rename', (value) =>
                    parsePreferencesIntentBody(value, 'rename'),
                  );
            },
            proposal: (body) => body.title ?? body.system_prompt ?? '',
            emptyReentry: mode === 'prompt' ? '' : undefined,
          }
        : undefined,
    }),
    [client, agentId, mode, initialTitle, viewerId],
  );
  const editor = useSettingsEditor(adapter, !!client.readAgentPreferences);
  const { snapshot: preferences, intent } = editor;
  const showIntent = intent && (mode !== 'prompt' || preferences || !viewerId);
  const value = editor.proposal ?? '';
  const [confirmClose, setConfirmClose] = useState(false);
  useUnconfirmedSettings(intent !== undefined && !viewerId);
  const close = () => {
    if (intent && !viewerId) setConfirmClose(true);
    else onClose();
  };
  const promptUnavailable =
    mode === 'prompt' && (!preferences?.prompt_supported || active || editor.pending);
  const invalid =
    mode === 'rename'
      ? !value.trim() || Array.from(value.trim()).length > 64
      : new TextEncoder().encode(value).length > 32768;
  const save = async () => {
    if (!editor.canApply || invalid || promptUnavailable || !client.updateAgentPreferences) return;
    const next = await editor.apply();
    if (!next) return;
    if (mode === 'rename') {
      onTitleUpdated?.(next.title ?? value.trim());
      onClose();
    } else if (!next.prompt_pending) onClose();
  };
  return (
    <Modal
      open
      title={mode === 'rename' ? 'Rename Agent' : 'Update system prompt'}
      dismissible={!editor.saving}
      onClose={close}
      footer={
        confirmClose ? (
          <>
            <Button onClick={() => setConfirmClose(false)}>Keep editing</Button>
            <Button onClick={onClose}>Discard local changes and close</Button>
          </>
        ) : (
          <>
            <Button disabled={editor.saving} onClick={close}>
              Cancel
            </Button>
            <Button
              variant="primary"
              disabled={!preferences || !editor.canApply || invalid || promptUnavailable}
              onClick={() => void save()}
            >
              {editor.saving ? 'Saving…' : showIntent ? 'Retry change' : 'Save'}
            </Button>
          </>
        )
      }
    >
      <div className="agent-preferences">
        {!preferences && !editor.failure ? <p role="status">Loading preferences…</p> : null}
        {preferences ? (
          <label>
            <span>{mode === 'rename' ? 'Agent name' : 'System prompt'}</span>
            {mode === 'rename' ? (
              <Input
                aria-label="Agent name"
                value={value}
                disabled={editor.locked}
                onChange={(event) => editor.edit(event.target.value)}
              />
            ) : (
              <textarea
                aria-label="Agent system prompt"
                value={value}
                disabled={editor.locked || promptUnavailable}
                onChange={(event) => editor.edit(event.target.value)}
                spellCheck={false}
              />
            )}
          </label>
        ) : null}
        {preferences ? <p>Current preferences revision: {preferences.revision}.</p> : null}
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
                : editor.pending
                  ? 'A system prompt update is still being applied.'
                  : 'Your updated instructions apply to subsequent turns in this Agent.'}
          </p>
        ) : null}
        {editor.confirmed ? <p role="status">The changes are applied to this Agent.</p> : null}
        {showIntent ? (
          <p role="status">
            {intent?.phase === 'reenter'
              ? 'Re-enter the exact original system prompt to check the pending change.'
              : unconfirmedSettingsWarning(viewerId)}
          </p>
        ) : null}
        {confirmClose ? (
          <p role="alert">
            Discard the local edits and retry details? This does not cancel a server update.
          </p>
        ) : null}
        {editor.waiting ? <p role="status">Waiting before another request is allowed.</p> : null}
        {editor.failure ? (
          <Alert
            tone="error"
            actions={
              <Button size="sm" disabled={editor.busy || editor.waiting} onClick={editor.refresh}>
                Reload preferences
              </Button>
            }
          >
            {editor.failure.message}
          </Alert>
        ) : null}
      </div>
    </Modal>
  );
}
