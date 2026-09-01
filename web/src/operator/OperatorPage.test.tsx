import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
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

const css = () => readFileSync(resolve(process.cwd(), 'src/operator/OperatorPage.css'), 'utf8');

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

  it('offers a configure-only caller both public settings editors', async () => {
    open(['configure_platform']);
    const tabs = screen.getAllByRole('tab');
    expect(tabs).toHaveLength(2);
    expect(tabs.map((tab) => tab.textContent)).toEqual(['Settings', 'Chat']);
    expect(await screen.findByRole('form', { name: 'Platform settings' })).toBeInTheDocument();
    await show(/^chat$/i);
    expect(await screen.findByRole('form', { name: 'Chat settings' })).toBeInTheDocument();
  });

  it('names the selected view by more than its colour', async () => {
    open(['read_user_directory', 'read_all_sessions']);
    const [people, sessions, hosts] = screen.getAllByRole('tab');
    expect(people).toHaveAttribute('aria-selected', 'true');
    expect(sessions).toHaveAttribute('aria-selected', 'false');
    expect(hosts).toHaveAttribute('aria-selected', 'false');

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

  it('keeps an unsaved chat draft across view switches', async () => {
    const user = userEvent.setup();
    open(['read_user_directory', 'configure_platform']);
    await user.click(screen.getByRole('tab', { name: 'Chat' }));
    const branch = await screen.findByLabelText('Branch');
    await user.clear(branch);
    await user.type(branch, 'unsaved-chat-branch');

    await user.click(screen.getByRole('tab', { name: 'People' }));
    await user.click(screen.getByRole('tab', { name: 'Chat' }));
    expect(screen.getByLabelText('Branch')).toHaveValue('unsaved-chat-branch');
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

describe('shared chat hosts', () => {
  const HOST = {
    id: 'chho_0123456789abcdef0123456789abcdef',
    owner: 'subject-chat-owner',
    status: 'active' as const,
    threadCount: 3,
    runtimeVersion: 'runtime-v2',
    settingsRevision: 17,
    createdAt: Date.now() - 2 * 60 * 60 * 1000,
  };

  it('shows the compact owner, status, threads, and age row', async () => {
    open(['read_all_sessions'], { chatHosts: [HOST] });
    await show(/^chat hosts$/i);
    const row = (await screen.findByText('subject-chat-owner')).closest('li') as HTMLElement;

    expect(within(row).getByText('active')).toBeInTheDocument();
    expect(within(row).getByText('3 threads')).toBeInTheDocument();
    expect(within(row).getByText('2 hr old')).toBeInTheDocument();
    expect(within(row).getByText(HOST.id)).toHaveClass('record__identifier');
  });

  it('asks inline before retirement, then rereads the host row', async () => {
    const onRetireHost = vi.fn();
    open(ALL, { chatHosts: [HOST], onRetireHost });
    await show(/^chat hosts$/i);
    const row = (await screen.findByText('subject-chat-owner')).closest('li') as HTMLElement;

    await userEvent.click(within(row).getByRole('button', { name: 'Retire' }));
    expect(onRetireHost).not.toHaveBeenCalled();
    expect(within(row).getByText(/retire this chat host\?/i)).toBeInTheDocument();
    await userEvent.click(within(row).getByRole('button', { name: 'Retire' }));

    await waitFor(() => {
      expect(onRetireHost).toHaveBeenCalledWith(HOST.id);
      expect(within(row).getByText('retired')).toBeInTheDocument();
    });
    expect(within(row).getByRole('button', { name: 'Retire' })).toBeDisabled();
  });

  it('keeps retirement visible but unavailable to an inventory-only caller', async () => {
    open(['read_all_sessions'], { chatHosts: [HOST] });
    await show(/^chat hosts$/i);
    const row = (await screen.findByText('subject-chat-owner')).closest('li') as HTMLElement;
    const retire = within(row).getByRole('button', { name: 'Retire' });

    expect(retire).toBeDisabled();
    expect(retire).toHaveAttribute('title', expect.stringMatching(/watch .* not act/i));
    expect(within(row).getByText(/watch .* not act/i)).toHaveClass('control-reason');
  });

  it('keeps host confirmation independent from session confirmation', async () => {
    const session = {
      id: HOST.id,
      owner: 'session-owner',
      ownerLabel: 'Session owner',
      status: 'ready',
      kernel: 'example-runtime',
    };
    open(ALL, { chatHosts: [HOST], sessions: [session] });
    await show(/^chat hosts$/i);
    const hostRow = (await screen.findByText('subject-chat-owner')).closest('li') as HTMLElement;
    await userEvent.click(within(hostRow).getByRole('button', { name: 'Retire' }));

    await show(/^sessions$/i);
    const sessionRow = (await screen.findByText('Session owner')).closest('li') as HTMLElement;
    expect(within(sessionRow).queryByText(/end this session\?/i)).not.toBeInTheDocument();
    await userEvent.click(within(sessionRow).getByRole('button', { name: 'End' }));
    expect(within(sessionRow).getByText(/end this session\?/i)).toBeInTheDocument();
  });
});

describe('when the control plane cannot be reached', () => {
  it('says so rather than drawing an empty list', async () => {
    open(['read_user_directory'], { failWith: 'the control plane is unreachable' });
    expect(await screen.findByRole('alert')).toHaveTextContent(/unreachable/i);
    expect(screen.queryByText(/nobody has signed in yet/i)).not.toBeInTheDocument();
  });
});

describe('the operations appearance', () => {
  it('uses the reference tab geometry and a glowing two-pixel selected edge', () => {
    const sheet = css();
    expect(sheet).toMatch(
      /\.operator__tabs\s*\{[^}]*gap: 2px[^}]*padding: 0 4px[^}]*border-bottom: 1px solid var\(--dsw-alias-border-l1\)/,
    );
    expect(sheet).toMatch(
      /\.operator__tab\s*\{[^}]*height: 42px[^}]*padding: 0 16px[^}]*font-size: 14px[^}]*font-weight: 500/,
    );
    expect(sheet).toMatch(
      /\.operator__tab--on\s*\{[^}]*color: var\(--dsw-alias-state-business-primary\)[^}]*font-weight: 600/,
    );
    expect(sheet).toMatch(
      /\.operator__tab--on::after\s*\{[^}]*height: 2px[^}]*background: var\(--dsw-alias-state-business-primary\)[^}]*box-shadow: var\(--dsw-glow-accent\)/,
    );
  });

  it('keeps page-owned filters compact and focused in the appearance accent', () => {
    const sheet = css();
    expect(sheet).toMatch(
      /\.operator__filter-label\s*\{[^}]*font-family: var\(--ds-font-family-code\)[^}]*font-size: 10\.5px[^}]*letter-spacing: 0\.14em/,
    );
    expect(sheet).toMatch(
      /\.operator__filter select\s*\{[^}]*height: 32px[^}]*background-color: var\(--dsw-alias-bg-layer-1\)[^}]*border-radius: 6px/,
    );
    expect(sheet).toMatch(
      /\.operator__filter select:focus-visible\s*\{[^}]*border-color: var\(--dsw-alias-state-business-primary\)[^}]*box-shadow: var\(--dsw-ring-accent-soft\)/,
    );
  });

  it('keeps seven tabs scrollable and stacks record controls on narrow screens', () => {
    const sheet = css();
    expect(sheet).toMatch(/\.operator__tabs\s*\{[^}]*overflow-x: auto/);
    expect(sheet).toMatch(
      /@media \(max-width: 29\.999rem\)[\s\S]*\.operator__record-row\s*\{[^}]*grid-template-columns: minmax\(0, 1fr\)/,
    );
    expect(sheet).toMatch(
      /\.operator \.record__actions\s*\{[^}]*flex-wrap: wrap[^}]*justify-content: flex-start/,
    );
  });
});
