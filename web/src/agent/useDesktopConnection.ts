import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import type RFB from '@novnc/novnc';
import { desktopInput } from './desktopInput';

export type DesktopPhase = 'connecting' | 'loading' | 'ready' | 'error';

/** Observe only the initial screen; stop all pixel reads as soon as it arrives. */
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

export function useDesktopConnection(agentId: string, attempt: number, inputAllowed: boolean) {
  const screen = useRef<HTMLDivElement>(null);
  const connection = useRef<RFB>(undefined);
  const input = useRef<ReturnType<typeof desktopInput>>(undefined);
  const [phase, setPhase] = useState<DesktopPhase>('connecting');
  const [error, setError] = useState('');
  const [generation, setGeneration] = useState({ agentId, attempt });
  if (generation.agentId !== agentId || generation.attempt !== attempt) {
    setGeneration({ agentId, attempt });
    setPhase('connecting');
    setError('');
  }
  useLayoutEffect(() => {
    const current = connection.current;
    if (!current) return;
    const viewOnly = !inputAllowed || phase !== 'ready';
    if (viewOnly && !current.viewOnly) input.current?.release();
    current.viewOnly = viewOnly;
  }, [inputAllowed, phase]);

  useEffect(() => {
    let disposed = false;
    let failed = false;
    let rfb: RFB | undefined;
    let stopWaiting: (() => void) | undefined;
    const fail = (message: string) => {
      if (disposed || failed) return;
      failed = true;
      window.clearTimeout(deadline);
      stopWaiting?.();
      input.current?.release();
      if (rfb) rfb.viewOnly = true;
      setError(message);
      setPhase('error');
    };
    const deadline = window.setTimeout(() => {
      fail('The screen is taking longer than expected. Reconnect to try again.');
      rfb?.disconnect();
    }, 30_000);
    void import('@novnc/novnc')
      .then(({ default: Client }) => {
        if (disposed || failed || !screen.current) return;
        const target = screen.current;
        const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
        rfb = new Client(
          target,
          `${scheme}://${location.host}/api/v1/agents/${encodeURIComponent(agentId)}/desktop/ws`,
        );
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
          });
        });
        rfb.addEventListener('disconnect', () =>
          fail('Connection lost. Reconnect, and close any other desktop viewer if needed.'),
        );
        rfb.addEventListener('securityfailure', () => fail('Desktop connection refused.'));
      })
      .catch(() => fail('Could not load the desktop viewer. Reconnect to try again.'));
    return () => {
      disposed = true;
      window.clearTimeout(deadline);
      stopWaiting?.();
      input.current?.dispose();
      input.current = undefined;
      connection.current = undefined;
      rfb?.disconnect();
    };
  }, [agentId, attempt]);

  return { screen, phase, error, focus: () => connection.current?.focus() };
}
