import { Picker } from '../ui/Picker';
import { useState } from 'react';
import type { CreateInlineWorkspaceRequest } from '@cma/client';
import type { CmaWebClient } from '../api/client';
import type { WorkspaceDetail } from '../api/types';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { Presence } from '../ui/Presence';
import { useConnectionGate } from '../connections/useConnectionGate';
import { validRepository } from './repository';
import { useCreationCommand } from './transaction';
import './Create.css';

type InlineSourceType = CreateInlineWorkspaceRequest['source']['type'];
const EMPTY_SOURCE = 'empty' as const;
const GIT_SOURCE = 'git_repository' as const;

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
  const [sourceType, setSourceType] = useState<InlineSourceType>(EMPTY_SOURCE);
  const [name, setName] = useState('workspace');
  const [url, setUrl] = useState('');
  const [repositoryTouched, setRepositoryTouched] = useState(false);
  const [reference, setReference] = useState('');
  const command = useCreationCommand(onCreated);
  const connections = useConnectionGate();
  const repositoryUrl = url.trim();
  // The Workspace API checks the raw authority before URL normalization.
  const repositoryReady =
    repositoryUrl.startsWith('https://github.com/') && validRepository(repositoryUrl);
  const repositoryError = repositoryTouched && !repositoryReady;
  const ready = sourceType === GIT_SOURCE ? repositoryReady : name.trim().length > 0;

  const submit = () => {
    if (!ready) return;
    const source =
      sourceType === EMPTY_SOURCE
        ? ({ type: 'empty' } as const)
        : {
            type: GIT_SOURCE,
            url: repositoryUrl,
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
          <Picker
            id="inline-workspace-source"
            value={sourceType}
            disabled={command.locked}
            onChange={(choice) => setSourceType(choice as InlineSourceType)}
            label="Workspace source"
            options={[
              { value: EMPTY_SOURCE, label: 'Empty workspace' },
              { value: GIT_SOURCE, label: 'Clone a GitHub repository' },
            ]}
          />
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
        {sourceType === GIT_SOURCE ? (
          <>
            <div className="create-form__field">
              <label htmlFor="inline-workspace-url">GitHub repository URL</label>
              <input
                id="inline-workspace-url"
                type="url"
                value={url}
                disabled={command.locked}
                onChange={(event) => setUrl(event.target.value)}
                onBlur={() => setRepositoryTouched(true)}
                aria-invalid={repositoryError || undefined}
                aria-describedby={repositoryError ? 'inline-workspace-url-error' : undefined}
                placeholder="https://github.com/org/repository"
                required
              />
              {repositoryError ? (
                <p id="inline-workspace-url-error" className="field__error" role="alert">
                  Enter an HTTPS GitHub repository URL.
                </p>
              ) : null}
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
