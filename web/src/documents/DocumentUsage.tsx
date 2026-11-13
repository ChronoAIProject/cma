import { useEffect, useMemo, useState } from 'react';
import type { DocumentsClient, DocumentUsage as Usage } from '../api/documents';
import { documentBytes, documentQuantity } from '../vocabulary/documentUsage';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';

export function DocumentUsage({
  client,
  active = true,
  refresh = 0,
}: {
  readonly client: Pick<DocumentsClient, 'documentUsage'>;
  readonly active?: boolean;
  readonly refresh?: number;
}) {
  const [retry, setRetry] = useState(0);
  const context = useMemo(
    () => ({ client, active, refresh, retry }),
    [client, active, refresh, retry],
  );
  const [state, setState] = useState<{
    context: typeof context;
    usage?: Usage;
    failed?: boolean;
  }>();
  useEffect(() => {
    if (!context.active) return;
    const request = new AbortController();
    void context.client.documentUsage({ signal: request.signal }).then(
      (value) => {
        if (!request.signal.aborted) setState({ context, usage: value });
      },
      () => {
        if (!request.signal.aborted) setState({ context, failed: true });
      },
    );
    return () => request.abort();
  }, [context]);
  const usage = state?.context === context ? state.usage : undefined;
  const failed = state?.context === context && state.failed;
  if (failed)
    return (
      <Alert
        tone="warning"
        actions={
          <Button size="sm" variant="outline" onClick={() => setRetry((value) => value + 1)}>
            Retry usage
          </Button>
        }
      >
        Document usage could not be loaded.
      </Alert>
    );
  if (!usage) return <p role="status">Loading document usage…</p>;
  const enforced = usage.enforcement === 'enforced';
  const values = [
    usage.documents &&
      `${documentQuantity(usage.documents.used)} / ${documentQuantity(usage.documents.limit)} documents`,
    usage.bytes && `${documentBytes(usage.bytes.used)} / ${documentBytes(usage.bytes.limit)}`,
  ].filter(Boolean);
  return (
    <>
      <p>
        {!enforced
          ? 'Document quotas are not enforced.'
          : values.length
            ? values.join(' · ')
            : 'Document usage is unavailable.'}
      </p>
      {usage.pending_deletions !== null && usage.pending_deletions > 0 ? (
        <p>
          {enforced
            ? 'Storage is being released. Deleted documents still count toward your limits until cleanup finishes.'
            : 'Storage is being released. Cleanup of deleted documents is still pending.'}
        </p>
      ) : null}
    </>
  );
}
