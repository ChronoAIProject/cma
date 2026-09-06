import { useEffect, useRef, useState } from 'react';
import {
  ApiError,
  type CreatedServiceConnection,
  type ServiceConnectionMetadata,
  type ServiceConnectionState,
} from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { connectionCopy, connectionStates } from '../vocabulary/connect';
import { refusalSentence } from '../vocabulary/errors';
import { publicCopy } from '../vocabulary/needs';
import type { PublicClient } from './client';
import {
  clearConnectionReturn,
  readConnectionReturn,
  rememberConnectionReturn,
} from './connectionReturn';

export function secureSetupUrl(raw: string): string | undefined {
  try {
    const url = new URL(raw);
    return url.protocol === 'https:' && url.username === '' && url.password === ''
      ? url.href
      : undefined;
  } catch {
    return undefined;
  }
}

export function ConnectAssistant({
  client,
  requestId,
  service,
  disabled = false,
  onSatisfied,
  onRecheck,
}: {
  readonly client: PublicClient;
  readonly requestId: string;
  readonly service: ServiceConnectionMetadata;
  readonly disabled?: boolean;
  readonly onSatisfied: (service: string) => void;
  readonly onRecheck: () => void;
}) {
  const [handoff, setHandoff] = useState<
    (Omit<CreatedServiceConnection, 'hosted_url'> & { readonly hosted_url?: string }) | undefined
  >(() => readConnectionReturn(requestId, service.service));
  const [state, setState] = useState<ServiceConnectionState | undefined>(
    handoff === undefined ? undefined : 'pending',
  );
  const [busy, setBusy] = useState(false);
  const [blocked, setBlocked] = useState(false);
  const [error, setError] = useState<string>();
  const [pollRevision, setPollRevision] = useState(0);
  const popup = useRef<Window | null>(null);
  const command = useRef<{ key: string; intent: string } | undefined>(undefined);
  const active = useRef(false);
  const live = useRef(true);
  const satisfied = useRef(onSatisfied);
  useEffect(() => {
    satisfied.current = onSatisfied;
  }, [onSatisfied]);
  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
      popup.current?.close();
    };
  }, []);

  useEffect(() => {
    if (handoff === undefined) return;
    const controller = new AbortController();
    const startedAt = Date.now();
    const deadline = setTimeout(() => {
      setError(connectionCopy.timeout);
      controller.abort();
    }, 300_000);
    let timer: ReturnType<typeof setTimeout> | undefined;
    let failures = 0;
    const poll = async () => {
      try {
        const status = await client.requests.readConnection(handoff.handoff_id, {
          signal: controller.signal,
        });
        if (controller.signal.aborted) return;
        if (status.launch_id !== requestId || status.service_id !== service.service)
          throw new Error();
        setState(status.state);
        setError(undefined);
        failures = 0;
        if (status.state === 'satisfied') {
          clearTimeout(deadline);
          clearConnectionReturn(requestId, service.service);
          popup.current?.close();
          satisfied.current(service.service);
          return;
        }
        if (status.state !== 'pending') {
          clearTimeout(deadline);
          clearConnectionReturn(requestId, service.service);
          popup.current?.close();
          command.current = undefined;
          return;
        }
      } catch (cause) {
        if (controller.signal.aborted) return;
        failures += 1;
        setError(refusalSentence(cause));
      }
      if (Date.now() - startedAt >= 300_000) {
        setError(connectionCopy.timeout);
        return;
      }
      timer = setTimeout(
        () => void poll(),
        Math.min(1500 * 2 ** failures, 15_000, 300_000 - (Date.now() - startedAt)),
      );
    };
    timer = setTimeout(() => void poll(), 1500);
    return () => {
      controller.abort();
      clearTimeout(timer);
      clearTimeout(deadline);
    };
  }, [client, handoff, requestId, service.service, pollRevision]);

  const connect = () => {
    if (active.current) return;
    popup.current = window.open('about:blank', '_blank', 'popup,width=520,height=720');
    if (popup.current !== null) popup.current.opener = null;
    setBlocked(popup.current === null);
    command.current ??= { key: crypto.randomUUID(), intent: crypto.randomUUID() };
    const selected = command.current;
    active.current = true;
    setBusy(true);
    setError(undefined);
    void client.requests
      .createConnection(
        { service_id: service.service, launch_id: requestId, intent_id: selected.intent },
        { idempotencyKey: selected.key },
      )
      .then(
        (result) => {
          if (!live.current) return;
          const url = secureSetupUrl(result.hosted_url);
          if (url === undefined) {
            popup.current?.close();
            setError(connectionCopy.invalidUrl);
            return;
          }
          setHandoff({ ...result, hosted_url: url });
          rememberConnectionReturn(requestId, service.service, result);
          setState('pending');
          if (popup.current !== null && !popup.current.closed) popup.current.location.href = url;
          else setBlocked(true);
        },
        (cause: unknown) => {
          popup.current?.close();
          if (!live.current) return;
          if (cause instanceof ApiError && !cause.isRetryable) command.current = undefined;
          setError(refusalSentence(cause));
        },
      )
      .finally(() => {
        active.current = false;
        if (live.current) setBusy(false);
      });
  };
  const name = service.display_name ?? connectionCopy.unknown;
  const manualUrl = service.setup_url === undefined ? undefined : secureSetupUrl(service.setup_url);
  const pending = state === 'pending';
  return (
    <section className="public-connection" aria-label={connectionCopy.title(name)}>
      <h3>{connectionCopy.title(name)}</h3>
      {service.connection_mode === 'manual' ? (
        <>
          <p>{connectionCopy.manual}</p>
          {manualUrl === undefined ? null : (
            <a href={manualUrl} target="_blank" rel="noopener noreferrer">
              {connectionCopy.open}
            </a>
          )}
          <Button disabled={disabled} onClick={onRecheck}>
            {publicCopy.checkAgain}
          </Button>
        </>
      ) : service.connection_mode !== 'hosted' ? (
        <>
          <p>{connectionCopy.metadataUnavailable}</p>
          <Button disabled={disabled} onClick={onRecheck}>
            {publicCopy.checkAgain}
          </Button>
        </>
      ) : (
        <>
          {pending ? (
            <>
              <p role="status">{connectionStates.pending}</p>
              {blocked && handoff?.hosted_url !== undefined ? (
                <>
                  <p>{connectionCopy.blocked}</p>
                  <a href={handoff?.hosted_url}>{connectionCopy.sameTab}</a>
                </>
              ) : (
                <ol>
                  {connectionCopy.checklist.map((line) => (
                    <li key={line}>{line}</li>
                  ))}
                </ol>
              )}
              <Button disabled={disabled} onClick={() => setPollRevision((value) => value + 1)}>
                {connectionCopy.connected}
              </Button>
            </>
          ) : (
            <>
              {state === undefined ? null : <p role="status">{connectionStates[state]}</p>}
              {state === 'satisfied' ? null : (
                <Button disabled={disabled || busy} onClick={connect}>
                  {connectionCopy.connect}
                </Button>
              )}
            </>
          )}
        </>
      )}
      {error === undefined ? null : (
        <p className="public-error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}
