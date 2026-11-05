import { useCallback, useState } from 'react';
import type { UsageClient } from './client';
import { Button } from '../ui/Button';
import { Picker } from '../ui/Picker';
import { useUsageRead, useUsageVisible } from './metrics/useReport';
import { MetricsNotice } from './metrics/MetricsNotice';

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
    (signal: AbortSignal) => client.agents.list({ sandbox, cursor, limit: 50, signal }),
    [client, sandbox, cursor],
  );
  const page = useUsageRead(load, visible);
  return (
    <div className="usage-metric-selector">
      <Button size="sm" aria-expanded={open} onClick={() => setOpen((previous) => !previous)}>
        Agent: {value || 'All Agents in this Sandbox'}
      </Button>
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
                    label: agent.title?.trim() || agent.id,
                  })),
                  ...(value && !page.data.items.some((agent) => agent.id === value)
                    ? [{ value, label: value }]
                    : []),
                ]}
                onChange={(agent) => {
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
