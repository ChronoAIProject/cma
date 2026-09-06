import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Component, type ReactNode } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { AGENT_PROFILE, profileClient, SKILL_CATALOG } from '../test/profiles';
import { AgentProfileEditor } from './AgentProfileEditor';
import { ApiError } from '@chronoai/cma-sdk';

function problem(code: string, status = 409) {
  return Object.assign(new Error('Server refused the operation.'), { code, status });
}

class SaveErrorBoundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  render() {
    return this.state.failed ? <p>Navigation unavailable</p> : this.props.children;
  }
}

describe('Agent profile editing', () => {
  it('creates a private draft and reports its new profile identity', async () => {
    const actor = userEvent.setup();
    const createProfile = vi.fn(profileClient().createProfile);
    const onSaved = vi.fn();
    render(
      <AgentProfileEditor
        client={profileClient({ createProfile })}
        catalog={SKILL_CATALOG}
        onSaved={onSaved}
      />,
    );
    await actor.type(screen.getByLabelText('Name'), 'New research agent');
    await actor.click(screen.getByRole('button', { name: 'Save draft' }));
    await screen.findByText('Draft saved.');
    expect(createProfile).toHaveBeenCalledWith(
      expect.objectContaining({
        name: 'New research agent',
        content: expect.objectContaining({ permissionLevel: 'guarded' }),
      }),
      expect.objectContaining({ idempotencyKey: expect.any(String) }),
    );
    expect(onSaved).toHaveBeenCalledWith(
      expect.objectContaining({ id: AGENT_PROFILE.id, draftVersion: 1, visibility: 'private' }),
    );
  });
  it('reports every invalid field and keeps system prompt text uninterpreted', async () => {
    const createProfile = vi.fn();
    render(
      <AgentProfileEditor client={profileClient({ createProfile })} catalog={SKILL_CATALOG} />,
    );
    fireEvent.change(screen.getByLabelText('Description'), { target: { value: 'a'.repeat(2001) } });
    fireEvent.change(screen.getByLabelText('Model'), { target: { value: ' ' } });
    fireEvent.change(screen.getByLabelText('System prompt'), {
      target: { value: '# Plain text\n' + 'a'.repeat(32768) },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save draft' }));
    expect(screen.getByRole('alert')).toHaveTextContent('Review 4 fields');
    expect(screen.getByLabelText('Name')).toHaveAttribute('aria-invalid', 'true');
    expect(screen.getByLabelText('Name')).toHaveFocus();
    expect(screen.queryByRole('heading', { name: 'Plain text' })).not.toBeInTheDocument();
    expect(createProfile).not.toHaveBeenCalled();
  });

  it('does not reinterpret a successful write as uncertain when its consumer callback fails', async () => {
    const logged = vi.spyOn(console, 'error').mockImplementation(() => {});
    const createProfile = vi.fn(profileClient().createProfile);
    try {
      render(
        <SaveErrorBoundary>
          <AgentProfileEditor
            client={profileClient({ createProfile })}
            catalog={SKILL_CATALOG}
            onSaved={() => {
              throw new Error('Navigation failed');
            }}
          />
        </SaveErrorBoundary>,
      );
      fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Saved agent' } });
      fireEvent.click(screen.getByRole('button', { name: 'Save draft' }));
      await screen.findByText('Navigation unavailable');
      expect(createProfile).toHaveBeenCalledTimes(1);
      expect(screen.queryByRole('button', { name: 'Retry' })).not.toBeInTheDocument();
    } finally {
      logged.mockRestore();
    }
  });

  it('saves selected model, effort, permission and pinned skill versions through the public client', async () => {
    const actor = userEvent.setup();
    const saveProfile = vi.fn(profileClient().saveProfile);
    render(
      <AgentProfileEditor
        profileId={AGENT_PROFILE.id}
        client={profileClient({ saveProfile })}
        catalog={SKILL_CATALOG}
      />,
    );
    await screen.findByRole('heading', { name: AGENT_PROFILE.name });
    await actor.click(await screen.findByRole('button', { name: 'Add Research' }));
    expect(screen.getByRole('button', { name: 'Version for research' })).toHaveTextContent('1.2');
    await actor.click(screen.getByRole('button', { name: 'Version for research' }));
    await actor.click(screen.getByRole('menuitemradio', { name: '1.1' }));
    await actor.click(screen.getByRole('radio', { name: 'Very high' }));
    await actor.click(screen.getByRole('radio', { name: 'Autonomous' }));
    await actor.click(screen.getByRole('button', { name: 'Save draft' }));
    await screen.findByText('Draft saved.');
    expect(saveProfile).toHaveBeenCalledWith(
      AGENT_PROFILE.id,
      expect.objectContaining({
        draftVersion: 3,
        content: expect.objectContaining({
          thinkingEffort: 'x_high',
          permissionLevel: 'autonomous',
          skills: [{ source: 'ornn', id: 'research', version: '1.1' }],
        }),
      }),
      expect.objectContaining({ idempotencyKey: expect.any(String) }),
    );
  });

  it('keeps a conflicting draft until the owner explicitly retries against the current version', async () => {
    const actor = userEvent.setup();
    const readProfile = vi
      .fn()
      .mockResolvedValueOnce(AGENT_PROFILE)
      .mockResolvedValue({ ...AGENT_PROFILE, name: 'Another saved name', draftVersion: 4 });
    const saveProfile = vi
      .fn()
      .mockRejectedValueOnce(problem('profile_version_conflict'))
      .mockImplementation(profileClient().saveProfile);
    render(
      <AgentProfileEditor
        profileId={AGENT_PROFILE.id}
        client={profileClient({ readProfile, saveProfile })}
        catalog={SKILL_CATALOG}
      />,
    );
    await screen.findByRole('heading', { name: AGENT_PROFILE.name });
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'My draft' } });
    await actor.click(screen.getByRole('button', { name: 'Save draft' }));
    expect(
      await screen.findByText('The saved profile changed. Your draft is still here.'),
    ).toBeInTheDocument();
    expect(screen.getByLabelText('Name')).toHaveValue('My draft');
    await actor.click(screen.getByRole('button', { name: 'Retry save' }));
    await screen.findByText('Draft saved.');
    expect(saveProfile.mock.calls[1]?.[1]).toEqual(
      expect.objectContaining({ name: 'My draft', draftVersion: 4 }),
    );
    expect(saveProfile.mock.calls[1]?.[2].idempotencyKey).not.toBe(
      saveProfile.mock.calls[0]?.[2].idempotencyKey,
    );
  });

  it('loads the current profile only when the owner chooses Load current', async () => {
    const actor = userEvent.setup();
    const readProfile = vi
      .fn()
      .mockResolvedValueOnce(AGENT_PROFILE)
      .mockResolvedValue({ ...AGENT_PROFILE, name: 'Current saved name', draftVersion: 4 });
    render(
      <AgentProfileEditor
        profileId={AGENT_PROFILE.id}
        client={profileClient({
          readProfile,
          saveProfile: vi.fn().mockRejectedValue(problem('profile_version_conflict')),
        })}
        catalog={SKILL_CATALOG}
      />,
    );
    await screen.findByRole('heading', { name: AGENT_PROFILE.name });
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'My draft' } });
    await actor.click(screen.getByRole('button', { name: 'Save draft' }));
    await actor.click(await screen.findByRole('button', { name: 'Load current' }));
    expect(screen.getByLabelText('Name')).toHaveValue('Current saved name');
  });

  it.each(['network', 'idempotency_in_flight', 'idempotency_recovery_required'])(
    'retains a %s write and key before permitting further edits',
    async (code) => {
      const actor = userEvent.setup();
      const saveProfile = vi
        .fn()
        .mockRejectedValueOnce(
          code === 'network'
            ? new Error('Connection lost')
            : new ApiError(409, 'The command has not settled.', code),
        )
        .mockImplementation(profileClient().saveProfile);
      render(
        <AgentProfileEditor
          profileId={AGENT_PROFILE.id}
          client={profileClient({ saveProfile })}
          catalog={SKILL_CATALOG}
        />,
      );
      await screen.findByRole('heading', { name: AGENT_PROFILE.name });
      fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Retain this request' } });
      await actor.click(screen.getByRole('button', { name: 'Save draft' }));
      await screen.findByRole('button', { name: 'Retry' });
      expect(screen.getByLabelText('Name')).toBeDisabled();
      await actor.click(screen.getByRole('button', { name: 'Retry' }));
      await screen.findByText('Draft saved.');
      expect(saveProfile.mock.calls[1]?.[1]).toEqual(saveProfile.mock.calls[0]?.[1]);
      expect(saveProfile.mock.calls[1]?.[2].idempotencyKey).toBe(
        saveProfile.mock.calls[0]?.[2].idempotencyKey,
      );
      expect(screen.getByLabelText('Name')).toBeEnabled();
    },
  );

  it('locks conflict recovery while rereading and aborts the read on unmount', async () => {
    const actor = userEvent.setup();
    const readProfile = vi
      .fn()
      .mockResolvedValueOnce(AGENT_PROFILE)
      .mockRejectedValueOnce(new Error('Read unavailable'))
      .mockImplementation(() => new Promise(() => {}));
    const view = render(
      <AgentProfileEditor
        profileId={AGENT_PROFILE.id}
        client={profileClient({
          readProfile,
          saveProfile: vi.fn().mockRejectedValue(problem('profile_version_conflict')),
        })}
        catalog={SKILL_CATALOG}
      />,
    );
    await screen.findByRole('heading', { name: AGENT_PROFILE.name });
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Retained draft' } });
    await actor.click(screen.getByRole('button', { name: 'Save draft' }));
    await actor.click(await screen.findByRole('button', { name: 'Load current' }));
    await waitFor(() => expect(readProfile).toHaveBeenCalledTimes(3));
    expect(screen.getByLabelText('Name')).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Load current' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Retry save' })).toBeDisabled();
    const signal = readProfile.mock.calls[2]?.[1].signal as AbortSignal;
    expect(signal.aborted).toBe(false);
    view.unmount();
    expect(signal.aborted).toBe(true);
  });
});

describe('Agent profile publication and sharing', () => {
  it('names the immutable revision in a confirmation before publishing', async () => {
    const actor = userEvent.setup();
    const publishProfile = vi.fn(profileClient().publishProfile);
    render(
      <AgentProfileEditor
        profileId={AGENT_PROFILE.id}
        client={profileClient({ publishProfile })}
        catalog={SKILL_CATALOG}
      />,
    );
    await screen.findByRole('heading', { name: AGENT_PROFILE.name });
    await actor.click(screen.getByRole('button', { name: 'Publish revision' }));
    const dialog = screen.getByRole('dialog', { name: 'Publish revision 2' });
    expect(publishProfile).not.toHaveBeenCalled();
    await actor.click(within(dialog).getByRole('button', { name: 'Publish revision 2' }));
    await screen.findByText('Revision 2 published.');
    expect(publishProfile).toHaveBeenCalledWith(
      AGENT_PROFILE.id,
      { draftVersion: 3 },
      expect.objectContaining({ idempotencyKey: expect.any(String) }),
    );
  });

  it('explains unavailable public visibility before the first publication', async () => {
    const actor = userEvent.setup();
    render(
      <AgentProfileEditor
        profileId={AGENT_PROFILE.id}
        client={profileClient({
          readProfile: async () => ({ ...AGENT_PROFILE, latestRevision: undefined }),
        })}
        catalog={SKILL_CATALOG}
      />,
    );
    await screen.findByRole('heading', { name: AGENT_PROFILE.name });
    await actor.click(screen.getByRole('button', { name: 'Visibility' }));
    expect(screen.getByRole('menuitemradio', { name: /Public/ })).toHaveAttribute(
      'aria-disabled',
      'true',
    );
    expect(screen.getByText('Publish a revision first.')).toBeInTheDocument();
  });

  it('sets published visibility and retires only after confirmation', async () => {
    const actor = userEvent.setup();
    const setProfileVisibility = vi.fn(profileClient().setProfileVisibility);
    const retireProfile = vi.fn(profileClient().retireProfile);
    render(
      <AgentProfileEditor
        profileId={AGENT_PROFILE.id}
        client={profileClient({ setProfileVisibility, retireProfile })}
        catalog={SKILL_CATALOG}
      />,
    );
    await screen.findByRole('heading', { name: AGENT_PROFILE.name });
    await actor.click(screen.getByRole('button', { name: 'Visibility' }));
    await actor.click(screen.getByRole('menuitemradio', { name: 'Public' }));
    await screen.findByText('Sharing updated.');
    expect(setProfileVisibility).toHaveBeenCalledWith(
      AGENT_PROFILE.id,
      { visibility: 'public' },
      expect.objectContaining({ idempotencyKey: expect.any(String) }),
    );
    await actor.click(screen.getByRole('button', { name: 'Retire' }));
    expect(retireProfile).not.toHaveBeenCalled();
    await actor.click(screen.getByRole('button', { name: 'Retire' }));
    await screen.findByText('Profile retired.');
    await waitFor(() => expect(screen.getByLabelText('Name')).toBeDisabled());
  });
});
