import { useEffect, useState } from 'react';
import { ApiError, type AgentResources } from '@chronoai/cma-sdk';
import { useCmaClient } from '../api/context';
import type { AgentUsage } from '../api/types';
import './Resources.css';

export function cpuMillis(
  previous: AgentResources | undefined,
  current: AgentResources,
): number | undefined {
  if (
    previous === undefined ||
    previous.sandbox_id !== current.sandbox_id ||
    previous.cpu_usage_usec === null ||
    current.cpu_usage_usec === null ||
    current.uptime_ms <= previous.uptime_ms ||
    current.cpu_usage_usec < previous.cpu_usage_usec
  )
    return undefined;
  return (
    (current.cpu_usage_usec - previous.cpu_usage_usec) / (current.uptime_ms - previous.uptime_ms)
  );
}

export function formatBytes(value: number): string {
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  const unit =
    value < 1024 ? 0 : Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
  return `${new Intl.NumberFormat('en', { maximumFractionDigits: 1 }).format(value / 1024 ** unit)} ${units[unit]}`;
}

function BoundedReading({
  label,
  value,
  limit,
  format,
  missing = 'not reported',
}: {
  readonly label: string;
  readonly value?: number | null;
  readonly limit?: number | null;
  readonly format: (value: number) => string;
  readonly missing?: string;
}) {
  const percent =
    value === undefined || value === null || limit === undefined || limit === null || limit <= 0
      ? undefined
      : Math.min(100, Math.max(0, (value / limit) * 100));
  return (
    <div className="resources__row">
      <span className="resources__figure">
        {label} ·{' '}
        {value === undefined || value === null ? (
          missing
        ) : (
          <>
            {format(value)}
            {limit === undefined || limit === null ? '' : ` / ${format(limit)}`}
          </>
        )}
      </span>
      {percent === undefined ? null : (
        <span
          className="resources__track"
          role="progressbar"
          aria-label={label}
          aria-valuemin={0}
          aria-valuemax={limit ?? 0}
          aria-valuenow={Math.min(limit ?? 0, value ?? 0)}
          aria-valuetext={`${format(value ?? 0)} of ${format(limit ?? 0)}`}
        >
          <span className="resources__fill" style={{ width: `${String(percent)}%` }} />
        </span>
      )}
    </div>
  );
}

const count = (value: number) => new Intl.NumberFormat('en').format(value);
const cores = (value: number) =>
  `${new Intl.NumberFormat('en', { maximumFractionDigits: 2 }).format(value / 1000)} cores`;

export function Resources({
  agentId,
  sandboxId,
  usage,
}: {
  readonly agentId: string;
  readonly sandboxId: string;
  readonly usage?: AgentUsage;
}) {
  const client = useCmaClient();
  const [state, setState] = useState<{
    readonly agentId: string;
    readonly reading?: AgentResources;
    readonly cpu?: number;
    readonly error?: string;
  }>({ agentId });
  useEffect(() => {
    const request = new AbortController();
    let previous: AgentResources | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let pending = false;
    const poll = async () => {
      if (pending || request.signal.aborted || document.visibilityState === 'hidden') return;
      clearTimeout(timer);
      pending = true;
      try {
        const reading = await client.readAgentResources(agentId, { signal: request.signal });
        if (reading.agent_id !== agentId || reading.sandbox_id !== sandboxId)
          throw new Error('Resource identity mismatch');
        if (!request.signal.aborted)
          setState({ agentId, reading, cpu: cpuMillis(previous, reading) });
        previous = reading;
      } catch (cause) {
        previous = undefined;
        if (!request.signal.aborted)
          setState((current) =>
            current.agentId === agentId
              ? {
                  ...current,
                  error:
                    current.reading === undefined
                      ? cause instanceof ApiError && cause.code === 'agent_resources_unsupported'
                        ? 'This sandbox does not report resources'
                        : cause instanceof ApiError && cause.status === 403
                          ? 'Resources are not permitted for this Agent'
                          : 'Resources unavailable'
                      : 'Last reading; reconnecting',
                }
              : { agentId, error: 'Resources unavailable' },
          );
      } finally {
        pending = false;
        if (!request.signal.aborted) timer = setTimeout(() => void poll(), 5000);
      }
    };
    const onVisibility = () => {
      if (document.visibilityState === 'visible') void poll();
    };
    void poll();
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      request.abort();
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, [agentId, client, sandboxId]);
  const current = state.agentId === agentId ? state : undefined;
  const reading = current?.reading;
  return (
    <section className="agent-instruments" aria-label="Agent metrics">
      <div className="agent-instruments__usage" aria-label="Agent usage">
        <div className="resources__row">
          <span className="resources__figure">
            Tokens ·{' '}
            {usage === undefined
              ? 'not reported'
              : `${count(usage.inputTokens)} input · ${count(usage.outputTokens)} output`}
          </span>
        </div>
        <BoundedReading
          label="Context"
          value={usage?.contextUsed}
          limit={usage?.contextWindow}
          format={count}
        />
      </div>
      <div className="resources" aria-label="Resources">
        <BoundedReading
          label="RAM"
          value={reading?.memory_bytes_used}
          limit={reading?.memory_bytes_limit}
          format={formatBytes}
          missing={
            reading === undefined && current?.error === undefined ? 'waiting' : 'not reported'
          }
        />
        <BoundedReading
          label="CPU"
          value={current?.cpu}
          limit={reading?.cpu_millis_limit}
          format={cores}
          missing={
            reading === undefined
              ? current?.error === undefined
                ? 'waiting'
                : 'not reported'
              : reading.cpu_usage_usec === null
                ? 'not reported'
                : 'measuring'
          }
        />
        <div className="resources__row">
          <span className="resources__figure">
            Network ·{' '}
            {reading?.received_bytes === undefined || reading.received_bytes === null
              ? 'not reported'
              : `↓ ${formatBytes(reading.received_bytes)}`}
            {reading?.sent_bytes === undefined || reading.sent_bytes === null
              ? ''
              : ` · ↑ ${formatBytes(reading.sent_bytes)}`}
          </span>
        </div>
      </div>
      {current?.error === undefined ? null : (
        <p className="resources__status" role="status">
          {current.error}
        </p>
      )}
    </section>
  );
}
