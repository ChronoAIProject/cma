import { describe, expect, it } from 'vitest';
import type { SessionLifecycle } from './types';
import { labelFor, toneFor } from './types';

const ALL: readonly SessionLifecycle[] = [
  'provisioning',
  'ready',
  'running',
  'rotating',
  'idle',
  'terminated',
  'unknown',
];

describe('session lifecycle presentation', () => {
  it('never presents idle as a failure', () => {
    // docs/API_Overview.md calls this out as the most commonly misread state:
    // the bot pod was reclaimed to save resources and the session is revivable
    // under the same id. Styling it as danger would teach operators to panic.
    expect(toneFor('idle')).toBe('wait');
    expect(toneFor('idle')).not.toBe('fail');
    expect(labelFor('idle')).toBe('paused');
  });

  it('treats healthy states as healthy', () => {
    expect(toneFor('ready')).toBe('live');
    expect(toneFor('running')).toBe('live');
  });

  it('shows a future lifecycle under the server word without failure tone', () => {
    expect(labelFor('unknown', 'hibernating')).toBe('hibernating');
    expect(toneFor('unknown')).not.toBe('fail');
  });

  it('gives every lifecycle state a tone and a label', () => {
    for (const state of ALL) {
      expect(toneFor(state)).toBeTruthy();
      expect(labelFor(state)).toBeTruthy();
    }
  });

  it('always pairs colour with a word', () => {
    // Web_Style.md forbids conveying state through colour alone, so every
    // state must have a non-empty label to sit beside its dot.
    for (const state of ALL) {
      expect(labelFor(state).trim().length).toBeGreaterThan(0);
    }
  });
});
