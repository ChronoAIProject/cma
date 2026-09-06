import { render, screen } from '@testing-library/react';
import { expect, it } from 'vitest';
import {
  AGENT_PROFILE,
  profileClient,
  SKILL_CATALOG,
  documentClient,
  marketplaceClient,
  OWN_PROFILES,
  PROFILE_BOUNDS,
  SANDBOX_PROFILE,
  SERVICE_CATALOG,
  WORKSPACE_PROFILE,
} from '../test/profiles';
import { AgentProfileEditor } from './AgentProfileEditor';
import { SandboxProfileEditor } from './SandboxProfileEditor';
import { WorkspaceProfileEditor } from './WorkspaceProfileEditor';
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

it.each(['workspace', 'sandbox'])(
  'renders readable %s controls instead of internal enum values',
  async (kind) => {
    const { container } = render(
      kind === 'workspace' ? (
        <WorkspaceProfileEditor
          client={documentClient(WORKSPACE_PROFILE)}
          profiles={OWN_PROFILES}
          marketplace={marketplaceClient()}
          profileId={WORKSPACE_PROFILE.id}
        />
      ) : (
        <SandboxProfileEditor
          client={documentClient(SANDBOX_PROFILE)}
          profiles={OWN_PROFILES}
          marketplace={marketplaceClient()}
          services={SERVICE_CATALOG}
          bounds={PROFILE_BOUNDS}
          profileId={SANDBOX_PROFILE.id}
        />
      ),
    );
    await screen.findByRole('heading', {
      name: kind === 'workspace' ? WORKSPACE_PROFILE.name : SANDBOX_PROFILE.name,
    });
    await screen.findByRole('button', {
      name: kind === 'workspace' ? 'Add Research agent' : 'Add Research workspace',
    });
    expect(container.textContent).not.toMatch(
      /\b(?:git_repository|chronoai_managed|own_credentials|max_workspaces|max_agents_per_workspace|software_install_commands|environment_variables)\b/,
    );
  },
);

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

it('associates named secret refusals with environment fields without needing their values', () => {
  const failure = profileFailure({
    status: 422,
    code: 'profile_public_refused',
    field: 'environment_variables.SERVICE_CONFIGURATION',
    reason: 'secret_like_environment_value',
  });
  expect(failure.fields).toEqual({
    environmentVariables:
      'Remove the sensitive value for SERVICE_CONFIGURATION before making this profile public.',
  });
  expect(failure.message).toBe(failure.fields.environmentVariables);
  expect(failure.uncertain).toBe(false);
  const unavailable = profileFailure({ status: 503, code: 'profile_secret_scanner_unavailable' });
  expect(unavailable.message).toBe(
    'Environment values could not be checked. This profile was not published publicly.',
  );
  expect(unavailable.uncertain).toBe(true);
});
