import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import {
  AGENT_PROFILE,
  documentClient,
  marketplaceClient,
  OWN_PROFILES,
  PROFILE_BOUNDS,
  SANDBOX_PROFILE,
  WORKSPACE_PROFILE,
} from '../test/profiles';
import { ProfileViews, type ProfileViewsClient } from './ProfileViews';

function client(): ProfileViewsClient {
  return {
    agent: documentClient(AGENT_PROFILE),
    workspace: documentClient(WORKSPACE_PROFILE),
    sandbox: documentClient(SANDBOX_PROFILE),
    profiles: OWN_PROFILES,
    marketplace: marketplaceClient(),
  };
}

it('routes new profiles by kind and replaces their local new identity after creation', async () => {
  const actor = userEvent.setup();
  const onNavigate = vi.fn();
  const dependencies = client();
  const view = render(
    <ProfileViews
      route={{ kind: 'profiles' }}
      client={dependencies}
      onNavigate={onNavigate}
      bounds={PROFILE_BOUNDS}
    />,
  );
  await actor.click(screen.getByRole('button', { name: 'New profile' }));
  expect(onNavigate).toHaveBeenCalledWith({
    kind: 'profile',
    profileKind: 'sandbox',
    profileId: 'new',
  });
  view.rerender(
    <ProfileViews
      route={{ kind: 'profile', profileKind: 'sandbox', profileId: 'new' }}
      client={dependencies}
      onNavigate={onNavigate}
      bounds={PROFILE_BOUNDS}
    />,
  );
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'New sandbox' } });
  await actor.click(screen.getByRole('button', { name: 'Save draft' }));
  await screen.findByText('Draft saved.');
  expect(onNavigate).toHaveBeenLastCalledWith({
    kind: 'profile',
    profileKind: 'sandbox',
    profileId: SANDBOX_PROFILE.id,
  });
});

it('renders Agent editing with explicit catalogue unavailability when no public catalogue is bound', async () => {
  render(
    <ProfileViews
      route={{ kind: 'profile', profileKind: 'agent', profileId: AGENT_PROFILE.id }}
      client={client()}
      onNavigate={vi.fn()}
    />,
  );
  await screen.findByRole('heading', { name: AGENT_PROFILE.name });
  expect(screen.getByText('The skill catalogue is unavailable.')).toBeInTheDocument();
  expect(screen.getByLabelText('Search Ornn skills')).toBeDisabled();
});
