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
    'https://catalog.example.test/services/catalog-github',
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
    {
      id: 'github',
      name: 'Other owner service',
      catalogId: 'other-catalog',
      detailUrl: 'https://other.example.test/services/other-catalog',
    },
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
      'https://other.example.test/services/other-catalog',
    ),
  );
});

it('restores a settled name synchronously when the same view returns', async () => {
  const profiles = referenceClient();
  const services = vi.spyOn(profiles.authoring!, 'listServices');
  const draw = (visible: boolean) => (
    <CmaClientProvider client={unavailableClient} profiles={profiles}>
      <ReferenceProvider onNavigate={vi.fn()}>
        {visible ? <ReferenceLink kind="service" id="github" /> : null}
      </ReferenceProvider>
    </CmaClientProvider>
  );
  const view = render(draw(true));
  await screen.findByRole('link', { name: 'GitHub' });
  view.rerender(draw(false));
  view.rerender(draw(true));
  expect(screen.getByRole('link', { name: 'GitHub' })).toHaveAttribute(
    'href',
    'https://catalog.example.test/services/catalog-github',
  );
  expect(screen.queryByText('Loading name')).not.toBeInTheDocument();
  expect(services).toHaveBeenCalledOnce();
  await screen.findByRole('link', { name: 'GitHub' });
});

it('drops resolved names immediately when the client changes without a provider remount', async () => {
  const first = referenceClient(),
    second = referenceClient();
  vi.spyOn(second.authoring!, 'listServices').mockResolvedValue([
    {
      id: 'github',
      name: 'Replacement service',
      catalogId: 'replacement-catalog',
      detailUrl: 'https://other.example.test/services/replacement-catalog',
    },
  ]);
  const draw = (profiles: typeof first) => (
    <CmaClientProvider client={unavailableClient} profiles={profiles}>
      <ReferenceProvider onNavigate={vi.fn()}>
        <ReferenceLink kind="service" id="github" />
      </ReferenceProvider>
    </CmaClientProvider>
  );
  const view = render(draw(first));
  await screen.findByRole('link', { name: 'GitHub' });
  view.rerender(draw(second));
  expect(screen.queryByRole('link', { name: 'GitHub' })).not.toBeInTheDocument();
  expect(screen.getByText('Loading name')).toBeVisible();
  expect(await screen.findByRole('link', { name: 'Replacement service' })).toHaveAttribute(
    'href',
    'https://other.example.test/services/replacement-catalog',
  );
});

it('keeps service names when the catalogue supplies no safe detail URL', async () => {
  const profiles = referenceClient();
  vi.spyOn(profiles.authoring!, 'listServices').mockResolvedValue([
    { id: 'old', name: 'Older server', catalogId: 'legacy' },
    { id: 'missing', name: 'Missing catalog identity' },
    {
      id: 'unsafe',
      name: 'Unsafe destination',
      catalogId: 'unsafe-id',
      detailUrl: 'http://catalog.example.test/services/unsafe-id',
    },
  ]);
  render(
    <CmaClientProvider client={unavailableClient} profiles={profiles}>
      <ReferenceProvider onNavigate={vi.fn()}>
        <ReferenceLink kind="service" id="old" />
        <ReferenceLink kind="service" id="missing" />
        <ReferenceLink kind="service" id="unsafe" />
      </ReferenceProvider>
    </CmaClientProvider>,
  );
  for (const label of ['Older server', 'Missing catalog identity', 'Unsafe destination']) {
    expect(await screen.findByText(label)).toBeVisible();
    expect(screen.queryByRole('link', { name: label })).not.toBeInTheDocument();
  }
});
