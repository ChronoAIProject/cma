import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { ApiError, Cma, TimeoutError } from '@cma/client';
import type { PrivacyExportInventory } from '../api-client/privacy-export';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';

const client = new Cma().privacyExports;

export function PrivacyExportDialog({
  owner,
  onClose,
}: {
  readonly owner: string;
  readonly onClose: () => void;
}) {
  const [inventoryRequest, setInventoryRequest] = useState<{ cursor?: string }>({});
  const [page, setPage] = useState<PrivacyExportInventory>();
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string>();
  const [selected, setSelected] = useState('');
  const download = useRef<AbortController | undefined>(undefined);
  const [downloading, setDownloading] = useState(false);

  useEffect(() => {
    const { cursor } = inventoryRequest;
    const request = new AbortController();
    void client.inventory(owner, cursor, { signal: request.signal }).then(
      (result) => {
        if (request.signal.aborted) return;
        setPage(result);
        setSelected(result.agents[0]?.id ?? '');
        setLoading(false);
      },
      () => {
        if (request.signal.aborted) return;
        setError(
          cursor === undefined
            ? 'The Agent list could not be loaded. Reopen account export to try again.'
            : 'The next page could not be loaded. Select More Agents to try again.',
        );
        setLoading(false);
      },
    );
    return () => request.abort();
  }, [owner, inventoryRequest]);

  useLayoutEffect(
    () => () => {
      const operation = download.current;
      download.current = undefined;
      operation?.abort();
    },
    [owner],
  );

  const startDownload = async () => {
    if (!selected || download.current) return;
    const operation = new AbortController();
    const deadline = window.setTimeout(() => operation.abort(), 120_000);
    download.current = operation;
    setDownloading(true);
    setError(undefined);
    try {
      const blob = await client.retainedConversation(owner, selected, operation.signal);
      operation.signal.throwIfAborted();
      saveDownload(blob);
    } catch (error) {
      if (!operation.signal.aborted) {
        setError(downloadError(error));
      } else if (download.current === operation) {
        setError('The download timed out.');
      }
    } finally {
      window.clearTimeout(deadline);
      if (download.current === operation) {
        download.current = undefined;
        setDownloading(false);
      }
    }
  };

  return (
    <Modal
      open
      title="Export account data"
      description="Download recorded messages and turn outcomes from an Agent you own. Messages may include text shared with other participants."
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Close</Button>
          <Button
            variant="primary"
            disabled={loading || downloading || selected === ''}
            onClick={() => void startDownload()}
          >
            {downloading ? 'Preparing download…' : 'Download conversation'}
          </Button>
        </>
      }
    >
      <p>
        Workspace and saved-document exports are handled separately. This download covers only
        retained Agent conversations.
      </p>
      {loading && <p role="status">Loading your Agents…</p>}
      {!loading && page?.agents.length === 0 && <p>No Agents to export.</p>}
      {!loading && page !== undefined && page.agents.length > 0 && (
        <label>
          Agent
          <select
            value={selected}
            disabled={downloading}
            onChange={(event) => setSelected(event.target.value)}
          >
            {page.agents.map(({ id, title }) => (
              <option key={id} value={id}>
                {title ?? id}
              </option>
            ))}
          </select>
        </label>
      )}
      {!loading && page?.next != null && (
        <Button
          disabled={downloading}
          onClick={() => {
            setLoading(true);
            setError(undefined);
            // Each attempt must run, even when it retries the same continuation.
            setInventoryRequest({ cursor: page.next ?? undefined });
          }}
        >
          More Agents
        </Button>
      )}
      {error !== undefined && <p role="alert">{error}</p>}
    </Modal>
  );
}

function downloadError(error: unknown): string {
  if (error instanceof TimeoutError) return 'The download timed out. Try again.';
  if (!(error instanceof ApiError)) {
    return 'The conversation could not be verified or downloaded. Try again.';
  }
  const code = error.code && /^[a-z0-9_]{1,80}$/.test(error.code) ? error.code : undefined;
  const reference = code ? `HTTP ${error.status}, ${code}` : `HTTP ${error.status}`;
  if (code === 'account_deletion_in_progress') {
    return `Account deletion is in progress. Conversation export is unavailable (${reference}).`;
  }
  if (error.status === 401 || code === 'privacy_export_owner_session_required') {
    return `Sign in again as the Agent owner, then retry (${reference}).`;
  }
  if (error.status === 403 || error.status === 404) {
    return `Reopen account export and select an Agent you own (${reference}).`;
  }
  if (error.status === 409) {
    return `The conversation changed during export. Wait for the turn to finish, then retry (${reference}).`;
  }
  return `CMA could not prepare the conversation. Try again later (${reference}).`;
}

function saveDownload(blob: Blob) {
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  try {
    link.href = url;
    link.download = 'cma-retained-conversation.json';
    document.body.append(link);
    link.click();
  } finally {
    link.remove();
    window.setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
}
