import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { AgentLifecycleOperationView, CmaWebClient } from '../api/client';
import { agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { taskExperience } from '../test/taskExperience';
import { readPreparationDraft } from '../preparation/preparationDraftStorage';
import { TaskHome } from './TaskHome';
import { lifecycleStorageKey } from './lifecycleIntentStorage';

afterEach(() => vi.restoreAllMocks());
const request = '  Original task\n中文 and exact spacing  ';
const followup = '  Follow-up only\nNever send automatically  ';
function fixture(overrides: Partial<CmaWebClient> = {}) {
  const create = vi.fn(async () => ({ id: 'operation-1', status: 'provisioning' as const }));
  const client = agentClient({
    taskExperience: taskExperience('owner'),
    readTaskCreationSupport: async () => true,
    readTaskTitleModeSupport: async () => false,
    createAgentLifecycle: create,
    readAgentLifecycle: async () => ({
      id: 'operation-1',
      status: 'queued',
      phase: 'queued',
      events: [],
    }),
    readSandbox: async () => ({ ...sandboxFixture(), owner: 'owner' }),
    ...overrides,
  });
  const navigate = vi.fn();
  const draw = () => (
    <TaskHome
      client={client}
      viewerId="owner"
      canCreateSandbox
      onCreated={navigate}
      home={{ navigation: { status: 'ready', items: [] } }}
    />
  );
  return { create, client, navigate, draw };
}
async function submit() {
  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: request } });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start task' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  return screen.findByRole('textbox', { name: 'Follow-up draft' });
}

it('renders one preparation host and keeps the original request separate from a reloadable unsent draft', async () => {
  const h = fixture();
  let view = render(h.draw());
  const input = await submit();
  await screen.findByText('Getting its cloud computer ready');
  expect(screen.getAllByRole('heading', { name: 'New agent' })).toHaveLength(1);
  expect(screen.queryByText('Task request')).not.toBeInTheDocument();
  expect(screen.queryByRole('link', { name: 'Help starting a task' })).not.toBeInTheDocument();
  expect(screen.getByRole('region', { name: 'Original request' }).textContent).toBe(request);
  expect(screen.getAllByText('Setup details')).toHaveLength(1);
  expect(document.querySelectorAll('.agent-mascot__artwork')).toHaveLength(1);
  fireEvent.change(input, { target: { value: followup } });
  fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true });
  fireEvent.keyDown(input, { key: 'Enter', metaKey: true });
  expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
  const original = sessionStorage.getItem(lifecycleStorageKey('owner'));
  expect(h.create).toHaveBeenCalledTimes(1);
  view.unmount();
  view = render(h.draw());
  expect(await screen.findByRole('textbox', { name: 'Follow-up draft' })).toHaveValue(followup);
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toBe(original);
  expect(h.create).toHaveBeenCalledTimes(1);
  expect(h.navigate).not.toHaveBeenCalled();
  view.unmount();
});

it.each([false, true])(
  'binds the same source before navigation, retaining memory-only text on storage failure=%s',
  async (storageFailure) => {
    let finish!: (value: AgentLifecycleOperationView) => void;
    const operation = new Promise<AgentLifecycleOperationView>((resolve) => {
      finish = resolve;
    });
    const h = fixture({ readAgentLifecycle: async () => operation });
    render(h.draw());
    const input = await submit();
    fireEvent.change(input, { target: { value: followup } });
    const raw = sessionStorage.getItem(lifecycleStorageKey('owner'))!;
    const saved = JSON.parse(raw) as { intent: { key: string } };
    const scope = { owner: 'owner', intentKey: saved.intent.key };
    const set = storageFailure
      ? vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
          throw new Error('Full');
        })
      : undefined;
    await act(async () => {
      finish({
        id: 'operation-1',
        status: 'succeeded',
        phase: 'complete',
        agent_id: 'agt_1',
        events: [],
      });
    });
    if (storageFailure) {
      await screen.findByText(/This draft could not be saved/);
      expect(h.navigate).not.toHaveBeenCalled();
      expect(input).toHaveValue(followup);
      expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toBe(raw);
      set!.mockRestore();
      fireEvent.click(screen.getByRole('button', { name: 'Retry saving draft' }));
    }
    await waitFor(() =>
      expect(h.navigate).toHaveBeenCalledWith({ kind: 'agent', agentId: 'agt_1' }),
    );
    expect(readPreparationDraft(scope)).toMatchObject({
      text: followup,
      binding: {
        operationId: 'operation-1',
        agentId: 'agt_1',
        sandboxId: 'sbx_1',
        workspaceId: 'wsp_1',
        worktreeId: 'wkt_1',
      },
    });
    expect(h.create).toHaveBeenCalledTimes(1);
  },
);
