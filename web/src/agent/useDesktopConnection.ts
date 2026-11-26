import { useCmaClient } from '../api/context';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import type RFB from '@novnc/novnc';
import {
  desktopSocketUrl,
  takePreparedDesktop,
  type DesktopPreparationScope,
} from './desktopPreparation';
import { desktopInput } from './desktopInput';
import { watchDesktopDisplay } from './desktopDisplay';
import {
  checkDesktopBudget,
  desktopStableMs,
  desktopStopMessages,
  failDesktopAttempt,
  newDesktopReconnect,
  startDesktopAttempt,
  type DesktopReconnect,
  type DesktopStop,
} from './desktopReconnect';

export type DesktopPhase = 'waiting' | 'connecting' | 'loading' | 'recovering' | 'ready' | 'error';

/** Poll the first frame quickly; the settled display uses a separate sparse monitor. */
export function waitForDesktopFrame(target: HTMLElement, ready: () => void) {
  let timer: number | undefined;
  const check = () => {
    const canvas = target.querySelector('canvas');
    // noVNC's canvas is transparent until a remote rectangle is drawn. Alpha
    // also accepts a genuinely black desktop, unlike a non-black-pixel test.
    if (canvas && canvas.width > 0 && canvas.height > 0) {
      const pixel = canvas
        .getContext('2d')
        ?.getImageData(Math.floor(canvas.width / 2), Math.floor(canvas.height / 2), 1, 1);
      if (pixel?.data[3]) {
        ready();
        return;
      }
    }
    timer = window.setTimeout(check, 50);
  };
  check();
  return () => window.clearTimeout(timer);
}

export function useDesktopConnection(
  agentId: string,
  attempt: number,
  inputAllowed: boolean,
  runtimeReady: boolean,
  preparation?: DesktopPreparationScope,
  runtimeVersion?: string,
) {
  const discovery = useCmaClient().resourceViews?.agents;
  const { context, owner } = preparation ?? {};
  const recovery = useRef({
    agentId,
    owner,
    attempt,
    runtimeReady,
    runtimeVersion,
    policy: newDesktopReconnect(),
  });
  const screen = useRef<HTMLDivElement>(null);
  const connection = useRef<RFB>(undefined);
  const input = useRef<ReturnType<typeof desktopInput>>(undefined);
  const [phase, setPhase] = useState<DesktopPhase>(runtimeReady ? 'connecting' : 'waiting');
  const [error, setError] = useState('');
  const [stop, setStop] = useState<DesktopStop>();
  const [generation, setGeneration] = useState({
    agentId,
    owner,
    attempt,
    runtimeReady,
    runtimeVersion,
  });
  const sameGeneration = generation.agentId === agentId && generation.owner === owner;
  // A refresh may briefly withhold the version. Treat that as the same admitted
  // runtime, as the reconnect effect does, so a waiting retry keeps its status.
  const admittedVersion = sameGeneration
    ? (runtimeVersion ?? generation.runtimeVersion)
    : runtimeVersion;
  if (
    !sameGeneration ||
    generation.attempt !== attempt ||
    generation.runtimeReady !== runtimeReady ||
    generation.runtimeVersion !== admittedVersion
  ) {
    setGeneration({ agentId, owner, attempt, runtimeReady, runtimeVersion: admittedVersion });
    setPhase(runtimeReady ? 'connecting' : 'waiting');
    setError('');
    setStop(undefined);
  }
  useLayoutEffect(() => {
    const current = connection.current;
    if (!current) return;
    const viewOnly = !inputAllowed || phase !== 'ready';
    if (viewOnly && !current.viewOnly) input.current?.release();
    const gainedControl = current.viewOnly && !viewOnly;
    current.viewOnly = viewOnly;
    if (gainedControl) current.focus();
  }, [inputAllowed, phase]);

  useEffect(() => {
    const previous = recovery.current;
    const sameResource = previous.agentId === agentId && previous.owner === owner;
    // An authority refresh temporarily withholds the version. Remember the
    // last admitted runtime so restoring the same one cannot renew its budget.
    const admittedVersion = sameResource
      ? (runtimeVersion ?? previous.runtimeVersion)
      : runtimeVersion;
    const newRuntime =
      admittedVersion !== previous.runtimeVersion ||
      (admittedVersion === undefined && runtimeReady !== previous.runtimeReady);
    const reset = !sameResource || previous.attempt !== attempt || newRuntime;
    recovery.current = {
      agentId,
      owner,
      attempt,
      runtimeReady,
      runtimeVersion: admittedVersion,
      policy: reset
        ? newDesktopReconnect(sameResource ? previous.policy.retryAt : 0)
        : previous.policy,
    };
    const session = recovery.current;
    if (!runtimeReady) return;
    let disposed = false;
    let version = 0;
    let request: AbortController | undefined;
    let rfb: RFB | undefined;
    let preparedSocket: WebSocket | undefined;
    let stopWaiting: (() => void) | undefined;
    let stopWatching: (() => void) | undefined;
    let retry: ReturnType<typeof setTimeout> | undefined;
    let deadline: ReturnType<typeof setTimeout> | undefined;
    let stable: ReturnType<typeof setTimeout> | undefined;
    let budget: ReturnType<typeof setTimeout> | undefined;
    let reportedStop = false;
    // The budget before the attempt now in progress began. An attempt that a
    // rebuild or a hidden tab supersedes has not failed: restoring this keeps
    // its restart from being spent as an automatic retry (#3295).
    let unsettled: DesktopReconnect | undefined;
    const supersede = () => {
      if (unsettled) session.policy = unsettled;
      unsettled = undefined;
    };
    const hidden = () => document.hidden || document.visibilityState === 'hidden';
    const release = () => {
      // Invalidate callbacks before disconnect (noVNC may emit synchronously).
      version += 1;
      request?.abort();
      request = undefined;
      preparedSocket?.close();
      preparedSocket = undefined;
      clearTimeout(retry);
      clearTimeout(deadline);
      clearTimeout(stable);
      clearTimeout(budget);
      stopWaiting?.();
      stopWatching?.();
      stopWaiting = stopWatching = undefined;
      input.current?.dispose();
      input.current = undefined;
      connection.current = undefined;
      if (rfb) rfb.viewOnly = true;
      const previous = rfb;
      rfb = undefined;
      previous?.disconnect();
    };
    const stopped = () => {
      const reason = session.policy.terminal;
      if (!reason) return false;
      release();
      if (!reportedStop) {
        reportedStop = true;
        console.warn('Desktop observation retries stopped', {
          reason,
          attempts: session.policy.automaticAttempts,
        });
      }
      setStop(reason);
      setError(desktopStopMessages[reason]);
      setPhase('error');
      return true;
    };
    const failed = (message: string, cause?: unknown, refused = false) => {
      if (disposed) return;
      unsettled = undefined;
      release();
      session.policy = failDesktopAttempt(session.policy, Date.now(), cause, refused);
      if (stopped()) return;
      setError(message);
      setPhase('recovering');
      const { failures, retryAt } = session.policy;
      if (failures === 1 || (failures & (failures - 1)) === 0)
        console.warn('Desktop observation retry scheduled', {
          attempt: failures,
          delayMs: retryAt - Date.now(),
        });
      schedule();
    };
    const begin = async () => {
      if (disposed || session.policy.terminal || hidden() || request) return;
      const before = session.policy;
      session.policy = startDesktopAttempt(before, Date.now());
      if (stopped()) return;
      unsettled = before;
      const current = ++version;
      const valid = () => !disposed && current === version && !hidden();
      request = new AbortController();
      setPhase(session.policy.automaticAttempts ? 'recovering' : 'connecting');
      deadline = setTimeout(() => {
        if (valid()) failed('The screen is taking longer than expected.');
      }, 30_000);
      try {
        // A browser WebSocket hides failed-handshake headers. Discovery exposes
        // the shared route budget's Retry-After before opening another stream.
        const prepared = takePreparedDesktop({ context, owner }, agentId);
        preparedSocket = prepared?.socket;
        if (!prepared) await discovery?.desktop(agentId, { signal: request.signal });
        if (!valid()) {
          prepared?.socket.close();
          return;
        }
        const { default: Client } = await import('@novnc/novnc');
        if (!valid() || !screen.current) {
          prepared?.socket.close();
          return;
        }
        const target = screen.current;
        try {
          rfb = new Client(target, prepared?.socket ?? desktopSocketUrl(agentId));
        } catch (cause) {
          prepared?.socket.close();
          throw cause;
        }
        preparedSocket = undefined;
        rfb.scaleViewport = true;
        rfb.resizeSession = false;
        rfb.background = 'var(--dsw-alias-bg-layer-3)';
        rfb.viewOnly = true;
        input.current = desktopInput(rfb, target);
        connection.current = rfb;
        let connected = false;
        rfb.addEventListener('connect', () => {
          if (!valid() || connected) return;
          connected = true;
          setPhase(session.policy.automaticAttempts ? 'recovering' : 'loading');
          stopWaiting = waitForDesktopFrame(target, () => {
            if (!valid()) return;
            clearTimeout(deadline);
            clearTimeout(budget);
            setError('');
            setPhase('ready');
            // Short-lived connections must not reset an ongoing failure storm.
            stable = setTimeout(() => {
              if (valid()) {
                unsettled = undefined;
                session.policy = newDesktopReconnect(0, true);
                console.debug('Desktop observation stable; retry budget reset');
              }
            }, desktopStableMs);
            stopWatching = watchDesktopDisplay(
              target,
              () => {
                if (rfb) rfb.scaleViewport = true;
              },
              () => {
                if (valid()) failed('Restoring the desktop display…');
              },
            );
          });
        });
        rfb.addEventListener('disconnect', () => {
          if (valid()) failed('Connection lost.');
        });
        rfb.addEventListener('securityfailure', () => {
          if (valid()) failed('Desktop connection refused.', undefined, true);
        });
        prepared?.replay();
      } catch (cause) {
        if (valid()) failed('Desktop is temporarily unavailable.', cause);
      }
    };
    function schedule() {
      clearTimeout(retry);
      if (disposed || request) return;
      session.policy = checkDesktopBudget(session.policy, Date.now());
      if (stopped() || hidden()) return;
      // Availability refreshes may rebuild the viewer during an existing backoff.
      if (session.policy.started) setPhase('recovering');
      clearTimeout(budget);
      // Bound backoff, discovery and first-frame waits by the same deadline.
      // A painted stream may finish its stability window without being torn down.
      if (session.policy.expiresAt !== undefined)
        budget = setTimeout(
          () => {
            session.policy = checkDesktopBudget(session.policy, Date.now());
            stopped();
          },
          Math.max(0, session.policy.expiresAt - Date.now()),
        );
      const delay = Math.max(0, session.policy.retryAt - Date.now());
      if (delay) retry = setTimeout(() => void begin(), delay);
      else void begin();
    }
    const visibility = () => {
      if (session.policy.terminal) return;
      if (hidden()) {
        supersede();
        release();
        setPhase('recovering');
      } else schedule();
    };
    document.addEventListener('visibilitychange', visibility);
    schedule();
    return () => {
      disposed = true;
      supersede();
      release();
      document.removeEventListener('visibilitychange', visibility);
    };
  }, [agentId, attempt, context, discovery, owner, runtimeReady, runtimeVersion]);

  return { screen, phase, error, stop };
}
