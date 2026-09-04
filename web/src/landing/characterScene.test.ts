import { describe, expect, it } from 'vitest';
import { provisioningPose } from './characterScene';

describe('the provisioning character motion', () => {
  it('rotates continuously while keeping a gentle vertical float', () => {
    const first = provisioningPose(1, false);
    const later = provisioningPose(4, false);

    expect(first.yaw).toBeCloseTo(0.42);
    expect(later.yaw).toBeCloseTo(1.68);
    expect(Math.abs(first.y)).toBeLessThanOrEqual(0.028);
    expect(Math.abs(later.y)).toBeLessThanOrEqual(0.028);
    expect(first.y).not.toBe(later.y);
  });

  it('is completely static under reduced motion', () => {
    expect(provisioningPose(42, true)).toEqual({ y: 0, yaw: 0, roll: 0 });
  });
});
