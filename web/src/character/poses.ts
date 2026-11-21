import { REST_FRAME, type RigFrame } from './rig';

/**
 * Every pose the character can hold. Each one moves only the shipped rig's channels — head
 * look, float and sway, lift, scale, tilt, turn, hem breathing, lens glint and exposure — so
 * nothing is ever added to the model to explain a state.
 */
export const CHARACTER_POSES = [
  'brand',
  'idle',
  'thinking',
  'working',
  'replying',
  'approval',
  'done',
  'failed',
  'paused',
  'resuming',
  'preparing',
  'running',
  'greet',
] as const;

export type CharacterPose = (typeof CHARACTER_POSES)[number];

export const BASE_EXPOSURE = 1.12;

/** The pose's intent before it is reduced to rig channels. */
export interface PoseMotion {
  /** Pointer-look inputs; the head turns 0.42 rad and nods 0.24 rad per unit. */
  readonly px: number;
  readonly py: number;
  /** The landing float (2.8% lift on a 6s sine) and sway. */
  readonly float: boolean;
  readonly sway: boolean;
  readonly hem: number;
  readonly rotY: number;
  readonly lift: number;
  readonly sx: number;
  readonly sy: number;
  readonly sz: number;
  readonly tiltX: number;
  readonly tiltZ: number;
  readonly glint: number;
  readonly exposure: number;
}

export interface PoseDefinition {
  /** Seconds a one-shot pose plays before it holds its last frame; absent for loops. */
  readonly duration?: number;
  /** The moment that best explains the pose when it is drawn still. */
  readonly rest: number;
}

export const POSES: Readonly<Record<CharacterPose, PoseDefinition>> = {
  brand: { rest: 0 },
  idle: { rest: 0 },
  thinking: { rest: 0.75 },
  working: { rest: 0.4 },
  replying: { rest: 0 },
  approval: { rest: 0 },
  done: { rest: 0.3 },
  failed: { duration: 1.2, rest: 1.2 },
  paused: { rest: 0 },
  resuming: { rest: 1.2 },
  preparing: { rest: 0 },
  running: { rest: 0 },
  greet: { rest: 0.32 },
};

const smooth = (x: number) => x * x * (3 - 2 * x);
const clamp01 = (x: number) => Math.min(1, Math.max(0, x));
const bump = (c: number, start: number, end: number) =>
  c < start || c > end ? 0 : Math.sin(((c - start) / (end - start)) * Math.PI);

/** Piecewise smoothstep through `[time, value]` keys over a looping period. */
export function keyed(t: number, period: number, points: readonly (readonly [number, number])[]) {
  const c = ((t % period) + period) % period;
  for (let index = 1; index < points.length; index += 1) {
    const current = points[index];
    const previous = points[index - 1];
    if (current === undefined || previous === undefined) continue;
    if (c <= current[0]) {
      const span = current[0] - previous[0];
      return span <= 0
        ? current[1]
        : previous[1] + (current[1] - previous[1]) * smooth((c - previous[0]) / span);
    }
  }
  return points.at(-1)?.[1] ?? 0;
}

/** Progress of a 0.62s glint that starts every `period` seconds, or -1 between glints. */
export function glintAt(t: number, period: number, offset: number) {
  const c = (((t - offset) % period) + period) % period;
  return c < 0.62 ? c / 0.62 : -1;
}

const LOOK_AROUND: readonly (readonly [number, number])[] = [
  [0, 0],
  [0.56, 0],
  [1.26, -1.3],
  [2.66, -1.3],
  [3.36, 0],
  [3.92, 0],
  [4.62, 1.3],
  [6.02, 1.3],
  [6.72, 0],
  [7, 0],
];

const STILL: PoseMotion = {
  px: 0,
  py: 0,
  float: false,
  sway: false,
  hem: 0,
  rotY: 0,
  lift: 0,
  sx: 1,
  sy: 1,
  sz: 1,
  tiltX: 0,
  tiltZ: 0,
  glint: -1,
  exposure: BASE_EXPOSURE,
};

/** What the pose asks of the rig at `t` seconds. */
export function poseMotion(pose: CharacterPose, t: number): PoseMotion {
  switch (pose) {
    case 'idle':
      return {
        ...STILL,
        float: true,
        sway: true,
        hem: t,
        px: keyed(t, 7, LOOK_AROUND),
        py: 0.1,
        glint: glintAt(t, 7, 3.4),
      };
    case 'thinking': {
      const drift = Math.sin((t * Math.PI * 2) / 3);
      return {
        ...STILL,
        float: true,
        sway: true,
        hem: t * 1.5,
        px: 0.9 + 0.2 * drift,
        py: -0.95 + 0.1 * drift,
        tiltZ: -0.05,
        glint: glintAt(t, 1.6, 0),
      };
    }
    case 'working':
      return {
        ...STILL,
        hem: t * 2,
        lift: 0.014 * (0.5 - 0.5 * Math.cos((t * Math.PI * 2) / 0.5)),
        px: 0.7 * Math.sin((t * Math.PI * 2) / 1.6),
        py: 0.55,
        glint: glintAt(t, 1.1, 0.2),
      };
    case 'replying': {
      const beat = Math.sin((t * Math.PI * 2) / 0.7);
      return {
        ...STILL,
        float: true,
        sway: true,
        hem: t * 1.3,
        sx: 1 + 0.03 * beat,
        sy: 1 - 0.035 * beat,
        sz: 1 + 0.03 * beat,
        py: 0.12 * beat,
      };
    }
    case 'approval': {
      const c = t % 2.8;
      return {
        ...STILL,
        hem: t,
        sx: 1.05,
        sy: 1.05,
        sz: 1.05,
        tiltX: 0.12,
        py: -0.15,
        lift: 0.05 * bump(c, 0, 0.32) + 0.03 * bump(c, 0.32, 0.6),
        glint: glintAt(t, 2.8, 1.2),
      };
    }
    case 'done': {
      // The shipped completed step: one 0.6s, 0.08-unit hop every two seconds.
      const phase = (t % 2) / 0.6;
      return { ...STILL, lift: phase < 1 ? 0.08 * Math.sin(phase * Math.PI) : 0 };
    }
    case 'failed': {
      const c = Math.min(t, POSES.failed.duration ?? t);
      const droop = smooth(clamp01((c - 0.6) / 0.6));
      return {
        ...STILL,
        rotY: c < 0.7 ? 0.3 * Math.sin(c * 28) * (1 - c / 0.7) : 0,
        lift: -0.035 * droop,
        tiltZ: 0.1 * droop,
        py: 1.3 * droop,
        px: -0.4 * droop,
        sy: 1 - 0.04 * droop,
      };
    }
    case 'paused': {
      const breath = Math.sin((t * Math.PI * 2) / 4);
      return {
        ...STILL,
        hem: t * 0.35,
        lift: -0.04,
        py: 1.4,
        sx: 1 + 0.012 * breath,
        sy: 1 + 0.02 * breath,
        sz: 1 + 0.012 * breath,
        exposure: 0.78,
      };
    }
    case 'resuming': {
      const c = t % 3;
      const up = smooth(clamp01((c - 0.6) / 0.4));
      const settle = c > 1 ? smooth(clamp01((c - 1) / 0.3)) : 0;
      const shakeOff = c > 1 && c < 1.6 ? 0.8 * Math.sin((c - 1) * 18) * (1 - (c - 1) / 0.6) : 0;
      return {
        ...STILL,
        float: c > 1.6,
        sway: c > 1.6,
        hem: t,
        lift: -0.04 + 0.07 * up - 0.03 * settle,
        sy: 1 + 0.08 * bump(c, 0.6, 1.2),
        py: 1.4 - 1.7 * up + (c > 1 ? 0.3 * smooth(clamp01((c - 1) / 0.4)) : 0),
        px: shakeOff,
        exposure: 0.78 + (BASE_EXPOSURE - 0.78) * up,
      };
    }
    case 'preparing':
      // The shipped pending step: one full turn per second, hem at rest.
      return { ...STILL, rotY: t * Math.PI * 2 };
    case 'running':
      // The shipped running step: a half turn per second with the hem breathing.
      return { ...STILL, rotY: t * Math.PI, hem: t };
    case 'greet': {
      const c = t % 2.4;
      const lift = c < 0.65 ? Math.sin((c / 0.65) * Math.PI) : 0;
      return {
        ...STILL,
        float: true,
        sway: true,
        hem: t,
        lift: 0.08 * lift,
        sx: 1 + 0.06 * lift,
        sy: 1 + 0.06 * lift,
        sz: 1 + 0.06 * lift,
        glint: glintAt(t, 2.4, 1.1),
      };
    }
    case 'brand':
      return { ...STILL, float: true, sway: true, hem: t, glint: glintAt(t, 5.5, 2.4) };
  }
}

/** Reduce a pose at `t` to rig channels, with hem swing from the head's own turn. */
export function poseFrame(
  pose: CharacterPose,
  t: number,
): { readonly frame: RigFrame; readonly exposure: number } {
  const motion = poseMotion(pose, t);
  const before = poseMotion(pose, Math.max(t - 1 / 60, 0));
  return {
    exposure: motion.exposure,
    frame: {
      ...REST_FRAME,
      headYaw: motion.px * 0.42,
      headPitch: motion.py * 0.24,
      swing: (motion.px - before.px) * 9,
      hem: motion.hem,
      y: (motion.float ? Math.sin(t * 1.05) * 0.028 : 0) + motion.lift,
      rotX: motion.tiltX,
      rotY: motion.rotY,
      rotZ: (motion.sway ? Math.sin(t * 0.72) * 0.028 : 0) - motion.px * 0.05 + motion.tiltZ,
      scale: [motion.sx, motion.sy, motion.sz],
      glint: motion.glint,
    },
  };
}

function shortestTurn(from: number, to: number) {
  const full = Math.PI * 2;
  return ((((to - from + Math.PI) % full) + full) % full) - Math.PI;
}

/** Linear blend between two frames, so a pose change eases instead of jumping. */
export function blendFrames(from: RigFrame, to: RigFrame, amount: number): RigFrame {
  const k = clamp01(amount);
  const mix = (a: number, b: number) => a + (b - a) * k;
  return {
    headYaw: mix(from.headYaw, to.headYaw),
    headPitch: mix(from.headPitch, to.headPitch),
    swing: mix(from.swing, to.swing),
    hem: to.hem,
    y: mix(from.y, to.y),
    rotX: mix(from.rotX, to.rotX),
    // Turning poses accumulate whole turns; blend through the shorter way round.
    rotY: from.rotY + shortestTurn(from.rotY, to.rotY) * k,
    rotZ: mix(from.rotZ, to.rotZ),
    scale: [
      mix(from.scale[0], to.scale[0]),
      mix(from.scale[1], to.scale[1]),
      mix(from.scale[2], to.scale[2]),
    ],
    glint: k < 1 ? -1 : to.glint,
  };
}
