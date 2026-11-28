import { useEffect, useState } from 'react';
import type { CmaWebClient } from './api/client';
import { stopDeletionBlock } from './agent/stopDeletion';
import { provisioningDuration } from './create/agentProvisioning';
import { useProvisioningClock } from './create/useProvisioningClock';
import { observeDeletion, type DeletionObservation, type DeletionRequest } from './deletion';
import { transitionRefreshDelay } from './sandbox/lifecycle';
import { Button } from './ui/Button';
import { StateDot } from './ui/StateDot';
import './DeletionNotice.css';

export function AgentDeletionNotice({
  client,
  request,
  disabled,
  onViewStatus,
}: {
  readonly client: CmaWebClient;
  readonly request: DeletionRequest;
  readonly disabled: boolean;
  readonly onViewStatus: () => void;
}) {
  const [started] = useState(() => new Date().toISOString());
  const [read, setRead] = useState<{ observation?: DeletionObservation; interrupted?: boolean }>();
  const settled = Boolean(read?.observation?.complete || read?.observation?.agentDeletionFailure);
  const now = useProvisioningClock(!settled);
  useEffect(() => {
    const controller = new AbortController();
    const startedAt = Date.now();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let agentEvents: DeletionObservation['agentEvents'];
    let reading = false;
    let done = false;
    const poll = async () => {
      if (reading || done || controller.signal.aborted || document.hidden) return;
      clearTimeout(timer);
      reading = true;
      try {
        const observation = await observeDeletion(
          client,
          request.action,
          controller.signal,
          true,
          [],
          agentEvents,
        );
        if (controller.signal.aborted) return;
        if (stopDeletionBlock(request, observation)) throw new Error('Deletion target changed');
        agentEvents = observation.agentEvents;
        done = Boolean(observation.complete || observation.agentDeletionFailure);
        setRead({ observation });
      } catch {
        if (controller.signal.aborted) return;
        setRead({ interrupted: true });
      } finally {
        reading = false;
        if (!controller.signal.aborted && !done)
          timer = setTimeout(poll, transitionRefreshDelay(startedAt));
      }
    };
    document.addEventListener('visibilitychange', poll);
    void poll();
    return () => {
      controller.abort();
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', poll);
    };
  }, [client, request]);

  if (read?.observation?.complete) return null;
  const failure = read?.observation?.agentDeletionFailure;
  const acceptedAt = provisioningDuration(request.acceptedAt, now);
  const elapsed = acceptedAt ?? provisioningDuration(started, now);
  return (
    <section className="deletion-summary" aria-label="Agent deletion">
      <div>
        {failure ? (
          <p role="alert">
            <strong>Agent deletion failed.</strong> {failure.detail}
          </p>
        ) : (
          <>
            <p role="status" className="deletion-summary__status">
              <StateDot state={read?.interrupted ? 'warning' : 'ongoing'} />
              {read?.interrupted ? 'Checking Agent deletion status' : 'Deleting Agent'}
            </p>
            {elapsed ? (
              <p>
                {acceptedAt ? 'Elapsed' : 'Watching for'} {elapsed}
              </p>
            ) : null}
          </>
        )}
      </div>
      <Button size="sm" onClick={onViewStatus} disabled={disabled}>
        View status
      </Button>
    </section>
  );
}
