import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { testClient } from '../test/client';
import { AgentPreferencesError } from '../api/rest/agentPreferences';
import { AgentPreferencesDialog } from './AgentPreferencesDialog';
import type { AgentPreferencesView } from '../api/client';

const original = {
  title: 'Original',
  system_prompt: 'Original prompt',
  revision: 3,
  prompt_pending: false,
  prompt_supported: true,
};
const conflict = () =>
  new AgentPreferencesError(
    'Private operation details',
    409,
    undefined,
    'agent_operation_in_flight',
    'stale_revision',
  );
const read = () => vi.fn().mockResolvedValue(original);
function form(
  client: ReturnType<typeof testClient>,
  mode: 'rename' | 'prompt',
  agentId = 'agent-one',
  close = vi.fn(),
  title = vi.fn(),
) {
  return (
    <CmaClientProvider client={client}>
      <AgentPreferencesDialog
        agentId={agentId}
        mode={mode}
        currentTitle="Original"
        active={false}
        onClose={close}
        onTitleUpdated={title}
      />
    </CmaClientProvider>
  );
}
const input = (mode: 'rename' | 'prompt') =>
  screen.getByRole('textbox', {
    name: mode === 'rename' ? 'Agent name' : 'Agent system prompt',
  });
const flush = () => act(() => vi.advanceTimersByTimeAsync(0));
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

for (const mode of ['rename', 'prompt'] as const) {
  it.each([
    ['lost', 401],
    ['lost', 403],
    ['503', 401],
    ['503', 403],
    ['lost', 404],
    ['lost', 410],
    ['lost', 400],
    ['lost', 422],
    ['lost', 409],
  ] as const)(`keeps an uncertain ${mode} command after %s then %i`, async (first, status) => {
    const updates = vi
      .fn()
      .mockRejectedValueOnce(
        first === 'lost'
          ? new TypeError('response lost')
          : new AgentPreferencesError('private', 503),
      )
      .mockRejectedValueOnce(new AgentPreferencesError('private', status))
      .mockResolvedValue({ ...original, title: 'Changed', system_prompt: 'Changed', revision: 4 });
    const reads = read()
      .mockResolvedValueOnce(original)
      .mockResolvedValue({ ...original, title: 'Changed', system_prompt: 'Changed', revision: 9 });
    const close = vi.fn();
    render(
      form(
        testClient({ readAgentPreferences: reads, updateAgentPreferences: updates }),
        mode,
        'agent-one',
        close,
      ),
    );
    fireEvent.change(await screen.findByRole('textbox'), { target: { value: 'Changed' } });
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Save' })));
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Retry change' })));
    expect(updates).toHaveBeenCalledTimes(2);
    expect(screen.getByText(/Leaving loses the local retry details/)).toBeVisible();
    const retry = () => screen.getByRole('button', { name: 'Retry change' });
    expect(retry()).toBeDisabled();
    fireEvent.click(retry());
    expect(updates).toHaveBeenCalledTimes(2);
    const unload = new Event('beforeunload', { cancelable: true });
    window.dispatchEvent(unload);
    expect(unload.defaultPrevented).toBe(true);
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(close).not.toHaveBeenCalled();
    expect(screen.getByText(/This does not cancel a server update/)).toBeVisible();
    fireEvent.click(screen.getByRole('button', { name: 'Keep editing' }));
    fireEvent.click(screen.getByRole('button', { name: 'Reload preferences' }));
    await waitFor(() => expect(retry()).toBeEnabled());
    expect(reads).toHaveBeenCalledTimes(2);
    expect(updates).toHaveBeenCalledTimes(2);
    expect(screen.getByText('Current preferences revision: 9.')).toBeVisible();
    expect(screen.getByText(/Leaving loses the local retry details/)).toBeVisible();
    expect(screen.queryByText('The changes are applied to this Agent.')).toBeNull();
    expect(close).not.toHaveBeenCalled();
    await act(async () => fireEvent.click(retry()));
    expect(updates).toHaveBeenCalledTimes(3);
    for (const call of updates.mock.calls.slice(1)) {
      expect(call[1]).toEqual(updates.mock.calls[0]?.[1]);
      expect(call[2].idempotencyKey).toBe(updates.mock.calls[0]?.[2].idempotencyKey);
    }
    expect(updates.mock.calls[2]?.[1].expected_revision).toBe(3);
    expect(close).toHaveBeenCalledOnce();
  });
}

it.each(['rename', 'prompt'] as const)(
  'preserves %s through a conflict refresh and edits made while that read is waiting',
  async (mode) => {
    let finish!: (value: AgentPreferencesView) => void;
    const reread = new Promise<AgentPreferencesView>((resolve) => {
      finish = resolve;
    });
    const reads = read().mockResolvedValueOnce(original).mockReturnValueOnce(reread);
    const updates = vi
      .fn()
      .mockRejectedValueOnce(conflict())
      .mockResolvedValue({ ...original, revision: 9 });
    render(
      form(testClient({ readAgentPreferences: reads, updateAgentPreferences: updates }), mode),
    );
    await screen.findByRole('textbox');
    fireEvent.change(input(mode), { target: { value: 'My proposal' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await screen.findByRole('alert');
    expect(input(mode)).toHaveValue('My proposal');
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Reload preferences' }));
    await waitFor(() => expect(reads).toHaveBeenCalledTimes(2));
    fireEvent.change(input(mode), { target: { value: 'A newer proposal' } });
    await act(async () =>
      finish({ ...original, title: 'Elsewhere', system_prompt: 'Elsewhere', revision: 8 }),
    );
    expect(input(mode)).toHaveValue('A newer proposal');
    expect(screen.getByText('Current preferences revision: 8.')).toBeVisible();
    expect(updates).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(updates).toHaveBeenCalledTimes(2));
    expect(updates.mock.calls[1]?.[1]).toEqual({
      expected_revision: 8,
      ...(mode === 'rename'
        ? { title: 'A newer proposal' }
        : { system_prompt: 'A newer proposal' }),
    });
    expect(updates.mock.calls[1]?.[2].idempotencyKey).not.toBe(
      updates.mock.calls[0]?.[2].idempotencyKey,
    );
  },
);

it.each([429, 503])(
  'keeps one intent for %i and prevents every button from bypassing Retry-After',
  async (status) => {
    vi.useFakeTimers();
    const reads = read();
    const updates = vi
      .fn()
      .mockRejectedValueOnce(new AgentPreferencesError('private', status, 3000))
      .mockResolvedValueOnce({ ...original, title: 'New title', revision: 4 });
    render(
      form(testClient({ readAgentPreferences: reads, updateAgentPreferences: updates }), 'rename'),
    );
    await flush();
    fireEvent.change(input('rename'), { target: { value: 'New title' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    fireEvent.click(screen.getByRole('button', { name: 'Saving…' }));
    await flush();
    expect(updates).toHaveBeenCalledTimes(1);
    expect(input('rename')).toHaveValue('New title');
    expect(screen.getByRole('button', { name: 'Retry change' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Reload preferences' })).toBeDisabled();
    await act(() => vi.advanceTimersByTimeAsync(2999));
    fireEvent.click(screen.getByRole('button', { name: 'Retry change' }));
    expect(updates).toHaveBeenCalledTimes(1);
    await act(() => vi.advanceTimersByTimeAsync(1));
    expect(reads).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole('button', { name: 'Retry change' }));
    await flush();
    expect(updates).toHaveBeenCalledTimes(2);
    expect(updates.mock.calls[1]?.[1]).toEqual(updates.mock.calls[0]?.[1]);
    expect(updates.mock.calls[1]?.[2].idempotencyKey).toBe(
      updates.mock.calls[0]?.[2].idempotencyKey,
    );
  },
);

it('keeps a prompt pending until its read confirms application, without another write', async () => {
  vi.useFakeTimers();
  const pending = { ...original, revision: 4, system_prompt: 'New prompt', prompt_pending: true };
  const reads = read()
    .mockResolvedValueOnce(original)
    .mockResolvedValueOnce(pending)
    .mockResolvedValue({ ...pending, prompt_pending: false });
  const updates = vi.fn().mockResolvedValue(pending);
  const close = vi.fn();
  render(
    form(
      testClient({ readAgentPreferences: reads, updateAgentPreferences: updates }),
      'prompt',
      'agent-one',
      close,
    ),
  );
  await flush();
  fireEvent.change(input('prompt'), { target: { value: 'New prompt' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await flush();
  expect(screen.getByText('A system prompt update is still being applied.')).toBeVisible();
  expect(screen.queryByText('The changes are applied to this Agent.')).toBeNull();
  expect(close).not.toHaveBeenCalled();
  await act(() => vi.advanceTimersByTimeAsync(2100));
  expect(screen.getByText('The changes are applied to this Agent.')).toBeVisible();
  expect(updates).toHaveBeenCalledOnce();
});

it.each([401, 403, 404, 410])(
  'does not mutate after %i until an explicit authorized refresh succeeds',
  async (status) => {
    const reads = read();
    const updates = vi
      .fn()
      .mockRejectedValueOnce(new AgentPreferencesError('private', status))
      .mockResolvedValue({ ...original, title: 'New title', revision: 4 });
    render(
      form(testClient({ readAgentPreferences: reads, updateAgentPreferences: updates }), 'rename'),
    );
    await screen.findByRole('textbox');
    fireEvent.change(input('rename'), { target: { value: 'New title' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await screen.findByRole('alert');
    expect(screen.getByText(/Leaving loses the local retry details/)).toBeVisible();
    expect(screen.getByRole('button', { name: 'Retry change' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Retry change' }));
    expect(updates).toHaveBeenCalledOnce();
    fireEvent.click(screen.getByRole('button', { name: 'Reload preferences' }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Retry change' })).toBeEnabled());
    expect(input('rename')).toHaveValue('New title');
    expect(updates).toHaveBeenCalledOnce();
    expect(screen.queryByText('The changes are applied to this Agent.')).toBeNull();
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Retry change' })));
    expect(updates.mock.calls[1]?.[1]).toEqual(updates.mock.calls[0]?.[1]);
    expect(updates.mock.calls[1]?.[2].idempotencyKey).toBe(
      updates.mock.calls[0]?.[2].idempotencyKey,
    );
  },
);

it('requires acknowledgement before closing an unconfirmed update after a denied replay', async () => {
  const close = vi.fn();
  const updates = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('lost response'))
    .mockRejectedValue(new AgentPreferencesError('private', 403));
  render(
    form(
      testClient({ readAgentPreferences: read(), updateAgentPreferences: updates }),
      'rename',
      'agent-one',
      close,
    ),
  );
  await screen.findByRole('textbox');
  fireEvent.change(input('rename'), { target: { value: 'New title' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  const retry = await screen.findByRole('button', { name: 'Retry change' });
  await act(async () => fireEvent.click(retry));
  expect(screen.getByText(/Leaving loses the local retry details/)).toBeVisible();
  const unload = new Event('beforeunload', { cancelable: true });
  window.dispatchEvent(unload);
  expect(unload.defaultPrevented).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(close).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Keep editing' }));
  expect(input('rename')).toHaveValue('New title');
  fireEvent.click(screen.getByRole('button', { name: 'Close' }));
  fireEvent.click(screen.getByRole('button', { name: 'Discard local changes and close' }));
  expect(close).toHaveBeenCalledOnce();
  expect(updates).toHaveBeenCalledTimes(2);
});

it('ignores a late update after switching Agents', async () => {
  let finish!: (view: AgentPreferencesView) => void;
  const updates = vi.fn().mockReturnValue(
    new Promise<AgentPreferencesView>((resolve) => {
      finish = resolve;
    }),
  );
  const client = testClient({ readAgentPreferences: read(), updateAgentPreferences: updates });
  const close = vi.fn();
  const title = vi.fn();
  const page = render(form(client, 'rename', 'agent-one', close, title));
  await screen.findByRole('textbox');
  fireEvent.change(input('rename'), { target: { value: 'Old Agent title' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  page.rerender(form(client, 'rename', 'agent-two', close, title));
  await waitFor(() => expect(input('rename')).toHaveValue('Original'));
  expect(updates.mock.calls[0]?.[2].signal.aborted).toBe(true);
  await act(async () => finish({ ...original, title: 'Old Agent title' }));
  expect(input('rename')).toHaveValue('Original');
  expect(close).not.toHaveBeenCalled();
  expect(title).not.toHaveBeenCalled();
  expect(updates).toHaveBeenCalledOnce();
});
