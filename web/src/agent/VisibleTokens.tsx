import { useEffect, useMemo, useRef, useState } from 'react';
import {
  newlyVisibleText,
  VISIBLE_TEXT_LIMIT,
  type VisibleText,
  type VisibleTokenEstimate,
} from './tokenEstimate';

/** Lazy worker, one request in flight, at most five tokenizer updates per second. */
export function VisibleTokens({
  parts,
  anchor,
  reported,
}: {
  readonly parts: readonly VisibleText[];
  readonly anchor?: readonly VisibleText[];
  readonly reported?: { readonly inputTokens: number; readonly outputTokens: number };
}) {
  // Loaded history may already have usage. Its visible text is an initial anchor;
  // subsequent durable reports carry the precise reducer-time text boundary.
  const [initial] = useState(() => (reported === undefined ? [] : parts));
  const pending = useMemo(
    () => (reported === undefined ? parts : newlyVisibleText(parts, anchor ?? initial)),
    [parts, anchor, initial, reported],
  );
  const basis =
    reported === undefined ? 'unreported' : `${reported.inputTokens}:${reported.outputTokens}`;
  const latest = useRef({ parts: pending, basis });
  const publish = useRef<(() => void) | undefined>(undefined);
  const [estimate, setEstimate] = useState<VisibleTokenEstimate & { readonly basis: string }>();
  const [unavailable, setUnavailable] = useState(() => typeof Worker === 'undefined');
  useEffect(() => {
    if (typeof Worker === 'undefined') return;
    let worker: Worker;
    try {
      worker = new Worker(new URL('./visibleTokens.worker.ts', import.meta.url), {
        type: 'module',
      });
    } catch {
      queueMicrotask(() => setUnavailable(true));
      return;
    }
    let timer: number | undefined;
    let inFlight = false;
    let changed = true;
    let sentAt = -Infinity;
    let disposed = false;
    const send = () => {
      timer = undefined;
      if (disposed || inFlight || !changed) return;
      changed = false;
      inFlight = true;
      sentAt = performance.now();
      let remaining = VISIBLE_TEXT_LIMIT;
      // Keep worker messages bounded too; one sentinel records omitted text.
      const bounded: VisibleText[] = [];
      for (const part of latest.current.parts) {
        if (remaining === 0) {
          if (part.text) bounded.push({ id: 'omitted', text: 'x' });
          break;
        }
        bounded.push({ id: part.id, text: part.text.slice(0, remaining + 1) });
        remaining = Math.max(0, remaining - part.text.length);
      }
      worker.postMessage({ parts: bounded, basis: latest.current.basis });
    };
    const schedule = () => {
      if (timer !== undefined || inFlight || disposed) return;
      const delay = Math.max(0, 200 - (performance.now() - sentAt));
      timer = window.setTimeout(send, delay);
    };
    publish.current = () => {
      changed = true;
      schedule();
    };
    worker.onmessage = (event: MessageEvent<VisibleTokenEstimate & { readonly basis: string }>) => {
      if (disposed) return;
      inFlight = false;
      setEstimate(event.data);
      if (changed) schedule();
    };
    worker.onerror = () => {
      if (disposed) return;
      setUnavailable(true);
      worker.terminate();
      publish.current = undefined;
      disposed = true;
      window.clearTimeout(timer);
    };
    schedule();
    return () => {
      disposed = true;
      publish.current = undefined;
      window.clearTimeout(timer);
      worker.terminate();
    };
  }, []);
  useEffect(() => {
    latest.current = { parts: pending, basis };
    publish.current?.();
  }, [pending, basis]);
  if (!pending.some((part) => part.text))
    return reported === undefined ? <>output unavailable</> : null;
  if (unavailable) return <>{reported === undefined ? '' : ' · '}output estimate unavailable</>;
  if (estimate === undefined || estimate.basis !== basis || estimate.tokens === 0)
    return reported === undefined ? <>estimating visible output…</> : null;
  return (
    <span
      aria-label={`Estimated visible output tokens: ${estimate.tokens.toLocaleString('en')}${estimate.partial ? ', partial' : ''}`}
      title="Approximate tokens in visible reply and reasoning text since the last usage report, counted with o200k_base. Excludes hidden reasoning, tool calls and inputs; model usage may differ. Never added to reported counts or session totals. Long turns may show a partial estimate."
    >
      {reported === undefined ? '≈ ' : ' + ≈ '}
      {estimate.tokens.toLocaleString('en')} {reported === undefined ? 'visible' : 'newly visible'}{' '}
      output tokens
      {estimate.partial ? ' (partial)' : ''}
    </span>
  );
}
