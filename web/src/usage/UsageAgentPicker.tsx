import { useCallback, useState } from 'react';
import { ProtocolError } from '@cma/client';
import type { UsageClient } from './client';
import { Button } from '../ui/Button';
import { Picker } from '../ui/Picker';
import { useUsageRead, useUsageVisible } from './metrics/useReport';
import { MetricsNotice } from './metrics/MetricsNotice';
import { IconChevronDownOutline14 } from '../ui/icons';
import { resourceLabelKey, useSelectedResourceLabel } from './metrics/selectedResourceLabel';

export function UsageAgentPicker(props: {
  readonly client: UsageClient;
  readonly sandbox: string;
  readonly value: string;
  readonly active: boolean;
  readonly onChange: (agent: string) => void;
}) {
  return props.sandbox ? (
    <AgentChoices key={props.sandbox} {...props} />
  ) : (
    <p>Select one of your Sandboxes to optionally choose an Agent.</p>
  );
}
function AgentChoices({
  client,
  sandbox,
  value,
  active,
  onChange,
}: {
  readonly client: UsageClient;
  readonly sandbox: string;
  readonly value: string;
  readonly active: boolean;
  readonly onChange: (agent: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const cursor = cursors[cursors.length - 1];
  const visible = useUsageVisible(active && open);
  const load = useCallback(
    async (signal: AbortSignal) => {
      const result = await client.agents.list({ sandbox, cursor, limit: 50, signal });
      if (result.items.some((agent) => agent.sandboxId !== sandbox))
        throw new ProtocolError('Agent Sandbox selection did not match');
      return result;
    },
    [client, sandbox, cursor],
  );
  const page = useUsageRead(load, visible, client.agents);
  const resolve = useCallback(
    async (signal: AbortSignal) => {
      const agent = await client.agents.read(value, { signal });
      if (agent.id !== value || agent.sandboxId !== sandbox)
        throw new ProtocolError('Agent selection did not match');
      return agent.name.trim() || 'Untitled Agent';
    },
    [client, sandbox, value],
  );
  const selected = useSelectedResourceLabel({
    key: resourceLabelKey('agent', value, sandbox),
    load: resolve,
    active: active && !!value,
    scope: client.agents,
  });
  const name = !value
    ? 'All Agents in this Sandbox'
    : selected.error
      ? 'Selected Agent unavailable'
      : (selected.name ?? 'Loading Agent…');
  return (
    <div className="usage-metric-selector">
      <span className="usage-filter-label">Agent</span>
      <Button
        size="md"
        variant="outline"
        placement="usage-discovery-control"
        aria-label={`Agent: ${name}`}
        aria-expanded={open}
        onClick={() => setOpen((previous) => !previous)}
      >
        <span className="usage-discovery-value" title={name}>
          {name}
        </span>
        <IconChevronDownOutline14 />
      </Button>
      {value && (
        <Button size="sm" onClick={() => onChange('')}>
          Clear Agent
        </Button>
      )}
      <MetricsNotice
        failure={selected.error}
        onRetry={selected.retry}
        retryLabel="Retry Agent name"
      />
      {open && (
        <>
          <p>Shared Sandbox usage. Selecting an Agent reports its Sandbox's shared resources.</p>
          <MetricsNotice failure={page.error} onRetry={page.retry} />
          {page.loading && <p role="status">Loading owned Sandbox Agents…</p>}
          {page.data && (
            <>
              <Picker
                label="Usage Agent"
                value={value}
                options={[
                  { value: '', label: 'All Agents in this Sandbox' },
                  ...page.data.items.map((agent) => ({
                    value: agent.id,
                    label: `${agent.name.trim() || 'Untitled Agent'} (${agent.id})`,
                  })),
                  ...(value && !page.data.items.some((agent) => agent.id === value)
                    ? [{ value, label: name }]
                    : []),
                ]}
                onChange={(agent) => {
                  const choice = page.data?.items.find((item) => item.id === agent);
                  if (choice)
                    selected.remember(
                      resourceLabelKey('agent', agent, sandbox),
                      choice.name.trim() || 'Untitled Agent',
                    );
                  onChange(agent);
                  setOpen(false);
                }}
              />
              {page.data.items.length === 0 && <p>No Agents in this Sandbox.</p>}
              <Button
                size="sm"
                disabled={cursors.length === 1}
                onClick={() => setCursors((previous) => previous.slice(0, -1))}
              >
                Previous Agents
              </Button>
              <Button
                size="sm"
                disabled={!page.data.next}
                onClick={() => {
                  const next = page.data?.next;
                  if (next && !cursors.includes(next))
                    setCursors((previous) => [...previous, next]);
                }}
              >
                Next Agents
              </Button>
            </>
          )}
        </>
      )}
    </div>
  );
}
