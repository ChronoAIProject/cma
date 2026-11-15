import { check } from './checks.mjs';

export function scalingSequence(peak = 4) {
  check(peak === 4 || peak === 5, 'invalid_scaling_peak', true);
  return `2-${peak}-2`;
}

export function expectedReplicas(phase, peak = 4) {
  const scaling = scalingSequence(peak);
  return phase === 'n1' ? 1 : phase === 'n2' ? 2 : scaling;
}
