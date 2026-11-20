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
  desktopStableMs,
  desktopStopMessages,
  failDesktopAttempt,
  newDesktopReconnect,
  startDesktopAttempt,
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
  if (
    generation.agentId !== agentId ||
    generation.owner !== owner ||
    generation.attempt !== attempt ||
    generation.runtimeReady !== runtimeReady ||
    generation.runtimeVersion !== runtimeVersion
  ) {
    setGeneration({ agentId, owner, attempt, runtimeReady, runtimeVersion });
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
      setStop(reason);
      setError(desktopStopMessages[reason]);
      setPhase('error');
      return true;
    };
    const failed = (message: string, cause?: unknown, refused = false) => {
      if (disposed) return;
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
      session.policy = startDesktopAttempt(session.policy);
      if (stopped()) return;
      const current = ++version;
      const valid = () => !disposed && current === version && !hidden();
      request = new AbortController();
      setPhase(session.policy.automaticAttempts ? 'recovering' : 'connecting');
      deadline = setTimeout(() => {
        if (valid()) failed('The screen is taking longer than expected. Reconnecting…');
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
          setPhase('loading');
          stopWaiting = waitForDesktopFrame(target, () => {
            if (!valid()) return;
            clearTimeout(deadline);
            setError('');
            setPhase('ready');
            // Short-lived connections must not reset an ongoing failure storm.
            stable = setTimeout(() => {
              if (valid()) session.policy = newDesktopReconnect(0, true);
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
          if (valid()) failed('Connection lost. Reconnecting automatically…');
        });
        rfb.addEventListener('securityfailure', () => {
          if (valid()) failed('Desktop connection refused.', undefined, true);
        });
        prepared?.replay();
      } catch (cause) {
        if (valid()) failed('Desktop is temporarily unavailable. Reconnecting…', cause);
      }
    };
    function schedule() {
      clearTimeout(retry);
      if (disposed || stopped() || hidden() || request) return;
      const delay = Math.max(0, session.policy.retryAt - Date.now());
      if (delay) retry = setTimeout(() => void begin(), delay);
      else void begin();
    }
    const visibility = () => {
      if (session.policy.terminal) return;
      if (hidden()) {
        release();
        setPhase('recovering');
      } else schedule();
    };
    document.addEventListener('visibilitychange', visibility);
    schedule();
    return () => {
      disposed = true;
      release();
      document.removeEventListener('visibilitychange', visibility);
    };
  }, [agentId, attempt, context, discovery, owner, runtimeReady, runtimeVersion]);

  return { screen, phase, error, stop };
}
