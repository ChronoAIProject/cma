import { Presence } from '../ui/Presence';
import { useEffect, useMemo, useState } from 'react';
import { ApiError, type AgentResources } from '@chronoai/cma-sdk';
import { useCmaClient } from '../api/context';
import type { ConfiguredSandboxCapacity, ProfileReference } from '../api/types';
import type { SandboxResourceAvailability } from './sandboxInput';
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

export function BoundedReading({
  label,
  value,
  limit,
  configured = false,
  format,
  missing = 'not reported',
}: {
  readonly label: string;
  readonly value?: number | null;
  readonly limit?: number | null;
  readonly configured?: boolean;
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
            {limit === undefined || limit === null
              ? ''
              : ` / ${format(limit)}${configured ? ' configured' : ''}`}
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
          aria-valuetext={`${format(value ?? 0)} of ${format(limit ?? 0)}${configured ? ' configured' : ''}`}
        >
          <span className="resources__fill" style={{ width: `${String(percent)}%` }} />
        </span>
      )}
    </div>
  );
}

const cores = (value: number) =>
  `${new Intl.NumberFormat('en', { maximumFractionDigits: 2 }).format(value / 1000)} cores`;

export function Resources({
  agentId,
  sandboxId,
  sandboxProfile,
  availability,
}: {
  readonly agentId: string;
  readonly sandboxId: string;
  readonly sandboxProfile?: ProfileReference;
  readonly availability: SandboxResourceAvailability;
}) {
  const client = useCmaClient();
  const profileId = sandboxProfile?.id;
  const profileRevision = sandboxProfile?.revision;
  const profileDigest = sandboxProfile?.digest;
  const [capacityState, setCapacityState] = useState<{
    readonly agentId: string;
    readonly sandboxId: string;
    readonly profile: ProfileReference;
    readonly capacity: ConfiguredSandboxCapacity;
  }>();
  useEffect(() => {
    if (profileId === undefined || profileRevision === undefined) return;
    const request = new AbortController();
    const profile = { id: profileId, revision: profileRevision, digest: profileDigest };
    void client.readSandboxProfileCapacity(profile, { signal: request.signal }).then(
      (capacity) => {
        if (!request.signal.aborted) setCapacityState({ agentId, sandboxId, profile, capacity });
      },
      () => {
        if (!request.signal.aborted) setCapacityState(undefined);
      },
    );
    return () => request.abort();
  }, [agentId, client, profileDigest, profileId, profileRevision, sandboxId]);
  const capacity =
    capacityState?.agentId === agentId &&
    capacityState.sandboxId === sandboxId &&
    capacityState.profile.id === profileId &&
    capacityState.profile.revision === profileRevision &&
    capacityState.profile.digest === profileDigest
      ? capacityState.capacity
      : undefined;
  const runtimeVersion = availability.state === 'live' ? availability.runtimeVersion : undefined;
  const sampleIdentity = useMemo(
    () => ({ agentId, sandboxId, runtimeVersion }),
    [agentId, sandboxId, runtimeVersion],
  );
  const [state, setState] = useState<{
    readonly sampleIdentity: typeof sampleIdentity;
    readonly reading?: AgentResources;
    readonly cpu?: number;
    readonly error?: string;
  }>({ sampleIdentity });
  useEffect(() => {
    if (runtimeVersion === undefined) return;
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
          setState({ sampleIdentity, reading, cpu: cpuMillis(previous, reading) });
        previous = reading;
      } catch (cause) {
        previous = undefined;
        if (!request.signal.aborted)
          setState((current) =>
            current.sampleIdentity === sampleIdentity
              ? {
                  ...current,
                  error:
                    current.reading === undefined
                      ? cause instanceof ApiError && cause.code === 'agent_resources_unsupported'
                        ? 'This Sandbox does not report usage readings.'
                        : cause instanceof ApiError && cause.status === 403
                          ? 'Resources are not permitted for this Agent'
                          : 'Usage readings are unavailable right now. Your Agent keeps working.'
                      : 'Last reading; reconnecting',
                }
              : {
                  sampleIdentity,
                  error: 'Usage readings are unavailable right now. Your Agent keeps working.',
                },
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
  }, [agentId, client, sandboxId, runtimeVersion, sampleIdentity]);
  const current =
    runtimeVersion !== undefined && state.sampleIdentity === sampleIdentity ? state : undefined;
  const reading = current?.reading;
  const inactive = availability.state !== 'live';
  const missing =
    availability.state === 'paused'
      ? 'paused'
      : availability.state === 'checking'
        ? 'waiting'
        : 'unavailable';
  const notice = inactive ? availability.notice : current?.error;
  const memoryObserved = (reading?.memory_bytes_limit ?? 0) > 0;
  const cpuObserved = (reading?.cpu_millis_limit ?? 0) > 0;
  return (
    <section className="sandbox-instruments" aria-label="Sandbox resources">
      <div className="resources" aria-label="Resources">
        <BoundedReading
          label="RAM"
          value={reading?.memory_bytes_used}
          limit={memoryObserved ? reading?.memory_bytes_limit : capacity?.memoryBytes}
          configured={!memoryObserved && capacity !== undefined}
          format={formatBytes}
          missing={
            inactive
              ? missing
              : reading === undefined && current?.error === undefined
                ? 'waiting'
                : 'not reported'
          }
        />
        <BoundedReading
          label="CPU"
          value={current?.cpu}
          limit={cpuObserved ? reading?.cpu_millis_limit : capacity?.cpuMillis}
          configured={!cpuObserved && capacity !== undefined}
          format={cores}
          missing={
            inactive
              ? missing
              : reading === undefined
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
            {inactive
              ? missing
              : reading?.received_bytes === undefined || reading.received_bytes === null
                ? 'not reported'
                : `↓ ${formatBytes(reading.received_bytes)}`}
            {reading?.sent_bytes === undefined || reading.sent_bytes === null
              ? ''
              : ` · ↑ ${formatBytes(reading.sent_bytes)}`}
          </span>
        </div>
      </div>
      <Presence>
        {notice === undefined ? null : (
          <p className="resources__status" role="status">
            {notice}
          </p>
        )}
      </Presence>
    </section>
  );
}
