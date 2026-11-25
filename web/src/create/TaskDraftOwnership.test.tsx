import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AGENT_DETAIL, agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { taskExperience } from '../test/taskExperience';
import { AgentLifecycleFollowup } from './AgentLifecycleFollowup';
import { lifecycleRecoveryKey, lifecycleStorageKey } from './lifecycleIntentStorage';
import { taskDraftStorageKey } from './taskDraft';
import { TaskHome } from './TaskHome';

afterEach(() => sessionStorage.clear());

it.each(['pending', 'bound'] as const)(
  'keeps an identical %s B draft owned by B when A completes',
  async (state) => {
    let completedA = false;
    let creations = 0;
    const client = agentClient({
      taskExperience: taskExperience('owner'),
      readTaskCreationSupport: async () => true,
      createAgentLifecycle: async () => ({ id: ++creations === 1 ? 'operation-a' : 'operation-b' }),
      readAgentLifecycle: async (id) => ({
        id,
        status: id === 'operation-a' && completedA ? 'succeeded' : 'running',
        phase: 'agent_startup',
        ...(id === 'operation-a' || state === 'bound'
          ? { agent_id: id === 'operation-a' ? 'agt_1' : 'agt_2' }
          : {}),
        events: [],
      }),
      readAgent: async (id) => ({
        ...AGENT_DETAIL,
        id,
        worktree: { ...AGENT_DETAIL.worktree, agentId: id },
      }),
      readSandbox: async () => ({ ...sandboxFixture(), owner: 'owner' }),
    });
    const opened = vi.fn();
    const home = <TaskHome client={client} viewerId="owner" canCreateSandbox onCreated={opened} />;
    const first = render(home);
    fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'hello' } });
    await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
    fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
    await waitFor(() => expect(opened).toHaveBeenCalledOnce());
    const draftKey = taskDraftStorageKey('owner');
    const originalRevision = JSON.parse(sessionStorage.getItem(draftKey)!).revision;
    first.unmount();
    const second = render(home);
    // Even submitting the unchanged text again establishes a new draft owner.
    await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
    fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
    await waitFor(() => expect(creations).toBe(2));
    const recoveryA = lifecycleRecoveryKey(
      { owner: 'owner' },
      { agentId: 'agt_1', operationId: 'operation-a' },
    );
    const slotB =
      state === 'bound'
        ? lifecycleRecoveryKey({ owner: 'owner' }, { agentId: 'agt_2', operationId: 'operation-b' })
        : lifecycleStorageKey('owner');
    await waitFor(() => expect(sessionStorage.getItem(slotB)).toContain('operation-b'));
    const requestB = sessionStorage.getItem(slotB);
    const draftB = sessionStorage.getItem(draftKey);
    expect(JSON.parse(draftB!).revision).not.toBe(originalRevision);
    completedA = true;
    second.rerender(
      <>
        {home}
        <AgentLifecycleFollowup client={client} viewerId="owner" agentId="agt_1" />
      </>,
    );
    await waitFor(() => expect(sessionStorage.getItem(recoveryA)).toBeNull());
    expect(sessionStorage.getItem(slotB)).toBe(requestB);
    expect(sessionStorage.getItem(draftKey)).toBe(draftB);
    expect(screen.getByRole('region', { name: 'Original request' })).toHaveTextContent('hello');
    expect(creations).toBe(2);
  },
);
