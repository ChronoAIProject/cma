import { useEffect, useState } from 'react';
import { ApiError } from '@cma/client';
import { useCmaClient } from '../api/context';
import type { SandboxDetail } from '../api/types';
import { sandboxProgressCopy as copy } from '../vocabulary/sandboxProgress';
import { LifecycleProgress } from './LifecycleProgress';

export function LaunchSandboxProgress({ sandboxId }: { readonly sandboxId: string }) {
  const client = useCmaClient();
  const [observation, setObservation] = useState<{
    sandbox: SandboxDetail;
    observedAt: string;
  }>();
  const [error, setError] = useState(false);
  useEffect(() => {
    const abort = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let inFlight = false;
    const poll = async () => {
      if (abort.signal.aborted || document.hidden || inFlight) return;
      clearTimeout(timer);
      inFlight = true;
      let delay = 1500;
      try {
        const sandbox = await client.readSandbox(sandboxId, { signal: abort.signal });
        if (abort.signal.aborted) return;
        if (sandbox.id !== sandboxId) {
          setObservation(undefined);
          throw new Error('Sandbox identity mismatch');
        }
        setObservation({ sandbox, observedAt: new Date().toISOString() });
        setError(false);
      } catch (cause) {
        if (abort.signal.aborted) return;
        if (cause instanceof ApiError && [401, 403, 404].includes(cause.status))
          setObservation(undefined);
        setError(true);
        delay = 5000;
      } finally {
        inFlight = false;
        if (!abort.signal.aborted) timer = setTimeout(() => void poll(), delay);
      }
    };
    const visible = () => void poll();
    document.addEventListener('visibilitychange', visible);
    void poll();
    return () => {
      abort.abort();
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', visible);
    };
  }, [client, sandboxId]);
  const current = observation?.sandbox.id === sandboxId ? observation : undefined;
  if (current === undefined) return <p role="status">{error ? copy.unavailable : copy.checking}</p>;
  return (
    <LifecycleProgress sandbox={current.sandbox} observedAt={current.observedAt} stale={error} />
  );
}
