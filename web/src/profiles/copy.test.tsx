import { render, screen } from '@testing-library/react';
import { expect, it } from 'vitest';
import { AGENT_PROFILE, profileClient, SKILL_CATALOG } from '../test/profiles';
import { AgentProfileEditor } from './AgentProfileEditor';
import { profileFailure } from './failure';

it('renders human labels for profile options instead of wire vocabulary', async () => {
  const { container } = render(
    <AgentProfileEditor
      profileId={AGENT_PROFILE.id}
      client={profileClient()}
      catalog={SKILL_CATALOG}
    />,
  );
  await screen.findByRole('heading', { name: AGENT_PROFILE.name });
  expect(container.textContent).not.toMatch(/\b[a-z]+_[a-z_]+\b/);
  expect(screen.getByRole('radio', { name: 'Very high' })).toHaveAttribute('value', 'x_high');
});

it('turns public validation extensions into field-level sentences without exposing machine codes', () => {
  const failure = profileFailure({
    status: 422,
    code: 'profile_invalid',
    field: 'system_prompt',
    reason: 'system_prompt_too_long',
  });
  expect(failure.fields).toEqual({
    systemPrompt: 'Shorten the system prompt to fit 32 KB of text.',
  });
  expect(failure.uncertain).toBe(false);
  expect(failure.message).not.toMatch(/\b[a-z]+_[a-z_]+\b/);
});
