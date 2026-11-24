import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { agentClient } from '../test/agent';
import type { AgentSettingsView, CmaWebClient } from '../api/client';
import { AgentSettingsError } from '../api/rest/agentSettings';
import { SessionSettings } from './SessionSettings';

const defaults: AgentSettingsView = {
  model: 'gpt-6',
  reasoning_effort: 'high',
  permission_mode: 'autonomous',
  pending: false,
};
function draw(
  client: ReturnType<typeof agentClient>,
  agentId = 'agent-one',
  authorizeUpdate?: (signal: AbortSignal) => Promise<void>,
  active = true,
) {
  return (
    <CmaClientProvider client={client}>
      <SessionSettings
        agentId={agentId}
        active={active}
        runtimeReady
        busy={false}
        authorizeUpdate={authorizeUpdate}
      />
    </CmaClientProvider>
  );
}
const clientFor = (
  read: NonNullable<CmaWebClient['readAgentSettings']>,
  update: NonNullable<CmaWebClient['updateAgentSettings']>,
) =>
  agentClient({
    readAgentSettings: read,
    updateAgentSettings: update,
    readAgentModels: async () => [{ id: 'gpt-6', label: 'GPT-6', available: true }],
  });
async function expand() {
  fireEvent.click(await screen.findByRole('button', { name: /^Agent settings:/ }));
}
function permission(label: 'Guarded' | 'Autonomous') {
  fireEvent.click(screen.getByRole('button', { name: 'Permission mode' }));
  fireEvent.click(screen.getByRole('menuitemradio', { name: label }));
}
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

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
] as const)('keeps an uncertain settings command after %s then %i', async (first, status) => {
  const updates = vi
    .fn()
    .mockRejectedValueOnce(
      first === 'lost' ? new TypeError('response lost') : new AgentSettingsError('private', 503),
    )
    .mockRejectedValueOnce(new AgentSettingsError('private', status))
    .mockResolvedValue({ ...defaults, permission_mode: 'guarded' });
  const reads = vi
    .fn()
    .mockResolvedValueOnce(defaults)
    .mockResolvedValue({ ...defaults, permission_mode: 'guarded', reasoning_effort: 'low' });
  render(draw(clientFor(reads, updates)));
  await expand();
  permission('Guarded');
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Apply settings' })));
  await act(async () =>
    fireEvent.click(screen.getByRole('button', { name: 'Retry settings update' })),
  );
  expect(updates).toHaveBeenCalledTimes(2);
  expect(screen.getByText(/Leaving loses the local retry details/)).toBeVisible();
  const retry = () => screen.getByRole('button', { name: 'Retry settings update' });
  expect(retry()).toBeDisabled();
  fireEvent.click(retry());
  const unload = new Event('beforeunload', { cancelable: true });
  window.dispatchEvent(unload);
  expect(unload.defaultPrevented).toBe(true);
  expect(updates).toHaveBeenCalledTimes(2);
  fireEvent.click(screen.getByRole('button', { name: 'Refresh current settings' }));
  await waitFor(() => expect(retry()).toBeEnabled());
  expect(reads).toHaveBeenCalledTimes(2);
  expect(updates).toHaveBeenCalledTimes(2);
  expect(screen.getByText(/Leaving loses the local retry details/)).toBeVisible();
  expect(screen.queryByText('Settings applied to this Agent.')).toBeNull();
  await act(async () => fireEvent.click(retry()));
  expect(updates).toHaveBeenCalledTimes(3);
  for (const call of updates.mock.calls.slice(1)) {
    expect(call[1]).toEqual(updates.mock.calls[0]?.[1]);
    expect(call[2].idempotencyKey).toBe(updates.mock.calls[0]?.[2].idempotencyKey);
  }
  expect(screen.getByText('Settings applied to this Agent.')).toBeVisible();
  expect(screen.queryByText(/Leaving loses the local retry details/)).toBeNull();
});

it('does not mistake pending=false metadata for an unsaved proposal', async () => {
  render(draw(clientFor(vi.fn().mockResolvedValue(defaults), vi.fn())));
  await expand();
  expect(screen.queryByRole('button', { name: 'Apply settings' })).toBeNull();
});

it('keeps only edited fields after conflict refresh and does not infer an active turn', async () => {
  const reads = vi
    .fn()
    .mockResolvedValueOnce(defaults)
    .mockResolvedValue({ ...defaults, reasoning_effort: 'low' });
  const updates = vi
    .fn()
    .mockRejectedValueOnce(
      new AgentSettingsError(
        'The turn is active.',
        409,
        undefined,
        'agent_operation_in_flight',
        'admission_conflict',
      ),
    )
    .mockResolvedValue({ ...defaults, reasoning_effort: 'low', permission_mode: 'guarded' });
  render(draw(clientFor(reads, updates)));
  await expand();
  permission('Guarded');
  fireEvent.click(screen.getByRole('button', { name: 'Apply settings' }));
  const error = await screen.findByRole('alert');
  expect(error).toHaveTextContent('The Agent changed during the update.');
  expect(error).not.toHaveTextContent('The turn is active.');
  expect(screen.getByRole('button', { name: /^Agent settings:/ })).toHaveAccessibleName(
    /High.*Autonomous/,
  );
  expect(screen.getByRole('button', { name: 'Apply settings' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Refresh current settings' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Apply settings' })).toBeEnabled());
  expect(screen.getByRole('button', { name: /^Agent settings:/ })).toHaveAccessibleName(
    /Low.*Autonomous/,
  );
  expect(screen.getByRole('button', { name: /^Model and reasoning effort:/ })).toHaveAccessibleName(
    /Low/,
  );
  expect(screen.getByRole('button', { name: 'Permission mode' })).toHaveTextContent('Guarded');
  expect(screen.getByText('Unapplied changes to permission mode.')).toBeVisible();
  expect(screen.queryByText('Settings applied to this Agent.')).toBeNull();
  expect(updates).toHaveBeenCalledOnce();
  fireEvent.click(screen.getByRole('button', { name: 'Apply settings' }));
  await screen.findByText('Settings applied to this Agent.');
  expect(updates).toHaveBeenCalledTimes(2);
  expect(updates.mock.calls[1]?.[1]).toEqual({
    model: 'gpt-6',
    reasoning_effort: 'low',
    permission_mode: 'guarded',
  });
  expect(updates.mock.calls[1]?.[2].idempotencyKey).not.toBe(
    updates.mock.calls[0]?.[2].idempotencyKey,
  );
  expect(screen.getByRole('button', { name: /^Agent settings:/ })).toHaveAccessibleName(
    /Low.*Guarded/,
  );
  expect(screen.queryByText(/Unapplied changes/)).toBeNull();
});

it('preserves a new edit during a refresh even when it returns to the previous baseline', async () => {
  let finish!: (view: AgentSettingsView) => void;
  const reads = vi
    .fn()
    .mockResolvedValueOnce(defaults)
    .mockReturnValue(
      new Promise<AgentSettingsView>((resolve) => {
        finish = resolve;
      }),
    );
  const updates = vi
    .fn()
    .mockRejectedValue(
      new AgentSettingsError(
        'private',
        409,
        undefined,
        'agent_operation_in_flight',
        'admission_conflict',
      ),
    );
  render(draw(clientFor(reads, updates)));
  await expand();
  permission('Guarded');
  fireEvent.click(screen.getByRole('button', { name: 'Apply settings' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Refresh current settings' }));
  await waitFor(() => expect(reads).toHaveBeenCalledTimes(2));
  permission('Autonomous');
  await act(async () =>
    finish({ ...defaults, reasoning_effort: 'low', permission_mode: 'guarded' }),
  );
  expect(screen.getByRole('button', { name: /^Agent settings:/ })).toHaveAccessibleName(
    /Low.*Guarded/,
  );
  expect(screen.getByRole('button', { name: /^Model and reasoning effort:/ })).toHaveAccessibleName(
    /Low/,
  );
  expect(screen.getByRole('button', { name: 'Permission mode' })).toHaveTextContent('Autonomous');
  expect(screen.getByText('Unapplied changes to permission mode.')).toBeVisible();
  expect(updates).toHaveBeenCalledOnce();
});

it.each([
  [429, undefined, /Too many requests/],
  [503, undefined, /The update outcome is unconfirmed/],
  [503, 'agent_owner_unavailable', /could not confirm the update with the server responsible/],
] as const)(
  'retains settings intent and deadline after %i (%s), including double clicks',
  async (status, code, message) => {
    vi.useFakeTimers();
    const updates = vi
      .fn()
      .mockRejectedValueOnce(new AgentSettingsError('private', status, 2500, code))
      .mockResolvedValue({ ...defaults, permission_mode: 'guarded' });
    render(draw(clientFor(vi.fn().mockResolvedValue(defaults), updates)));
    await act(() => vi.advanceTimersByTimeAsync(0));
    fireEvent.click(screen.getByRole('button', { name: /^Agent settings:/ }));
    permission('Guarded');
    fireEvent.click(screen.getByRole('button', { name: 'Apply settings' }));
    fireEvent.click(screen.getByRole('button', { name: 'Applying settings…' }));
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(updates).toHaveBeenCalledOnce();
    expect(screen.getByRole('alert')).toHaveTextContent(message);
    expect(screen.getByRole('button', { name: 'Retry settings update' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Refresh current settings' })).toBeDisabled();
    await act(() => vi.advanceTimersByTimeAsync(2499));
    fireEvent.click(screen.getByRole('button', { name: 'Retry settings update' }));
    expect(updates).toHaveBeenCalledOnce();
    await act(() => vi.advanceTimersByTimeAsync(1));
    fireEvent.click(screen.getByRole('button', { name: 'Retry settings update' }));
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(updates).toHaveBeenCalledTimes(2);
    expect(updates.mock.calls[1]?.[1]).toEqual(updates.mock.calls[0]?.[1]);
    expect(updates.mock.calls[1]?.[2].idempotencyKey).toBe(
      updates.mock.calls[0]?.[2].idempotencyKey,
    );
  },
);

it.each([
  ['a lost response', new TypeError('response lost')],
  [
    'an ambiguous conflict',
    new AgentSettingsError('private', 409, undefined, 'agent_delivery_ambiguous'),
  ],
])('keeps the original command after %s even when the GET matches', async (_, failure) => {
  const reads = vi
    .fn()
    .mockResolvedValueOnce(defaults)
    .mockResolvedValue({ ...defaults, permission_mode: 'guarded' });
  const updates = vi.fn().mockRejectedValue(failure);
  render(draw(clientFor(reads, updates)));
  await expand();
  permission('Guarded');
  fireEvent.click(screen.getByRole('button', { name: 'Apply settings' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Refresh current settings' }));
  await screen.findByRole('button', { name: /Agent settings:.*High.*Guarded/ });
  expect(reads).toHaveBeenCalledTimes(2);
  expect(updates).toHaveBeenCalledOnce();
  expect(screen.queryByText('Settings applied to this Agent.')).toBeNull();
  expect(screen.getByText(/Leaving loses the local retry details/)).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Retry settings update' }));
  await waitFor(() => expect(updates).toHaveBeenCalledTimes(2));
  expect(JSON.stringify(updates.mock.calls[1]?.[1])).toBe(
    JSON.stringify(updates.mock.calls[0]?.[1]),
  );
  expect(updates.mock.calls[1]?.[2].idempotencyKey).toBe(updates.mock.calls[0]?.[2].idempotencyKey);
});

it.each([401, 403, 404, 410])('blocks more settings writes after %i', async (status) => {
  const updates = vi
    .fn()
    .mockRejectedValueOnce(new AgentSettingsError('private', status))
    .mockResolvedValue({ ...defaults, permission_mode: 'guarded' });
  const reads = vi.fn().mockResolvedValue(defaults);
  render(draw(clientFor(reads, updates)));
  await expand();
  permission('Guarded');
  fireEvent.click(screen.getByRole('button', { name: 'Apply settings' }));
  await screen.findByRole('alert');
  expect(screen.getByText(/Leaving loses the local retry details/)).toBeVisible();
  expect(screen.getByRole('button', { name: 'Retry settings update' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Retry settings update' }));
  expect(updates).toHaveBeenCalledOnce();
  fireEvent.click(screen.getByRole('button', { name: 'Refresh current settings' }));
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Retry settings update' })).toBeEnabled(),
  );
  expect(reads).toHaveBeenCalledTimes(2);
  expect(updates).toHaveBeenCalledOnce();
  expect(screen.queryByText('Settings applied to this Agent.')).toBeNull();
  await act(async () =>
    fireEvent.click(screen.getByRole('button', { name: 'Retry settings update' })),
  );
  expect(updates.mock.calls[1]?.[1]).toEqual(updates.mock.calls[0]?.[1]);
  expect(updates.mock.calls[1]?.[2].idempotencyKey).toBe(updates.mock.calls[0]?.[2].idempotencyKey);
});

it('keeps an in-flight authorized update through parent rerenders, but discards its result after switching Agent', async () => {
  let finish!: (view: AgentSettingsView) => void;
  const updates = vi.fn().mockReturnValue(
    new Promise<AgentSettingsView>((resolve) => {
      finish = resolve;
    }),
  );
  const client = clientFor(vi.fn().mockResolvedValue(defaults), updates);
  const page = render(draw(client, 'agent-one', async () => {}));
  await expand();
  permission('Guarded');
  fireEvent.click(screen.getByRole('button', { name: 'Apply settings' }));
  await waitFor(() => expect(updates).toHaveBeenCalledOnce());
  page.rerender(draw(client, 'agent-one', async () => {}));
  expect(updates.mock.calls[0]?.[2].signal.aborted).toBe(false);
  expect(screen.getByText(/before switching Agents or leaving/)).toBeVisible();
  page.rerender(draw(client, 'agent-two', async () => {}));
  await expand();
  expect(updates.mock.calls[0]?.[2].signal.aborted).toBe(true);
  await act(async () => finish({ ...defaults, permission_mode: 'guarded' }));
  expect(screen.getByRole('button', { name: 'Permission mode' })).toHaveTextContent('Autonomous');
  expect(screen.queryByText('Settings applied to this Agent.')).toBeNull();
  expect(updates).toHaveBeenCalledOnce();
});

it('retains an interrupted save for explicit retry when access becomes certain again', async () => {
  let finish!: (view: AgentSettingsView) => void;
  const updates = vi
    .fn()
    .mockReturnValueOnce(
      new Promise<AgentSettingsView>((resolve) => {
        finish = resolve;
      }),
    )
    .mockResolvedValue({ ...defaults, permission_mode: 'guarded' });
  const reads = vi.fn().mockResolvedValue(defaults);
  const client = clientFor(reads, updates);
  const page = render(draw(client));
  await expand();
  permission('Guarded');
  fireEvent.click(screen.getByRole('button', { name: 'Apply settings' }));
  await waitFor(() => expect(updates).toHaveBeenCalledOnce());
  page.rerender(draw(client, 'agent-one', undefined, false));
  expect(updates.mock.calls[0]?.[2].signal.aborted).toBe(true);
  expect(screen.getByRole('button', { name: 'Retry settings update' })).toBeDisabled();
  await act(async () => finish({ ...defaults, permission_mode: 'guarded' }));
  expect(screen.queryByText('Settings applied to this Agent.')).toBeNull();
  page.rerender(draw(client));
  await waitFor(() => expect(reads).toHaveBeenCalledTimes(2));
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Retry settings update' })).toBeEnabled(),
  );
  expect(updates).toHaveBeenCalledOnce();
  fireEvent.click(screen.getByRole('button', { name: 'Retry settings update' }));
  await screen.findByText('Settings applied to this Agent.');
  expect(updates.mock.calls[1]?.[1]).toEqual(updates.mock.calls[0]?.[1]);
  expect(updates.mock.calls[1]?.[2].idempotencyKey).toBe(updates.mock.calls[0]?.[2].idempotencyKey);
});
