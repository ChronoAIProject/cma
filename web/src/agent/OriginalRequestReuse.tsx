import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { ApiError, raceWithAbortSignal } from '@cma/client';
import type { CmaWebClient } from '../api/client';
import { prepareHomeRequestImport, type HomeRequestImport } from '../create/taskDraftImport';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import type { RetiredAgentIdentity } from './retiredPresentation';
import { rateLimitDelay } from './transport/readRetry';

type Result =
  | { readonly kind: 'idle' | 'loading' | 'unavailable' }
  | { readonly kind: 'error'; readonly message: string; readonly retryAt: number }
  | { readonly kind: 'offer'; readonly text: string; readonly offer: HomeRequestImport }
  | { readonly kind: 'save-error'; readonly text: string; readonly message: string };

/** An explicit read offers exact Task text to Home; no creation or send capability is used. */
export function OriginalRequestReuse({
  client,
  identity,
  onImported,
}: {
  readonly client: CmaWebClient;
  readonly identity: RetiredAgentIdentity;
  readonly onImported: () => void;
}) {
  const identityKey = JSON.stringify(identity);
  const scope = useMemo(() => ({ client, identityKey }), [client, identityKey]);
  const active = useRef<typeof scope | undefined>(undefined);
  const request = useRef<AbortController | undefined>(undefined);
  const retry = useRef<{ scope: typeof scope; at: number } | undefined>(undefined);
  const [state, setState] = useState<{ scope: typeof scope; value: Result }>();
  useLayoutEffect(() => {
    active.current = scope;
    return () => {
      active.current = undefined;
      request.current?.abort();
      request.current = undefined;
    };
  }, [scope]);
  const current = () => active.current === scope;
  const publish = (value: Result) => {
    if (current()) setState({ scope, value });
  };
  const commit = (offer: HomeRequestImport, text: string, replace: boolean) => {
    if (!current()) return;
    try {
      if (offer.commit(replace) && current()) {
        publish({ kind: 'idle' });
        onImported();
      }
    } catch (error) {
      publish({
        kind: 'save-error',
        text,
        message:
          error instanceof Error
            ? error.message
            : 'The request could not be saved. Copy it before leaving this page.',
      });
    }
  };
  const offerText = (text: string) => {
    if (!current()) return;
    try {
      const offer = prepareHomeRequestImport(identity.viewerId, text, current);
      if (offer.conflict) publish({ kind: 'offer', text, offer });
      else commit(offer, text, false);
    } catch (error) {
      publish({
        kind: 'save-error',
        text,
        message:
          error instanceof Error
            ? error.message
            : 'The request could not be saved. Copy it before leaving this page.',
      });
    }
  };
  const read = async () => {
    if (
      !current() ||
      request.current ||
      (retry.current?.scope === scope && Date.now() < retry.current.at)
    )
      return;
    const source = client.readAgentOriginalRequest;
    if (!source) {
      publish({ kind: 'unavailable' });
      return;
    }
    const operation = new AbortController();
    request.current = operation;
    publish({ kind: 'loading' });
    let timedOut = false;
    const timeout = window.setTimeout(() => {
      timedOut = true;
      operation.abort();
    }, 120_000);
    try {
      const result = await raceWithAbortSignal(operation.signal, () =>
        source(identity.agentId, { signal: operation.signal }),
      );
      if (!current() || operation.signal.aborted) return;
      if (result.agentId !== identity.agentId)
        throw new Error('The original request did not match this Agent.');
      if (result.kind === 'unavailable') {
        publish({ kind: 'unavailable' });
        return;
      }
      if (result.source !== 'admitted-first-task') throw new Error('Original request unavailable');
      offerText(result.text);
    } catch (error) {
      if (!current() || (operation.signal.aborted && !timedOut)) return;
      if (error instanceof ApiError && error.status === 404) publish({ kind: 'unavailable' });
      else {
        const retryAt = Date.now() + rateLimitDelay(error, 0);
        retry.current = { scope, at: retryAt };
        publish({
          kind: 'error',
          retryAt,
          message: timedOut
            ? 'The original request read timed out. Try again.'
            : error instanceof Error
              ? error.message
              : 'The original request could not be loaded.',
        });
      }
    } finally {
      window.clearTimeout(timeout);
      if (request.current === operation) request.current = undefined;
    }
  };
  const cancel = () => {
    if (!current()) return;
    request.current?.abort();
    request.current = undefined;
    publish({ kind: 'idle' });
  };
  const value = state?.scope === scope ? state.value : { kind: 'idle' as const };
  const retryAt = value.kind === 'error' ? value.retryAt : 0;
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const timer = window.setTimeout(() => setNow(Date.now()), Math.max(0, retryAt - Date.now()));
    return () => window.clearTimeout(timer);
  }, [retryAt]);
  return (
    <>
      <Button
        variant="primary"
        disabled={value.kind === 'loading' || value.kind === 'offer' || now < retryAt}
        onClick={() => void read()}
      >
        {value.kind === 'loading'
          ? 'Loading original request…'
          : 'Start a new agent with this request'}
      </Button>
      {value.kind === 'loading' ? <Button onClick={cancel}>Cancel</Button> : null}
      {value.kind === 'unavailable' ? <p role="status">Original request unavailable</p> : null}
      {value.kind === 'error' ? <p role="alert">{value.message}</p> : null}
      {value.kind === 'save-error' ? (
        <div className="retained-agent__request">
          <p role="alert">{value.message}</p>
          <pre>{value.text}</pre>
          <Button onClick={() => offerText(value.text)}>Try saving request again</Button>
          <CopyRequest text={value.text} />
        </div>
      ) : null}
      <Modal
        open={value.kind === 'offer'}
        title="Use this request in Home?"
        description="Your current task draft will remain available in Saved drafts. Starting an Agent still requires your confirmation in Home."
        onClose={() => publish({ kind: 'idle' })}
        footer={
          value.kind === 'offer' ? (
            <>
              <Button onClick={() => publish({ kind: 'idle' })}>Keep current draft</Button>
              <Button variant="primary" onClick={() => commit(value.offer, value.text, true)}>
                Use this request
              </Button>
            </>
          ) : undefined
        }
      >
        {value.kind === 'offer' ? (
          <pre className="retained-agent__request-text">{value.text}</pre>
        ) : null}
      </Modal>
    </>
  );
}

function CopyRequest({ text }: { readonly text: string }) {
  const [message, setMessage] = useState<string>();
  return (
    <>
      <Button
        onClick={() => {
          if (!navigator.clipboard) {
            setMessage('Copy failed. Select the request text to copy it.');
            return;
          }
          void navigator.clipboard.writeText(text).then(
            () => setMessage('Copied'),
            () => setMessage('Copy failed. Select the request text to copy it.'),
          );
        }}
      >
        Copy request
      </Button>
      {message ? <p role="status">{message}</p> : null}
    </>
  );
}
