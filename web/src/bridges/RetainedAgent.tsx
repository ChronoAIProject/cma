import { useMemo, useState } from 'react';
import type { BridgePlacement } from '@chronoai/cma-sdk';
import type { BridgeViewsClient } from './client';
import { useAuthorizedRead } from '../resources/useAuthorizedRead';
import { Picker } from '../ui/Picker';
import { Button } from '../ui/Button';
import { RetainedReplyPreview } from './RetainedReplyPreview';

type Target = Extract<BridgePlacement, { kind: 'existing_agent' }>['target'];
export function RetainedAgent({
  client,
  value,
  disabled,
  onChange,
}: {
  readonly client: BridgeViewsClient;
  readonly value: Target;
  readonly disabled: boolean;
  readonly onChange: (target: Target) => void;
}) {
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const cursor = cursors.at(-1);
  const read = useMemo(
    () =>
      disabled
        ? undefined
        : async (signal: AbortSignal) => {
            const page = await client.agents.list({ cursor, limit: 50, signal });
            if (page.next !== undefined && (page.next === cursor || cursors.includes(page.next)))
              throw new Error('Agent page did not advance');
            return { value: page };
          },
    [client, disabled, cursor, cursors],
  );
  const inventory = useAuthorizedRead(read);
  const page = inventory.state.status === 'ready' ? inventory.state.value : undefined;
  const detailRead = useMemo(
    () =>
      disabled || value.agent_id === ''
        ? undefined
        : async (signal: AbortSignal) => ({
            value: await client.agents.read(value.agent_id, { signal }),
          }),
    [client, disabled, value.agent_id],
  );
  const detail = useAuthorizedRead(detailRead);
  const selected = detail.state.status === 'ready' ? detail.state.value : undefined;
  const options = [
    { value: '', label: 'Select an Agent' },
    ...(page?.items ?? []).map((item) => ({
      value: item.id,
      label: item.title?.trim() || item.id,
      unavailable: ['deleted', 'stopped', 'failed'].includes(item.status)
        ? 'Agent cannot accept work'
        : undefined,
    })),
  ];
  if (value.agent_id !== '' && !options.some((item) => item.value === value.agent_id))
    options.push({ value: value.agent_id, label: 'Selected retained Agent' });
  return (
    <div>
      <Picker
        label="Retained Agent"
        value={value.agent_id}
        options={options}
        disabled={disabled || page === undefined}
        onChange={(id) => {
          const item = page?.items.find((agent) => agent.id === id);
          if (item !== undefined)
            onChange({
              agent_id: item.id,
              sandbox_id: item.sandbox_id,
              workspace_id: item.workspace_id,
              worktree_id: item.worktree_id,
            });
        }}
      />
      {selected === undefined ? (
        <p>Select an Agent with a currently authorized full resource view.</p>
      ) : (
        <dl className="bridges__facts">
          <dt>Sandbox</dt>
          <dd>{selected.sandbox.name}</dd>
          <dt>Workspace</dt>
          <dd>{selected.workspace.name}</dd>
          <dt>Worktree</dt>
          <dd>{selected.worktree.name}</dd>
          <dt>Agent</dt>
          <dd>{selected.title?.trim() || selected.id}</dd>
          <dt>Model</dt>
          <dd>{selected.profile.model ?? 'Selected by the Agent profile'}</dd>
          <dt>Existing history</dt>
          <dd>Retained. Messages from allowed channel senders enter this existing conversation.</dd>
        </dl>
      )}
      <RetainedReplyPreview
        client={client}
        agentId={value.agent_id}
        disabled={disabled || selected === undefined}
      />
      {page?.next !== undefined || cursors.length > 1 ? (
        <div className="bridges__actions">
          <Button
            disabled={disabled || cursors.length === 1}
            onClick={() => setCursors((items) => items.slice(0, -1))}
          >
            Previous Agents
          </Button>
          <Button
            disabled={disabled || page?.next === undefined}
            onClick={() => setCursors((items) => [...items, page?.next])}
          >
            More Agents
          </Button>
        </div>
      ) : null}
    </div>
  );
}
