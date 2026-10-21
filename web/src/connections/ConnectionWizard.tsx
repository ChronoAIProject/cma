import { useEffect, useRef, useState } from 'react';
import { LandingCharacter } from '../landing/LandingCharacter';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import {
  connectionClient,
  connectionUrl,
  subscriptionService,
  type ConnectionClient,
  type ConnectionReport,
  type ConnectionTarget,
  type StartedConnection,
} from './client';
import './Connections.css';

const POLL_MS = 2000;

interface WizardProps {
  readonly target: ConnectionTarget;
  readonly onReady: (report: ConnectionReport) => void;
  readonly onCancel?: () => void;
  readonly forceSubscription?: boolean;
  readonly expectedOwner?: string;
  readonly presentation?: 'inline' | 'dialog';
  readonly dialogTitle?: string;
  readonly client?: ConnectionClient;
}

/**
 * A connection's name as the person reads it.
 *
 * The subscription row's recorded name carries the agent provider's product
 * name, which CMA copy never shows; the person signs in to ChatGPT, so that is
 * what the row says.
 */
function connectionName(row: { readonly service: string; readonly display_name: string }) {
  return row.service === subscriptionService ? 'Your ChatGPT subscription' : row.display_name;
}

export function ConnectionWizard(props: WizardProps) {
  const [retry, setRetry] = useState(0);
  return (
    <ConnectionSequence
      key={`${JSON.stringify(props.target)}:${props.forceSubscription === true}:${retry}`}
      {...props}
      onRetry={() => setRetry((value) => value + 1)}
    />
  );
}

function ConnectionSequence({
  target,
  onReady,
  onCancel,
  onRetry,
  forceSubscription = false,
  expectedOwner,
  presentation = 'inline',
  dialogTitle = 'Connect required services',
  client = connectionClient,
}: WizardProps & { readonly onRetry: () => void }) {
  const key = JSON.stringify(target);
  const [report, setReport] = useState<ConnectionReport>();
  const [handoff, setHandoff] = useState<StartedConnection>();
  const [running, setRunning] = useState(false);
  const [busy, setBusy] = useState(false);
  const [blocked, setBlocked] = useState(false);
  const [error, setError] = useState<string>();
  const [done, setDone] = useState(false);
  const popup = useRef<Window | null>(null);
  const live = useRef(false);
  const starting = useRef(false);
  const completed = useRef(false);
  const forceDone = useRef(!forceSubscription);
  const ready = useRef(onReady);
  useEffect(() => {
    ready.current = onReady;
  }, [onReady]);
  const owner = useRef<string | undefined>(expectedOwner);
  const attempt = useRef<StartedConnection | undefined>(undefined);
  const sequence = useRef(false);
  const generation = useRef(0);
  const cancelled = useRef(false);

  const persist = (value: StartedConnection | undefined) => {
    attempt.current = value;
    if (live.current) setHandoff(value);
    try {
      if (value === undefined) sessionStorage.removeItem(`cma.connection.${key}`);
      else sessionStorage.setItem(`cma.connection.${key}`, JSON.stringify(value));
    } catch {
      /* The server owns connection authority even if browser storage is unavailable. */
    }
  };

  const openPopup = () => {
    if (popup.current === null || popup.current.closed) {
      popup.current = window.open(
        'about:blank',
        'cma-service-connections',
        'popup,width=580,height=760',
      );
      if (popup.current !== null) popup.current.opener = null;
    }
    setBlocked(popup.current === null || popup.current.closed);
  };

  const navigatePopup = (url: string) => {
    try {
      if (popup.current !== null && !popup.current.closed) {
        popup.current.location.href = url;
        popup.current.focus();
        setBlocked(false);
      } else setBlocked(true);
    } catch {
      setBlocked(true);
    }
  };

  const startNext = async (current: ConnectionReport) => {
    if (starting.current || !sequence.current || !live.current) return;
    const next = current.services.find(
      (row) =>
        row.state !== 'satisfied' || (!forceDone.current && row.service === subscriptionService),
    );
    if (next === undefined) return;
    starting.current = true;
    const epoch = generation.current;
    setBusy(true);
    setError(undefined);
    try {
      const pending = { service_id: next.service, intent_id: crypto.randomUUID() };
      const created = await client.start(
        target,
        pending,
        !forceDone.current && next.service === subscriptionService,
      );
      if (!live.current || epoch !== generation.current || cancelled.current) {
        void client.cancel(target, created).catch(() => undefined);
        return;
      }
      if (
        created.owner !== current.owner ||
        created.service_id !== pending.service_id ||
        created.intent_id !== pending.intent_id
      )
        throw new Error('Account changed. Reopen this connection step to continue.');
      const url = connectionUrl(created.hosted_url);
      if (url === undefined || !Number.isFinite(Date.parse(created.expires_at)))
        throw new Error('The sign-in link is unavailable. Please try again.');
      persist({ ...created, hosted_url: url });
      navigatePopup(url);
    } catch (cause) {
      if (!live.current || epoch !== generation.current) return;
      sequence.current = false;
      setRunning(false);
      setError(
        cause instanceof Error ? cause.message : 'Sign-in could not start. Please try again.',
      );
    } finally {
      if (epoch === generation.current) {
        starting.current = false;
        if (live.current) setBusy(false);
      }
    }
  };

  useEffect(() => {
    live.current = true;
    generation.current += 1;
    cancelled.current = false;
    starting.current = false;
    sequence.current = false;
    attempt.current = undefined;
    completed.current = false;
    forceDone.current = !forceSubscription;
    owner.current = expectedOwner;
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let failures = 0;
    let saved: StartedConnection | undefined;
    try {
      const stored = sessionStorage.getItem(`cma.connection.${key}`);
      if (stored !== null) {
        const value = JSON.parse(stored) as StartedConnection;
        if (
          typeof value.owner === 'string' &&
          typeof value.intent_id === 'string' &&
          typeof value.service_id === 'string' &&
          connectionUrl(value.hosted_url) !== undefined &&
          Date.parse(value.expires_at) > Date.now()
        )
          saved = value;
      }
    } catch {
      /* Expired or malformed browser recovery is discarded. */
    }
    const poll = async () => {
      if (controller.signal.aborted || completed.current) return;
      try {
        // First establish the current owner before restoring browser recovery.
        const active = attempt.current;
        const result = await client.check(target, active, controller.signal);
        if (controller.signal.aborted || cancelled.current) return;
        if (owner.current !== undefined && result.owner !== owner.current)
          throw new Error('Your CMA account changed. Reopen this connection step to continue.');
        owner.current = result.owner;
        if (saved !== undefined) {
          const restore =
            saved.owner === result.owner &&
            result.services.some((row) => row.service === saved?.service_id);
          if (restore) {
            persist(saved);
            sequence.current = true;
            setRunning(true);
            setBlocked(true);
          } else persist(undefined);
          saved = undefined;
          if (restore) {
            setReport(result);
            timer = setTimeout(() => void poll(), 0);
            return;
          }
        }
        // A sign-in started while this read was in flight; verify that exact intent next.
        if (attempt.current?.intent_id !== active?.intent_id || starting.current) {
          timer = setTimeout(() => void poll(), POLL_MS);
          return;
        }
        failures = 0;
        setError(undefined);
        setReport(result);
        const current = attempt.current;
        if (current !== undefined) {
          const row = result.services.find((row) => row.service === current.service_id);
          if (row?.state === 'satisfied') {
            if (current.service_id === subscriptionService) forceDone.current = true;
            persist(undefined);
          } else if (
            Date.parse(current.expires_at) <= Date.now() ||
            ['expired', 'cancelled', 'failed'].includes(result.active_state ?? '')
          ) {
            sequence.current = false;
            setRunning(false);
            persist(undefined);
            setError(
              'This sign-in did not finish. Connect again to retry. For ChatGPT subscription sign-in, enable device-code login in your ChatGPT security settings if prompted.',
            );
          }
        }
        if (result.ready && forceDone.current && attempt.current === undefined) {
          completed.current = true;
          setDone(true);
          sequence.current = false;
          setRunning(false);
          popup.current?.close();
          ready.current(result);
          return;
        }
        if (sequence.current && attempt.current === undefined) await startNext(result);
        if (attempt.current !== undefined && (popup.current === null || popup.current.closed))
          setBlocked(true);
      } catch (cause) {
        if (controller.signal.aborted) return;
        failures += 1;
        setError(
          cause instanceof Error
            ? cause.message
            : 'The connection could not be verified. Please try again.',
        );
      }
      if (!controller.signal.aborted)
        timer = setTimeout(() => void poll(), Math.min(POLL_MS * 2 ** failures, 15000));
    };
    void poll();
    return () => {
      live.current = false;
      generation.current += 1;
      controller.abort();
      clearTimeout(timer);
      popup.current?.close();
      popup.current = null;
    };
    // The serialized target makes this lifecycle independent of parent renders.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, client, forceSubscription, expectedOwner]);

  const connect = () => {
    if (busy || report === undefined) return;
    cancelled.current = false;
    openPopup();
    sequence.current = true;
    setRunning(true);
    setError(undefined);
    if (attempt.current !== undefined) navigatePopup(attempt.current.hosted_url);
    else void startNext(report);
  };
  const cancel = () => {
    generation.current += 1;
    cancelled.current = true;
    starting.current = false;
    setBusy(false);
    sequence.current = false;
    setRunning(false);
    if (attempt.current !== undefined)
      void client.cancel(target, attempt.current).catch(() => undefined);
    persist(undefined);
    popup.current?.close();
    popup.current = null;
    onCancel?.();
  };
  const connected = report?.services.filter((row) => row.state === 'satisfied').length ?? 0;
  const missing = report?.services.filter((row) => row.state !== 'satisfied') ?? [];
  const content = (
    <section
      className="connection-wizard"
      aria-label="Required connections"
      aria-busy={busy || report === undefined}
    >
      <p>
        Connect the accounts this sandbox needs. CMA verifies each connection automatically, then
        continues opening your Sandbox.
      </p>
      {report === undefined ? (
        <p role="status">Checking your connections…</p>
      ) : (
        <>
          <p className="connection-wizard__count" role="status">
            {connected} of {report.services.length} connections verified
          </p>
          {(['nyxid', 'codex'] as const).map((kind) => (
            <div className="connection-wizard__group" key={kind}>
              <h3>{kind === 'nyxid' ? 'NyxID services' : 'Model subscription'}</h3>
              {report.services
                .filter((row) => row.kind === kind)
                .map((row) => (
                  <div className="connection-wizard__row" key={row.service}>
                    <LandingCharacter
                      compact
                      state={row.state === 'satisfied' ? 'completed' : 'pending'}
                    />
                    <span>{connectionName(row)}</span>
                    <small>
                      {row.state === 'satisfied'
                        ? 'Connected'
                        : row.service === handoff?.service_id
                          ? 'Waiting for sign-in'
                          : 'Connection needed'}
                    </small>
                  </div>
                ))}
              {kind === 'nyxid' && !report.services.some((row) => row.kind === kind) ? (
                <p>No additional services required.</p>
              ) : null}
            </div>
          ))}
        </>
      )}
      {handoff === undefined ? null : (
        <div className="connection-wizard__handoff">
          {handoff.user_code === undefined ? null : (
            <>
              <p>Sign in to your ChatGPT subscription using this one-time code:</p>
              <strong className="connection-wizard__code">{handoff.user_code}</strong>
              <p>
                Enter it on the OpenAI page. Your subscription stays connected to your CMA account.
              </p>
            </>
          )}
          <p role="status">
            {blocked
              ? 'Open the sign-in window to continue. CMA will keep checking here.'
              : 'Finish sign-in in the other window. CMA is checking automatically…'}
          </p>
        </div>
      )}
      {error === undefined ? null : (
        <p role="alert" className="connection-wizard__error">
          {error}
        </p>
      )}
      <div className="connection-wizard__actions">
        {done ? (
          <p>All connections verified.</p>
        ) : (
          <Button variant="primary" onClick={connect} disabled={busy || report === undefined}>
            {busy
              ? 'Preparing sign-in…'
              : handoff !== undefined
                ? 'Open sign-in window'
                : forceSubscription
                  ? 'Sign in to your ChatGPT subscription again'
                  : missing.length === 1 && missing[0]?.service === subscriptionService
                    ? 'Sign in to your ChatGPT subscription'
                    : 'Connect all'}
          </Button>
        )}
        {error === undefined ? null : <Button onClick={onRetry}>Verify again</Button>}
        {running || onCancel !== undefined ? <Button onClick={cancel}>Cancel</Button> : null}
      </div>
    </section>
  );
  if (presentation === 'inline') return content;
  // Check connected sessions without moving focus. Open recovery only when
  // the server reports work to do, or verification needs a retry.
  return (
    <Modal
      open={
        !done &&
        (error !== undefined ||
          (report !== undefined && (!report.ready || forceSubscription || handoff !== undefined)))
      }
      title={dialogTitle}
      onClose={cancel}
    >
      {content}
    </Modal>
  );
}
