import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import {
  documentClient,
  marketplaceClient,
  OWN_PROFILES,
  PROFILE_BOUNDS,
  SANDBOX_PROFILE,
  WORKSPACE_PROFILE,
} from '../test/profiles';
import { SandboxProfileEditor } from './SandboxProfileEditor';
import { WorkspaceProfileEditor } from './WorkspaceProfileEditor';

describe.each(['workspace', 'sandbox'] as const)('%s profile transactions', (kind) => {
  const record = kind === 'workspace' ? WORKSPACE_PROFILE : SANDBOX_PROFILE;
  const conflict = Object.assign(new Error('Conflict'), {
    status: 409,
    code: 'profile_version_conflict',
  });

  it.each(['Retry save', 'Load current'])(
    'preserves the draft until the owner chooses %s',
    async (action) => {
      const actor = userEvent.setup();
      const readProfile = vi
        .fn()
        .mockResolvedValueOnce(record)
        .mockResolvedValue({ ...record, name: 'Current profile', draftVersion: 4 });
      const saveProfile = vi
        .fn()
        .mockRejectedValueOnce(conflict)
        .mockImplementation(async (_id, input) => ({
          ...record,
          name: input.name,
          draft: input.content,
          draftVersion: 5,
        }));
      render(
        kind === 'workspace' ? (
          <WorkspaceProfileEditor
            profileId={record.id}
            client={documentClient(WORKSPACE_PROFILE, { readProfile, saveProfile })}
            profiles={OWN_PROFILES}
            marketplace={marketplaceClient()}
          />
        ) : (
          <SandboxProfileEditor
            profileId={record.id}
            client={documentClient(SANDBOX_PROFILE, { readProfile, saveProfile })}
            profiles={OWN_PROFILES}
            marketplace={marketplaceClient()}
            bounds={PROFILE_BOUNDS}
          />
        ),
      );
      await screen.findByRole('heading', { name: record.name });
      fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'My draft' } });
      await actor.click(screen.getByRole('button', { name: 'Save draft' }));
      await screen.findByText('The saved profile changed. Your draft is still here.');
      expect(screen.getByLabelText('Name')).toHaveValue('My draft');
      await actor.click(screen.getByRole('button', { name: action }));
      if (action === 'Retry save') {
        await screen.findByText('Draft saved.');
        expect(saveProfile.mock.calls[1]?.[1]).toEqual(
          expect.objectContaining({ name: 'My draft', draftVersion: 4 }),
        );
        expect(saveProfile.mock.calls[1]?.[2].idempotencyKey).not.toBe(
          saveProfile.mock.calls[0]?.[2].idempotencyKey,
        );
      } else expect(screen.getByLabelText('Name')).toHaveValue('Current profile');
    },
  );

  it('confirms publication and explains the public-visibility rule', async () => {
    const actor = userEvent.setup();
    const readProfile = vi.fn().mockResolvedValue({ ...record, latestRevision: undefined });
    const publishProfile = vi.fn().mockResolvedValue({
      profileId: record.id,
      revision: 1,
      digest: 'published-content-digest',
      publishedAt: '2026-09-06T00:01:00Z',
      content: record.draft,
    });
    render(
      kind === 'workspace' ? (
        <WorkspaceProfileEditor
          profileId={record.id}
          client={documentClient(WORKSPACE_PROFILE, { readProfile, publishProfile })}
          profiles={OWN_PROFILES}
          marketplace={marketplaceClient()}
        />
      ) : (
        <SandboxProfileEditor
          profileId={record.id}
          client={documentClient(SANDBOX_PROFILE, { readProfile, publishProfile })}
          profiles={OWN_PROFILES}
          marketplace={marketplaceClient()}
          bounds={PROFILE_BOUNDS}
        />
      ),
    );
    await screen.findByRole('heading', { name: record.name });
    await actor.click(screen.getByRole('button', { name: 'Visibility' }));
    expect(screen.getByRole('menuitemradio', { name: /Public/ })).toHaveAttribute(
      'aria-disabled',
      'true',
    );
    await actor.keyboard('{Escape}');
    await actor.click(screen.getByRole('button', { name: 'Publish revision' }));
    const dialog = screen.getByRole('dialog', { name: 'Publish revision 1' });
    expect(publishProfile).not.toHaveBeenCalled();
    await actor.click(within(dialog).getByRole('button', { name: 'Publish revision 1' }));
    await screen.findByText('Revision 1 published.');
    expect(publishProfile).toHaveBeenCalledWith(
      record.id,
      { draftVersion: 3 },
      expect.objectContaining({ idempotencyKey: expect.any(String) }),
    );
  });
});
