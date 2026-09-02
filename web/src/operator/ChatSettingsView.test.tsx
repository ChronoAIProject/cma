import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { ChatSettingsView, validateChatSettings } from './ChatSettingsView';
import { createMockOperatorStore, MOCK_CHAT_SETTINGS } from './mock';
import type { ChatSettings, OperatorStore } from './types';

function chat(): ChatSettings {
  if (MOCK_CHAT_SETTINGS.chat === null) {
    throw new Error('the chat fixture must be configured');
  }
  return structuredClone(MOCK_CHAT_SETTINGS.chat);
}

async function open(store: OperatorStore = createMockOperatorStore()): Promise<OperatorStore> {
  render(<ChatSettingsView store={store} />);
  await screen.findByRole('form', { name: 'Chat settings' });
  return store;
}

describe('the chat settings document', () => {
  it('draws all seven sections, every fixed choice, and the shipped dark defaults', async () => {
    await open(createMockOperatorStore({ chatSettings: { revision: 0, chat: null } }));

    expect(
      screen.getAllByRole('heading', { level: 3 }).map((heading) => heading.textContent),
    ).toEqual(['Workspace', 'Agent', 'Messages', 'Setup', 'Lifetime', 'Host resources', 'Enabled']);
    expect(screen.getByLabelText('Repository URL')).toHaveValue(
      'https://github.com/the-omega-institute/trureturing',
    );
    expect(screen.getByLabelText('Branch')).toHaveValue('dev');
    expect(screen.getByLabelText('Thinking effort')).toHaveValue('high');
    expect(screen.getByLabelText('Prepended texts 1')).toHaveValue('$codex-formal-answer');
    expect(
      (screen.getByLabelText('Software-installation commands 1') as HTMLInputElement).value,
    ).toContain('skills/codex-formal-answer/SKILL.md');
    expect(screen.getByRole('switch', { name: 'Accept new chats' })).not.toBeChecked();
    expect(screen.getByText('Chat sessions run with full access (unrestricted).')).toBeVisible();
    expect(
      screen.getByText(
        'Run inside the prepared repository and again after every revive. Commands run as uid 10001 without root access. Keep them idempotent and prefer workspace-local installation so toolchains survive pause checkpoints.',
      ),
    ).toBeVisible();
    expect(screen.getByText('Lean fixture').nextElementSibling).toHaveTextContent(
      '[ -x .toolchain/bin/lake ] || (curl -sSf https://elan.lean-lang.org/elan-init.sh | ELAN_HOME=$PWD/.toolchain sh -s -- -y --default-toolchain "$(cat lean-toolchain)")',
    );
  });

  it('round-trips every editable field through the operator store in one CAS write', async () => {
    const user = userEvent.setup();
    const onChatSettingsChange = vi.fn();
    await open(createMockOperatorStore({ onChatSettingsChange }));

    await user.clear(screen.getByLabelText('Repository URL'));
    await user.type(screen.getByLabelText('Repository URL'), 'https://example.test/team/work.git');
    await user.clear(screen.getByLabelText('Branch'));
    await user.type(screen.getByLabelText('Branch'), 'release');
    await user.type(screen.getByLabelText('Model'), 'gpt-5.2-codex');
    await user.selectOptions(screen.getByLabelText('Thinking effort'), 'x_high');
    await user.type(screen.getByLabelText('Instructions (agent.md)'), 'Keep changes focused.');
    await user.click(screen.getByRole('checkbox', { name: 'Record bounded tool bodies' }));

    await user.clear(screen.getByLabelText('Prepended texts 1'));
    await user.type(screen.getByLabelText('Prepended texts 1'), '/formalize');
    await user.click(screen.getByRole('button', { name: 'Add prepended texts' }));
    await user.type(screen.getByLabelText('Prepended texts 2'), '/review');

    await user.click(screen.getByRole('button', { name: 'Add software-installation commands' }));
    await user.type(screen.getByLabelText('Software-installation commands 1'), 'npm ci');
    await user.clear(screen.getByLabelText('Auto-pause after (minutes)'));
    await user.type(screen.getByLabelText('Auto-pause after (minutes)'), '30');
    await user.clear(screen.getByLabelText('Max chat windows per user'));
    await user.type(screen.getByLabelText('Max chat windows per user'), '12');

    await user.click(screen.getByRole('checkbox', { name: 'Use deployment default' }));
    await user.clear(screen.getByLabelText('CPU (millicores)'));
    await user.type(screen.getByLabelText('CPU (millicores)'), '3000');
    await user.clear(screen.getByLabelText('Memory (MiB)'));
    await user.type(screen.getByLabelText('Memory (MiB)'), '6144');
    await user.click(screen.getByRole('switch', { name: 'Accept new chats' }));
    await user.click(screen.getByRole('button', { name: 'Save chat settings' }));

    await waitFor(() => {
      expect(onChatSettingsChange).toHaveBeenCalledTimes(1);
    });
    expect(onChatSettingsChange).toHaveBeenCalledWith({
      enabled: true,
      workspace: {
        repositoryUrl: 'https://example.test/team/work.git',
        branch: 'release',
      },
      agent: {
        model: 'gpt-5.2-codex',
        reasoningEffort: 'x_high',
        instructions: 'Keep changes focused.',
        recordToolBodies: false,
      },
      messagePrefixes: ['/formalize', '/review'],
      inactivity: { pauseAfterSeconds: 1800 },
      setup: { commands: ['npm ci'] },
      host: { size: { cpuMillis: 3000, memoryMib: 6144 }, maxThreads: 12 },
    });
    expect(screen.getByRole('status')).toHaveTextContent('Saved revision 1.');
  });

  it('retains an explicit host size while deployment defaults are selected', async () => {
    const user = userEvent.setup();
    const configured = chat();
    const store = createMockOperatorStore({
      chatSettings: {
        revision: 3,
        chat: {
          ...configured,
          host: { ...configured.host, size: { cpuMillis: 1750, memoryMib: 3584 } },
        },
      },
    });
    await open(store);

    const useDefault = screen.getByRole('checkbox', { name: 'Use deployment default' });
    await user.click(useDefault);
    expect(screen.getByLabelText('CPU (millicores)')).toBeDisabled();
    expect(screen.getByLabelText('CPU (millicores)')).toHaveAttribute(
      'placeholder',
      'Deployment default',
    );
    await user.click(useDefault);
    expect(screen.getByLabelText('CPU (millicores)')).toHaveValue(1750);
    expect(screen.getByLabelText('Memory (MiB)')).toHaveValue(3584);
  });
});

describe('ordered editors', () => {
  it('adds, reorders, and removes both prepended texts and setup commands', async () => {
    const user = userEvent.setup();
    const configured = chat();
    await open(
      createMockOperatorStore({
        chatSettings: {
          revision: 0,
          chat: {
            ...configured,
            messagePrefixes: ['first', 'second'],
            setup: { commands: ['one', 'two'] },
          },
        },
      }),
    );

    await user.click(screen.getByRole('button', { name: 'Move prepended texts 2 up' }));
    expect(screen.getByLabelText('Prepended texts 1')).toHaveValue('second');
    expect(screen.getByLabelText('Prepended texts 2')).toHaveValue('first');
    await user.click(screen.getByRole('button', { name: 'Remove prepended texts 1' }));
    expect(screen.getAllByRole('textbox', { name: /prepended texts \d+/iu })).toHaveLength(1);
    expect(screen.getByLabelText('Prepended texts 1')).toHaveValue('first');

    await user.click(
      screen.getByRole('button', { name: 'Move software-installation commands 1 down' }),
    );
    expect(screen.getByLabelText('Software-installation commands 1')).toHaveValue('two');
    await user.click(
      screen.getByRole('button', { name: 'Remove software-installation commands 2' }),
    );
    await user.click(screen.getByRole('button', { name: 'Add software-installation commands' }));
    expect(
      screen.getAllByRole('textbox', { name: /software-installation commands \d+/iu }),
    ).toHaveLength(2);
    expect(screen.getByLabelText('Software-installation commands 2')).toHaveValue('');
  });

  it('stops adding at the server collection limits', async () => {
    const configured = chat();
    await open(
      createMockOperatorStore({
        chatSettings: {
          revision: 0,
          chat: {
            ...configured,
            messagePrefixes: Array.from({ length: 8 }, (_, index) => `prefix-${String(index)}`),
            setup: {
              commands: Array.from({ length: 16 }, (_, index) => `command-${String(index)}`),
            },
          },
        },
      }),
    );

    expect(screen.getByRole('button', { name: 'Add prepended texts' })).toBeDisabled();
    expect(
      screen.getByRole('button', { name: 'Add software-installation commands' }),
    ).toBeDisabled();
  });
});

describe('validation', () => {
  it('mirrors byte, URL, collection, time, thread, and host-size boundaries', () => {
    const base = chat();
    const cases: ChatSettings[] = [
      { ...base, workspace: { ...base.workspace, repositoryUrl: 'http://example.test/repo' } },
      { ...base, workspace: { ...base.workspace, branch: 'é'.repeat(101) } },
      { ...base, messagePrefixes: ['💡'.repeat(65)] },
      { ...base, setup: { commands: ['💡'.repeat(1025)] } },
      { ...base, inactivity: { pauseAfterSeconds: 59 } },
      { ...base, inactivity: { pauseAfterSeconds: 86_401 } },
      { ...base, host: { ...base.host, maxThreads: 0 } },
      { ...base, host: { ...base.host, maxThreads: 51 } },
      { ...base, host: { ...base.host, size: { cpuMillis: 0, memoryMib: 1 } } },
      { ...base, host: { ...base.host, size: { cpuMillis: 1, memoryMib: 4_294_967_296 } } },
    ];

    expect(cases.map((candidate) => validateChatSettings(candidate).valid)).toEqual(
      cases.map(() => false),
    );
    expect(validateChatSettings(base)).toEqual({ valid: true, errors: {} });
  });

  it('renders field-specific messages and refuses to call the store', async () => {
    const user = userEvent.setup();
    const onChatSettingsChange = vi.fn();
    await open(createMockOperatorStore({ onChatSettingsChange }));

    await user.clear(screen.getByLabelText('Repository URL'));
    await user.type(
      screen.getByLabelText('Repository URL'),
      'https://user:secret@example.test/repo',
    );
    await user.clear(screen.getByLabelText('Max chat windows per user'));
    await user.type(screen.getByLabelText('Max chat windows per user'), '51');

    expect(screen.getByText(/credential-free HTTPS repository URL/i)).toBeVisible();
    expect(screen.getByText(/whole number from 1 to 50/i)).toBeVisible();
    expect(screen.getByRole('button', { name: 'Save chat settings' })).toBeDisabled();
    expect(onChatSettingsChange).not.toHaveBeenCalled();
  });
});

describe('conditional writes', () => {
  it('retains a stale draft and saves it only after explicit conflict recovery', async () => {
    const user = userEvent.setup();
    const store = await open();
    await user.clear(screen.getByLabelText('Branch'));
    await user.type(screen.getByLabelText('Branch'), 'operator-draft');

    const external = chat();
    await store.changeChatSettings(0, {
      ...external,
      workspace: { ...external.workspace, branch: 'concurrent-change' },
    });
    await user.click(screen.getByRole('button', { name: 'Save chat settings' }));

    const conflict = await screen.findByRole('alert');
    expect(within(conflict).getByText('Settings changed elsewhere')).toBeVisible();
    expect(screen.getByLabelText('Branch')).toHaveValue('operator-draft');
    expect(screen.getByText(/Revision 1 is current/i)).toBeVisible();

    await user.click(screen.getByRole('button', { name: 'Review retained draft' }));
    await user.click(screen.getByRole('button', { name: 'Save chat settings' }));
    await waitFor(async () => {
      expect((await store.readChatSettings()).revision).toBe(2);
    });
    expect((await store.readChatSettings()).chat?.workspace.branch).toBe('operator-draft');
  });

  it('can discard a stale draft by loading the current values', async () => {
    const user = userEvent.setup();
    const store = await open();
    await user.clear(screen.getByLabelText('Branch'));
    await user.type(screen.getByLabelText('Branch'), 'discard-me');

    const external = chat();
    await store.changeChatSettings(0, {
      ...external,
      workspace: { ...external.workspace, branch: 'server-value' },
    });
    await user.click(screen.getByRole('button', { name: 'Save chat settings' }));
    await user.click(await screen.findByRole('button', { name: 'Load current values' }));

    expect(screen.getByLabelText('Branch')).toHaveValue('server-value');
    expect(screen.getByRole('button', { name: 'Save chat settings' })).toBeDisabled();
  });
});

describe('read failures', () => {
  it('reports the failure and retries without replacing the store', async () => {
    const user = userEvent.setup();
    const delegate = createMockOperatorStore();
    let reads = 0;
    const store: OperatorStore = {
      ...delegate,
      readChatSettings: async () => {
        reads += 1;
        if (reads === 1) throw new Error('chat settings store is unavailable');
        return delegate.readChatSettings();
      },
    };
    render(<ChatSettingsView store={store} />);

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'chat settings store is unavailable',
    );
    await user.click(screen.getByRole('button', { name: 'Try again' }));
    expect(await screen.findByRole('form', { name: 'Chat settings' })).toBeVisible();
    expect(reads).toBe(2);
  });
});
