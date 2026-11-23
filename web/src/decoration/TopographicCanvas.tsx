import { useEffect, useRef } from 'react';
import { loadTopographicSurface } from './topographicLoader';
import { readTopographicPalette } from './topographicPalette';
import {
  acquirePlayback,
  animationAllowed,
  boundedViewport,
  finishPlayback,
  hasInteractionFocus,
  reportTopographicFault,
  type TopographicFault,
  type TopographicPlacement,
} from './topographicPolicy';

function attach(canvas: HTMLCanvasElement, placement: TopographicPlacement): () => void {
  const host = canvas.parentElement!;
  const document = canvas.ownerDocument;
  const cleanups: (() => void)[] = [];
  let settled = false;
  let intersecting = false;
  let frame = 0;
  let deferred = 0;
  let expiry = 0;
  let lease: ReturnType<typeof acquirePlayback> = null;
  let context: WebGL2RenderingContext | null = null;
  let surface: ReturnType<NonNullable<Awaited<ReturnType<typeof loadTopographicSurface>>>> | null =
    null;
  let loading = false;
  let factory: Awaited<ReturnType<typeof loadTopographicSurface>> = null;
  let palette = readPalette();

  function readPalette() {
    try {
      return readTopographicPalette(host);
    } catch {
      return null;
    }
  }
  const safely = (operation: () => void) => {
    try {
      operation();
    } catch {
      // Cleanup is best effort for each independently owned resource.
    }
  };
  const stop = (permanent: boolean, fault?: TopographicFault) => {
    if (settled) return;
    settled = true;
    if (permanent) finishPlayback(document, placement);
    safely(() => cancelAnimationFrame(frame));
    safely(() => clearTimeout(deferred));
    safely(() => clearTimeout(expiry));
    for (const cleanup of cleanups.splice(0).reverse()) safely(cleanup);
    safely(() => surface?.dispose());
    surface = null;
    safely(() => context?.getExtension('WEBGL_lose_context')?.loseContext());
    context = null;
    safely(() => {
      canvas.hidden = true;
      canvas.width = canvas.height = 1;
    });
    lease?.release();
    if (fault) safely(() => reportTopographicFault(document, fault));
  };
  const guard = (operation: () => void) => {
    if (settled) return;
    try {
      operation();
    } catch {
      stop(true, 'render_failed');
    }
  };
  const visible = () => {
    if (document.hidden || !intersecting || !host.isConnected || !host.getClientRects().length)
      return false;
    for (let node: HTMLElement | null = host; node; node = node.parentElement) {
      if (node.hidden) return false;
      const style = getComputedStyle(node);
      if (
        style.display === 'none' ||
        style.visibility === 'hidden' ||
        style.contentVisibility === 'hidden'
      )
        return false;
    }
    return host.clientWidth > 0 && host.clientHeight > 0;
  };
  const eligible = () => {
    if (!animationAllowed() || hasInteractionFocus(document) || (lease && lease.remaining() <= 0)) {
      stop(true);
      return false;
    }
    return visible();
  };
  const tick = () => {
    frame = 0;
    guard(() => {
      if (!eligible() || !surface || !palette) return;
      const viewport = boundedViewport(host.clientWidth, host.clientHeight, devicePixelRatio);
      if (!viewport) return;
      if (surface.renderFrame(viewport, palette)) canvas.hidden = false;
      schedule();
    });
  };
  const schedule = () => {
    if (!frame && !settled) frame = requestAnimationFrame(tick);
  };
  const start = () =>
    guard(() => {
      deferred = 0;
      if (!eligible()) return;
      lease = acquirePlayback(document, placement);
      if (!lease) return stop(false);
      expiry = window.setTimeout(() => stop(true), lease.remaining());
      try {
        context = canvas.getContext('webgl2', {
          alpha: true,
          antialias: false,
          premultipliedAlpha: true,
          preserveDrawingBuffer: true,
          depth: false,
          stencil: false,
        });
      } catch {
        return stop(true, 'gl_unavailable');
      }
      if (!context) return stop(true, 'gl_unavailable');
      loading = true;
      void loadTopographicSurface()
        .then((createSurface) =>
          guard(() => {
            loading = false;
            if (!createSurface) return stop(true, 'chunk_unavailable');
            factory = createSurface;
            sync();
          }),
        )
        .catch(() => stop(true, 'chunk_unavailable'));
    });
  const sync = () =>
    guard(() => {
      if (!eligible()) {
        cancelAnimationFrame(frame);
        clearTimeout(deferred);
        frame = deferred = 0;
        return;
      }
      if (!surface && factory && context) {
        surface = factory(canvas, context, () => {
          if (!eligible()) return null;
          const grant = lease?.takeFrame();
          return grant
            ? { elapsed: grant.elapsed, submit: (draw) => eligible() && grant.submit(draw) }
            : null;
        });
      }
      if (surface) schedule();
      else if (!lease && !loading && !deferred && !frame) {
        // A painted interactive frame precedes the import; this is the only RAF chain.
        frame = requestAnimationFrame(() =>
          guard(() => {
            frame = 0;
            deferred = window.setTimeout(start, 0);
          }),
        );
      }
    });
  const listen = (target: EventTarget, name: string, callback: () => void) => {
    const listener = () => guard(callback);
    target.addEventListener(name, listener);
    cleanups.push(() => target.removeEventListener(name, listener));
  };
  guard(() => {
    if (!animationAllowed() || hasInteractionFocus(document) || !palette) return stop(true);
    const lost = () => stop(true, 'context_lost');
    listen(canvas, 'webglcontextlost', lost);
    listen(document, 'focusin', () => {
      if (hasInteractionFocus(document)) stop(true);
    });
    listen(document, 'visibilitychange', sync);
    listen(window, 'resize', sync);
    listen(matchMedia('(prefers-reduced-motion: reduce)'), 'change', sync);
    listen(matchMedia('(pointer: coarse)'), 'change', sync);
    const intersection = new IntersectionObserver((entries) =>
      guard(() => {
        intersecting = entries.some((entry) => entry.target === host && entry.isIntersecting);
        sync();
      }),
    );
    cleanups.push(() => intersection.disconnect());
    intersection.observe(host);
    const resize = new ResizeObserver(sync);
    cleanups.push(() => resize.disconnect());
    resize.observe(host);
    const attributes = new MutationObserver(() =>
      guard(() => {
        palette = readPalette();
        if (!palette) return stop(true);
        sync();
      }),
    );
    cleanups.push(() => attributes.disconnect());
    for (let node: HTMLElement | null = host; node; node = node.parentElement)
      attributes.observe(node, {
        attributes: true,
        attributeFilter: ['hidden', 'aria-hidden', 'style', 'class', 'data-ds-dark-theme'],
      });
  });
  return () => stop(false);
}

export function TopographicCanvas({ placement }: { readonly placement: TopographicPlacement }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    if (!canvas.current) return;
    try {
      return attach(canvas.current, placement);
    } catch {
      finishPlayback(document, placement);
      reportTopographicFault(document, 'render_failed');
    }
  }, [placement]);
  return <canvas ref={canvas} width={1} height={1} hidden aria-hidden="true" />;
}
