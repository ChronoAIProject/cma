import { useAccessSummaries } from './useAccessSummaries';
import { SkeletonRows } from '../ui/Skeleton';
import { ResourceAccessSummary } from './ResourceAccessSummary';
import { useMemo, useState } from 'react';
import type {
  AgentCreationCategory,
  AgentStatus,
  AgentProvenanceInventoryOptions,
} from '@chronoai/cma-sdk';
import type { ResourceViewsClient } from './client';
import { useAuthorizedRead } from './useAuthorizedRead';
import { creationLabels } from '../agent/provenance';
import { statusLabel } from '../vocabulary/status';
import { Button } from '../ui/Button';
import { Picker } from '../ui/Picker';
import type { AppRoute } from '../routing/routes';
import { routePath } from '../routing/routes';
import { PageHeader } from '../ui/PageHeader';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import './AgentInventory.css';
import { scopeValue, scopeRoute } from './scope';
import { ExportInventoryPage } from './ExportInventoryPage';
import { channelFilters, channelQuery, channelLabel, type ChannelFilter } from './channelLabels';

type Category = 'all' | AgentCreationCategory;
type Status = 'all' | AgentStatus;

interface Props {
  readonly client: ResourceViewsClient;
  readonly organization?: string;
  readonly shared?: boolean;
  readonly onNavigate: (route: AppRoute) => void;
}

const categories: readonly { value: Category; label: string }[] = [
  { value: 'all', label: 'Created any way' },
  ...Object.entries(creationLabels).map(([value, label]) => ({
    value: value as AgentCreationCategory,
    label,
  })),
];
const statuses: readonly { value: Status; label: string }[] = [
  { value: 'all', label: 'All statuses' },
  ...(
    [
      'starting',
      'ready',
      'working',
      'waiting_approval',
      'paused',
      'stopped',
      'failed',
      'deleted',
    ] as const
  ).map((value) => ({ value, label: statusLabel(value) })),
];

export function AgentInventory({ client, organization, shared, onNavigate }: Props) {
  return (
    <Inventory
      key={scopeValue(organization, shared)}
      client={client}
      organization={organization}
      shared={shared}
      onNavigate={onNavigate}
    />
  );
}

function Inventory({ client, organization, shared, onNavigate }: Props) {
  useDocumentTitle('Agents');
  const [category, setCategory] = useState<Category>('all');
  const [status, setStatus] = useState<Status>('all');
  const [channel, setChannel] = useState<ChannelFilter>('all');
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const cursor = cursors.at(-1);
  const options = useMemo<AgentProvenanceInventoryOptions>(
    () => ({
      organization,
      shared,
      creationOrigin: category === 'all' ? undefined : category,
      status: status === 'all' ? undefined : status,
      ...channelQuery(channel),
      cursor,
      limit: 50,
    }),
    [organization, shared, category, status, channel, cursor],
  );
  const read = useMemo(
    () => async (signal: AbortSignal) => {
      const page = await client.agents.provenanceInventory({ ...options, signal });
      if (
        page.next !== undefined &&
        (page.next === '' || (cursor !== undefined && page.next <= cursor))
      )
        throw new Error('Agent inventory pagination did not advance. Refresh the inventory.');
      let previous = cursor;
      for (const row of page.items) {
        if (previous !== undefined && row.agent_id <= previous)
          throw new Error('Agent inventory order changed. Refresh the inventory.');
        previous = row.agent_id;
      }
      return { value: page };
    },
    [client, options, cursor],
  );
  const inventory = useAuthorizedRead(read);
  const readChoices = useMemo(
    () => async (signal: AbortSignal) => ({
      value: (await client.resourceAccess.organizations({ signal })).items,
    }),
    [client],
  );
  const organizations = useAuthorizedRead(readChoices);
  const choices = organizations.state.status === 'ready' ? organizations.state.value : [];
  const selected = choices.find((choice) => choice.id === organization);
  const scopeLabel =
    shared === true
      ? 'Shared'
      : organization === undefined
        ? 'Personal'
        : `${selected?.display_name?.trim() || 'Organization'} (Organization)`;
  const scopes = [
    { value: '', label: 'Personal' },
    { value: 'shared', label: 'Shared' },
    ...choices.map((choice, index) => ({
      value: scopeValue(choice.id),
      label: `${choice.display_name?.trim() || `Organization ${String(index + 1)}`} (Organization)`,
    })),
  ];
  if (organization !== undefined && selected === undefined)
    scopes.push({ value: scopeValue(organization), label: 'Selected organization (Organization)' });
  const page = inventory.state.status === 'ready' ? inventory.state.value : undefined;
  const accessSummaries = useAccessSummaries(
    client,
    'agent',
    page?.items.map((row) => row.agent_id) ?? [],
    scopeValue(organization, shared),
  );
  return (
    <section className="agent-inventory" aria-labelledby="inventory-title">
      <div className="agent-inventory__header">
        <PageHeader
          breadcrumb={[
            {
              label: 'Agent Manager',
              href: routePath({ kind: 'agent_manager' }),
              onSelect: () => onNavigate({ kind: 'agent_manager' }),
            },
            { label: 'Agents' },
          ]}
          title="Agents"
          headingId="inventory-title"
          description="Find an Agent by how it was created, what it is doing now, or whether it is linked to a chat channel."
          actions={<Button onClick={inventory.reload}>Refresh</Button>}
        />
        <div className="agent-inventory__filters">
          <Picker
            label="Resource scope"
            value={scopeValue(organization, shared)}
            options={scopes}
            onChange={(value) => onNavigate(scopeRoute(value))}
          />
          <Picker
            label="Creation origin"
            value={category}
            options={categories}
            onChange={(value) => {
              setCategory(value);
              setCursors([undefined]);
            }}
          />
          <Picker
            label="Agent status"
            value={status}
            options={statuses}
            onChange={(value) => {
              setStatus(value);
              setCursors([undefined]);
            }}
          />
          <Picker
            label="Channel activity"
            value={channel}
            options={channelFilters}
            onChange={(value) => {
              setChannel(value);
              setCursors([undefined]);
            }}
          />
        </div>
        <p className="agent-inventory__scope">
          {scopeLabel}
          {selected === undefined
            ? null
            : ` · ${selected.role === 'admin' ? 'Organization admin' : selected.role === 'member' ? 'Organization member' : 'Organization viewer'}`}
        </p>
        {organizations.state.status === 'error' ? (
          <p role="status">
            Organization choices are unavailable.{' '}
            <Button size="sm" onClick={organizations.reload}>
              Retry choices
            </Button>
          </p>
        ) : null}
      </div>
      <div className="agent-inventory__body">
        {inventory.state.status === 'loading' ? (
          <SkeletonRows rows={5} label="Loading Agents" />
        ) : null}
        {inventory.state.status === 'error' ? (
          <div role="alert">
            <p>{inventory.state.message}</p>
            <Button onClick={inventory.reload}>Try again</Button>
          </div>
        ) : null}
        {page === undefined ? null : (
          <>
            {page.items.length === 0 ? (
              <p>
                {page.next === undefined
                  ? 'No Agents match this scope and these filters.'
                  : 'No matches on this page. Continue to check the next page.'}
              </p>
            ) : (
              <ul className="agent-inventory__rows" aria-label="Agents in this scope">
                {page.items.map((row) => {
                  const route: AppRoute = { kind: 'agent', agentId: row.agent_id };
                  return (
                    <li key={row.agent_id}>
                      <a
                        href={routePath(route)}
                        onClick={(event) => {
                          if (
                            event.button !== 0 ||
                            event.metaKey ||
                            event.ctrlKey ||
                            event.shiftKey ||
                            event.altKey
                          )
                            return;
                          event.preventDefault();
                          onNavigate(route);
                        }}
                      >
                        {row.title?.trim() || 'Untitled agent'}
                      </a>
                      <ResourceAccessSummary
                        className="agent-inventory__access"
                        value={accessSummaries.get(row.agent_id)}
                      />
                      <span>{statusLabel(row.status)}</span>
                      <span>{creationLabels[row.creation_origin]}</span>
                      <span>{channelLabel(row.channel_usage)}</span>
                      <time dateTime={row.created_at}>
                        {new Date(row.created_at).toLocaleDateString()}
                      </time>
                    </li>
                  );
                })}
              </ul>
            )}
            <nav className="agent-inventory__pages" aria-label="Agent inventory pages">
              <Button
                disabled={cursors.length === 1}
                onClick={() => setCursors((values) => values.slice(0, -1))}
              >
                Previous page
              </Button>
              <span>Page {cursors.length}</span>
              <Button
                disabled={page.next === undefined}
                onClick={() => setCursors((values) => [...values, page.next])}
              >
                Next page
              </Button>
            </nav>
            <ExportInventoryPage
              onAccessChanged={inventory.reload}
              client={client}
              options={options}
              disabled={page.items.length === 0}
            />
          </>
        )}
      </div>
    </section>
  );
}
