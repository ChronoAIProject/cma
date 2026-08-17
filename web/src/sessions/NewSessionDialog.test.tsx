import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { NewSessionDialog } from './NewSessionDialog';
import type { EnvironmentProfile } from '../environment/types';
import { DEFAULT_AGENT } from '../environment/types';

/**
 * The dialog now makes one choice: which profile the session is started from.
 *
 * What the agent may do unattended is decided in the profile, so the cases
 * that matter here are that a session can always be started without choosing
 * one, and that choosing one is carried through exactly.
 */

function profile(id: string, name: string): EnvironmentProfile {
  return {
    id,
    name,
    agent: DEFAULT_AGENT,
    nyxid: { services: [] },
    variables: [],
    packages: [],
    mandatory: { packages: [], skills: [] },
    createdAt: '2026-08-14T00:00:00Z',
    updatedAt: '2026-08-14T00:00:00Z',
  };
}

function renderDialog(profiles: readonly EnvironmentProfile[] = [], onStart = vi.fn()) {
  const onCancel = vi.fn();
  render(<NewSessionDialog open onCancel={onCancel} onStart={onStart} profiles={profiles} />);
  return { onStart, onCancel };
}

describe('the new session dialog', () => {
  it('starts from this deployment’s defaults when no profile is chosen', async () => {
    // Naming nothing is a legitimate choice and must not require a profile to
    // exist first: the control plane still has an answer for how to start.
    const user = userEvent.setup();
    const { onStart } = renderDialog();

    await user.click(screen.getByRole('button', { name: /start session/i }));

    expect(onStart).toHaveBeenCalledWith(null);
  });

  it('carries the chosen profile through', async () => {
    const user = userEvent.setup();
    const { onStart } = renderDialog([profile('envp_1', 'Talos Controller')]);

    await user.selectOptions(
      screen.getByRole('combobox', { name: /environment profile/i }),
      'envp_1',
    );
    await user.click(screen.getByRole('button', { name: /start session/i }));

    expect(onStart).toHaveBeenCalledWith('envp_1');
  });

  it('offers every profile the person owns', () => {
    renderDialog([profile('envp_1', 'Talos Controller'), profile('envp_2', 'Reviewer')]);

    expect(screen.getByRole('option', { name: 'Talos Controller' })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: 'Reviewer' })).toBeInTheDocument();
  });

  it('no longer asks about the agent, which the profile decides', () => {
    // The same setting editable in two places is a setting that can disagree
    // with itself. These moved into the profile and must not reappear here.
    renderDialog([profile('envp_1', 'Talos Controller')]);

    expect(screen.queryByRole('combobox', { name: /^agent$/i })).toBeNull();
    expect(screen.queryByRole('combobox', { name: /thinking effort/i })).toBeNull();
    expect(screen.queryByRole('combobox', { name: /permissions/i })).toBeNull();
    expect(screen.queryByRole('combobox', { name: /model access/i })).toBeNull();
  });
});
