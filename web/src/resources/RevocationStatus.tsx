import { useEffect, useState } from 'react';
import type { ResourceRevocationStatus } from '@cma/client';
import type { ResourceViewsClient } from './client';
import type { RevocationIntent } from './revocationIntent';
import { Button } from '../ui/Button';

export function RevocationStatus({
  client,
  agentId,
  intent,
  onEffective,
}: {
  readonly client: ResourceViewsClient;
  readonly agentId: string;
  readonly intent: RevocationIntent;
  readonly onEffective: (shareId: string, unresolved: number) => void;
}) {
  const [status, setStatus] = useState<ResourceRevocationStatus>();
  const [error, setError] = useState<string>();
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    if (!intent.operation || !client.resourceRevocations) return;
    const request = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      try {
        const value = await client.resourceRevocations!.status(
          intent.operation!,
          'agent',
          agentId,
          { signal: request.signal },
        );
        if (request.signal.aborted) return;
        if (
          value.share_id !== intent.shareId ||
          value.expected_revision !== intent.revision ||
          (value.state === 'effective' && !value.receipt?.share.revoked_at)
        )
          throw new Error('The revocation result did not match this share change.');
        setStatus(value);
        setError(undefined);
        if (value.state === 'effective') onEffective(intent.shareId, value.unresolved_effects);
        else timer = setTimeout(() => void poll(), 2000);
      } catch {
        if (!request.signal.aborted)
          setError('Revocation could not be confirmed. Your original request is saved.');
      }
    };
    void poll();
    return () => {
      request.abort();
      clearTimeout(timer);
    };
  }, [client, agentId, intent.operation, intent.revision, intent.shareId, onEffective, retry]);
  return (
    <div aria-label="Revocation status">
      <p role="status">
        {status?.state === 'effective'
          ? 'Access revoked.'
          : status?.deadline_exceeded
            ? 'Revocation is taking longer than expected. Access removal has not been confirmed.'
            : status?.state === 'evidence_received'
              ? 'Work has stopped. Confirming access removal…'
              : 'Revocation pending. Waiting for in-flight work to stop.'}
      </p>
      {status && status.unresolved_effects > 0 ? (
        <p>Previously submitted external requests may still complete.</p>
      ) : null}
      {error ? (
        <p role="alert">
          {error} <Button onClick={() => setRetry((n) => n + 1)}>Check revocation</Button>
        </p>
      ) : null}
    </div>
  );
}
