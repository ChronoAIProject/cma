import { useEffect, useState } from 'react';
import { ApiError } from '@chronoai/cma-sdk';
import type { ResourceViewsClient } from './client';
import { Button } from '../ui/Button';
import {
  clearAgentShareToken,
  readAgentShareToken,
  retainAgentShareToken,
} from './agentShareToken';

export function AgentShareAccept({
  client,
  shareId,
  onAccepted,
}: {
  readonly client: ResourceViewsClient;
  readonly shareId: string;
  readonly onAccepted: (agentId: string) => void;
}) {
  const [token] = useState(() => readAgentShareToken(shareId));
  const [retry, setRetry] = useState(0);
  const [error, setError] = useState<string>();
  const [terminal, setTerminal] = useState(false);
  useEffect(() => {
    if (!token) return;
    const request = new AbortController();
    retainAgentShareToken(shareId);
    void client.resourceAccess
      .redeemAgentShareLink(shareId, token, { signal: request.signal })
      .then((link) => {
        if (request.signal.aborted) return;
        if (link.id !== shareId || link.revoked_at !== null)
          throw new Error('This share link is no longer available.');
        clearAgentShareToken(shareId);
        onAccepted(link.agent_id);
      })
      .catch((cause: unknown) => {
        if (request.signal.aborted) return;
        const refused =
          cause instanceof ApiError && [400, 401, 403, 404, 410, 422].includes(cause.status);
        if (refused) clearAgentShareToken(shareId);
        setTerminal(refused);
        setError(
          refused
            ? 'This link is unavailable, was revoked, or is not shared with your account.'
            : 'The share link could not be accepted. Try again.',
        );
      });
    return () => request.abort();
  }, [client, shareId, token, retry, onAccepted]);
  return (
    <section className="console__empty" aria-label="Open shared Agent">
      <h1>{error || !token ? 'Shared Agent unavailable' : 'Opening shared Agent'}</h1>
      {error || !token ? (
        <p role="alert">{error ?? 'Open the complete share link supplied by the Agent owner.'}</p>
      ) : (
        <p role="status">Checking your access…</p>
      )}
      {error && !terminal ? (
        <Button
          onClick={() => {
            setError(undefined);
            setRetry((value) => value + 1);
          }}
        >
          Try again
        </Button>
      ) : null}
    </section>
  );
}
