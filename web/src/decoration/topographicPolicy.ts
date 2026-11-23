export type TopographicPlacement = 'first-visit' | 'landing';
export type TopographicFault =
  'chunk_unavailable' | 'gl_unavailable' | 'context_lost' | 'render_failed';

type Playback = { started: number; deadline: number; finished: boolean };
type Memory = {
  placements: Map<TopographicPlacement, Playback>;
  active: symbol | null;
  lastFrame: number;
  reported: boolean;
};
const documents = new WeakMap<Document, Memory>();

function memory(document: Document): Memory {
  let value = documents.get(document);
  if (!value) {
    value = { placements: new Map(), active: null, lastFrame: -Infinity, reported: false };
    documents.set(document, value);
  }
  return value;
}

export function finishPlayback(document: Document, placement: TopographicPlacement): void {
  const state = memory(document);
  const playback = state.placements.get(placement);
  if (playback) playback.finished = true;
  else state.placements.set(placement, { started: 0, deadline: 0, finished: true });
}

export function acquirePlayback(document: Document, placement: TopographicPlacement) {
  const state = memory(document);
  const now = performance.now();
  const playback = state.placements.get(placement) ?? {
    started: now,
    deadline: now + 4_000,
    finished: false,
  };
  if (state.active || playback.finished || now >= playback.deadline) return null;
  state.placements.set(placement, playback);
  const owner = Symbol();
  state.active = owner;
  return {
    remaining: () => Math.max(0, playback.deadline - performance.now()),
    takeFrame(): { elapsed: number; submit: (draw: () => void) => boolean } | null {
      const time = performance.now();
      if (
        state.active !== owner ||
        playback.finished ||
        time >= playback.deadline ||
        time - state.lastFrame < 34
      )
        return null;
      return {
        elapsed: (time - playback.started) / 1_000,
        submit(draw) {
          const submittedAt = performance.now();
          if (
            state.active !== owner ||
            playback.finished ||
            submittedAt >= playback.deadline ||
            submittedAt - state.lastFrame < 34
          )
            return false;
          try {
            draw();
          } finally {
            state.lastFrame = performance.now();
          }
          return true;
        },
      };
    },
    finish: () => {
      playback.finished = true;
    },
    release: () => {
      if (state.active === owner) state.active = null;
    },
  };
}

export function reportTopographicFault(document: Document, fault: TopographicFault): void {
  const state = memory(document);
  if (state.reported) return;
  state.reported = true;
  console.warn('Topographic decoration:', fault);
}

export function hasInteractionFocus(document: Document): boolean {
  return !!document.activeElement?.closest(
    'input, textarea, select, button, a[href], [contenteditable]:not([contenteditable="false"])',
  );
}

export function animationAllowed(): boolean {
  const connection = (navigator as Navigator & { connection?: { saveData?: boolean } }).connection;
  return (
    !matchMedia('(prefers-reduced-motion: reduce)').matches &&
    !matchMedia('(pointer: coarse)').matches &&
    innerWidth > 720 &&
    connection?.saveData !== true &&
    typeof IntersectionObserver !== 'undefined' &&
    typeof ResizeObserver !== 'undefined'
  );
}

export function boundedViewport(width: number, height: number, dpr: number) {
  if (![width, height, dpr].every(Number.isFinite) || width <= 0 || height <= 0 || dpr <= 0)
    return null;
  width = Math.min(width, 560);
  height = Math.min(height, 220);
  const ratio = Math.min(dpr, 1.5, Math.sqrt(300_000 / (width * height)));
  return {
    width,
    height,
    ratio,
    pixelsWide: Math.max(1, Math.floor(width * ratio)),
    pixelsHigh: Math.max(1, Math.floor(height * ratio)),
  };
}
