import { useState } from 'react';
import type { CmaWebClient } from '../api/client';
import type { WorkspaceDetail } from '../api/types';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { Presence } from '../ui/Presence';
import { useConnectionGate } from '../connections/useConnectionGate';
import { useCreationCommand } from './transaction';
import './Create.css';

/** Direct Agent Manager workspace admission without a Workspace Profile picker. */
export function InlineWorkspaceCard({
  client,
  sandboxId,
  onCreated,
  onClose,
}: {
  readonly client: CmaWebClient;
  readonly sandboxId: string;
  readonly onCreated: (workspace: WorkspaceDetail) => void;
  readonly onClose: () => void;
}) {
  const [sourceType, setSourceType] = useState<'empty' | 'git_repository'>('empty');
  const [name, setName] = useState('workspace');
  const [url, setUrl] = useState('');
  const [reference, setReference] = useState('');
  const command = useCreationCommand(onCreated);
  const connections = useConnectionGate();
  const repositoryReady = sourceType === 'empty' || url.trim().length > 0;
  const ready = repositoryReady && (sourceType === 'git_repository' || name.trim().length > 0);

  const submit = () => {
    if (!ready) return;
    const source =
      sourceType === 'empty'
        ? ({ type: 'empty' } as const)
        : {
            type: 'git_repository' as const,
            url: url.trim(),
            ...(reference.trim() === '' ? {} : { reference: reference.trim() }),
          };
    connections.run(
      { type: 'sandbox', id: sandboxId },
      () =>
        void command.submit((options) =>
          client.createInlineWorkspace(
            sandboxId,
            {
              source,
              ...(name.trim() === '' ? {} : { name: name.trim() }),
            },
            options,
          ),
        ),
    );
  };

  if (connections.pending) return connections.dialog;
  return (
    <Modal
      open
      title="New workspace"
      onClose={onClose}
      dismissible={!command.locked}
      footer={
        <>
          <Button disabled={command.locked} onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            disabled={command.pending || (!command.failure?.uncertain && !ready)}
            onClick={command.failure?.uncertain ? command.retry : submit}
          >
            {command.pending
              ? 'Creating'
              : command.failure?.uncertain
                ? 'Retry creation'
                : 'Create workspace'}
          </Button>
        </>
      }
    >
      <div className="create-form">
        <div className="create-form__field">
          <label htmlFor="inline-workspace-source">Workspace source</label>
          <p className="profile-field__help">
            Agent Manager sends this source directly to agent-sandbox. No Workspace Profile is
            required.
          </p>
          <select
            id="inline-workspace-source"
            value={sourceType}
            disabled={command.locked}
            onChange={(event) => setSourceType(event.target.value as 'empty' | 'git_repository')}
          >
            <option value="empty">Empty workspace</option>
            <option value="git_repository">Clone a GitHub repository</option>
          </select>
        </div>
        <div className="create-form__field">
          <label htmlFor="inline-workspace-name">Workspace name</label>
          <input
            id="inline-workspace-name"
            value={name}
            disabled={command.locked}
            onChange={(event) => setName(event.target.value)}
            placeholder="workspace"
          />
        </div>
        {sourceType === 'git_repository' ? (
          <>
            <div className="create-form__field">
              <label htmlFor="inline-workspace-url">GitHub repository URL</label>
              <input
                id="inline-workspace-url"
                type="url"
                value={url}
                disabled={command.locked}
                onChange={(event) => setUrl(event.target.value)}
                placeholder="https://github.com/org/repository"
                required
              />
            </div>
            <div className="create-form__field">
              <label htmlFor="inline-workspace-reference">Branch or ref (optional)</label>
              <input
                id="inline-workspace-reference"
                value={reference}
                disabled={command.locked}
                onChange={(event) => setReference(event.target.value)}
                placeholder="main"
              />
            </div>
          </>
        ) : null}
        <Presence>
          {command.failure === undefined ? null : (
            <p className="create-error" role="alert">
              {command.failure.message}
            </p>
          )}
        </Presence>
      </div>
    </Modal>
  );
}
