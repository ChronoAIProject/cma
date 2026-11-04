import { describe, expect, it } from 'vitest';
import {
  BASE_EXPOSURE,
  CHARACTER_POSES,
  POSES,
  blendFrames,
  glintAt,
  keyed,
  poseFrame,
  poseMotion,
} from './poses';
import { REST_FRAME } from './rig';

describe('shipped step motion', () => {
  it('turns preparing characters exactly once per second without changing their height', () => {
    for (const time of [0, 0.25, 1, 2.5]) {
      const { frame } = poseFrame('preparing', time);
      expect(frame.rotY).toBeCloseTo(time * Math.PI * 2);
      expect(frame.y).toBe(0);
      expect(frame.hem).toBe(0);
    }
  });

  it('hops completed characters once every two seconds and rests between hops', () => {
    expect(poseFrame('done', 0).frame.y).toBe(0);
    expect(poseFrame('done', 0.3).frame.y).toBeCloseTo(0.08);
    expect(poseFrame('done', 0.6).frame.y).toBeCloseTo(0);
    expect(poseFrame('done', 1.9).frame.y).toBe(0);
    expect(poseFrame('done', 2.3).frame.y).toBeCloseTo(0.08);
  });

  it('turns running characters half a turn per second with the hem breathing', () => {
    const { frame } = poseFrame('running', 1.5);
    expect(frame.rotY).toBeCloseTo(1.5 * Math.PI);
    expect(frame.hem).toBe(1.5);
  });
});

describe('pose registry', () => {
  it.each(CHARACTER_POSES)('%s produces finite rig channels throughout its motion', (pose) => {
    for (let time = 0; time < 8; time += 0.37) {
      const { frame, exposure } = poseFrame(pose, time);
      for (const value of [
        frame.headYaw,
        frame.headPitch,
        frame.swing,
        frame.hem,
        frame.y,
        frame.rotX,
        frame.rotY,
        frame.rotZ,
        ...frame.scale,
        exposure,
      ])
        expect(Number.isFinite(value)).toBe(true);
    }
  });

  it('defines a still moment for every pose', () => {
    for (const pose of CHARACTER_POSES) expect(POSES[pose].rest).toBeGreaterThanOrEqual(0);
  });

  it('looks left, then right, while idle and faces forward between glances', () => {
    expect(poseFrame('idle', 0).frame.headYaw).toBe(0);
    expect(poseFrame('idle', 2).frame.headYaw).toBeLessThan(-0.4);
    expect(poseFrame('idle', 5).frame.headYaw).toBeGreaterThan(0.4);
  });

  it('looks up while thinking and down while working', () => {
    expect(poseFrame('thinking', 0.75).frame.headPitch).toBeLessThan(0);
    expect(poseFrame('working', 0.4).frame.headPitch).toBeGreaterThan(0);
  });

  it('leans toward the person and grows while asking for approval', () => {
    const { frame } = poseFrame('approval', 1.5);
    expect(frame.rotX).toBeGreaterThan(0);
    expect(frame.scale).toEqual([1.05, 1.05, 1.05]);
    expect(poseFrame('approval', 0.16).frame.y).toBeGreaterThan(0);
  });

  it('plays the failure once and holds its final droop', () => {
    const duration = POSES.failed.duration ?? 0;
    expect(duration).toBeGreaterThan(0);
    expect(poseFrame('failed', 0.2).frame.rotY).not.toBe(0);
    expect(poseFrame('failed', 5).frame).toEqual(poseFrame('failed', duration + 0.5).frame);
    expect(poseFrame('failed', duration).frame.y).toBeLessThan(0);
  });

  it('sleeps low with its head down and dimmed while paused', () => {
    const { frame, exposure } = poseFrame('paused', 0);
    expect(frame.y).toBeLessThan(0);
    expect(frame.headPitch).toBeGreaterThan(0.3);
    expect(exposure).toBeLessThan(BASE_EXPOSURE);
  });

  it('wakes from the paused pose to full exposure while resuming', () => {
    expect(poseMotion('resuming', 0.2).exposure).toBeCloseTo(0.78);
    expect(poseMotion('resuming', 1.2).exposure).toBeCloseTo(BASE_EXPOSURE);
    expect(poseMotion('resuming', 2).float).toBe(true);
  });

  it('pulses its scale in time while replying', () => {
    const a = poseFrame('replying', 0.175).frame.scale;
    const b = poseFrame('replying', 0.525).frame.scale;
    expect(a[1]).toBeLessThan(1);
    expect(b[1]).toBeGreaterThan(1);
  });

  it('bumps on its invitation beat while greeting', () => {
    expect(poseFrame('greet', 0.32).frame.scale[0]).toBeGreaterThan(1.05);
    expect(poseFrame('greet', 1.5).frame.scale[0]).toBe(1);
  });
});

describe('keyed', () => {
  const points = [
    [0, 0],
    [1, 10],
    [2, 10],
  ] as const;

  it('passes through its keys and eases between them', () => {
    expect(keyed(0, 2, points)).toBe(0);
    expect(keyed(1, 2, points)).toBe(10);
    expect(keyed(0.5, 2, points)).toBeCloseTo(5);
    expect(keyed(0.25, 2, points)).toBeLessThan(2.5);
  });

  it('loops over its period and handles negative time', () => {
    expect(keyed(2.5, 2, points)).toBeCloseTo(keyed(0.5, 2, points));
    expect(keyed(-1.5, 2, points)).toBeCloseTo(keyed(0.5, 2, points));
  });
});

describe('glintAt', () => {
  it('reports progress inside each 0.62s glint and -1 between them', () => {
    expect(glintAt(0, 2, 0)).toBe(0);
    expect(glintAt(0.31, 2, 0)).toBeCloseTo(0.5);
    expect(glintAt(1, 2, 0)).toBe(-1);
    expect(glintAt(2.31, 2, 0)).toBeCloseTo(0.5);
    expect(glintAt(0.1, 2, 1)).toBe(-1);
  });
});

describe('blendFrames', () => {
  const to = { ...REST_FRAME, y: 0.1, headYaw: 0.4, rotY: 0.2, scale: [1.2, 1.2, 1.2] as const };

  it('returns either endpoint at 0 and 1', () => {
    expect(blendFrames(REST_FRAME, to, 0)).toEqual({ ...REST_FRAME, hem: to.hem });
    const end = blendFrames(REST_FRAME, to, 1);
    expect(end.y).toBeCloseTo(to.y);
    expect(end.headYaw).toBeCloseTo(to.headYaw);
    expect(end.rotY).toBeCloseTo(to.rotY);
    expect(end.scale[1]).toBeCloseTo(to.scale[1]);
    expect(end.glint).toBe(to.glint);
  });

  it('mixes channels and hides glints until the blend completes', () => {
    const half = blendFrames(REST_FRAME, { ...to, glint: 0.5 }, 0.5);
    expect(half.y).toBeCloseTo(0.05);
    expect(half.scale[0]).toBeCloseTo(1.1);
    expect(half.glint).toBe(-1);
  });

  it('turns the shorter way round after accumulated rotations', () => {
    const spun = { ...REST_FRAME, rotY: Math.PI * 20 + 0.1 };
    const blended = blendFrames(spun, REST_FRAME, 1);
    expect(blended.rotY).toBeCloseTo(Math.PI * 20);
  });
});
