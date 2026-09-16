import { useMemo, useState } from 'react';
import type { SharedPlacement } from './placementAccess';
import type { BridgeViewsClient } from './client';
import { useAuthorizedRead } from '../resources/useAuthorizedRead';
import { Picker } from '../ui/Picker';
import { Button } from '../ui/Button';

export function SharedParentPicker({
  client,
  value,
  disabled,
  organization,
  onChange,
}: {
  readonly client: BridgeViewsClient;
  readonly value: SharedPlacement;
  readonly disabled: boolean;
  readonly organization?: string;
  readonly onChange: (value: SharedPlacement) => void;
}) {
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const cursor = cursors.at(-1);
  const read = useMemo(
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
            if (new Set(page.items.map((item) => item.id)).size !== page.items.length)
              throw new Error('Sandbox inventory contains repeated identities.');
            if (page.next !== undefined && cursors.includes(page.next))
              throw new Error('Sandbox page did not advance.');
            return { value: page };
          },
    [client, organization, disabled, cursor, cursors],
  );
  const sandboxes = useAuthorizedRead(read);
  const page = sandboxes.state.status === 'ready' ? sandboxes.state.value : undefined;
  const sandboxOptions = [
    { value: '', label: 'Select a Sandbox', unavailable: undefined as string | undefined },
    ...(page?.items ?? []).map((item) => ({
      value: item.id,
      label: item.name,
      unavailable: ['active', 'paused'].includes(item.status)
        ? undefined
        : 'Sandbox is not active or paused',
    })),
  ];
  if (value.sandbox_id !== '' && !sandboxOptions.some((item) => item.value === value.sandbox_id))
    sandboxOptions.push({
      value: value.sandbox_id,
      label: `Selected Sandbox · ${value.sandbox_id}`,
      unavailable: 'Current inventory has not verified this selection',
    });
  const sandboxId = value.sandbox_id;
  const workspaceMode = value.kind === 'worktree_agent_per_partition';
  const workspaceRead = useMemo(
    () =>
      disabled || !workspaceMode || sandboxId === ''
        ? undefined
        : async (signal: AbortSignal) => {
            const page = await client.workspaces.listAccessible(sandboxId, { signal });
            if (
              page.items.some((item) => item.sandbox_id !== sandboxId) ||
              new Set(page.items.map((item) => item.id)).size !== page.items.length
            )
              throw new Error('Workspace inventory does not match this Sandbox.');
            return { value: page };
          },
    [client, disabled, workspaceMode, sandboxId],
  );
  const workspaces = useAuthorizedRead(workspaceRead);
  const children = workspaces.state.status === 'ready' ? workspaces.state.value.items : undefined;
  const workspaceOptions = [
    { value: '', label: 'Select a Workspace', unavailable: undefined as string | undefined },
    ...(children ?? []).map((item) => ({
      value: item.id,
      label: item.name,
      unavailable: item.status === 'ready' ? undefined : 'Workspace is not ready',
    })),
  ];
  if (
    value.kind === 'worktree_agent_per_partition' &&
    value.workspace_id !== '' &&
    !workspaceOptions.some((item) => item.value === value.workspace_id)
  )
    workspaceOptions.push({
      value: value.workspace_id,
      label: `Selected Workspace · ${value.workspace_id}`,
      unavailable: 'Current inventory has not verified this selection',
    });
  return (
    <div>
      <label>
        Shared Sandbox
        <Picker
          label="Shared Sandbox"
          value={value.sandbox_id}
          options={sandboxOptions}
          disabled={disabled || page === undefined}
          onChange={(sandbox_id) =>
            onChange(
              value.kind === 'worktree_agent_per_partition'
                ? { ...value, sandbox_id, workspace_id: '' }
                : { ...value, sandbox_id },
            )
          }
        />
      </label>
      {value.kind === 'worktree_agent_per_partition' ? (
        <label>
          Shared Workspace
          <Picker
            label="Shared Workspace"
            value={value.workspace_id}
            options={workspaceOptions}
            disabled={disabled || children === undefined}
            onChange={(workspace_id) => onChange({ ...value, workspace_id })}
          />
        </label>
      ) : null}
      {sandboxes.state.status === 'error' ? (
        <p role="status">
          Sandbox choices are unavailable.{' '}
          <Button onClick={sandboxes.reload}>Retry Sandbox choices</Button>
        </p>
      ) : null}
      {workspaceMode && workspaces.state.status === 'error' ? (
        <p role="status">
          Workspace choices are unavailable.{' '}
          <Button onClick={workspaces.reload}>Retry Workspace choices</Button>
        </p>
      ) : null}
      {page?.next !== undefined || cursors.length > 1 ? (
        <div className="bridges__actions">
          <Button
            disabled={disabled || cursors.length === 1}
            onClick={() => setCursors((items) => items.slice(0, -1))}
          >
            Previous Sandboxes
          </Button>
          <Button
            disabled={disabled || page?.next === undefined}
            onClick={() => setCursors((items) => [...items, page?.next])}
          >
            More Sandboxes
          </Button>
        </div>
      ) : null}
    </div>
  );
}
