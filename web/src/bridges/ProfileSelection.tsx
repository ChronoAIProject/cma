import { useMemo, useState } from 'react';
import type { ProfileKind, ProfileReference } from '@chronoai/cma-sdk';
import type { BridgeViewsClient } from './client';
import { useAuthorizedRead } from '../resources/useAuthorizedRead';
import { Picker } from '../ui/Picker';
import { Button } from '../ui/Button';

export function ProfileSelection({
  client,
  kind,
  value,
  disabled,
  organization,
  onChange,
}: {
  readonly client: BridgeViewsClient;
  readonly kind: ProfileKind;
  readonly value: ProfileReference;
  readonly disabled: boolean;
  readonly organization?: string;
  readonly onChange: (value: ProfileReference) => void;
}) {
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const cursor = cursors.at(-1);
  const profiles =
    kind === 'agent'
      ? client.agentProfiles
      : kind === 'workspace'
        ? client.workspaceProfiles
        : client.sandboxProfiles;
  const read = useMemo(
    () =>
      disabled
        ? undefined
        : async (signal: AbortSignal) => {
            const page = await profiles.list({ cursor, limit: 50, signal });
            if (page.next !== undefined && (page.next === cursor || cursors.includes(page.next)))
              throw new Error('Profile page did not advance');
            return { value: page };
          },
    [disabled, profiles, cursor, cursors],
  );
  const choices = useAuthorizedRead(read);
  const page = choices.state.status === 'ready' ? choices.state.value : undefined;
  const available =
    page?.items.filter(
      (item) =>
        item.latest_revision !== undefined &&
        item.retired_at === undefined &&
        (organization === undefined || item.owner === organization || item.visibility === 'public'),
    ) ?? [];
  const selected = available.find(
    (item) => item.id === value.id && item.latest_revision?.revision === value.revision,
  );
  const options = [
    { value: '', label: `Select a ${kind} profile` },
    ...available.map((item) => ({
      value: item.id,
      label: `${item.name} · Revision ${item.latest_revision?.revision}`,
    })),
  ];
  if (value.id !== '' && selected === undefined)
    options.push({
      value: value.id,
      label: `Selected ${kind} profile · ${value.id} · Revision ${value.revision}`,
    });
  return (
    <div className="bridges__profile">
      <label>
        {kind === 'agent' ? 'Agent' : kind === 'workspace' ? 'Workspace' : 'Sandbox'} profile
        <Picker
          label={`${kind} profile`}
          disabled={disabled || page === undefined}
          value={value.id}
          options={options}
          onChange={(id) => {
            const item = available.find((entry) => entry.id === id);
            if (item?.latest_revision !== undefined)
              onChange({ id: item.id, revision: item.latest_revision.revision });
          }}
        />
      </label>
      {selected !== undefined ? <p>{selected.description}</p> : null}
      {choices.state.status === 'error' ? (
        <p role="status">
          Profile choices are unavailable. <Button onClick={choices.reload}>Retry profiles</Button>
        </p>
      ) : null}
      {page?.next !== undefined || cursors.length > 1 ? (
        <div className="bridges__actions">
          <Button
            disabled={disabled || cursors.length === 1}
            onClick={() => setCursors((items) => items.slice(0, -1))}
          >
            Previous {kind} profiles
          </Button>
          <Button
            disabled={disabled || page?.next === undefined}
            onClick={() => setCursors((items) => [...items, page?.next])}
          >
            More {kind} profiles
          </Button>
        </div>
      ) : null}
    </div>
  );
}
