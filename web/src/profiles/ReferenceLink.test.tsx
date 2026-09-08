import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { unavailableClient } from '../api/client';
import { referenceClient } from '../test/references';
import { ReferenceLink, ReferenceProvider } from './ReferenceLink';

it('resolves original names, deduplicates service reads, and links canonical identities', async () => {
  const profiles = referenceClient();
  const services = vi.spyOn(profiles.authoring!, 'listServices');
  const navigate = vi.fn();
  render(
    <CmaClientProvider client={unavailableClient} profiles={profiles}>
      <ReferenceProvider onNavigate={navigate}>
        <ReferenceLink kind="sandbox" id="sbp_internal" />
        <ReferenceLink kind="workspace" id="wkp_internal" />
        <ReferenceLink kind="agent" id="agp_internal" />
        <ReferenceLink kind="service" id="github" />
        <ReferenceLink kind="service" id="storage" />
        <ReferenceLink kind="skill" id="skill-internal" version="1.2" />
      </ReferenceProvider>
    </CmaClientProvider>,
  );
  expect(await screen.findByRole('link', { name: 'GitHub' })).toHaveAttribute(
    'href',
    'https://nyx.chrono-ai.fun/services/catalog-github',
  );
  expect(await screen.findByRole('link', { name: 'TypeScript' })).toHaveAttribute(
    'href',
    'https://ornn.chrono-ai.fun/skills/skill-internal?version=1.2',
  );
  expect(services).toHaveBeenCalledOnce();
  await userEvent.click(await screen.findByRole('link', { name: 'Research workspace' }));
  expect(navigate).toHaveBeenCalledWith({
    kind: 'profile',
    profileKind: 'workspace',
    profileId: 'wkp_internal',
  });
  expect(screen.queryByText('agp_internal')).not.toBeInTheDocument();
});

it('does not substitute UUIDs or make a service link out of an unresolved alias', async () => {
  const profiles = referenceClient();
  vi.spyOn(profiles.authoring!, 'listServices').mockRejectedValue(new Error('Offline'));
  vi.spyOn(profiles.authoring!, 'searchSkills').mockRejectedValue(new Error('Offline'));
  render(
    <CmaClientProvider client={unavailableClient} profiles={profiles}>
      <ReferenceProvider onNavigate={vi.fn()}>
        <ReferenceLink kind="service" id="unknown-internal" />
        <ReferenceLink kind="skill" id="unknown-skill" />
      </ReferenceProvider>
    </CmaClientProvider>,
  );
  await screen.findByText('Service unavailable');
  expect(screen.queryByText('unknown-internal')).not.toBeInTheDocument();
  expect(screen.queryByRole('link', { name: 'Service unavailable' })).not.toBeInTheDocument();
  await screen.findByRole('link', { name: 'Skill unavailable' });
});

it('drops previous-owner names and ignores late reads when the owner provider remounts', async () => {
  const first = referenceClient(),
    second = referenceClient();
  vi.spyOn(second.authoring!, 'listServices').mockResolvedValue([
    { id: 'github', name: 'Other owner service', catalogId: 'other-catalog' },
  ]);
  const draw = (owner: string, profiles: typeof first) => (
    <CmaClientProvider client={unavailableClient} profiles={profiles}>
      <ReferenceProvider key={owner} onNavigate={vi.fn()}>
        <ReferenceLink kind="service" id="github" />
      </ReferenceProvider>
    </CmaClientProvider>
  );
  const view = render(draw('one', first));
  await screen.findByRole('link', { name: 'GitHub' });
  view.rerender(draw('two', second));
  expect(screen.queryByRole('link', { name: 'GitHub' })).not.toBeInTheDocument();
  await waitFor(() =>
    expect(screen.getByRole('link', { name: 'Other owner service' })).toHaveAttribute(
      'href',
      'https://nyx.chrono-ai.fun/services/other-catalog',
    ),
  );
});
