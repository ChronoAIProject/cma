import { Presence } from '../ui/Presence';
import { useEffect, useState } from 'react';
import type { SandboxDetail } from '../api/types';
import type { CmaWebClient } from '../api/client';
import { useCmaClient } from '../api/context';
import { SandboxActivity } from './SandboxActivity';
import { sandboxLifecycleLabel } from './lifecycle';
import { sandboxHasParkedRuntimeUpdate } from './parkedRuntimeUpdate';
import {
  lifecycleStageCopy,
  pausePhases,
  restorePhases,
  sandboxProgressCopy as copy,
} from '../vocabulary/sandboxProgress';
import './LifecycleProgress.css';

export function elapsedDuration(startedAt: string, now: number): string | undefined {
  const start = Date.parse(startedAt);
  if (!Number.isFinite(start)) return undefined;
  const seconds = Math.max(0, Math.floor((now - start) / 1000));
  const minutes = Math.floor(seconds / 60);
  return minutes >= 60
    ? `${Math.floor(minutes / 60)}h ${minutes % 60}m ${seconds % 60}s`
    : `${minutes}m ${seconds % 60}s`;
}

export function LifecycleProgress({
  sandbox,
  observedAt,
  stale = false,
  client: suppliedClient,
}: {
  readonly sandbox: SandboxDetail;
  readonly observedAt?: string;
  readonly stale?: boolean;
  readonly client?: CmaWebClient;
}) {
  const contextClient = useCmaClient();
  const client = suppliedClient ?? contextClient;
  const operation = sandbox.lifecycle?.operation;
  const hasOperation = operation !== undefined;
  const parked = sandboxHasParkedRuntimeUpdate(sandbox);
  const hasProgress = hasOperation && !parked;
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!hasProgress) return;
    const tick = () => {
      if (!document.hidden) setNow(Date.now());
    };
    tick();
    const timer = setInterval(tick, 1000);
    document.addEventListener('visibilitychange', tick);
    return () => {
      clearInterval(timer);
      document.removeEventListener('visibilitychange', tick);
    };
  }, [hasProgress]);
  if (operation === undefined) {
    const hasProviderPersistence =
      sandbox.lifecycle?.backgroundSave !== undefined ||
      sandbox.lifecycle?.checkpoint !== undefined;
    if (sandbox.status === 'active' && !hasProviderPersistence) return null;
    return (
      <section className="lifecycle-progress" aria-label="Sandbox progress">
        <p className="lifecycle-progress__stage">{sandboxLifecycleLabel(sandbox)}</p>
        {hasProviderPersistence ? (
          <p>Your sandbox keeps its files and installed software on its saved home.</p>
        ) : null}
        <SandboxActivity
          client={client}
          sandboxId={sandbox.id}
          settled={['paused', 'retired', 'failed'].includes(sandbox.status)}
        />
      </section>
    );
  }
  const phases: readonly string[] =
    operation.purpose === 'pause'
      ? operation.phase === 'resume_writers'
        ? ['resume_writers']
        : pausePhases
      : operation.purpose === 'revive'
        ? restorePhases
        : [...pausePhases.slice(0, -1), ...restorePhases];
  const current = phases.indexOf(operation.phase);
  const stage = lifecycleStageCopy[operation.phase];
  const elapsed = elapsedDuration(operation.startedAt, now);
  return (
    <section className="lifecycle-progress" aria-label="Sandbox progress">
      <p className="lifecycle-progress__stage" aria-live="polite">
        {parked ? sandboxLifecycleLabel(sandbox) : (stage?.label ?? copy.unknown)}
      </p>
      <p>{parked ? copy.parked : (stage?.detail ?? copy.unknownDetail)}</p>
      <div className="lifecycle-progress__readings">
        {elapsed === undefined || parked ? null : (
          <span>
            {copy.elapsed}: {elapsed}
          </span>
        )}
        {observedAt === undefined ? null : (
          <span>
            {copy.checked}:{' '}
            <time dateTime={observedAt}>{new Date(observedAt).toLocaleTimeString()}</time>
          </span>
        )}
      </div>
      <Presence>{stale ? <p role="status">{copy.disconnected}</p> : null}</Presence>
      {parked ? null : <p>{copy.background}</p>}
      <SandboxActivity client={client} sandboxId={sandbox.id} parked={parked} />
      <details>
        <summary>{copy.activity}</summary>
        {elapsed === undefined ? null : (
          <p>
            {copy.started}:{' '}
            <time dateTime={operation.startedAt}>
              {new Date(operation.startedAt).toLocaleString()}
            </time>
          </p>
        )}
        {current < 0 ? null : (
          <ol>
            {phases.map((phase, index) => (
              <li key={phase} aria-current={index === current ? 'step' : undefined}>
                <span>{lifecycleStageCopy[phase]?.label ?? copy.unknown}</span>
                <span>
                  {index < current
                    ? 'Done'
                    : index === current
                      ? stale || parked
                        ? 'Last confirmed stage'
                        : 'In progress'
                      : 'Waiting'}
                </span>
              </li>
            ))}
          </ol>
        )}
      </details>
    </section>
  );
}
