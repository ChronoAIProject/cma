import { describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { NewSessionDialog } from './NewSessionDialog';
import { createMockProfileStore } from '../environment/mock';
import type { EnvironmentProfile, EnvironmentProfileStore } from '../environment/types';
import { DEFAULT_AGENT } from '../environment/types';
import type { KernelOffer } from './types';
import { chooseOption, openedOptions } from '../test/select';

/**
 * The dialog makes two decisions and both are fixed for the session's life:
 * which profile describes it, and which plane it is driven on.
 *
 * The plane cannot be chosen afterwards — moving between them is a rotation —
 * so what matters here is that it is offered where it can be chosen, that a
 * runtime which cannot serve it says so before anything is created, and that
 * the profile authored on the way is a real, reusable one.
 */

const BOTH: readonly KernelOffer[] = [
  { id: 'codex', modes: ['terminal', 'conversation'] },
  { id: 'claude-code', modes: ['conversation'] },
];

function profile(id: string, name: string, agent = DEFAULT_AGENT): EnvironmentProfile {
  return {
    id,
    name,
    agent,
    nyxid: { services: [] },
    variables: [],
    packages: [],
    mandatory: { packages: [], skills: [] },
    createdAt: '2026-08-14T00:00:00Z',
    updatedAt: '2026-08-14T00:00:00Z',
  };
}

interface DialogOptions {
  readonly profiles?: readonly EnvironmentProfile[];
  readonly kernels?: readonly KernelOffer[];
  readonly store?: EnvironmentProfileStore;
}

function renderDialog(options: DialogOptions = {}) {
  const onStart = vi.fn();
  const onCancel = vi.fn();
  const onProfileCreated = vi.fn();
  const store = options.store ?? createMockProfileStore();
  render(
    <NewSessionDialog
      open
      onCancel={onCancel}
      onStart={onStart}
      onProfileCreated={onProfileCreated}
      profiles={options.profiles ?? []}
      kernels={options.kernels ?? BOTH}
      store={store}
    />,
  );
  return { onStart, onCancel, onProfileCreated, store };
}

/** Open the authoring branch, which is where every field below lives. */
async function author(user: ReturnType<typeof userEvent.setup>) {
  await chooseOption(user, /environment profile/i, /new profile/i);
  // §7: the plane choice is two labelled cards in a radiogroup, not a select.
  await screen.findByRole('radiogroup', { name: /^plane$/i });
}

describe('starting from what already exists', () => {
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
    const { onStart } = renderDialog({ profiles: [profile('envp_1', 'Talos Controller')] });

    await chooseOption(user, /environment profile/i, 'Talos Controller');
    await user.click(screen.getByRole('button', { name: /start session/i }));

    expect(onStart).toHaveBeenCalledWith('envp_1');
  });

  it('offers every profile the person owns, and the option to author one', async () => {
    const user = userEvent.setup();
    renderDialog({ profiles: [profile('envp_1', 'Talos Controller'), profile('envp_2', 'Rev')] });

    expect(await openedOptions(user, /environment profile/i)).toEqual([
      "This deployment's defaults",
      'Talos Controller',
      'Rev',
      'New profile…',
    ]);
  });

  it('says what a chosen profile starts', async () => {
    // The plane is fixed at creation, so somebody choosing a profile is
    // choosing a surface. Being told after the session exists is being told
    // too late to decide anything.
    const user = userEvent.setup();
    renderDialog({
      profiles: [
        profile('envp_1', 'Reviewer', {
          ...DEFAULT_AGENT,
          kernel: 'claude-code',
          mode: 'conversation',
        }),
      ],
    });

    await chooseOption(user, /environment profile/i, 'Reviewer');

    expect(screen.getByText(/starts a conversation session on Claude Code/i)).toBeInTheDocument();
  });

  it('asks nothing about the agent until somebody authors a profile', () => {
    // The same setting editable in two places is a setting that can disagree
    // with itself. These live on the profile, and the dialog offers them only
    // where it is authoring one.
    renderDialog({ profiles: [profile('envp_1', 'Talos Controller')] });

    expect(screen.queryByRole('button', { name: /^agent$/i })).toBeNull();
    expect(screen.queryByRole('radiogroup', { name: /^plane$/i })).toBeNull();
    expect(screen.queryByRole('button', { name: /thinking effort/i })).toBeNull();
  });
});

describe('authoring a scoped profile in place', () => {
  it('creates a service-scoped conversation session in one dialog', async () => {
    // The flagship case: a person who has never authored a profile starts a
    // GitHub-scoped conversation session without leaving this dialog, and the
    // profile it produced is a real one they can start again.
    const user = userEvent.setup();
    const { onStart, onProfileCreated, store } = renderDialog();

    await author(user);
    await user.type(screen.getByLabelText(/^name$/i), 'GitHub reviewer');
    await user.click(screen.getByRole('radio', { name: /conversation/i }));
    await user.click(await screen.findByRole('checkbox', { name: /GitHub/ }));
    await user.click(screen.getByRole('button', { name: /start session/i }));

    await waitFor(() => {
      expect(onStart).toHaveBeenCalled();
    });
    const created = onProfileCreated.mock.calls[0]?.[0] as EnvironmentProfile;
    expect(created.name).toBe('GitHub reviewer');
    expect(created.agent.mode).toBe('conversation');
    expect(created.nyxid.services).toContain('api-github');
    expect(onStart).toHaveBeenCalledWith(created.id);
    // Reusable afterwards, which is the difference between authoring a profile
    // and filling in a form that vanishes.
    await expect(store.list()).resolves.toEqual([created]);
  });

  it('refuses a plane the chosen runtime cannot serve, by name, before anything is created', async () => {
    const user = userEvent.setup();
    const { onStart, store } = renderDialog();
    const create = vi.spyOn(store, 'create');

    await author(user);
    await user.type(screen.getByLabelText(/^name$/i), 'Screen work');
    // A new profile starts on the conversation plane, which Claude Code
    // serves, so the terminal has to be asked for to provoke the refusal.
    await user.click(screen.getByRole('radio', { name: /terminal/i }));
    await chooseOption(user, /^agent$/i, 'Claude Code');

    // Claude Code does not serve a terminal.
    expect(screen.getByRole('alert')).toHaveTextContent(/does not advertise “terminal”/i);
    expect(screen.getByRole('button', { name: /start session/i })).toBeDisabled();
    expect(create).not.toHaveBeenCalled();
    expect(onStart).not.toHaveBeenCalled();
  });

  it('shows a plane the runtime cannot serve as refused rather than hiding it', async () => {
    // §7: controls are disabled, never removed. A plane that silently vanished
    // would read as a plane this product does not have.
    const user = userEvent.setup();
    renderDialog();

    await author(user);
    await chooseOption(user, /^agent$/i, 'Claude Code');

    const terminal = screen.getByRole('radio', { name: /terminal/i });
    expect(terminal).toBeDisabled();
    expect(terminal.textContent).toContain('Not on Claude Code');
    expect(screen.getByRole('radio', { name: /conversation/i })).toBeEnabled();
  });

  it('will not author a profile when the deployment’s runtimes are unknown', async () => {
    // Choosing a runtime blind is how somebody fills a form in and then learns
    // the control plane refuses the pairing.
    const user = userEvent.setup();
    renderDialog({ kernels: [] });

    const options = await openedOptions(user, /environment profile/i);

    expect(options).toContain('New profile…This deployment’s runtimes could not be read');
  });

  it('needs a name before it saves anything', async () => {
    const user = userEvent.setup();
    const { onStart } = renderDialog();

    await author(user);
    await user.click(screen.getByRole('button', { name: /start session/i }));

    expect(screen.getByRole('alert')).toHaveTextContent(/give the profile a name/i);
    expect(onStart).not.toHaveBeenCalled();
  });
});

describe('when the deployment cannot answer', () => {
  it('still creates a session where there is no service catalogue', async () => {
    // Answered as the caller, so a deployment without one has no picker. The
    // profile still saves, and what this deployment mandates is added to it.
    const user = userEvent.setup();
    const store = createMockProfileStore({ failServicesWith: 'no catalogue here' });
    const { onStart } = renderDialog({ store });

    await author(user);
    await user.type(screen.getByLabelText(/^name$/i), 'Unscoped');
    await user.click(screen.getByRole('button', { name: /start session/i }));

    await waitFor(() => {
      expect(onStart).toHaveBeenCalled();
    });
    expect(screen.getByText(/no catalogue here/i)).toBeInTheDocument();
  });

  it('keeps the form when the profile cannot be saved', async () => {
    // Closing on a failure would lose the name and the services somebody just
    // chose, which is the detour this form exists to remove.
    const user = userEvent.setup();
    // Only the write fails: the catalogue answering normally is what puts a
    // second alert on the surface, and the one that must be read is this one.
    const store: EnvironmentProfileStore = {
      ...createMockProfileStore(),
      create: () => Promise.reject(new Error('the control plane refused')),
    };
    const { onStart } = renderDialog({ store });

    await author(user);
    await user.type(screen.getByLabelText(/^name$/i), 'Doomed');
    await user.click(screen.getByRole('button', { name: /start session/i }));

    await waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent(/the control plane refused/i);
    });
    expect(onStart).not.toHaveBeenCalled();
    expect(screen.getByLabelText(/^name$/i)).toHaveValue('Doomed');
  });
});
