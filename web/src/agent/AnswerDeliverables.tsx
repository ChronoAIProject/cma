import { useContext, useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { AnswerDeliverable } from '@chronoai/cma-sdk';
import { ApiError, DeliverableUnavailable } from '@chronoai/cma-sdk';
import { AnswerDeliveryContext, type AnswerDeliveryAccess } from './answerDeliveries';
import { Button } from '../ui/Button';
import { IconDownloadOutline16 } from '../ui/icons';
import './AnswerDeliverables.css';
import { DeliverablePreviewContext } from './previewSelection';
import { saveDelivery } from './saveDelivery';

export const statusText: Record<AnswerDeliverable['status'], string> = {
  ready: 'Ready to download',
  unchecked: 'Verify on download',
  changed: 'File changed since this answer',
  version_changed: 'Document has a newer revision; download keeps this revision',
  missing: 'File deleted',
  denied: 'Access unavailable',
  not_ready: 'Not ready',
  too_large: 'Exceeds the resource limit',
  unavailable: 'Download unavailable; retry',
};

export function downloadFailureStatus(error: unknown): AnswerDeliverable['status'] | undefined {
  if (error instanceof DeliverableUnavailable) return error.status;
  if (!(error instanceof ApiError)) return;
  const statuses: Readonly<Record<string, AnswerDeliverable['status']>> = {
    deliverable_changed: 'changed',
    file_conflict: 'changed',
    file_not_found: 'missing',
    file_too_large: 'too_large',
    file_runtime_not_ready: 'not_ready',
    file_runtime_unsupported: 'unavailable',
    file_unavailable: 'unavailable',
    deliverable_unavailable: 'unavailable',
  };
  return (
    statuses[error.code ?? ''] ?? ([401, 403, 404].includes(error.status) ? 'denied' : undefined)
  );
}

function preserveFollowingMessage(element: HTMLElement | null) {
  const scroller = element?.closest('.agent-surface__conversation');
  if (
    !(scroller instanceof HTMLElement) ||
    scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight <= 48
  )
    return;
  const viewport = scroller.getBoundingClientRect();
  let following = element?.closest('article')?.nextElementSibling;
  for (let count = 0; following && count < 32; count++, following = following.nextElementSibling) {
    const top = following.getBoundingClientRect().top;
    if (top >= viewport.bottom) return;
    if (top < viewport.top) continue;
    const anchor = following;
    const scrollTop = scroller.scrollTop;
    return () => {
      if (anchor.isConnected && Math.abs(scroller.scrollTop - scrollTop) <= 1)
        scroller.scrollTop += anchor.getBoundingClientRect().top - top;
    };
  }
}

function DeliveryRow({
  access,
  answerId,
  initial,
}: {
  readonly access: AnswerDeliveryAccess;
  readonly answerId: string;
  readonly initial: AnswerDeliverable;
}) {
  const preview = useContext(DeliverablePreviewContext);
  const [item, setItem] = useState(initial);
  const [busy, setBusy] = useState<'download' | 'retry'>();
  const [error, setError] = useState<string>();
  const pending = useRef<AbortController | undefined>(undefined);
  useEffect(() => () => pending.current?.abort(), [access, answerId]);
  async function download() {
    if (pending.current) return;
    const abort = new AbortController();
    pending.current = abort;
    setBusy('download');
    setError(undefined);
    try {
      const bytes = await access.client.downloadAnswerDeliverable(access.agentId, answerId, item, {
        signal: abort.signal,
      });
      if (abort.signal.aborted) return;
      saveDelivery(bytes, item.reference.name);
    } catch (error) {
      if (!abort.signal.aborted) {
        const status = downloadFailureStatus(error);
        if (status) setItem({ ...item, status });
        else setError('Download failed; retry');
      }
    } finally {
      if (!abort.signal.aborted) {
        pending.current = undefined;
        setBusy(undefined);
      }
    }
  }
  async function retry() {
    if (pending.current) return;
    const abort = new AbortController();
    pending.current = abort;
    setBusy('retry');
    setError(undefined);
    try {
      const result = await access.client.readAnswerDeliverable(access.agentId, answerId, item.id, {
        signal: abort.signal,
        verify: true,
      });
      if (!abort.signal.aborted) setItem(result.item);
    } catch {
      if (!abort.signal.aborted) setError('Verification failed; retry');
    } finally {
      if (!abort.signal.aborted) {
        pending.current = undefined;
        setBusy(undefined);
      }
    }
  }
  return (
    <li className="answer-deliveries__row">
      <span className="answer-deliveries__type" aria-hidden="true">
        {item.reference.name.split('.').at(-1)?.slice(0, 5).toUpperCase() || 'FILE'}
      </span>
      <div className="answer-deliveries__detail">
        {preview ? (
          <button
            type="button"
            className="answer-deliveries__name"
            aria-label={`Preview ${item.reference.name}`}
            aria-pressed={
              preview.selected?.access === access &&
              preview.selected.answerId === answerId &&
              preview.selected.item.id === item.id
            }
            onClick={(event) =>
              preview.open({ access, answerId, item, trigger: event.currentTarget })
            }
          >
            {item.reference.name}
          </button>
        ) : (
          <span className="answer-deliveries__name">{item.reference.name}</span>
        )}
        <span className="answer-deliveries__metadata">
          {item.reference.resource.kind === 'live_file'
            ? 'Live file'
            : item.reference.resource.kind === 'agent_artifact'
              ? 'Published artifact'
              : `Document revision ${item.reference.resource.revision}`}{' '}
          · {item.reference.size_bytes.toLocaleString()} bytes
        </span>
        <span className="answer-deliveries__status" role="status">
          {error ??
            (busy === 'download'
              ? 'Downloading…'
              : busy === 'retry'
                ? 'Verifying…'
                : statusText[item.status])}
        </span>
      </div>
      <div className="answer-deliveries__actions">
        <Button
          variant="outline"
          size="sm"
          aria-label={`Download ${item.reference.name}`}
          icon={<IconDownloadOutline16 />}
          disabled={
            busy !== undefined || !['ready', 'unchecked', 'version_changed'].includes(item.status)
          }
          onClick={() => void download()}
        />
        {item.status !== 'ready' || error ? (
          <Button
            size="sm"
            aria-label={`Retry ${item.reference.name}`}
            disabled={busy !== undefined}
            onClick={() => void retry()}
          >
            Retry
          </Button>
        ) : null}
      </div>
    </li>
  );
}

export function AnswerDeliverables({
  answerId,
  references,
}: {
  readonly answerId: string;
  readonly references: number;
}) {
  const access = useContext(AnswerDeliveryContext);
  const invalidate = useContext(DeliverablePreviewContext)?.invalidate;
  useEffect(
    () => () => {
      if (access) invalidate?.(access);
    },
    [access, invalidate],
  );
  const container = useRef<HTMLDivElement>(null);
  const restorePosition = useRef<(() => void) | undefined>(undefined);
  const [visible, setVisible] = useState(() => typeof IntersectionObserver === 'undefined');
  const [loaded, setLoaded] = useState<{
    access: AnswerDeliveryAccess;
    answerId: string;
    attempt: number;
    items: readonly AnswerDeliverable[];
  }>();
  const [failed, setFailed] = useState<AnswerDeliveryAccess>();
  const [attempt, setAttempt] = useState(0);
  useLayoutEffect(() => {
    restorePosition.current?.();
    restorePosition.current = undefined;
  }, [loaded]);
  useEffect(() => {
    if (!access || !references || typeof IntersectionObserver === 'undefined') return;
    const observer = new IntersectionObserver((entries) => {
      setVisible(entries.some((entry) => entry.isIntersecting));
    });
    if (container.current) observer.observe(container.current);
    return () => observer.disconnect();
  }, [access, references]);
  useEffect(() => {
    if (
      !visible ||
      !access ||
      !references ||
      (loaded?.access === access && loaded.answerId === answerId && loaded.attempt === attempt)
    )
      return;
    const abort = new AbortController();
    void access.client
      .answerDeliverables(access.agentId, answerId, { signal: abort.signal })
      .then((result) => {
        if (!abort.signal.aborted) {
          restorePosition.current = preserveFollowingMessage(container.current);
          setLoaded({ access, answerId, attempt, items: result.items });
        }
      })
      .catch(() => {
        if (!abort.signal.aborted) setFailed(access);
      });
    return () => abort.abort();
  }, [visible, access, answerId, references, attempt, loaded]);
  if (!access || !references) return null;
  const items =
    loaded?.access === access && loaded.answerId === answerId ? loaded.items : undefined;
  return (
    <div ref={container} className="answer-deliveries">
      {items?.length ? <h3 className="answer-deliveries__heading">Files from this task</h3> : null}
      {items ? (
        items.length ? (
          <ul aria-label="Answer deliverables" className="answer-deliveries__list">
            {items.map((item) => (
              <DeliveryRow key={item.id} access={access} answerId={answerId} initial={item} />
            ))}
          </ul>
        ) : null
      ) : (
        <div className="answer-deliveries__pending" role="status">
          {failed === access ? (
            <>
              Deliveries unavailable.{' '}
              <Button
                size="sm"
                onClick={() => {
                  setFailed(undefined);
                  setAttempt((value) => value + 1);
                }}
              >
                Retry deliveries
              </Button>
            </>
          ) : (
            'Checking answer deliveries…'
          )}
        </div>
      )}
    </div>
  );
}
