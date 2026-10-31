import type { CharacterPalette } from '../landing/characterPalette';
import { POSES, blendFrames, poseFrame, type CharacterPose } from './poses';
import type { CharacterRig, RigFrame } from './rig';

/** Which body alias a character wears; the colour repeats a status word, never replaces it. */
export type CharacterTone = 'brand' | 'warning' | 'error';

const BODY_PROPERTY: Readonly<Record<CharacterTone, string>> = {
  brand: '--dsw-alias-character-brand',
  warning: '--dsw-alias-character-pending',
  error: '--dsw-alias-character-failed',
};

export interface CharacterOptions {
  readonly pose: CharacterPose;
  /** Draw the pose's still moment once instead of animating it. */
  readonly still: boolean;
  readonly shadow: boolean;
  readonly tone: CharacterTone;
  /** Seconds added to this character's clock, so neighbours never move in lockstep. */
  readonly phase: number;
}

export interface CharacterHandle {
  update(options: CharacterOptions): void;
  dispose(): void;
}

/** The one WebGL drawing buffer every character on the page is rendered through. */
export interface StageSurface {
  readonly source: CanvasImageSource;
  /** Grow the buffer to at least this size and return its actual size. */
  ensureSize(width: number, height: number): { readonly width: number; readonly height: number };
  /** Render `rig` into the buffer's bottom-left `width`×`height` corner. */
  draw(rig: CharacterRig, width: number, height: number, exposure: number): void;
  onLost(listener: () => void): void;
}

/** Everything the stage needs from the browser, injected so the scheduling is testable. */
export interface StageEnvironment {
  createSurface(): StageSurface | undefined;
  createRig(): CharacterRig;
  readPalette(host: HTMLElement, bodyProperty: string): CharacterPalette;
  requestFrame(callback: (time: number) => void): number;
  cancelFrame(handle: number): void;
  reducedMotion(): boolean;
  pixelRatio(): number;
  documentVisible(): boolean;
  /** Each watcher returns its own stop function. */
  watchReducedMotion(listener: () => void): () => void;
  watchTheme(listener: () => void): () => void;
  watchDocumentVisibility(listener: () => void): () => void;
  watchVisibility(element: Element, listener: (visible: boolean) => void): () => void;
  watchResize(element: Element, listener: () => void): () => void;
  log(level: 'warn' | 'error', message: string, detail?: unknown): void;
}

export interface CharacterStage {
  attach(
    canvas: HTMLCanvasElement,
    options: CharacterOptions,
    onUnavailable: () => void,
  ): CharacterHandle | undefined;
  /** Characters currently attached; for diagnostics and tests. */
  count(): number;
}

interface Instance {
  /** Draw if needed; true when it wants another frame. */
  draw(delta: number): boolean;
  invalidate(): void;
  repaint(): void;
  unavailable(): void;
}

const BLEND_SECONDS = 0.3;
const MAX_PIXEL_RATIO = 2;

export function createCharacterStage(env: StageEnvironment): CharacterStage {
  let surface: StageSurface | undefined;
  let tried = false;
  let lost = false;
  let pending: number | null = null;
  let last: number | null = null;
  const instances = new Set<Instance>();
  const watchers: (() => void)[] = [];

  function invalidateAll() {
    for (const instance of instances) instance.invalidate();
    schedule();
  }

  function obtainSurface(): StageSurface | undefined {
    if (lost) return undefined;
    if (!tried) {
      tried = true;
      surface = env.createSurface();
      if (surface === undefined) {
        env.log('warn', 'Character stage unavailable; showing the still mark');
        return undefined;
      }
      surface.onLost(() => {
        lost = true;
        env.log('error', 'Character stage lost its WebGL context; showing the still mark', {
          characters: instances.size,
        });
        for (const instance of [...instances]) instance.unavailable();
        instances.clear();
        if (pending !== null) env.cancelFrame(pending);
        pending = null;
        for (const stop of watchers.splice(0)) stop();
      });
      watchers.push(
        env.watchReducedMotion(invalidateAll),
        env.watchTheme(() => {
          for (const instance of instances) instance.repaint();
          schedule();
        }),
        env.watchDocumentVisibility(() => {
          last = null;
          invalidateAll();
        }),
      );
    }
    return surface;
  }

  function schedule() {
    if (pending !== null || lost || instances.size === 0 || !env.documentVisible()) return;
    pending = env.requestFrame(tick);
  }

  function tick(time: number) {
    pending = null;
    if (!env.documentVisible()) {
      last = null;
      return;
    }
    const delta = last === null ? 0 : Math.min((time - last) / 1000, 0.05);
    let again = false;
    for (const instance of instances) {
      if (instance.draw(delta)) again = true;
    }
    last = again ? time : null;
    if (again) schedule();
  }

  function attach(
    canvas: HTMLCanvasElement,
    initial: CharacterOptions,
    onUnavailable: () => void,
  ): CharacterHandle | undefined {
    const target = obtainSurface();
    if (target === undefined) return undefined;
    const context = canvas.getContext('2d');
    if (context === null) {
      env.log('warn', 'Character canvas has no 2D context; showing the still mark');
      return undefined;
    }

    let options = initial;
    let rig: CharacterRig;
    try {
      rig = env.createRig();
      rig.applyPalette(env.readPalette(canvas, BODY_PROPERTY[options.tone]));
    } catch (error) {
      env.log('error', 'Character could not be built; showing the still mark', error);
      return undefined;
    }
    rig.setShadow(options.shadow);

    let time = 0;
    let visible = true;
    let dirty = true;
    let shown: RigFrame | undefined;
    let blend: { from: RigFrame; elapsed: number } | undefined;
    let disposed = false;

    const instance: Instance = {
      invalidate() {
        dirty = true;
      },
      repaint() {
        try {
          rig.applyPalette(env.readPalette(canvas, BODY_PROPERTY[options.tone]));
        } catch (error) {
          env.log('error', 'Character palette did not resolve', error);
        }
        dirty = true;
      },
      unavailable() {
        disposed = true;
        for (const stop of stops) stop();
        rig.dispose();
        onUnavailable();
      },
      draw(delta) {
        if (!visible) return false;
        const definition = POSES[options.pose];
        const moving = !options.still && !env.reducedMotion();
        const settled =
          definition.duration !== undefined && time >= definition.duration && blend === undefined;
        if (!dirty && (!moving || settled)) return false;

        let t: number;
        if (!moving) t = definition.rest;
        else {
          time += delta;
          t =
            definition.duration === undefined
              ? time + options.phase
              : Math.min(time, definition.duration);
        }
        const posed = poseFrame(options.pose, t);
        let frame = posed.frame;
        if (blend !== undefined) {
          if (moving) {
            blend.elapsed += delta;
            frame = blendFrames(blend.from, frame, blend.elapsed / BLEND_SECONDS);
            if (blend.elapsed >= BLEND_SECONDS) blend = undefined;
          } else {
            blend = undefined;
          }
        }

        const ratio = Math.min(env.pixelRatio(), MAX_PIXEL_RATIO);
        const width = Math.round(canvas.clientWidth * ratio);
        const height = Math.round(canvas.clientHeight * ratio);
        if (width === 0 || height === 0) return moving;
        if (canvas.width !== width) canvas.width = width;
        if (canvas.height !== height) canvas.height = height;
        const buffer = target.ensureSize(width, height);
        rig.applyFrame(frame);
        target.draw(rig, width, height, posed.exposure);
        context.clearRect(0, 0, width, height);
        context.drawImage(
          target.source,
          0,
          buffer.height - height,
          width,
          height,
          0,
          0,
          width,
          height,
        );
        shown = frame;
        dirty = false;
        return moving && !(definition.duration !== undefined && time >= definition.duration);
      },
    };

    const stops = [
      env.watchVisibility(canvas, (now) => {
        visible = now;
        if (now) {
          dirty = true;
          schedule();
        }
      }),
      env.watchResize(canvas, () => {
        dirty = true;
        schedule();
      }),
    ];
    instances.add(instance);
    schedule();

    return {
      update(next) {
        if (disposed) return;
        const previous = options;
        options = next;
        if (next.pose !== previous.pose) {
          if (shown !== undefined) blend = { from: shown, elapsed: 0 };
          time = 0;
        }
        if (next.tone !== previous.tone) instance.repaint();
        if (next.shadow !== previous.shadow) rig.setShadow(next.shadow);
        dirty = true;
        schedule();
      },
      dispose() {
        if (disposed) return;
        disposed = true;
        for (const stop of stops) stop();
        instances.delete(instance);
        rig.dispose();
      },
    };
  }

  return { attach, count: () => instances.size };
}
