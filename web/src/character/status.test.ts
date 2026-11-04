import { describe, expect, it } from 'vitest';
import { statusTone } from '../vocabulary/status';
import { characterForStatus, characterForTurn, type ResourceStatus } from './status';

const ALL: readonly ResourceStatus[] = [
  'provisioning',
  'installing',
  'active',
  'pausing',
  'paused',
  'reviving',
  'credential_rotating',
  'rotating',
  'failed',
  'retiring',
  'retired',
  'preparing',
  'ready',
  'removing',
  'removed',
  'creating',
  'starting',
  'working',
  'waiting_approval',
  'stopped',
  'deleted',
];

describe('characterForStatus', () => {
  it.each([
    ['ready', 'idle'],
    ['working', 'thinking'],
    ['waiting_approval', 'approval'],
    ['starting', 'preparing'],
    ['paused', 'paused'],
    ['reviving', 'resuming'],
    ['failed', 'failed'],
    ['stopped', 'paused'],
  ] as const)('poses %s as %s', (status, pose) => {
    expect(characterForStatus(status).pose).toBe(pose);
  });

  it('moves in a list only for work in progress or a person being asked', () => {
    for (const status of ALL) {
      const tone = statusTone(status);
      const { animate } = characterForStatus(status);
      // Deleting is active in the dot vocabulary but asks nothing of the reader.
      const expected =
        (tone === 'active' && !['retiring', 'removing'].includes(status)) || tone === 'warning';
      expect(animate, status).toBe(expected);
    }
  });

  it('repeats only the warning and error tones in the body colour', () => {
    expect(characterForStatus('waiting_approval').tone).toBe('warning');
    expect(characterForStatus('failed').tone).toBe('error');
    for (const status of ALL.filter((s) => !['waiting_approval', 'failed'].includes(s)))
      expect(characterForStatus(status).tone, status).toBe('brand');
  });
});

describe('characterForTurn', () => {
  it('works while a tool is pending, replies while text streams and thinks otherwise', () => {
    expect(characterForTurn({ pendingTools: 2, replying: true })).toBe('working');
    expect(characterForTurn({ pendingTools: 0, replying: true })).toBe('replying');
    expect(characterForTurn({ pendingTools: 0, replying: false })).toBe('thinking');
  });
});
