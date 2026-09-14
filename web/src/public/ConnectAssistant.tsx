import { Presence } from '../ui/Presence';
import { useEffect, useRef, useState } from 'react';
import {
  type CreatedServiceConnection,
  type ServiceConnectionMetadata,
  type ServiceConnectionState,
} from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { connectionCopy, connectionStates } from '../vocabulary/connect';
import { refusalSentence } from '../vocabulary/errors';
import { publicCopy } from '../vocabulary/needs';
import type { PublicClient } from './client';
import { isUnsettledCommand } from './admission';
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
  const [verifying, setVerifying] = useState(false);
  const [notConnected, setNotConnected] = useState(false);
  const [pollRevision, setPollRevision] = useState(0);
  const verifyRequested = useRef(false);
  const pollController = useRef<AbortController | undefined>(undefined);
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
    pollController.current = controller;
    const immediate = verifyRequested.current;
    verifyRequested.current = false;
    let explicitCheck = immediate;
    const startedAt = Date.now();
    const deadline = setTimeout(() => {
      if (controller.signal.aborted) return;
      setError(connectionCopy.timeout);
      setVerifying(false);
      controller.abort();
    }, 300_000);
    let timer: ReturnType<typeof setTimeout> | undefined;
    let failures = 0;
    const poll = async () => {
      if (controller.signal.aborted) return;
      try {
        const status = await client.requests.readConnection(handoff.handoff_id, {
          signal: controller.signal,
        });
        if (controller.signal.aborted) return;
        if (
          status.handoff_id !== handoff.handoff_id ||
          status.launch_id !== requestId ||
          status.service_id !== service.service
        )
          throw new Error();
        setState(status.state);
        setError(undefined);
        failures = 0;
        if (status.state === 'satisfied') {
          setNotConnected(false);
          clearTimeout(deadline);
          if (!explicitCheck) {
            // Returning from NyxID never assigns a sandbox until the user
            // explicitly verifies on this same launch page.
            setState('pending');
            return;
          }
          clearConnectionReturn(requestId, service.service);
          popup.current?.close();
          satisfied.current(service.service);
          return;
        }
        if (status.state !== 'pending') {
          setNotConnected(false);
          clearTimeout(deadline);
          clearConnectionReturn(requestId, service.service);
          popup.current?.close();
          command.current = undefined;
          return;
        }
        if (explicitCheck) setNotConnected(true);
      } catch (cause) {
        if (controller.signal.aborted) return;
        failures += 1;
        setNotConnected(false);
        setError(refusalSentence(cause));
      } finally {
        if (!controller.signal.aborted && explicitCheck) {
          explicitCheck = false;
          setVerifying(false);
        }
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
    if (immediate) void poll();
    else timer = setTimeout(() => void poll(), 1500);
    return () => {
      controller.abort();
      clearTimeout(timer);
      clearTimeout(deadline);
    };
  }, [client, handoff, requestId, service.service, pollRevision]);

  const connect = () => {
    if (active.current) return;
    popup.current?.close();
    popup.current = window.open('about:blank', '_blank', 'popup,width=520,height=720');
    if (popup.current !== null) popup.current.opener = null;
    setBlocked(popup.current === null);
    setError(undefined);
    setNotConnected(false);
    if (
      state === 'pending' &&
      handoff?.hosted_url !== undefined &&
      Date.parse(handoff.expires_at) > Date.now()
    ) {
      if (popup.current !== null) popup.current.location.href = handoff.hosted_url;
      return;
    }
    pollController.current?.abort();
    if (handoff !== undefined && Date.parse(handoff.expires_at) <= Date.now())
      command.current = undefined;
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
          if (!isUnsettledCommand(cause)) command.current = undefined;
          setError(refusalSentence(cause));
        },
      )
      .finally(() => {
        active.current = false;
        if (live.current) setBusy(false);
      });
  };
  const verify = () => {
    if (handoff === undefined || state !== 'pending') {
      onRecheck();
      return;
    }
    setError(undefined);
    setNotConnected(false);
    setVerifying(true);
    verifyRequested.current = true;
    setPollRevision((value) => value + 1);
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
              {connectionCopy.connect}
            </a>
          )}
          <Button disabled={disabled} onClick={onRecheck}>
            {connectionCopy.verify}
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
          <Presence>
            {state === undefined ? null : <p role="status">{connectionStates[state]}</p>}
          </Presence>
          {pending ? (
            <>
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
            </>
          ) : null}
          <Presence>
            {notConnected ? <p role="status">{connectionCopy.notConnected}</p> : null}
          </Presence>
          {state === 'satisfied' ? null : (
            <div className="public-actions">
              <Button disabled={disabled || busy || verifying} onClick={connect}>
                {connectionCopy.connect}
              </Button>
              <Button disabled={disabled || busy || verifying} onClick={verify}>
                {verifying ? connectionCopy.verifying : connectionCopy.verify}
              </Button>
            </div>
          )}
        </>
      )}
      <Presence>
        {error === undefined ? null : (
          <p className="public-error" role="alert">
            {error}
          </p>
        )}
      </Presence>
    </section>
  );
}
