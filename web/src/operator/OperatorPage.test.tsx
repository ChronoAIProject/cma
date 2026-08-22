import { describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { Capability } from '../auth/types';
import { OperatorPage } from './OperatorPage';
import { createMockOperatorStore, type MockOperatorOptions } from './mock';

/**
 * What the panel actually draws.
 *
 * The rules it obeys are tested beside their own modules; what is pinned here
 * is that they reach the screen — that a view is never drawn empty because of
 * entitlement, that a provider nobody could ask is never rendered as one
 * running nothing, and that ending somebody's session asks first.
 */

const ALL: readonly Capability[] = [
  'read_user_directory',
  'read_activity_log',
  'read_runtime_inventory',
  'read_all_sessions',
  'control_any_session',
  'configure_platform',
];

function open(capabilities: readonly Capability[] = ALL, options: MockOperatorOptions = {}) {
  const store = createMockOperatorStore(options);

  return {
    store,
    ...render(<OperatorPage store={store} capabilities={capabilities} />),
  };
}

/** Switch to a view by the name its tab carries. */
async function show(view: RegExp) {
  await userEvent.click(screen.getByRole('tab', { name: view }));
}

/** The rows themselves, so a filter's options cannot be mistaken for one. */
function rows() {
  return within(screen.getByRole('list'));
}

const ADA = {
  subject: 'subject-a',
  displayName: 'Ada Lovelace',
  email: 'ada@example.com',
  firstSeen: Date.parse('2026-08-01T09:14:22Z'),
  lastSeen: Date.parse('2026-08-14T11:02:05Z'),
};

describe('the view switch', () => {
  it('offers only the views this caller holds a capability for', async () => {
    open(['read_activity_log']);
    const tabs = screen.getAllByRole('tab');
    expect(tabs).toHaveLength(1);
    expect(tabs[0]).toHaveTextContent(/activity/i);
    expect(await screen.findByText(/nothing has been recorded yet/i)).toBeInTheDocument();
  });

  it('offers a configure-only caller Operations with only Settings', async () => {
    open(['configure_platform']);
    const tabs = screen.getAllByRole('tab');
    expect(tabs).toHaveLength(1);
    expect(tabs[0]).toHaveTextContent('Settings');
    expect(await screen.findByRole('form', { name: 'Platform settings' })).toBeInTheDocument();
  });

  it('names the selected view by more than its colour', async () => {
    open(['read_user_directory', 'read_all_sessions']);
    const [people, sessions] = screen.getAllByRole('tab');
    expect(people).toHaveAttribute('aria-selected', 'true');
    expect(sessions).toHaveAttribute('aria-selected', 'false');

    await userEvent.click(sessions as HTMLElement);
    expect(sessions).toHaveAttribute('aria-selected', 'true');
  });

  it('keeps an unsaved settings draft across view switches', async () => {
    const user = userEvent.setup();
    open(['read_user_directory', 'configure_platform']);
    await user.click(screen.getByRole('tab', { name: 'Settings' }));
    const heading = await screen.findByLabelText(/granted-services heading/i);
    await user.clear(heading);
    await user.type(heading, 'Unsaved heading');

    await user.click(screen.getByRole('tab', { name: 'People' }));
    await user.click(screen.getByRole('tab', { name: 'Settings' }));
    expect(screen.getByLabelText(/granted-services heading/i)).toHaveValue('Unsaved heading');
  });
});

describe('people', () => {
  it('names a person, and never by their subject identifier', async () => {
    open(['read_user_directory'], { people: [ADA] });
    expect(await screen.findByText('Ada Lovelace')).toBeInTheDocument();
    expect(screen.queryByText('subject-a')).not.toBeInTheDocument();
  });

  it('shows the identifier as an identifier when nothing else is known', async () => {
    open(['read_user_directory'], { people: [{ subject: '76fe9d91' }] });
    const shown = await screen.findByText('76fe9d91');
    expect(shown).toHaveClass('record__identifier');
  });

  it('says what would have been there when nobody has signed in', async () => {
    open(['read_user_directory']);
    expect(await screen.findByText(/nobody has signed in yet/i)).toBeInTheDocument();
  });
});

describe('activity', () => {
  const ENTRY = {
    id: 'actv_1',
    at: Date.parse('2026-08-14T11:02:05Z'),
    actor: 'subject-a',
    action: 'session_terminate',
    outcome: 'succeeded',
    detail: [{ key: 'subject', value: 'subject-b' }],
  };

  it('reads an action as words and carries its outcome', async () => {
    open(['read_activity_log', 'read_user_directory'], {
      activity: [ENTRY],
      people: [ADA],
    });
    await show(/activity/i);
    expect(await rows().findByText(/session terminate/)).toBeInTheDocument();
    expect(rows().getByText('succeeded')).toBeInTheDocument();
    // The whole reason an operator's action is worth recording: whose work it
    // reached, which is not the actor.
    expect(rows().getByText(/subject: subject-b/)).toBeInTheDocument();
  });

  it('draws an unattributed action as a fact rather than an empty cell', async () => {
    open(['read_activity_log'], {
      activity: [{ ...ENTRY, actor: undefined }],
    });
    expect(await screen.findByText(/not signed in/i)).toBeInTheDocument();
  });

  it('asks the control plane to narrow by person rather than filtering a page', async () => {
    // The record is served with a limit, so filtering in the browser would show
    // three of somebody's entries where asking shows a hundred.
    const asked: (string | undefined)[] = [];
    const store = createMockOperatorStore({ activity: [ENTRY], people: [ADA] });
    const spied = {
      ...store,
      activity: async (query: { actor?: string }) => {
        asked.push(query.actor);
        return store.activity(query);
      },
    };
    render(
      <OperatorPage store={spied} capabilities={['read_activity_log', 'read_user_directory']} />,
    );
    await show(/activity/i);
    await rows().findByText(/session terminate/);

    await userEvent.selectOptions(screen.getByLabelText(/person/i), 'subject-a');
    await waitFor(() => {
      expect(asked).toContain('subject-a');
    });
  });

  it('shows the record without names when the caller may not read the directory', async () => {
    // Each view is offered on its own capability. A view refused for want of a
    // second grant would be a view drawn empty by entitlement.
    open(['read_activity_log'], { activity: [ENTRY] });
    const list = within(await screen.findByRole('list'));
    expect(list.getByText(/session terminate/)).toBeInTheDocument();
    expect(list.getByText('subject-a')).toHaveClass('record__identifier');
  });
});

describe('the namespace', () => {
  it('renders a provider nobody could ask as unknown, carrying its reason', async () => {
    open(['read_runtime_inventory'], {
      inventory: {
        fullyObserved: false,
        providers: [{ kernel: 'example-runtime', observable: false, reason: 'cannot enumerate' }],
      },
    });
    expect(await screen.findByText('unknown')).toBeInTheDocument();
    expect(screen.getByText('cannot enumerate')).toBeInTheDocument();
    // A partial total presented as a total is the same lie as an empty list.
    expect(screen.getByText(/running total withheld/i)).toBeInTheDocument();
  });

  it('renders a provider running nothing as running nothing', async () => {
    open(['read_runtime_inventory'], {
      inventory: {
        running: 0,
        fullyObserved: true,
        providers: [{ kernel: 'example-runtime', observable: true, runtimes: [] }],
      },
    });
    expect(await screen.findByText(/running nothing/i)).toBeInTheDocument();
    expect(screen.getByText('0 running')).toBeInTheDocument();
  });
});

describe('acting on somebody else’s session', () => {
  const RUNNING = {
    id: 'sesn_1',
    owner: 'subject-a',
    ownerLabel: 'Ada Lovelace',
    status: 'ready',
    kernel: 'example-runtime',
  };

  it('asks before ending one, per the destructive pattern', async () => {
    const onTerminate = vi.fn();
    open(ALL, { sessions: [RUNNING], onTerminate });
    await show(/sessions/i);
    const row = (await screen.findByText('Ada Lovelace')).closest('li') as HTMLElement;

    await userEvent.click(within(row).getByRole('button', { name: 'End' }));
    expect(onTerminate).not.toHaveBeenCalled();
    expect(within(row).getByText(/end this session\?/i)).toBeInTheDocument();

    await userEvent.click(within(row).getByRole('button', { name: 'End' }));
    await waitFor(() => {
      expect(onTerminate).toHaveBeenCalledWith('sesn_1');
    });
  });

  it('stops one without asking, because stopping is reversible', async () => {
    const onStop = vi.fn();
    open(ALL, { sessions: [RUNNING], onStop });
    await show(/sessions/i);
    const row = (await screen.findByText('Ada Lovelace')).closest('li') as HTMLElement;

    await userEvent.click(within(row).getByRole('button', { name: 'Stop' }));
    await waitFor(() => {
      expect(onStop).toHaveBeenCalledWith('sesn_1');
    });
  });

  it('disables rather than removes a control, and states why', async () => {
    open(['read_all_sessions'], { sessions: [RUNNING] });
    const row = (await screen.findByText('Ada Lovelace')).closest('li') as HTMLElement;

    const stop = within(row).getByRole('button', { name: 'Stop' });
    expect(stop).toBeDisabled();
    expect(stop).toHaveAttribute('title', expect.stringMatching(/watch .* not act/i));
  });

  it('never offers to delete somebody else’s session', async () => {
    open(ALL, { sessions: [RUNNING] });
    await show(/sessions/i);
    await screen.findByText('Ada Lovelace');
    expect(screen.queryByRole('button', { name: /delete|remove/i })).not.toBeInTheDocument();
  });
});

describe('when the control plane cannot be reached', () => {
  it('says so rather than drawing an empty list', async () => {
    open(['read_user_directory'], { failWith: 'the control plane is unreachable' });
    expect(await screen.findByRole('alert')).toHaveTextContent(/unreachable/i);
    expect(screen.queryByText(/nobody has signed in yet/i)).not.toBeInTheDocument();
  });
});
