import { describe, expect, it } from 'vitest';
import { draftOf, emptyDraft, isValidKey, validate } from './types';
import { DEFAULT_AGENT } from './types';
import type { EnvironmentProfile } from './types';

const PROFILE: EnvironmentProfile = {
  id: 'envp_1',
  name: 'build tools',
  variables: [{ key: 'BUILD_MODE', value: 'release' }],
  packages: [{ name: 'nyxid' }, { name: 'ripgrep', version: '14' }],
  agent: { ...DEFAULT_AGENT, skills: ['nyxid', 'chosen'] },
  nyxid: { services: [] },
  mandatory: { packages: ['nyxid'], skills: ['nyxid'] },
  createdAt: '2026-08-12T00:00:00Z',
  updatedAt: '2026-08-13T00:00:00Z',
};

describe('isValidKey', () => {
  it('accepts the POSIX shape the server accepts', () => {
    for (const key of ['PATH', '_HIDDEN', 'a1', 'MY_VAR_2']) {
      expect(isValidKey(key)).toBe(true);
    }
  });

  it('rejects anything a session would not see under that name', () => {
    for (const key of ['', '1LEADING_DIGIT', 'HAS-DASH', 'HAS SPACE', 'HAS=EQUALS']) {
      expect(isValidKey(key)).toBe(false);
    }
  });
});

describe('validate', () => {
  it('passes a named profile with nothing in it', () => {
    expect(validate({ ...emptyDraft(), name: 'mine' })).toBeNull();
  });

  it('asks for a name rather than saving an unnamed profile', () => {
    expect(validate(emptyDraft())).toMatch(/name/i);
    expect(validate({ ...emptyDraft(), name: '   ' })).toMatch(/name/i);
  });

  it('names the offending variable so the user knows which field to fix', () => {
    const message = validate({
      ...emptyDraft(),
      name: 'mine',
      variables: [{ key: 'NOT VALID', value: 'x' }],
    });
    expect(message).toContain('NOT VALID');
  });

  it('refuses the same variable twice, because a session cannot hold both', () => {
    const message = validate({
      ...emptyDraft(),
      name: 'mine',
      variables: [
        { key: 'TOKEN', value: 'a' },
        { key: 'TOKEN', value: 'b' },
      ],
    });
    expect(message).toContain('TOKEN');
    expect(message).toMatch(/twice/i);
  });

  it('never quotes a value back, since that is where a credential lives', () => {
    const message = validate({
      ...emptyDraft(),
      name: 'mine',
      variables: [
        { key: 'TOKEN', value: 'super-secret' },
        { key: 'TOKEN', value: 'super-secret' },
      ],
    });
    expect(message).not.toContain('super-secret');
  });
});

describe('draftOf', () => {
  it('reproduces the profile it came from', () => {
    const draft = draftOf(PROFILE);
    expect(draft.name).toBe('build tools');
    expect(draft.variables).toEqual(PROFILE.variables);
    expect(draft.agent.skills).toEqual(PROFILE.agent.skills);
  });

  it('copies rather than aliases, so editing a draft cannot mutate the profile', () => {
    const draft = draftOf(PROFILE);
    expect(draft.variables).not.toBe(PROFILE.variables);
    expect(draft.agent.skills).not.toBe(PROFILE.agent.skills);
  });
});
