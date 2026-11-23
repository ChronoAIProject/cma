import { useEffect, useState } from 'react';
import type { CmaWebClient } from './api/client';
import type { WorkspaceDetail, WorkspaceSummary } from './api/types';
import type { DeleteAction, DeletionObservation } from './deletion';
import { usePresenceExiting } from './ui/Presence';
import { statusLabel } from './vocabulary/status';
import { characterForStatus } from './character/status';
import { taskTitle } from './shell/taskTitle';
import { CharacterAvatar } from './character/CharacterAvatar';
import { routePath } from './routing/routes';
import { IconFileOutline16, IconFolderClose16 } from './ui/icons';
import './DeletionPresentation.css';

const previewLimit = 8;
type Impact = {
  readonly client: CmaWebClient;
  readonly identity: string;
  readonly workspaces: readonly WorkspaceSummary[];
  readonly details: readonly WorkspaceDetail[];
};

interface ImpactProps {
  readonly client: CmaWebClient;
  readonly viewerId: string;
  readonly action: DeleteAction;
  readonly observation?: DeletionObservation;
  readonly available: boolean;
}

export function DeletionImpactSummary(props: ImpactProps) {
  const exiting = usePresenceExiting();
  const available = props.available && !exiting;
  return (
    <ImpactPreview
      key={JSON.stringify([props.viewerId, props.action, available])}
      {...props}
      available={available}
    />
  );
}

function ImpactPreview({ client, viewerId, action, observation, available }: ImpactProps) {
  const identity = JSON.stringify([viewerId, action]);
  const [impact, setImpact] = useState<Impact>();
  const [failed, setFailed] = useState<string>();
  useEffect(() => {
    if (!available || action.kind === 'delete_agent') return;
    const controller = new AbortController();
    const { signal } = controller;
    const read = async () => {
      if (action.kind === 'remove_workspace') {
        const workspace = await client.readWorkspace(action.workspaceId, { signal });
        if (workspace.id !== action.workspaceId) throw new Error('Mismatched Workspace');
        return { workspaces: [workspace], details: [workspace] };
      }
      const sandbox = await client.readSandbox(action.sandboxId, { signal });
      if (sandbox.id !== action.sandboxId) throw new Error('Mismatched Sandbox');
      const workspaces = sandbox.workspaces.slice(0, previewLimit);
      const details: WorkspaceDetail[] = [];
      for (const summary of workspaces) {
        if (signal.aborted) return undefined;
        if (summary.sandboxId !== sandbox.id) throw new Error('Mismatched parent');
        try {
          const workspace = await client.readWorkspace(summary.id, { signal });
          if (workspace.id !== summary.id || workspace.sandboxId !== sandbox.id)
            throw new Error('Mismatched Workspace');
          details.push(workspace);
        } catch {
          // A summary remains useful when its Agent inventory cannot be read.
        }
      }
      return { workspaces, details };
    };
    void read().then(
      (result) => {
        if (!signal.aborted && result) {
          setImpact({ ...result, client, identity });
          setFailed(undefined);
        }
      },
      () => {
        if (!signal.aborted) setFailed(identity);
      },
    );
    return () => controller.abort();
    // Identity includes the account, action kind and exact target.
  }, [client, identity, available, action]);
  const current =
    available && impact?.identity === identity && impact.client === client ? impact : undefined;
  const name = observation?.name;
  const agents =
    current?.details
      .flatMap((workspace) =>
        workspace.agents.filter(
          (agent) => agent.workspaceId === workspace.id && agent.sandboxId === workspace.sandboxId,
        ),
      )
      .slice(0, previewLimit) ?? [];
  return (
    <span className="deletion-impact">
      <strong className="deletion-impact__target">{name ?? 'Checking deletion target…'}</strong>
      {action.kind === 'delete_agent' ? (
        <span>
          {observation?.stopsAgent
            ? 'This first stops the Agent and ends its active work, then deletes it after stopping is confirmed. '
            : ''}
          Delete "{name ?? 'Agent'}" and its Worktree. This cannot be undone. Its Sandbox
          environment remains and still counts toward your environment limit.{' '}
          {observation?.sandboxId ? (
            <a href={routePath({ kind: 'sandbox', sandboxId: observation.sandboxId })}>
              Review parent environment
            </a>
          ) : null}
        </span>
      ) : (
        <>
          <span>
            {action.kind === 'delete_sandbox'
              ? 'This deletes the whole Sandbox environment, including all Agents, Workspaces, and files inside it.'
              : 'This deletes the Workspace, its Agents, Worktrees, and runtime files. Its Sandbox environment remains.'}
          </span>
          <span
            className="deletion-impact__list"
            role="list"
            aria-label="Known affected resources"
            tabIndex={0}
          >
            {agents.map((agent) => (
              <span className="deletion-impact__row" role="listitem" key={agent.id}>
                <CharacterAvatar
                  pose={characterForStatus(agent.status).pose}
                  tone={characterForStatus(agent.status).tone}
                  size={24}
                  still
                />
                <strong>{taskTitle(agent)}</strong>
                <span>Agent · {statusLabel(agent.status)}</span>
              </span>
            ))}
            {current?.workspaces.map((workspace) => (
              <span className="deletion-impact__row" role="listitem" key={workspace.id}>
                <IconFolderClose16 />
                <strong>{workspace.name || 'Workspace'}</strong>
                <span>Workspace</span>
              </span>
            ))}
            <span className="deletion-impact__row" role="listitem">
              <IconFileOutline16 />
              <strong>Runtime files</strong>
              <span>In this {action.kind === 'delete_sandbox' ? 'environment' : 'Workspace'}</span>
            </span>
          </span>
          <span className="deletion-impact__note">
            {!available || failed === identity
              ? 'Affected resources could not be fully read. This is not an empty environment.'
              : current === undefined
                ? 'Reading known affected resources…'
                : 'Preview of known resources; the full impact may not be listed. Resources can change before deletion is accepted.'}
          </span>
          <span className="deletion-impact__warning">
            <strong>This is more than stopping one agent</strong>
            <span>
              {observation?.stopsRuntimeUpdate ? 'This will stop the paused runtime update. ' : ''}
              Export anything you need first. This cannot be undone.{' '}
              {action.kind === 'delete_sandbox'
                ? 'The slot frees up only after cleanup is confirmed.'
                : 'Deleting this Workspace does not release its Sandbox slot.'}
            </span>
          </span>
        </>
      )}
      <span className="deletion-impact__note">
        Permission and environment state are checked again before acceptance. Retained history
        follows the retention policy.
      </span>
    </span>
  );
}
