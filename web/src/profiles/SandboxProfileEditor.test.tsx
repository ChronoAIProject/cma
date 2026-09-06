import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import {
  documentClient,
  marketplaceClient,
  OWN_PROFILES,
  PROFILE_BOUNDS,
  SANDBOX_PROFILE,
  SERVICE_CATALOG,
} from '../test/profiles';
import { SandboxProfileEditor } from './SandboxProfileEditor';

it('converts custom cores and GiB, selects services, and saves environment and ordered workspace settings', async () => {
  const actor = userEvent.setup();
  const createProfile = vi.fn(documentClient(SANDBOX_PROFILE).createProfile);
  render(
    <SandboxProfileEditor
      client={documentClient(SANDBOX_PROFILE, { createProfile })}
      profiles={OWN_PROFILES}
      marketplace={marketplaceClient()}
      bounds={PROFILE_BOUNDS}
      services={SERVICE_CATALOG}
    />,
  );
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Custom research' } });
  await actor.click(screen.getByRole('radio', { name: 'Custom' }));
  fireEvent.change(screen.getByLabelText('CPU cores'), { target: { value: '1.5' } });
  fireEvent.change(screen.getByLabelText('Memory (GiB)'), { target: { value: '2.25' } });
  await actor.click(await screen.findByRole('checkbox', { name: /Storage/ }));
  await actor.click(screen.getByRole('radio', { name: 'Your own account' }));
  await actor.click(await screen.findByRole('button', { name: 'Add Research workspace' }));
  await actor.click(screen.getByRole('button', { name: 'Add variable' }));
  fireEvent.change(screen.getByLabelText('Variable 1 name'), { target: { value: 'RUN_MODE' } });
  fireEvent.change(screen.getByLabelText('Variable 1 value'), { target: { value: 'research' } });
  await actor.click(screen.getByRole('button', { name: 'Save draft' }));
  await screen.findByText('Draft saved.');
  expect(createProfile.mock.calls[0]?.[0].content).toEqual(
    expect.objectContaining({
      cpuMillis: 1500,
      memoryMib: 2304,
      nyxidServices: ['storage'],
      modelAccess: 'own_credentials',
      workspaceProfiles: [{ id: 'wkp_research', revision: 1 }],
      environmentVariables: [{ name: 'RUN_MODE', value: 'research' }],
    }),
  );
});

it('reports every invalid size, limit, command and variable without sending a write', async () => {
  const actor = userEvent.setup();
  const createProfile = vi.fn();
  render(
    <SandboxProfileEditor
      client={documentClient(SANDBOX_PROFILE, { createProfile })}
      profiles={OWN_PROFILES}
      marketplace={marketplaceClient()}
      bounds={PROFILE_BOUNDS}
    />,
  );
  await actor.click(screen.getByRole('radio', { name: 'Custom' }));
  fireEvent.change(screen.getByLabelText('CPU cores'), { target: { value: '99' } });
  fireEvent.change(screen.getByLabelText('Memory (GiB)'), { target: { value: '99' } });
  fireEvent.change(screen.getByLabelText('Maximum workspaces'), { target: { value: '0' } });
  fireEvent.change(screen.getByLabelText('Maximum agents per workspace'), {
    target: { value: '17' },
  });
  await actor.click(screen.getByRole('button', { name: 'Add command' }));
  await actor.click(screen.getByRole('button', { name: 'Add variable' }));
  fireEvent.change(screen.getByLabelText('Variable 1 name'), { target: { value: 'PATH' } });
  await actor.click(screen.getByRole('button', { name: 'Save draft' }));
  expect(screen.getByRole('alert')).toHaveTextContent('Review 7 fields');
  expect(screen.getByLabelText('CPU cores')).toHaveAttribute('aria-invalid', 'true');
  expect(screen.getByLabelText('Variable 1 name')).toHaveAttribute('aria-invalid', 'true');
  expect(createProfile).not.toHaveBeenCalled();
});

it('does not infer capacity bounds when deployment policy is unavailable', async () => {
  const actor = userEvent.setup();
  const createProfile = vi.fn();
  render(
    <SandboxProfileEditor
      client={documentClient(SANDBOX_PROFILE, { createProfile })}
      profiles={OWN_PROFILES}
      marketplace={marketplaceClient()}
    />,
  );
  expect(screen.getByRole('radio', { name: 'Standard' })).toBeDisabled();
  expect(screen.getByLabelText('CPU cores')).toBeDisabled();
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Bounded sandbox' } });
  await actor.click(screen.getByRole('button', { name: 'Save draft' }));
  expect(createProfile).not.toHaveBeenCalled();
  expect(
    screen.getByText('Resource limits could not be loaded. Try again before saving.'),
  ).toBeInTheDocument();
});
