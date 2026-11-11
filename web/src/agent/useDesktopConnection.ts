import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import type RFB from '@novnc/novnc';
import {
  desktopSocketUrl,
  takePreparedDesktop,
  type DesktopPreparationScope,
} from './desktopPreparation';
import { desktopInput } from './desktopInput';
import { watchDesktopDisplay } from './desktopDisplay';

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
) {
  const { context, owner } = preparation ?? {};
  const screen = useRef<HTMLDivElement>(null);
  const connection = useRef<RFB>(undefined);
  const input = useRef<ReturnType<typeof desktopInput>>(undefined);
  const [phase, setPhase] = useState<DesktopPhase>(runtimeReady ? 'connecting' : 'waiting');
  const [error, setError] = useState('');
  const [recovery, setRecovery] = useState(0);
  const [generation, setGeneration] = useState({ agentId, attempt, runtimeReady });
  if (
    generation.agentId !== agentId ||
    generation.attempt !== attempt ||
    generation.runtimeReady !== runtimeReady
  ) {
    setGeneration({ agentId, attempt, runtimeReady });
    setPhase(runtimeReady ? 'connecting' : 'waiting');
    setError('');
    setRecovery(0);
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
    // The stream is owner-authorized only after the Sandbox becomes Active.
    // A later revival opens a fresh viewer without restoring manual input.
    if (!runtimeReady) return;
    let disposed = false;
    let failed = false;
    let rfb: RFB | undefined;
    let stopWaiting: (() => void) | undefined;
    let stopWatching: (() => void) | undefined;
    let retry: number | undefined;
    const fail = (message: string) => {
      if (disposed || failed) return;
      failed = true;
      window.clearTimeout(deadline);
      stopWaiting?.();
      stopWatching?.();
      input.current?.release();
      if (rfb) rfb.viewOnly = true;
      setError(message);
      setPhase('error');
    };
    const recoverDisplay = () => {
      if (disposed || failed) return;
      if (recovery >= 2) {
        fail('The desktop display could not be restored. Reconnect to try again.');
        rfb?.disconnect();
        return;
      }
      failed = true;
      window.clearTimeout(deadline);
      stopWaiting?.();
      stopWatching?.();
      input.current?.release();
      if (rfb) rfb.viewOnly = true;
      setPhase('recovering');
      rfb?.disconnect();
      // Release this viewer's old stream before opening another. Never restart
      // the Machine or restore input ownership as part of display recovery.
      retry = window.setTimeout(() => setRecovery(recovery + 1), 750);
    };
    const deadline = window.setTimeout(() => {
      fail('The screen is taking longer than expected. Reconnect to try again.');
      rfb?.disconnect();
    }, 30_000);
    void import('@novnc/novnc')
      .then(({ default: Client }) => {
        if (disposed || failed || !screen.current) return;
        const target = screen.current;
        const prepared = takePreparedDesktop({ context, owner }, agentId);
        try {
          rfb = new Client(target, prepared?.socket ?? desktopSocketUrl(agentId));
        } catch (error) {
          prepared?.socket.close();
          throw error;
        }
        rfb.scaleViewport = true;
        // Keep Agent screenshot coordinates stable while the viewer scales.
        rfb.resizeSession = false;
        rfb.background = 'var(--dsw-alias-bg-layer-3)';
        rfb.viewOnly = true;
        input.current = desktopInput(rfb, target);
        connection.current = rfb;
        rfb.addEventListener('connect', () => {
          if (disposed || failed) return;
          setPhase('loading');
          stopWaiting = waitForDesktopFrame(target, () => {
            if (disposed || failed) return;
            window.clearTimeout(deadline);
            setPhase('ready');
            stopWatching = watchDesktopDisplay(
              target,
              () => {
                if (rfb) rfb.scaleViewport = true;
              },
              recoverDisplay,
            );
          });
        });
        rfb.addEventListener('disconnect', () =>
          fail('Connection lost. Reconnect, and close any other desktop viewer if needed.'),
        );
        rfb.addEventListener('securityfailure', () => fail('Desktop connection refused.'));
        prepared?.replay();
      })
      .catch(() => fail('Could not load the desktop viewer. Reconnect to try again.'));
    return () => {
      disposed = true;
      window.clearTimeout(deadline);
      window.clearTimeout(retry);
      stopWaiting?.();
      stopWatching?.();
      input.current?.dispose();
      input.current = undefined;
      connection.current = undefined;
      rfb?.disconnect();
    };
  }, [agentId, attempt, context, owner, recovery, runtimeReady]);

  return { screen, phase, error };
}
