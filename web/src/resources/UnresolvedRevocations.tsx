import { useEffect, useMemo, useState } from 'react';
import type { ResourceViewsClient } from './client';
import { Button } from '../ui/Button';
import {
  readRevocationEvidence,
  subscribeRevocationEvidence,
  type RevocationEvidence,
  type RevocationScope,
} from './revocationEvidence';
import { observeRevocationEffects } from './revocationObservation';

export function UnresolvedRevocations({
  client,
  owner,
  kind,
  id,
}: {
  readonly client: ResourceViewsClient;
  readonly owner: string | undefined;
  readonly kind: RevocationScope['kind'];
  readonly id: string;
}) {
  return owner ? (
    <EvidenceList
      key={JSON.stringify([owner, kind, id])}
      client={client}
      scope={{ owner, kind, id }}
    />
  ) : null;
}

function EvidenceList({
  client,
  scope,
}: {
  readonly client: ResourceViewsClient;
  readonly scope: RevocationScope;
}) {
  const [records, setRecords] = useState<readonly RevocationEvidence[]>([]);
  const [error, setError] = useState(false);
  const { owner, kind, id } = scope;
  useEffect(() => {
    const load = () => {
      try {
        setRecords(readRevocationEvidence({ owner, kind, id }));
        setError(false);
      } catch {
        setError(true);
      }
    };
    const unsubscribe = subscribeRevocationEvidence(load);
    load();
    return unsubscribe;
  }, [owner, kind, id]);
  if (!records.length && !error) return null;
  return (
    <section aria-label="Unresolved external requests">
      <h3>Earlier access changes</h3>
      {error ? (
        <p role="alert">
          Saved external request status could not be read. Restore session storage to check it.
        </p>
      ) : null}
      {records.map((record) => (
        <EvidenceRow key={record.operation} client={client} scope={scope} record={record} />
      ))}
    </section>
  );
}

function EvidenceRow({
  client,
  scope,
  record,
}: {
  readonly client: ResourceViewsClient;
  readonly scope: RevocationScope;
  readonly record: RevocationEvidence;
}) {
  const { owner, kind, id } = scope;
  const { operation, shareId, revision, organization, mode, revoked } = record;
  const evidence = useMemo(
    () => ({ operation, shareId, revision, organization, mode, revoked }),
    [operation, shareId, revision, organization, mode, revoked],
  );
  const [retry, setRetry] = useState(0);
  const context = useMemo(
    () => ({ client, owner, kind, id, evidence, retry }),
    [client, owner, kind, id, evidence, retry],
  );
  const [result, setResult] = useState<{ context: typeof context; error: boolean }>();
  const busy = result?.context !== context;
  const error = !busy && result.error;
  useEffect(() => {
    const request = new AbortController();
    let retired = false;
    const timer = setTimeout(() => request.abort(), 15_000);
    void observeRevocationEffects(context.client, context, context.evidence, request.signal)
      .then(() => {
        if (!retired) setResult({ context, error: false });
      })
      .catch(() => {
        if (!retired) setResult({ context, error: true });
      })
      .finally(() => {
        clearTimeout(timer);
      });
    return () => {
      retired = true;
      request.abort();
      clearTimeout(timer);
    };
  }, [context]);
  return (
    <div>
      <p role="status">
        {revoked ? 'Access revoked.' : 'Access change confirmed.'} Previously submitted external
        requests may still complete.
      </p>
      {error ? (
        <p role="alert">
          The latest external request status could not be checked. The previous warning is retained.
        </p>
      ) : null}
      <Button disabled={busy} onClick={() => setRetry((value) => value + 1)}>
        Check external requests
      </Button>
    </div>
  );
}
