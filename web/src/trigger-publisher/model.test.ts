import { describe, expect, it } from 'vitest';
import {
  blankContent,
  placementError,
  resultSentence,
  reuseExplanation,
  validateStep,
} from './model';
import { content, profiles } from './fixtures';

describe('Trigger publisher validation', () => {
  it('accepts the inline default and validates legacy target revisions', () => {
    expect(validateStep(0, blankContent(), {})).toEqual({});
    expect(validateStep(0, content, profiles)).toEqual({});
    expect(
      validateStep(0, content, {
        ...profiles,
        workspace: { ...profiles.workspace, public: false },
      }),
    ).toEqual({
      'target.workspace_profile': 'Make this profile public before publishing a Trigger.',
    });
    expect(
      validateStep(0, content, { ...profiles, agent: { ...profiles.agent, revision: 5 } }),
    ).toHaveProperty('target.agent_profile');
  });
  it('matches server byte limits and HTTPS placement rules', () => {
    const invalid = {
      ...content,
      presentation: {
        ...content.presentation,
        title: 'x'.repeat(121),
        description: 'x'.repeat(2001),
        signup_hint: 'x'.repeat(201),
      },
      first_message: 'x'.repeat(32769),
    };
    expect(Object.keys(validateStep(2, invalid, profiles))).toEqual([
      'presentation.title',
      'presentation.description',
      'first_message',
      'presentation.signup_hint',
    ]);
    expect(placementError('', 'https://github.com/chronoai/trureturing#readme')).toBeUndefined();
    for (const source of [
      'not a URL',
      'http://example.com',
      'https://user:password@example.com',
      `https://example.com/${'x'.repeat(512)}`,
    ])
      expect(placementError('README', source)).toBeDefined();
    expect(placementError('x'.repeat(81), 'https://example.com')).toBeDefined();
  });
  it('explains all three reuse levels using only the visitor resources', () => {
    expect(reuseExplanation({ ...content.reuse, sandbox: 'always_new' })).toContain(
      'new Sandbox, Workspace, and Agent',
    );
    expect(reuseExplanation({ ...content.reuse, workspace: 'always_new' })).toContain(
      'new Workspace and Agent',
    );
    expect(reuseExplanation({ ...content.reuse, agent: 'always_new' })).toContain(
      'starts a new Agent',
    );
    expect(reuseExplanation(content.reuse)).toContain('chooses whether to continue');
    expect(reuseExplanation({ ...content.reuse, agent: 'resume_existing' })).toContain(
      'continues their previous Agent',
    );
    expect(resultSentence(profiles)).toBe(
      'Visitors get Codex in a trureturing workspace inside a Standard sandbox that reaches github.',
    );
  });
});
