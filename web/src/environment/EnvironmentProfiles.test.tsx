import { describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { EnvironmentProfiles } from './EnvironmentProfiles';
import { createMockProfileStore } from './mock';
import { emptyDraft } from './types';
import { chooseOption, chosenOption } from '../test/select';
import type { KernelOffer } from '../sessions/types';

/** What this deployment runs, in the tests that care which runtime does what. */
const KERNELS: readonly KernelOffer[] = [
  { id: 'codex', modes: ['terminal', 'conversation'] },
  { id: 'claude-code', modes: ['conversation'] },
];

function open(store = createMockProfileStore(), onClose = vi.fn(), kernels = KERNELS) {
  return {
    onClose,
    store,
    ...render(<EnvironmentProfiles store={store} onClose={onClose} kernels={kernels} />),
  };
}

describe('EnvironmentProfiles', () => {
  it('says what a profile is for when there are none', async () => {
    open();
    expect(await screen.findByText(/no profiles yet/i)).toBeInTheDocument();
  });

  it('lists what each profile carries', async () => {
    open(
      createMockProfileStore({
        initial: [
          {
            ...emptyDraft(),
            name: 'build tools',
            variables: [{ key: 'BUILD_MODE', value: 'release' }],
          },
        ],
      }),
    );

    expect(await screen.findByText('build tools')).toBeInTheDocument();
    // Counts include what the platform mandates, because that is what the
    // session will actually carry.
    expect(screen.getByText(/1 variable · 1 package · 2 skills/)).toBeInTheDocument();
  });

  it('creates a profile with a variable and a skill', async () => {
    const user = userEvent.setup();
    const { store } = open();

    await user.click(await screen.findByRole('button', { name: /new profile/i }));
    await user.type(screen.getByLabelText(/^name$/i), 'build tools');
    await user.click(screen.getByRole('button', { name: /add variable/i }));
    await user.type(screen.getByLabelText(/variable 1 name/i), 'BUILD_MODE');
    await user.type(screen.getByLabelText(/variable 1 value/i), 'release');
    await user.click(await screen.findByRole('checkbox', { name: /aevatar-platform-map/i }));
    await user.click(screen.getByRole('button', { name: /save profile/i }));

    await waitFor(async () => {
      expect(await store.list()).toHaveLength(1);
    });
    const [saved] = await store.list();
    expect(saved?.variables).toEqual([{ key: 'BUILD_MODE', value: 'release' }]);
    expect(saved?.agent.skills).toContain('aevatar-platform-map');
    // Back to the list, which is where the user started.
    expect(await screen.findByText('build tools')).toBeInTheDocument();
  });

  it('splits the form into what the agent is and what surrounds it', async () => {
    // The two questions a profile answers. Kept apart because the settings in
    // the first half were once chosen again on every session start.
    const user = userEvent.setup();
    open();

    await user.click(await screen.findByRole('button', { name: /new profile/i }));

    expect(screen.getByRole('heading', { name: /agent configuration/i })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: /environment configuration/i })).toBeInTheDocument();
    for (const label of [
      /^agent$/i,
      /^plane$/i,
      /^model$/i,
      /thinking effort/i,
      /permissions/i,
      /model access/i,
    ]) {
      expect(screen.getByLabelText(label)).toBeInTheDocument();
    }
  });

  it('saves the agent settings the profile was given', async () => {
    const user = userEvent.setup();
    const { store } = open();

    await user.click(await screen.findByRole('button', { name: /new profile/i }));
    await user.type(screen.getByLabelText(/^name$/i), 'unattended');
    await chooseOption(user, /thinking effort/i, /minimal/i);
    await chooseOption(user, /permissions/i, /work unattended/i);
    await user.type(screen.getByLabelText(/^model$/i), 'a-specific-model');
    await user.click(screen.getByRole('button', { name: /save profile/i }));

    await waitFor(async () => {
      expect(await store.list()).toHaveLength(1);
    });
    const [saved] = await store.list();
    expect(saved?.agent.reasoningEffort).toBe('minimal');
    expect(saved?.agent.permission).toBe('autonomous');
    expect(saved?.agent.model).toBe('a-specific-model');
  });

  it('starts a new profile where an unqualified session would', async () => {
    // A profile someone names and saves without touching anything must behave
    // the way this deployment already does, not arrive at no value at all.
    const user = userEvent.setup();
    open();

    await user.click(await screen.findByRole('button', { name: /new profile/i }));

    expect(chosenOption(/thinking effort/i)).toMatch(/high/i);
    expect(chosenOption(/permissions/i)).toMatch(/ask first/i);
    expect(chosenOption(/model access/i)).toMatch(/chronoai/i);
  });

  it('lets a profile widen what a session may reach', async () => {
    const user = userEvent.setup();
    const { store } = open();

    await user.click(await screen.findByRole('button', { name: /new profile/i }));
    await user.type(screen.getByLabelText(/^name$/i), 'with storage');
    await user.click(await screen.findByRole('checkbox', { name: /chrono-storage-service/i }));
    await user.click(screen.getByRole('button', { name: /save profile/i }));

    await waitFor(async () => {
      expect(await store.list()).toHaveLength(1);
    });
    const [saved] = await store.list();
    expect(saved?.nyxid.services).toContain('chrono-storage-service');
  });

  it('will not let the session lose the services it cannot work without', async () => {
    // One credential serves the agent's own model calls as well as everything
    // else, so unticking the model endpoint would not narrow the session — it
    // would stop it thinking. Shown, and fixed.
    const user = userEvent.setup();
    open();

    await user.click(await screen.findByRole('button', { name: /new profile/i }));

    const required = await screen.findByRole('checkbox', { name: /chrono-llm/i });
    expect(required).toBeChecked();
    expect(required).toBeDisabled();
    expect(await screen.findByRole('checkbox', { name: /ornn-api/i })).toBeDisabled();
  });

  it('saves a profile even when the services cannot be listed', async () => {
    // The list is a convenience. Losing it must cost the choice, not the form:
    // a session still reaches what the deployment mandates.
    const user = userEvent.setup();
    const store = createMockProfileStore();
    store.services = () => Promise.reject(new Error('platform is down'));
    open(store);

    await user.click(await screen.findByRole('button', { name: /new profile/i }));
    await user.type(screen.getByLabelText(/^name$/i), 'still saves');
    await user.click(screen.getByRole('button', { name: /save profile/i }));

    await waitFor(async () => {
      expect(await store.list()).toHaveLength(1);
    });
  });

  it('masks a variable value until the user reveals it', async () => {
    const user = userEvent.setup();
    open();

    await user.click(await screen.findByRole('button', { name: /new profile/i }));
    await user.click(screen.getByRole('button', { name: /add variable/i }));
    const value = screen.getByLabelText(/variable 1 value/i);

    // A value field is where a user puts a token, so it is not on screen by
    // default — see docs/Web_Style.md section 6.
    expect(value).toHaveAttribute('type', 'password');
    await user.click(screen.getByRole('button', { name: /show value of variable 1/i }));
    expect(value).toHaveAttribute('type', 'text');
    await user.click(screen.getByRole('button', { name: /hide value of variable 1/i }));
    expect(value).toHaveAttribute('type', 'password');
  });

  it('refuses to save a nameless profile and says what to do', async () => {
    const user = userEvent.setup();
    const { store } = open();

    await user.click(await screen.findByRole('button', { name: /new profile/i }));
    await user.click(screen.getByRole('button', { name: /save profile/i }));

    expect(screen.getByRole('alert')).toHaveTextContent(/name/i);
    expect(await store.list()).toHaveLength(0);
  });

  it('refuses a malformed variable name and names the offender', async () => {
    const user = userEvent.setup();
    const { store } = open();

    await user.click(await screen.findByRole('button', { name: /new profile/i }));
    await user.type(screen.getByLabelText(/^name$/i), 'mine');
    await user.click(screen.getByRole('button', { name: /add variable/i }));
    await user.type(screen.getByLabelText(/variable 1 name/i), 'NOT VALID');
    await user.click(screen.getByRole('button', { name: /save profile/i }));

    expect(screen.getByRole('alert')).toHaveTextContent('NOT VALID');
    expect(await store.list()).toHaveLength(0);
  });

  it('shows a mandatory skill as required and does not let it be deselected', async () => {
    const user = userEvent.setup();
    open(createMockProfileStore({ initial: [{ ...emptyDraft(), name: 'seeded' }] }));

    await user.click(await screen.findByRole('button', { name: /^edit$/i }));

    const required = await screen.findByRole('checkbox', { name: /ornn-agent-manual-cli/i });
    expect(required).toBeChecked();
    expect(required).toBeDisabled();
    // Explained rather than merely blocked.
    expect(
      within(required.closest('label') as HTMLElement).getByText(/required/i),
    ).toBeInTheDocument();
  });

  it('marks required skills on a first profile, before any profile exists', async () => {
    // The gap this closes: a user authoring their very first profile has no
    // stored profile to learn the mandated entries from, and would otherwise
    // watch skills appear that they never selected.
    const user = userEvent.setup();
    open();

    await user.click(await screen.findByRole('button', { name: /new profile/i }));

    const required = await screen.findByRole('checkbox', { name: /ornn-agent-manual-cli/i });
    expect(required).toBeChecked();
    expect(required).toBeDisabled();
    expect(screen.getByText(/every session also carries/i)).toBeInTheDocument();
  });

  it('keeps a pinned version of a mandatory package through an edit', async () => {
    // The mandated entry itself is implied and hidden, but a pin is the
    // owner's own choice: a round trip through the editor must not silently
    // reset it to whatever the runtime ships.
    const user = userEvent.setup();
    const { store } = open(
      createMockProfileStore({
        initial: [
          { ...emptyDraft(), name: 'pinned', packages: [{ name: 'nyxid', version: '0.10.0' }] },
        ],
      }),
    );

    await user.click(await screen.findByRole('button', { name: /^edit$/i }));
    expect(screen.getByLabelText(/software/i)).toHaveValue('nyxid@0.10.0');
    await user.click(screen.getByRole('button', { name: /save profile/i }));

    await waitFor(async () => {
      expect((await store.list())[0]?.packages).toEqual([{ name: 'nyxid', version: '0.10.0' }]);
    });
  });

  it('edits an existing profile in place', async () => {
    const user = userEvent.setup();
    const { store } = open(
      createMockProfileStore({ initial: [{ ...emptyDraft(), name: 'first' }] }),
    );

    await user.click(await screen.findByRole('button', { name: /^edit$/i }));
    const name = screen.getByLabelText(/^name$/i);
    await user.clear(name);
    await user.type(name, 'second');
    await user.click(screen.getByRole('button', { name: /save profile/i }));

    await waitFor(async () => {
      expect((await store.list())[0]?.name).toBe('second');
    });
    expect(await store.list()).toHaveLength(1);
  });

  it('confirms before removing, because a profile cannot be recovered', async () => {
    const user = userEvent.setup();
    const { store } = open(
      createMockProfileStore({ initial: [{ ...emptyDraft(), name: 'doomed' }] }),
    );

    await user.click(await screen.findByRole('button', { name: /^remove$/i }));
    expect(await store.list()).toHaveLength(1);

    // The question is asked inline, in place of the row's own controls (§7).
    // A second dialog over this one is what that rule exists to forbid.
    expect(screen.getByText('Remove this profile?')).toBeInTheDocument();
    expect(screen.getAllByRole('dialog')).toHaveLength(1);

    // And the way out of the confirmation keeps it.
    await user.click(screen.getByRole('button', { name: /keep it/i }));
    expect(await store.list()).toHaveLength(1);

    await user.click(screen.getAllByRole('button', { name: /^remove$/i })[0] as HTMLElement);
    await user.click(screen.getAllByRole('button', { name: /^remove$/i }).at(-1) as HTMLElement);
    await waitFor(async () => {
      expect(await store.list()).toHaveLength(0);
    });
  });

  it('explains a store that cannot be read rather than showing an empty list', async () => {
    open(createMockProfileStore({ failWith: 'The control plane is unreachable.' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('The control plane is unreachable.');
  });

  it('still saves a profile when the skill registry is down', async () => {
    // The registry is one input among four. Losing it must narrow the form,
    // not close it.
    const user = userEvent.setup();
    const { store } = open(
      createMockProfileStore({ failSkillsWith: 'The registry is unreachable.' }),
    );

    await user.click(await screen.findByRole('button', { name: /new profile/i }));
    expect(await screen.findByText(/registry is unreachable/i)).toBeInTheDocument();
    await user.type(screen.getByLabelText(/^name$/i), 'mine');
    await user.click(screen.getByRole('button', { name: /save profile/i }));

    await waitFor(async () => {
      expect(await store.list()).toHaveLength(1);
    });
  });

  it('closes without saving when cancelled', async () => {
    const user = userEvent.setup();
    const { onClose } = open();

    await user.click(await screen.findByRole('button', { name: /close/i }));
    expect(onClose).toHaveBeenCalledOnce();
  });
});

describe('the plane a profile starts sessions on', () => {
  it('saves the plane the profile was given', async () => {
    // On the profile because that is where the core keeps it: two sessions
    // started from one profile must agree about which plane they are.
    const user = userEvent.setup();
    const { store } = open();

    await user.click(await screen.findByRole('button', { name: /new profile/i }));
    await user.type(screen.getByLabelText(/^name$/i), 'reviewer');
    await chooseOption(user, /^plane$/i, 'Conversation');
    await user.click(screen.getByRole('button', { name: /save profile/i }));

    await waitFor(async () => {
      expect(await store.list()).toHaveLength(1);
    });
    const [saved] = await store.list();
    expect(saved?.agent.mode).toBe('conversation');
  });

  it('says why a runtime cannot serve the chosen plane', async () => {
    const user = userEvent.setup();
    open();

    await user.click(await screen.findByRole('button', { name: /new profile/i }));
    // A new profile starts on the conversation plane, which Claude Code
    // serves — so the refusal has to be provoked by asking for the plane it
    // does not serve, rather than by arriving on it.
    await chooseOption(user, /^plane$/i, 'Terminal');
    await chooseOption(user, /^agent$/i, 'Claude Code');

    expect(screen.getByRole('alert')).toHaveTextContent(/does not advertise “terminal”/i);
  });

  it('starts a new profile on the conversation plane', async () => {
    // The conversation plane is the default surface: it is where approvals
    // are answered and where a failure can be explained rather than printed.
    const user = userEvent.setup();
    open();

    await user.click(await screen.findByRole('button', { name: /new profile/i }));
    expect(await screen.findByRole('combobox', { name: /^plane$/i })).toHaveTextContent(
      /conversation/i,
    );
  });

  it('refuses nothing when this deployment’s runtimes could not be read', async () => {
    // An empty list is "nothing is known", not "everything is refused".
    // Emptying a form over a failed request would be the second.
    const user = userEvent.setup();
    open(createMockProfileStore(), vi.fn(), []);

    await user.click(await screen.findByRole('button', { name: /new profile/i }));

    expect(screen.queryByRole('alert')).toBeNull();
    expect(chosenOption(/^agent$/i)).toContain('Codex');
  });
});
