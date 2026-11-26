import { selectTaskTemplate } from '../test/taskTemplateSelection';
import { act, fireEvent, render, renderHook, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import { unavailableClient, type CmaWebClient } from '../api/client';
import { createRestWebClient } from '../api/rest';
import { AVAILABLE_CAPACITY } from '../test/capacity';
import { taskExperience } from '../test/taskExperience';
import { TaskHome } from './TaskHome';
import { legacyTaskDraftStorageKey, taskDraftStorageKey, useTaskDraft } from './taskDraft';
import { taskTemplates } from './taskTemplateModel';

const owner = 'synthetic-owner';
const retained = { id: 'research', version: 1, fields: { topic: 'Original topic' } } as const;
function seed() {
  const view = renderHook(() => useTaskDraft(owner));
  act(() => view.result.current.change('Independent free text'));
  act(() => view.result.current.changeTemplate(retained));
  view.unmount();
}
const saved = () => JSON.parse(sessionStorage.getItem(taskDraftStorageKey(owner))!);
function setup(overrides: Partial<CmaWebClient> = {}) {
  const create = vi.fn().mockRejectedValue(new Error('Synthetic response lost'));
  const client: CmaWebClient = {
    ...unavailableClient,
    taskExperience: taskExperience(owner),
    readTaskCreationSupport: async () => true,
    readTaskTemplateSupport: async () => true,
    readSandboxCapacity: async () => AVAILABLE_CAPACITY,
    createAgentLifecycle: create,
    ...overrides,
  };
  const view = render(
    <TaskHome client={client} viewerId={owner} canCreateSandbox onCreated={vi.fn()} />,
  );
  return { ...view, create };
}
async function ready() {
  await waitFor(() => expect(screen.queryByText('Checking task creation…')).toBeNull());
}

it.each(['off', 'unsupported', 'missing', 'checking', 'error'] as const)(
  'keeps v1 editable without promoting local metadata when v2 is %s',
  async (mode) => {
    seed();
    const view = setup({
      taskExperience: taskExperience(owner, mode !== 'off'),
      readTaskTemplateV2Support:
        mode === 'missing'
          ? undefined
          : async () => {
              if (mode === 'checking') return new Promise<boolean>(() => {});
              if (mode === 'error') throw new Error('Synthetic unavailable');
              return mode === 'off';
            },
    });
    await ready();
    fireEvent.change(screen.getByLabelText('Topic'), { target: { value: 'Edited topic' } });
    expect(saved().template).toEqual({ ...retained, fields: { topic: 'Edited topic' } });
    expect(saved().text).toBe('Independent free text');
    selectTaskTemplate('Write code');
    expect(saved().template.version).toBe(1);
    expect(view.create).not.toHaveBeenCalled();
  },
);

it('does not upgrade legacy hydration, capability arrival, reselection or an unchanged field', async () => {
  sessionStorage.setItem(
    legacyTaskDraftStorageKey(owner),
    JSON.stringify({ version: 1, text: taskTemplates[0].legacy }),
  );
  let finish!: (value: boolean) => void;
  const view = setup({
    readTaskTemplateV2Support: () =>
      new Promise<boolean>((resolve) => {
        finish = resolve;
      }),
  });
  await ready();
  expect(saved().template.version).toBe(1);
  await act(async () => finish(true));
  expect(saved().template.version).toBe(1);
  selectTaskTemplate('Research a topic');
  fireEvent.change(screen.getByLabelText('Topic'), { target: { value: '' } });
  expect(saved().template.version).toBe(1);
  fireEvent.change(screen.getByLabelText('Topic'), { target: { value: 'Explicit new topic' } });
  expect(saved().template.version).toBe(2);
  expect(view.create).not.toHaveBeenCalled();
});

it.each([true, false, undefined, 'true', 'error'] as const)(
  'admits an unchanged v1 draft using its own real REST capability, v2=%s',
  async (v2) => {
    seed();
    const sdk = createClient({
      fetch: async () => {
        if (v2 === 'error') throw new Error('Synthetic unavailable');
        return Response.json({
          openapi: '3.1.0',
          info: { title: 'Synthetic', version: '1' },
          paths: {},
          'x-cma-task-templates-v1': true,
          'x-cma-task-templates-v2': v2,
        });
      },
    });
    const rest = createRestWebClient(sdk);
    const view = setup({
      readTaskTemplateSupport: rest.readTaskTemplateSupport,
      readTaskTemplateV2Support: rest.readTaskTemplateV2Support,
    });
    await ready();
    await waitFor(() => expect(screen.queryByText('Checking template availability…')).toBeNull());
    expect(saved().template).toEqual(retained);
    const start = screen.getByRole('button', { name: 'Start task' });
    if (v2 === 'error') expect(start).toBeDisabled();
    else expect(start).toBeEnabled();
    expect(view.create).not.toHaveBeenCalled();
  },
);

it('keeps a retained v2 draft and refuses v2 submission on a v1-only service without downgrading it', async () => {
  const draft = renderHook(() => useTaskDraft(owner));
  act(() => draft.result.current.changeTemplate({ ...retained, version: 2 }));
  draft.unmount();
  const view = setup({ readTaskTemplateV2Support: async () => false });
  await ready();
  await screen.findByText('Template tasks are not available on this service.');
  fireEvent.change(screen.getByLabelText('Topic'), { target: { value: 'Retained v2 edit' } });
  expect(saved().template.version).toBe(2);
  expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
  fireEvent.submit(screen.getByRole('button', { name: 'Start task' }).closest('form')!);
  expect(view.create).not.toHaveBeenCalled();
});

it('replays a retained v2 request with its original body/key after the service loses v2 support', async () => {
  const first = setup({ readTaskTemplateV2Support: async () => true });
  await ready();
  selectTaskTemplate('Research a topic');
  fireEvent.change(screen.getByLabelText('Topic'), { target: { value: 'Original v2 command' } });
  fireEvent.click(screen.getByRole('button', { name: 'Start task' }));
  await screen.findByRole('button', { name: 'Recover original task' });
  const original = first.create.mock.calls[0]!;
  expect(original[0].template.version).toBe(2);
  first.unmount();
  const next = setup({ readTaskTemplateV2Support: async () => false });
  await ready();
  expect(next.create).not.toHaveBeenCalled();
  fireEvent.click(await screen.findByRole('button', { name: 'Recover original task' }));
  await waitFor(() => expect(next.create).toHaveBeenCalledOnce());
  expect(next.create.mock.calls[0]![0]).toEqual(original[0]);
  expect(next.create.mock.calls[0]![1].idempotencyKey).toBe(original[1].idempotencyKey);
});
