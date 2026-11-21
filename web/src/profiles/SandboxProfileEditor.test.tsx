import { fireEvent, render, screen, within } from '@testing-library/react';
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

it('separates model access from NyxID integrations and removes old model grants when switching', async () => {
  const actor = userEvent.setup();
  const record = {
    ...SANDBOX_PROFILE,
    draft: {
      ...SANDBOX_PROFILE.draft,
      modelAccess: 'chronoai_managed' as const,
      nyxidServices: ['chrono-llm', 'chrono-llm-public', 'api-github', 'ornn-api'],
    },
  };
  const saveProfile = vi.fn().mockResolvedValue({ ...record, draftVersion: 4 });
  const services = {
    listServices: vi.fn().mockResolvedValue([
      { id: 'chrono-llm', name: 'Chrono LLM' },
      { id: 'chrono-llm-public', name: 'Chrono LLM (public)' },
      { id: 'api-github', name: 'GitHub OAuth' },
      { id: 'ornn-api', name: 'Ornn API' },
    ]),
  };
  render(
    <SandboxProfileEditor
      client={documentClient(record, { saveProfile })}
      profileId={record.id}
      profiles={OWN_PROFILES}
      marketplace={marketplaceClient()}
      bounds={PROFILE_BOUNDS}
      services={services}
    />,
  );
  const provider = await screen.findByRole('radiogroup', { name: 'Model access' });
  expect(within(provider).getByRole('radio', { name: 'Chrono LLM (public)' })).toBeChecked();
  expect(await screen.findByRole('checkbox', { name: /GitHub OAuth/ })).toBeChecked();
  expect(screen.getByRole('checkbox', { name: /Ornn API/ })).toBeChecked();
  expect(screen.queryByRole('checkbox', { name: /Chrono LLM/ })).not.toBeInTheDocument();
  await actor.click(within(provider).getByRole('radio', { name: 'Your ChatGPT subscription' }));
  await actor.click(screen.getByRole('button', { name: 'Save draft' }));
  await screen.findByText('Draft saved.');
  expect(saveProfile.mock.calls[0]?.[1]).toMatchObject({
    content: { modelAccess: 'own_credentials', nyxidServices: ['api-github', 'ornn-api'] },
  });
});

it('enables warmup only for public published profiles and keeps manual availability editable', async () => {
  const actor = userEvent.setup();
  render(
    <SandboxProfileEditor
      client={documentClient(SANDBOX_PROFILE)}
      profileId={SANDBOX_PROFILE.id}
      profiles={OWN_PROFILES}
      marketplace={marketplaceClient()}
      bounds={PROFILE_BOUNDS}
    />,
  );
  expect(await screen.findByRole('checkbox', { name: /Enable warm up/ })).toBeDisabled();
  expect(screen.getByLabelText('Warm sandboxes')).toBeDisabled();
  const keepAlive = screen.getByRole('checkbox', { name: /Always keep alive/ });
  expect(keepAlive).toBeEnabled();
  await actor.click(keepAlive);
  expect(keepAlive).toBeChecked();
});

it('saves a five-sandbox warmup policy, validates the bound and retains it through later edits', async () => {
  const actor = userEvent.setup();
  const availability = { warmupEnabled: true, warmupCount: 5, alwaysKeepAlive: true };
  const record = {
    ...SANDBOX_PROFILE,
    visibility: 'public' as const,
    availability: { warmupEnabled: false, warmupCount: 1, alwaysKeepAlive: false },
  };
  const saveProfile = vi.fn().mockResolvedValue({ ...record, availability, draftVersion: 4 });
  render(
    <SandboxProfileEditor
      client={documentClient(record, { saveProfile })}
      profileId={record.id}
      profiles={OWN_PROFILES}
      marketplace={marketplaceClient()}
      bounds={PROFILE_BOUNDS}
    />,
  );
  await actor.click(await screen.findByRole('checkbox', { name: /Enable warm up/ }));
  const count = screen.getByLabelText('Warm sandboxes');
  expect(count).toBeEnabled();
  fireEvent.change(count, { target: { value: '6' } });
  await actor.click(screen.getByRole('button', { name: 'Save draft' }));
  expect(saveProfile).not.toHaveBeenCalled();
  expect(count).toHaveAttribute('aria-invalid', 'true');
  fireEvent.change(count, { target: { value: '5' } });
  await actor.click(screen.getByRole('checkbox', { name: /Always keep alive/ }));
  await actor.click(screen.getByRole('button', { name: 'Save draft' }));
  await screen.findByText('Draft saved.');
  expect(saveProfile.mock.calls[0]?.[1]).toMatchObject({
    availability,
    draftVersion: 3,
    content: record.draft,
  });
  expect(screen.getByRole('checkbox', { name: /Enable warm up/ })).toBeChecked();
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'New display name' } });
  await actor.click(screen.getByRole('button', { name: 'Save draft' }));
  await screen.findByText('Draft saved.');
  expect(saveProfile.mock.calls[1]?.[1]).toMatchObject({ availability, draftVersion: 4 });
});

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
  await actor.click(screen.getByRole('radio', { name: 'Your ChatGPT subscription' }));
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
