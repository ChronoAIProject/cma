import { useEffect, useRef, useState } from 'react';
import {
  navigateToServiceAuthorization,
  startServiceAuthorization,
} from '../auth/serviceAuthorization';
import {
  connectionClient,
  connectionUrl,
  subscriptionService,
  type ConnectionClient,
  type ConnectionReport,
  type ConnectionTarget,
  type StartedConnection,
} from './client';
import { ConnectionWizardView } from './ConnectionWizardView';
import { ConnectionIntentRecovery, savedConnection } from './intentRecovery';
import { needsManualGitHubRepair } from './RepositoryConnectionRepair';
import { rememberSameTabReturn } from './sameTabReturn';
import './Connections.css';

export { RepositoryConnectionRepair } from './RepositoryConnectionRepair';

const POLL_MS = 2000;

interface WizardProps {
  readonly target: ConnectionTarget;
  readonly onReady: (report: ConnectionReport) => void;
  readonly onCancel?: () => void;
  readonly forceSubscription?: boolean;
  readonly expectedOwner?: string;
  readonly presentation?: 'inline' | 'dialog';
  readonly dialogTitle?: string;
  readonly description?: string;
  readonly repositoryRecovery?: boolean;
  readonly client?: ConnectionClient;
}

export function ConnectionWizard(props: WizardProps) {
  const [retry, setRetry] = useState(0);
  // Rechecking this wizard cannot rearm the action that opened it.
  const recoverySelected = useRef(false);
  return (
    <ConnectionSequence
      key={`${JSON.stringify(props.target)}:${props.forceSubscription === true}:${props.expectedOwner ?? ''}:${retry}`}
      {...props}
      onRecoverySelected={() => {
        recoverySelected.current = true;
      }}
      onReady={(report) => {
        if (recoverySelected.current) props.onCancel?.();
        else props.onReady(report);
      }}
      onRetry={() => setRetry((value) => value + 1)}
    />
  );
}

function ConnectionSequence({
  target,
  onReady,
  onCancel,
  onRetry,
  onRecoverySelected,
  forceSubscription = false,
  expectedOwner,
  repositoryRecovery = false,
  presentation = 'inline',
  dialogTitle = 'Connect required services',
  description = 'Connect the accounts this sandbox needs. CMA verifies each connection automatically, then continues opening your Sandbox.',
  client = connectionClient,
}: WizardProps & { readonly onRetry: () => void; readonly onRecoverySelected: () => void }) {
  const key = JSON.stringify(target);
  const recoveryEnabled = import.meta.env.VITE_CONNECTION_INTENT_RECOVERY_ENABLED === 'true';
  const [intentRecovery] = useState(
    () => new ConnectionIntentRecovery(`${key}:${forceSubscription}`, recoveryEnabled),
  );
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
  const recovery = useRef<AbortController | null>(null);
  const recoveryPopup = useRef<Window | null>(null);

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
    if (repositoryRecovery && needsManualGitHubRepair(current)) return;
    const next =
      current.services.find((row) => row.service === intentRecovery.pending?.service_id) ??
      current.services.find(
        (row) =>
          row.state !== 'satisfied' || (!forceDone.current && row.service === subscriptionService),
      );
    if (next === undefined) return;
    starting.current = true;
    const epoch = generation.current;
    setBusy(true);
    setError(undefined);
    try {
      const prepared = intentRecovery.prepare(
        next.service,
        !forceDone.current && next.service === subscriptionService,
      );
      const pending = { service_id: prepared.service_id, intent_id: prepared.intent_id };
      const created = await client.start(target, pending, prepared.force);
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
      intentRecovery.acknowledge({ ...created, hosted_url: url });
      persist({ ...created, hosted_url: url });
      navigatePopup(url);
    } catch (cause) {
      if (!live.current || epoch !== generation.current) return;
      try {
        intentRecovery.interrupt(false);
      } catch {
        /* Keep the in-memory fence on storage failure. */
      }
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

  const cancelConnection = (mode: 'cancel' | 'repair', saved?: StartedConnection) => {
    generation.current += 1;
    cancelled.current = true;
    starting.current = false;
    recovery.current?.abort();
    recovery.current = null;
    recoveryPopup.current?.close();
    recoveryPopup.current = null;
    setBusy(false);
    sequence.current = false;
    setRunning(false);
    const active = attempt.current ?? intentRecovery.pending ?? saved;
    if (active !== undefined) void client.cancel(target, active).catch(() => undefined);
    try {
      intentRecovery.interrupt(true, mode === 'cancel');
    } catch {
      /* Cancellation remains fenced in memory. */
    }
    persist(undefined);
    popup.current?.close();
    popup.current = null;
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
    let saved = savedConnection(key, recoveryEnabled);
    const poll = async () => {
      if (controller.signal.aborted || completed.current) return;
      if (recovery.current !== null) {
        timer = setTimeout(() => void poll(), POLL_MS);
        return;
      }
      try {
        // First establish the current owner before restoring browser recovery.
        const active = attempt.current ?? intentRecovery.pending;
        const result = await client.check(target, active, controller.signal);
        if (controller.signal.aborted || cancelled.current) return;
        if (recovery.current !== null) {
          timer = setTimeout(() => void poll(), POLL_MS);
          return;
        }
        if (owner.current !== undefined && result.owner !== owner.current)
          throw new Error('Your CMA account changed. Reopen this connection step to continue.');
        owner.current = result.owner;
        intentRecovery.bind(result, forceSubscription ? undefined : saved);
        if (repositoryRecovery && needsManualGitHubRepair(result)) {
          onRecoverySelected();
          cancelConnection('repair', saved?.owner === result.owner ? saved : undefined);
          setReport(result);
          return;
        }
        if (recoveryEnabled) {
          if (saved !== undefined && intentRecovery.pending === undefined) persist(undefined);
          saved = undefined;
        }
        if (recoveryEnabled && active === undefined && intentRecovery.pending !== undefined) {
          persist(intentRecovery.pending.started);
          sequence.current = !intentRecovery.needsResume;
          setRunning(sequence.current);
          setReport(result);
          timer = setTimeout(() => void poll(), 0);
          return;
        }
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
        if (
          (attempt.current ?? intentRecovery.pending)?.intent_id !== active?.intent_id ||
          starting.current
        ) {
          timer = setTimeout(() => void poll(), POLL_MS);
          return;
        }
        failures = 0;
        setError(undefined);
        setReport(result);
        const current = attempt.current ?? intentRecovery.pending;
        if (current !== undefined) {
          const row = result.services.find((row) => row.service === current.service_id);
          const started = attempt.current;
          const terminal = ['expired', 'cancelled', 'failed'].includes(result.active_state ?? '');
          const expired = started !== undefined && Date.parse(started.expires_at) <= Date.now();
          if (
            recoveryEnabled &&
            (terminal ||
              (expired && result.active_state !== 'pending' && result.active_state !== 'satisfied'))
          ) {
            sequence.current = false;
            setRunning(false);
            intentRecovery.interrupt(true);
            persist(undefined);
            setError('This sign-in did not finish. Connect again to retry.');
          } else if (
            row?.state === 'satisfied' &&
            (!recoveryEnabled || result.active_state !== 'pending')
          ) {
            if (current.service_id === subscriptionService) forceDone.current = true;
            intentRecovery.settle();
            persist(undefined);
          } else if (!recoveryEnabled && (expired || terminal)) {
            sequence.current = false;
            setRunning(false);
            persist(undefined);
            setError(
              'This sign-in did not finish. Connect again to retry. For ChatGPT subscription sign-in, enable device-code login in your ChatGPT security settings if prompted.',
            );
          }
        }
        if (
          result.ready &&
          forceDone.current &&
          attempt.current === undefined &&
          intentRecovery.pending === undefined &&
          !intentRecovery.needsResume
        ) {
          completed.current = true;
          setDone(true);
          sequence.current = false;
          setRunning(false);
          popup.current?.close();
          ready.current(result);
          return;
        }
        if (
          sequence.current &&
          attempt.current === undefined &&
          intentRecovery.pending === undefined
        )
          await startNext(result);
        if (attempt.current !== undefined && (popup.current === null || popup.current.closed))
          setBlocked(true);
      } catch (cause) {
        if (controller.signal.aborted) return;
        if (recoveryEnabled) setReport(undefined);
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
      recovery.current?.abort();
      recovery.current = null;
      recoveryPopup.current?.close();
      recoveryPopup.current = null;
      clearTimeout(timer);
      popup.current?.close();
      popup.current = null;
    };
    // The serialized target makes this lifecycle independent of parent renders.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, client, forceSubscription, expectedOwner, repositoryRecovery]);

  const connect = () => {
    if (busy || report === undefined) return;
    cancelled.current = false;
    if (
      intentRecovery.needsResume &&
      report.ready &&
      forceDone.current &&
      intentRecovery.pending === undefined
    ) {
      try {
        intentRecovery.confirm();
        setError(undefined);
      } catch {
        setError('Browser recovery could not be saved. Please try again.');
      }
      return;
    }
    openPopup();
    sequence.current = true;
    setRunning(true);
    setError(undefined);
    if (attempt.current !== undefined) navigatePopup(attempt.current.hosted_url);
    else void startNext(report);
  };
  const cancel = () => {
    cancelConnection('cancel');
    onCancel?.();
  };
  const authorize = async () => {
    if (busy || report === undefined || recovery.current !== null) return;
    onRecoverySelected();
    const authorizationWindow = window.open('about:blank', '_blank', 'popup,width=580,height=760');
    if (authorizationWindow === null) {
      setError('Allow pop-ups to authorize required services. Your draft is still here.');
      return;
    }
    authorizationWindow.opener = null;
    recoveryPopup.current = authorizationWindow;
    const controller = new AbortController();
    recovery.current = controller;
    setBusy(true);
    setError(undefined);
    try {
      const location = window.location;
      const destination = await startServiceAuthorization(
        target,
        report.owner,
        location.pathname + location.search,
        controller.signal,
      );
      if (controller.signal.aborted) return;
      // Leave the draft tab open while the separate window requests consent.
      recoveryPopup.current = null;
      cancel();
      if (!authorizationWindow.closed)
        navigateToServiceAuthorization(authorizationWindow, destination);
    } catch {
      if (!controller.signal.aborted) {
        authorizationWindow.close();
        recoveryPopup.current = null;
        setError('Service authorization could not start. Your draft is still here.');
        setBusy(false);
      }
    } finally {
      if (recovery.current === controller) recovery.current = null;
    }
  };
  const sameTabUrl =
    recoveryEnabled &&
    target.type === 'launch' &&
    blocked &&
    !busy &&
    !intentRecovery.needsResume &&
    handoff !== undefined &&
    report?.owner === handoff.owner &&
    report.services.some((row) => row.service === handoff.service_id && row.kind === 'nyxid')
      ? handoff.hosted_url
      : undefined;
  const continueInSameTab = () => {
    if (
      sameTabUrl !== undefined &&
      handoff !== undefined &&
      target.type === 'launch' &&
      rememberSameTabReturn(target.id, handoff.expires_at)
    )
      return true;
    setError('Your return to CMA could not be saved. Allow pop-ups or try again.');
    return false;
  };
  return (
    <ConnectionWizardView
      {...{
        report,
        handoff,
        running,
        busy,
        blocked,
        sameTabUrl,
        continueInSameTab,
        error,
        done,
        forceSubscription,
        description,
        presentation,
        dialogTitle,
        onCancel,
        onRetry,
        connect,
        cancel,
        authorize,
        repositoryRepair: repositoryRecovery ? target : undefined,
        resumeRequired: intentRecovery.needsResume,
      }}
    />
  );
}
