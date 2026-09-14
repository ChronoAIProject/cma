import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import {
  documentClient,
  marketplaceClient,
  OWN_PROFILES,
  WORKSPACE_PROFILE,
} from '../test/profiles';
import { WorkspaceProfileEditor } from './WorkspaceProfileEditor';

it('creates a repository workspace only after server verification and preserves ordered commands and references', async () => {
  const actor = userEvent.setup();
  const createProfile = vi.fn(documentClient(WORKSPACE_PROFILE).createProfile);
  const resolveRepository = vi.fn(async (input) => input);
  render(
    <WorkspaceProfileEditor
      client={documentClient(WORKSPACE_PROFILE, { createProfile })}
      profiles={OWN_PROFILES}
      marketplace={marketplaceClient()}
      resolver={{ resolveRepository }}
    />,
  );
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Research checkout' } });
  fireEvent.change(screen.getByLabelText('Repository URL'), {
    target: { value: 'https://github.com/example/research.git' },
  });
  fireEvent.change(screen.getByLabelText('Branch or tag'), { target: { value: 'release' } });
  expect(screen.getByLabelText('Directory name')).toHaveValue('research');
  await actor.click(screen.getByRole('button', { name: 'Save draft' }));
  expect(createProfile).not.toHaveBeenCalled();
  expect(
    screen.getByText('Verify the repository and reference before saving.'),
  ).toBeInTheDocument();
  await actor.click(screen.getByRole('button', { name: 'Verify repository' }));
  await screen.findByText('Repository verified');
  expect(
    screen.queryByText('Verify the repository and reference before saving.'),
  ).not.toBeInTheDocument();
  expect(screen.getByLabelText('Repository URL')).not.toHaveAttribute('aria-invalid', 'true');
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(resolveRepository).toHaveBeenCalledWith(
    {
      type: 'git_repository',
      url: 'https://github.com/example/research.git',
      reference: 'release',
    },
    expect.objectContaining({ signal: expect.any(AbortSignal) }),
  );
  await actor.click(screen.getByRole('button', { name: 'Add command' }));
  fireEvent.change(screen.getByLabelText('Setup command 1'), {
    target: { value: 'mkdir -p reports' },
  });
  await actor.click(screen.getByRole('button', { name: 'Add command' }));
  fireEvent.change(screen.getByLabelText('Setup command 2'), { target: { value: 'pwd' } });
  await actor.click(screen.getByRole('button', { name: 'Move setup command 2 up' }));
  await actor.click(await screen.findByRole('button', { name: 'Add Research agent' }));
  await actor.click(screen.getByRole('button', { name: 'Save draft' }));
  await screen.findByText('Draft saved.');
  expect(createProfile.mock.calls[0]?.[0].content).toEqual({
    location: {
      type: 'git_repository',
      url: 'https://github.com/example/research.git',
      reference: 'release',
    },
    name: 'research',
    setupCommands: ['pwd', 'mkdir -p reports'],
    agentProfiles: [{ id: 'agp_research', revision: 1 }],
  });
});

it('invalidates repository verification when the branch changes and validates directory names live', async () => {
  const actor = userEvent.setup();
  const createProfile = vi.fn();
  render(
    <WorkspaceProfileEditor
      client={documentClient(WORKSPACE_PROFILE, { createProfile })}
      profiles={OWN_PROFILES}
      marketplace={marketplaceClient()}
      resolver={{ resolveRepository: async (input) => input }}
    />,
  );
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Checkout' } });
  fireEvent.change(screen.getByLabelText('Repository URL'), {
    target: { value: 'https://github.com/example/research' },
  });
  await actor.click(screen.getByRole('button', { name: 'Verify repository' }));
  await screen.findByText('Repository verified');
  fireEvent.change(screen.getByLabelText('Branch or tag'), { target: { value: 'next' } });
  expect(screen.getByText('Repository verified')).toHaveAttribute('aria-hidden', 'true');
  await waitFor(() => expect(screen.queryByText('Repository verified')).not.toBeInTheDocument());
  fireEvent.change(screen.getByLabelText('Directory name'), { target: { value: '../escape' } });
  expect(screen.getByLabelText('Directory name')).toHaveAttribute('aria-invalid', 'true');
  await actor.click(screen.getByRole('button', { name: 'Save draft' }));
  expect(createProfile).not.toHaveBeenCalled();
  expect(screen.getByRole('alert')).toHaveTextContent('Review 2 fields');
});

it('creates an empty directory without requiring repository verification', async () => {
  const actor = userEvent.setup();
  const createProfile = vi.fn(documentClient(WORKSPACE_PROFILE).createProfile);
  render(
    <WorkspaceProfileEditor
      client={documentClient(WORKSPACE_PROFILE, { createProfile })}
      profiles={OWN_PROFILES}
      marketplace={marketplaceClient()}
    />,
  );
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Empty research' } });
  await actor.click(screen.getByRole('radio', { name: 'Empty directory' }));
  expect(screen.getByLabelText('Directory name')).toHaveValue('workspace');
  fireEvent.change(screen.getByLabelText('Empty directory name'), { target: { value: 'notes' } });
  expect(screen.getByLabelText('Directory name')).toHaveValue('notes');
  await actor.click(screen.getByRole('button', { name: 'Save draft' }));
  await screen.findByText('Draft saved.');
  expect(createProfile.mock.calls[0]?.[0].content.location).toEqual({
    type: 'directory',
    name: 'notes',
  });
});
