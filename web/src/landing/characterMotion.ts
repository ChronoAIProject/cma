import type { CharacterState } from './LandingCharacter';

export function stepMotion(state: CharacterState | undefined, seconds: number) {
  const jumpPhase = (seconds % 2) / 0.6;
  return {
    rotation: state === 'pending' ? seconds * Math.PI * 2 : 0,
    lift: state === 'completed' && jumpPhase < 1 ? 0.08 * Math.sin(jumpPhase * Math.PI) : 0,
  };
}
