import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { createMockOperatorStore, MOCK_PLATFORM_SETTINGS, type MockOperatorOptions } from './mock';
import { PlatformSettingsView } from './PlatformSettingsView';
import type { PlatformSettings, SessionBriefing, StoredPlatformSettings } from './types';

type SettingsChange = Omit<
  Partial<PlatformSettings>,
  'briefing' | 'deploymentCapacity' | 'sizeBounds'
> & {
  readonly briefing?: Partial<SessionBriefing>;
  readonly deploymentCapacity?: Partial<PlatformSettings['deploymentCapacity']>;
  readonly sizeBounds?: Partial<PlatformSettings['sizeBounds']>;
};

function changedSettings(change: SettingsChange = {}): PlatformSettings {
  const { briefing, deploymentCapacity, sizeBounds, ...rest } = change;
  return {
    ...MOCK_PLATFORM_SETTINGS.settings,
    ...rest,
    briefing: {
      ...MOCK_PLATFORM_SETTINGS.settings.briefing,
      ...briefing,
    },
    deploymentCapacity: {
      ...MOCK_PLATFORM_SETTINGS.settings.deploymentCapacity,
      ...deploymentCapacity,
    },
    sizeBounds: {
      ...MOCK_PLATFORM_SETTINGS.settings.sizeBounds,
      ...sizeBounds,
    },
  };
}

async function open(options: MockOperatorOptions = {}) {
  const store = createMockOperatorStore(options);
  render(<PlatformSettingsView store={store} />);
  await screen.findByRole('form', { name: 'Platform settings' });
  return store;
}

describe('the assembled session briefing', () => {
  it('exposes every prose field while keeping the generated grants fixed', async () => {
    await open();

    expect(screen.getAllByLabelText(/prose before granted services, block/i)).toHaveLength(2);
    expect(screen.getByLabelText(/granted-services heading/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/introduction when services are granted/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/closing prose when services are granted/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/prose when no services are granted/i)).toBeInTheDocument();
    expect(screen.getAllByLabelText(/prose after granted services, block/i)).toHaveLength(1);

    const generated = screen.getByLabelText('Generated granted-services list');
    expect(generated).toHaveTextContent(/generated from the services granted/i);
    expect(within(generated).queryByRole('button')).toBeNull();
  });

  it('states the Codex-only limit and the authentication-guidance cost', async () => {
    await open();
    expect(
      screen.getByText(/only Codex currently reads the session briefing/i),
    ).toBeInTheDocument();
    expect(screen.getByText(/clearing its prose removes/i)).toHaveTextContent(
      /already authenticated/i,
    );
    expect(screen.getByText(/clearing its prose removes/i)).toHaveTextContent(/nyxid login/i);
  });

  it('adds, orders, and removes prose blocks', async () => {
    const user = userEvent.setup();
    await open();
    const before = () => screen.getAllByLabelText(/prose before granted services, block/i);

    await user.clear(before()[0] as HTMLElement);
    await user.type(before()[0] as HTMLElement, 'Alpha');
    await user.clear(before()[1] as HTMLElement);
    await user.type(before()[1] as HTMLElement, 'Beta');
    await user.click(
      screen.getByRole('button', { name: 'Move prose before granted services block 2 up' }),
    );
    expect(before()[0]).toHaveValue('Beta');
    expect(before()[1]).toHaveValue('Alpha');

    await user.click(
      screen.getByRole('button', { name: 'Remove prose before granted services block 1' }),
    );
    expect(before()).toHaveLength(1);
    expect(before()[0]).toHaveValue('Alpha');

    await user.click(screen.getAllByRole('button', { name: /add block/i })[0] as HTMLElement);
    expect(before()).toHaveLength(2);
    expect(before()[1]).toHaveValue('');
  });
});

describe('conditional settings writes', () => {
  it('restores the last live-session value when its limit is toggled back on', async () => {
    const user = userEvent.setup();
    await open({
      platformSettings: {
        revision: 3,
        settings: changedSettings({ liveSessions: 7 }),
      },
    });

    const toggle = screen.getByRole('checkbox', { name: /limit live sessions per person/i });
    expect(screen.getByLabelText(/maximum live sessions/i)).toHaveValue(7);
    await user.click(toggle);
    expect(screen.queryByLabelText(/maximum live sessions/i)).toBeNull();
    await user.click(toggle);
    expect(screen.getByLabelText(/maximum live sessions/i)).toHaveValue(7);
  });

  it('saves prose, deployment capacity, bounds, and a live-session limit together', async () => {
    const user = userEvent.setup();
    const onSettingsChange = vi.fn();
    await open({ onSettingsChange });

    await user.clear(screen.getByLabelText(/granted-services heading/i));
    await user.type(screen.getByLabelText(/granted-services heading/i), 'Reach from this session');
    await user.clear(screen.getByLabelText('Maximum CPU'));
    await user.type(screen.getByLabelText('Maximum CPU'), '4500');
    await user.click(screen.getByRole('checkbox', { name: /declare a CPU total/i }));
    await user.clear(screen.getByLabelText('Total CPU'));
    await user.type(screen.getByLabelText('Total CPU'), '20000');
    await user.click(screen.getByRole('checkbox', { name: /declare a memory total/i }));
    await user.clear(screen.getByLabelText(/total memory/i));
    await user.type(screen.getByLabelText(/total memory/i), '40960');
    await user.click(
      screen.getByRole('checkbox', { name: /account for sessions without a recorded size/i }),
    );
    await user.clear(screen.getByLabelText(/assumed CPU/i));
    await user.type(screen.getByLabelText(/assumed CPU/i), '1000');
    await user.clear(screen.getByLabelText(/assumed memory/i));
    await user.type(screen.getByLabelText(/assumed memory/i), '1024');
    await user.click(screen.getByRole('checkbox', { name: /limit live sessions per person/i }));
    await user.clear(screen.getByLabelText(/maximum live sessions/i));
    await user.type(screen.getByLabelText(/maximum live sessions/i), '4');
    await user.click(screen.getByRole('button', { name: /save changes/i }));

    await waitFor(() => {
      expect(onSettingsChange).toHaveBeenCalledTimes(1);
    });
    const saved = onSettingsChange.mock.calls[0]?.[0] as PlatformSettings;
    expect(saved.briefing.reachHeading).toBe('Reach from this session');
    expect(saved.deploymentCapacity).toEqual({
      cpuMillis: 20_000,
      memoryMib: 40_960,
      assumedSessionSize: { cpuMillis: 1_000, memoryMib: 1_024 },
    });
    expect(saved.sizeBounds.maxCpuMillis).toBe(4500);
    expect(saved.liveSessions).toBe(4);
    expect(screen.getByText(/saved revision 1/i)).toBeInTheDocument();
  });

  it('warns when a declared capacity cannot account for unsized sessions', async () => {
    const user = userEvent.setup();
    await open();

    expect(screen.queryByText(/capacity admission is inactive/i)).toBeNull();
    await user.click(screen.getByRole('checkbox', { name: /declare a CPU total/i }));

    expect(screen.getByText(/capacity admission is inactive/i)).toHaveTextContent(
      /without a recorded size/i,
    );
    expect(
      screen.getByRole('checkbox', { name: /account for sessions without a recorded size/i }),
    ).not.toBeChecked();
  });

  it('loads previous values into the draft and writes only when Save is chosen', async () => {
    const user = userEvent.setup();
    const onSettingsChange = vi.fn();
    const previous = changedSettings({
      briefing: { reachHeading: 'Previous heading' },
      liveSessions: 6,
    });
    const platformSettings: StoredPlatformSettings = {
      revision: 4,
      settings: changedSettings({ briefing: { reachHeading: 'Current heading' } }),
      previous,
    };
    await open({ platformSettings, onSettingsChange });

    await user.click(screen.getByRole('button', { name: /use previous values/i }));
    expect(screen.getByLabelText(/granted-services heading/i)).toHaveValue('Previous heading');
    expect(screen.getByLabelText(/maximum live sessions/i)).toHaveValue(6);
    expect(onSettingsChange).not.toHaveBeenCalled();

    await user.click(screen.getByRole('button', { name: /save changes/i }));
    await waitFor(() => {
      expect(onSettingsChange).toHaveBeenCalledTimes(1);
    });
    expect((onSettingsChange.mock.calls[0]?.[0] as PlatformSettings).liveSessions).toBe(6);
  });

  it('resets only briefing prose and preserves capacity, bounds, and concurrency', async () => {
    const user = userEvent.setup();
    const onBriefingReset = vi.fn();
    const custom = changedSettings({
      briefing: { reachHeading: 'Custom heading' },
      sizeBounds: {
        minCpuMillis: 500,
        maxCpuMillis: 5000,
        minMemoryMib: 512,
        maxMemoryMib: 10_240,
      },
      deploymentCapacity: {
        cpuMillis: 12_000,
        memoryMib: 24_576,
        assumedSessionSize: { cpuMillis: 1_000, memoryMib: 1_024 },
      },
      liveSessions: 8,
    });
    const store = await open({
      platformSettings: { revision: 2, settings: custom },
      onBriefingReset,
    });

    await user.clear(screen.getByLabelText('Maximum CPU'));
    await user.type(screen.getByLabelText('Maximum CPU'), '6000');
    await user.click(screen.getByRole('button', { name: /reset briefing/i }));
    await waitFor(() => {
      expect(onBriefingReset).toHaveBeenCalledTimes(1);
    });

    const stored = await store.readPlatformSettings();
    expect(stored.settings.briefing.reachHeading).toBe(
      MOCK_PLATFORM_SETTINGS.settings.briefing.reachHeading,
    );
    expect(stored.settings.sizeBounds).toEqual(custom.sizeBounds);
    expect(stored.settings.deploymentCapacity).toEqual(custom.deploymentCapacity);
    expect(stored.settings.liveSessions).toBe(8);
    expect(screen.getByLabelText('Maximum CPU')).toHaveValue(6000);
    expect(screen.getByRole('button', { name: /save changes/i })).toBeEnabled();
  });

  it('refuses an inverted range before making a request', async () => {
    const user = userEvent.setup();
    const onSettingsChange = vi.fn();
    await open({ onSettingsChange });

    await user.clear(screen.getByLabelText('Minimum CPU'));
    await user.type(screen.getByLabelText('Minimum CPU'), '5000');
    await user.click(screen.getByRole('button', { name: /save changes/i }));

    expect(await screen.findByText(/minimum CPU cannot exceed maximum CPU/i)).toBeInTheDocument();
    expect(onSettingsChange).not.toHaveBeenCalled();
  });

  it('retains a stale draft and can save it against the newly read revision', async () => {
    const user = userEvent.setup();
    const store = await open();
    const heading = screen.getByLabelText(/granted-services heading/i);
    await user.clear(heading);
    await user.type(heading, 'Retained operator draft');

    const initial = await store.readPlatformSettings();
    await store.changePlatformSettings(
      initial.revision,
      changedSettings({ briefing: { reachHeading: 'Concurrent change' } }),
    );
    await user.click(screen.getByRole('button', { name: /save changes/i }));

    const conflictTitle = await screen.findByText(/settings changed elsewhere/i);
    expect(conflictTitle.closest('[role="alert"]')).toBeInTheDocument();
    expect(screen.getByLabelText(/granted-services heading/i)).toHaveValue(
      'Retained operator draft',
    );
    expect(screen.getByRole('button', { name: /load current values/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /review retained draft/i })).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: /review retained draft/i }));
    await user.click(screen.getByRole('button', { name: /save changes/i }));
    await waitFor(async () => {
      expect((await store.readPlatformSettings()).revision).toBe(2);
    });
    expect((await store.readPlatformSettings()).settings.briefing.reachHeading).toBe(
      'Retained operator draft',
    );
  });

  it('can discard a stale draft by loading the current values', async () => {
    const user = userEvent.setup();
    const store = await open();
    const heading = screen.getByLabelText(/granted-services heading/i);
    await user.clear(heading);
    await user.type(heading, 'Draft to discard');

    const initial = await store.readPlatformSettings();
    await store.changePlatformSettings(
      initial.revision,
      changedSettings({ briefing: { reachHeading: 'Current server heading' } }),
    );
    await user.click(screen.getByRole('button', { name: /save changes/i }));
    await user.click(await screen.findByRole('button', { name: /load current values/i }));

    expect(screen.getByLabelText(/granted-services heading/i)).toHaveValue(
      'Current server heading',
    );
    expect(screen.getByRole('button', { name: /save changes/i })).toBeDisabled();
  });

  it('offers the same explicit recovery when a briefing reset conflicts', async () => {
    const user = userEvent.setup();
    const store = await open();
    const initial = await store.readPlatformSettings();
    await store.changePlatformSettings(
      initial.revision,
      changedSettings({ sizeBounds: { maxCpuMillis: 5000 } }),
    );

    await user.click(screen.getByRole('button', { name: /reset briefing/i }));
    expect(await screen.findByText(/reset again against that revision/i)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /load current values/i })).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: /reset briefing/i }));
    await waitFor(async () => {
      expect((await store.readPlatformSettings()).revision).toBe(2);
    });
    const reset = await store.readPlatformSettings();
    expect(reset.settings.sizeBounds.maxCpuMillis).toBe(5000);
    expect(reset.settings.briefing.reachHeading).toBe(
      MOCK_PLATFORM_SETTINGS.settings.briefing.reachHeading,
    );
  });
});

describe('read failures', () => {
  it('reports the failure and can retry the same store', async () => {
    const user = userEvent.setup();
    const delegate = createMockOperatorStore();
    let attempts = 0;
    const store = {
      ...delegate,
      readPlatformSettings: async () => {
        attempts += 1;
        if (attempts === 1) {
          throw new Error('settings store is unavailable');
        }
        return delegate.readPlatformSettings();
      },
    };
    render(<PlatformSettingsView store={store} />);

    expect(await screen.findByRole('alert')).toHaveTextContent(/settings store is unavailable/i);
    await user.click(screen.getByRole('button', { name: /try again/i }));
    expect(await screen.findByRole('form', { name: 'Platform settings' })).toBeInTheDocument();
    expect(attempts).toBe(2);
  });
});
