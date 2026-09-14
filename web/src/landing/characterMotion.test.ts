import { expect, it } from 'vitest';
import { stepMotion } from './characterMotion';

it('rotates pending steps exactly once per second without changing their height', () => {
  for (const time of [0, 0.25, 1, 2.5]) {
    expect(stepMotion('pending', time)).toEqual({ rotation: time * Math.PI * 2, lift: 0 });
  }
});

it('jumps completed steps once every two seconds and rests between jumps', () => {
  expect(stepMotion('completed', 0)).toEqual({ rotation: 0, lift: 0 });
  expect(stepMotion('completed', 0.3).lift).toBeCloseTo(0.08);
  expect(stepMotion('completed', 0.6).lift).toBe(0);
  expect(stepMotion('completed', 1.9).lift).toBe(0);
  expect(stepMotion('completed', 2.3).lift).toBeCloseTo(0.08);
});

it.each(['brand', 'running', 'failed'] as const)('does not change %s step motion', (state) => {
  expect(stepMotion(state, 0.3)).toEqual({ rotation: 0, lift: 0 });
});
