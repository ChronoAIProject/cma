import { expect, it } from 'vitest';
import { AGENT_PROFILE } from '../test/profiles';
import { validateAgentProfile } from './validation';

it('validates UTF-8 byte limits and all independent Agent profile fields together', () => {
  const errors = validateAgentProfile({
    name: '\u754c'.repeat(27),
    description: 'a'.repeat(2001),
    content: {
      ...AGENT_PROFILE.draft,
      model: ' ',
      skills: Array.from({ length: 33 }, () => ({ source: 'ornn', id: 'skill' })),
      systemPrompt: '\u754c'.repeat(10923),
    },
  });
  expect(Object.keys(errors).sort()).toEqual([
    'description',
    'model',
    'name',
    'skills',
    'systemPrompt',
  ]);
});

it('accepts exactly bounded text without changing its contents', () => {
  expect(
    validateAgentProfile({
      name: 'a'.repeat(80),
      description: 'a'.repeat(2000),
      content: { ...AGENT_PROFILE.draft, systemPrompt: 'a'.repeat(32768) },
    }),
  ).toEqual({});
});
