import { useContext, useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { AnswerDeliverable } from '@chronoai/cma-sdk';
import { AnswerDeliveryContext, type AnswerDeliveryAccess } from './answerDeliveries';
import { Button } from '../ui/Button';
import { IconPaperclipOutline16 } from '../ui/icons';
import './AnswerDeliverables.css';

const statusText: Record<AnswerDeliverable['status'], string> = {
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
      const url = URL.createObjectURL(
        new Blob([new Uint8Array(bytes)], { type: 'application/octet-stream' }),
      );
      const link = document.createElement('a');
      link.href = url;
      link.download = item.reference.name;
      link.rel = 'noreferrer';
      document.body.append(link);
      link.click();
      link.remove();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    } catch {
      if (!abort.signal.aborted) setError('Download failed; retry');
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
      <IconPaperclipOutline16 aria-hidden="true" />
      <div className="answer-deliveries__detail">
        <span className="answer-deliveries__name">{item.reference.name}</span>
        <span className="answer-deliveries__metadata">
          {item.reference.resource.kind === 'live_file'
            ? 'Live file'
            : item.reference.resource.kind === 'agent_artifact'
              ? 'Published artifact'
              : `Document revision ${item.reference.resource.revision}`}{' '}
          · {item.reference.media_type} · {item.reference.size_bytes.toLocaleString()} bytes
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
          disabled={
            busy !== undefined || !['ready', 'unchecked', 'version_changed'].includes(item.status)
          }
          onClick={() => void download()}
        >
          Download
        </Button>
        <Button
          size="sm"
          aria-label={`Retry ${item.reference.name}`}
          disabled={busy !== undefined}
          onClick={() => void retry()}
        >
          Retry
        </Button>
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
