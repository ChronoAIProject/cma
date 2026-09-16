import { useMemo, useState } from 'react';
import type { BridgeViewsClient } from './client';
import type { RetainedTarget } from './retainedTargetAccess';
import type { PlacementEditor } from './usePlacementEditor';
import { loadPlacementHierarchy } from './placementAccess';
import { useAuthorizedRead } from '../resources/useAuthorizedRead';
import { Picker } from '../ui/Picker';
import { Button } from '../ui/Button';
import { RetainedReplyPreview } from './RetainedReplyPreview';

export function OrganizationRetainedAgent({
  client,
  value,
  organization,
  disabled,
  editor,
  onChange,
}: {
  readonly client: BridgeViewsClient;
  readonly value: RetainedTarget;
  readonly organization: string;
  readonly disabled: boolean;
  readonly editor: PlacementEditor;
  readonly onChange: (target: RetainedTarget) => void;
}) {
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const cursor = cursors.at(-1);
  const list = useMemo(
    () =>
      disabled
        ? undefined
        : async (signal: AbortSignal) => {
            const page = await client.sandboxes.listAccessible({
              organization,
              cursor,
              limit: 50,
              signal,
            });
            if (
              new Set(page.items.map((item) => item.id)).size !== page.items.length ||
              (page.next !== undefined && cursors.includes(page.next))
            )
              throw new Error('Sandbox inventory did not return distinct choices.');
            return { value: page };
          },
    [client, organization, disabled, cursor, cursors],
  );
  const inventory = useAuthorizedRead(list);
  const page = inventory.state.status === 'ready' ? inventory.state.value : undefined;
  const sandboxId = value.sandbox_id;
  const workspaceId = value.workspace_id;
  const read = useMemo(
    () =>
      disabled || sandboxId === ''
        ? undefined
        : async (signal: AbortSignal) => {
            const result = await loadPlacementHierarchy(
              client,
              sandboxId,
              workspaceId || undefined,
              organization,
              signal,
            );
            const { sandbox, workspace } = result.value;
            if (
              sandbox.workspaces.some((item) => item.sandbox_id !== sandboxId) ||
              new Set(sandbox.workspaces.map((item) => item.id)).size !== sandbox.workspaces.length
            )
              throw new Error('Workspace choices do not match this Sandbox.');
            if (
              workspace !== undefined &&
              (new Set(workspace.agents.map((item) => item.id)).size !== workspace.agents.length ||
                new Set(workspace.worktrees.map((item) => item.id)).size !==
                  workspace.worktrees.length ||
                workspace.agents.some(
                  (item) => item.sandbox_id !== sandboxId || item.workspace_id !== workspaceId,
                ) ||
                workspace.worktrees.some(
                  (item) => item.sandbox_id !== sandboxId || item.workspace_id !== workspaceId,
                ))
            )
              throw new Error('Agent choices do not match this Workspace.');
            return result;
          },
    [client, disabled, sandboxId, workspaceId, organization],
  );
  const hierarchy = useAuthorizedRead(read);
  const parents = hierarchy.state.status === 'ready' ? hierarchy.state.value : undefined;
  const sandboxOptions = [
    { value: '', label: 'Select a Sandbox', unavailable: undefined as string | undefined },
    ...(page?.items ?? []).map((item) => ({
      value: item.id,
      label: item.name,
      unavailable: ['active', 'paused'].includes(item.status)
        ? undefined
        : 'Sandbox is unavailable',
    })),
  ];
  const workspaceOptions = [
    { value: '', label: 'Select a Workspace', unavailable: undefined as string | undefined },
    ...(parents?.sandbox.workspaces ?? []).map((item) => ({
      value: item.id,
      label: item.name,
      unavailable: item.status === 'ready' ? undefined : 'Workspace is unavailable',
    })),
  ];
  const agentOptions = [
    { value: '', label: 'Select an Agent', unavailable: undefined as string | undefined },
    ...(parents?.workspace?.agents ?? []).map((item) => ({
      value: item.id,
      label: item.title?.trim() || item.id,
      unavailable: ['stopped', 'deleted', 'failed'].includes(item.status)
        ? 'Agent cannot accept work'
        : undefined,
    })),
  ];
  for (const [options, id, label] of [
    [sandboxOptions, sandboxId, 'Sandbox'],
    [workspaceOptions, workspaceId, 'Workspace'],
    [agentOptions, value.agent_id, 'Agent'],
  ] as const) {
    if (id !== '' && !options.some((option) => option.value === id))
      options.push({
        value: id,
        label: `Selected ${label} · ${id}`,
        unavailable: 'Current access has not verified this selection',
      });
  }
  const selected =
    editor.retainedAccess.state.status === 'ready' ? editor.retainedAccess.state.value : undefined;
  return (
    <div>
      <label>
        Retained Sandbox
        <Picker
          label="Retained Sandbox"
          value={sandboxId}
          options={sandboxOptions}
          disabled={disabled || page === undefined}
          onChange={(sandbox_id) =>
            onChange({ sandbox_id, workspace_id: '', worktree_id: '', agent_id: '' })
          }
        />
      </label>
      <label>
        Retained Workspace
        <Picker
          label="Retained Workspace"
          value={workspaceId}
          options={workspaceOptions}
          disabled={disabled || parents === undefined}
          onChange={(workspace_id) =>
            onChange({ ...value, workspace_id, worktree_id: '', agent_id: '' })
          }
        />
      </label>
      <label>
        Retained Agent
        <Picker
          label="Retained Agent"
          value={value.agent_id}
          options={agentOptions}
          disabled={disabled || parents?.workspace === undefined}
          onChange={(id) => {
            const agent = parents?.workspace?.agents.find((item) => item.id === id);
            if (agent !== undefined)
              onChange({
                sandbox_id: agent.sandbox_id,
                workspace_id: agent.workspace_id,
                worktree_id: agent.worktree_id,
                agent_id: agent.id,
              });
          }}
        />
      </label>
      <p>
        The organization owns this Agent and its parent resources. Allowed external senders share
        its existing conversation, files and connected services through this Bridge.
      </p>
      <RetainedReplyPreview
        client={client}
        agentId={value.agent_id}
        disabled={disabled || selected === undefined}
      />
      {value.agent_id === '' ? null : (
        <dl className="bridges__facts">
          <dt>Sandbox</dt>
          <dd>
            {selected?.sandbox.name ?? 'Current name unavailable'} · {sandboxId}
          </dd>
          <dt>Workspace</dt>
          <dd>
            {selected?.workspace.name ?? 'Current name unavailable'} · {workspaceId}
          </dd>
          <dt>Worktree</dt>
          <dd>{value.worktree_id}</dd>
          <dt>Agent</dt>
          <dd>
            {selected?.agent.title ?? 'Current name unavailable'} · {value.agent_id}
          </dd>
          <dt>Model</dt>
          <dd>{selected?.model ?? 'Unavailable in the current resource view'}</dd>
          <dt>Connected services</dt>
          <dd>
            {selected?.services === undefined
              ? 'Unavailable in the current resource view'
              : selected.services.join(', ') || 'None recorded'}
          </dd>
          <dt>Channel use</dt>
          <dd>
            {selected?.channelUsage === undefined
              ? 'Current channel use unavailable'
              : selected.channelUsage.active
                ? 'Linked to a channel'
                : selected.channelUsage.has_history
                  ? 'Previously linked to a channel'
                  : 'No channel activity recorded'}
          </dd>
          <dt>CMA access</dt>
          <dd>
            {selected === undefined
              ? 'Current access has not been verified'
              : 'Your current organization management and Agent interaction access are verified. Other people’s access follows the current resource grants and organization roles.'}
          </dd>
        </dl>
      )}
      {inventory.state.status !== 'error' ? null : (
        <p role="status">
          Sandbox choices are unavailable.{' '}
          <Button onClick={inventory.reload}>Retry retained Sandboxes</Button>
        </p>
      )}
      {hierarchy.state.status !== 'error' ? null : (
        <p role="status">
          Current management access or parent resources could not be verified.{' '}
          <Button onClick={hierarchy.reload}>Check retained hierarchy</Button>
        </p>
      )}
      {editor.retainedAccess.state.status !== 'error' ? null : (
        <p role="status">
          Current Agent access or ancestry could not be verified. Saved target IDs remain visible.{' '}
          <Button onClick={editor.retainedAccess.reload}>Check retained Agent</Button>
        </p>
      )}
      {page?.next === undefined && cursors.length === 1 ? null : (
        <div className="bridges__actions">
          <Button
            disabled={disabled || cursors.length === 1}
            onClick={() => setCursors((items) => items.slice(0, -1))}
          >
            Previous retained Sandboxes
          </Button>
          <Button
            disabled={disabled || page?.next === undefined}
            onClick={() => setCursors((items) => [...items, page?.next])}
          >
            More retained Sandboxes
          </Button>
        </div>
      )}
    </div>
  );
}
