import { chooseOptionValue } from '../test/select';
import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { agentClient } from '../test/agent';
import { SessionSettings } from './SessionSettings';
import { AgentSettingsError, agentSettingsClient } from '../api/rest/agentSettings';

const defaults = {
  model: 'gpt-6',
  reasoning_effort: 'high',
  permission_mode: 'autonomous',
} as const;
it('loads actual settings and retains the same update body and key after an uncertain response', async () => {
  const update = vi
    .fn()
    .mockRejectedValueOnce(new Error('Lost response'))
    .mockResolvedValue({ ...defaults, reasoning_effort: 'medium' });
  const client = agentClient({
    readAgentModels: async () => [
      { id: 'gpt-6', label: 'GPT-6' },
      { id: 'invalid', label: 'Retired model' },
      { id: 'gpt-6-updated', label: 'Updated model' },
    ],
    readAgentSettings: async () => defaults,
    updateAgentSettings: update,
  });
  render(
    <CmaClientProvider client={client}>
      <SessionSettings agentId="agent-one" active busy={false} />
    </CmaClientProvider>,
  );
  const model = await screen.findByLabelText('Model');
  expect(model).toHaveTextContent('GPT-6');
  expect(screen.getByLabelText('Reasoning effort')).toHaveTextContent('High');
  expect(screen.getByLabelText('Permission mode')).toHaveTextContent('YOLO (autonomous)');
  await chooseOptionValue(userEvent.setup(), screen.getByLabelText('Reasoning effort'), 'medium');
  await userEvent.click(screen.getByRole('button', { name: 'Apply settings' }));
  await screen.findByText('Lost response');
  expect(model).toBeDisabled();
  await userEvent.click(screen.getByRole('button', { name: 'Retry settings update' }));
  await screen.findByText('Settings applied to this Agent.');
  expect(update.mock.calls[1]?.[1]).toEqual(update.mock.calls[0]?.[1]);
  expect(update.mock.calls[1]?.[2].idempotencyKey).toBe(update.mock.calls[0]?.[2].idempotencyKey);
  expect(model).toBeEnabled();
});
it('does not offer an apply operation while a turn is active', async () => {
  const update = vi.fn();
  const client = agentClient({
    readAgentModels: async () => [
      { id: 'gpt-6', label: 'GPT-6' },
      { id: 'invalid', label: 'Retired model' },
      { id: 'gpt-6-updated', label: 'Updated model' },
    ],
    readAgentSettings: async () => defaults,
    updateAgentSettings: update,
  });
  render(
    <CmaClientProvider client={client}>
      <SessionSettings agentId="agent-one" active busy />
    </CmaClientProvider>,
  );
  await screen.findByLabelText('Model');
  await chooseOptionValue(userEvent.setup(), screen.getByLabelText('Reasoning effort'), 'low');
  expect(screen.getByRole('button', { name: 'Apply settings' })).toBeDisabled();
  expect(update).not.toHaveBeenCalled();
});
it('routes settings updates to Agent Manager with command identity and validates the response', async () => {
  const fetch = vi
    .spyOn(globalThis, 'fetch')
    .mockResolvedValue(new Response(JSON.stringify(defaults), { status: 200 }));
  const client = agentSettingsClient();
  await expect(
    client.updateAgentSettings!('agent-one', defaults, { idempotencyKey: 'fixed-key' }),
  ).resolves.toEqual(defaults);
  expect(fetch).toHaveBeenCalledWith(
    '/api/agent-manager/v1/agents/agent-one/settings',
    expect.objectContaining({
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json', 'Idempotency-Key': 'fixed-key' },
      body: JSON.stringify(defaults),
    }),
  );
  fetch.mockResolvedValue(
    new Response(JSON.stringify({ ...defaults, permission_mode: 'unknown' }), { status: 200 }),
  );
  await expect(client.readAgentSettings!('agent-one')).rejects.toThrow('invalid Agent settings');
  fetch.mockRestore();
});
it('allows correction after a definitive rejection without replaying its command key', async () => {
  const update = vi
    .fn()
    .mockRejectedValueOnce(new AgentSettingsError('Invalid model', 422))
    .mockResolvedValue(defaults);
  const client = agentClient({
    readAgentModels: async () => [
      { id: 'gpt-6', label: 'GPT-6' },
      { id: 'invalid', label: 'Retired model' },
      { id: 'gpt-6-updated', label: 'Updated model' },
    ],
    readAgentSettings: async () => defaults,
    updateAgentSettings: update,
  });
  render(
    <CmaClientProvider client={client}>
      <SessionSettings agentId="agent-one" active busy={false} />
    </CmaClientProvider>,
  );
  const model = await screen.findByLabelText('Model');
  await chooseOptionValue(userEvent.setup(), model, 'invalid');
  await userEvent.click(screen.getByRole('button', { name: 'Apply settings' }));
  await screen.findByText('Invalid model');
  expect(model).toBeEnabled();
  await chooseOptionValue(userEvent.setup(), model, 'gpt-6-updated');
  await userEvent.click(screen.getByRole('button', { name: 'Apply settings' }));
  await screen.findByText('Settings applied to this Agent.');
  expect(update.mock.calls[1]?.[2].idempotencyKey).not.toBe(
    update.mock.calls[0]?.[2].idempotencyKey,
  );
});
it('waits for runtime confirmation when the server reports pending settings', async () => {
  vi.useFakeTimers();
  const read = vi
    .fn()
    .mockResolvedValueOnce({ ...defaults, pending: true })
    .mockResolvedValue({ ...defaults, pending: false });
  const client = agentClient({
    readAgentModels: async () => [{ id: 'gpt-6', label: 'GPT-6' }],
    readAgentSettings: read,
    updateAgentSettings: vi.fn(),
  });
  render(
    <CmaClientProvider client={client}>
      <SessionSettings agentId="agent-one" active busy={false} />
    </CmaClientProvider>,
  );
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(screen.getByText('Waiting for the runtime to confirm these settings.')).toBeVisible();
  expect(screen.getByLabelText('Model')).toBeDisabled();
  expect(screen.queryByText('Settings applied to this Agent.')).not.toBeInTheDocument();
  await act(() => vi.advanceTimersByTimeAsync(2000));
  expect(
    screen.queryByText('Waiting for the runtime to confirm these settings.'),
  ).not.toBeInTheDocument();
  expect(screen.getByLabelText('Model')).toBeEnabled();
  vi.useRealTimers();
});

it('loads the scoped model catalog, preserves an unavailable current model, and never invents choices', async () => {
  const readModels = vi
    .fn()
    .mockRejectedValueOnce(new Error('Catalog unavailable'))
    .mockResolvedValue([{ id: 'available-model', label: 'Available model' }]);
  render(
    <CmaClientProvider
      client={agentClient({
        readAgentModels: readModels,
        readAgentSettings: async () => defaults,
        updateAgentSettings: vi.fn(),
      })}
    >
      <SessionSettings agentId="agent-one" active busy={false} />
    </CmaClientProvider>,
  );
  await screen.findByText('Catalog unavailable');
  expect(screen.getByLabelText('Model')).toBeDisabled();
  await userEvent.click(screen.getByRole('button', { name: 'Retry models' }));
  await userEvent.click(screen.getByRole('button', { name: 'Model' }));
  expect(
    await screen.findByRole('menuitemradio', {
      name: /gpt-6 Current model is not in the available catalog/,
    }),
  ).toHaveAttribute('aria-disabled', 'true');
  await userEvent.click(screen.getByRole('menuitemradio', { name: 'Available model' }));
  expect(screen.getByRole('button', { name: 'Model' })).toHaveTextContent('Available model');
});
it('validates scoped Chrono model responses and preserves same-origin authentication', async () => {
  const fetch = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
    new Response(JSON.stringify({ models: [{ id: 'available', label: 'Available' }] }), {
      status: 200,
    }),
  );
  try {
    const client = agentSettingsClient();
    await expect(client.readAgentModels!('agent/one')).resolves.toEqual([
      { id: 'available', label: 'Available' },
    ]);
    expect(fetch).toHaveBeenCalledWith(
      '/api/v1/agents/agent%2Fone/models',
      expect.objectContaining({ credentials: 'same-origin', cache: 'no-store' }),
    );
    fetch.mockResolvedValue(
      new Response(
        JSON.stringify({
          models: [
            { id: 'duplicate', label: 'First' },
            { id: 'duplicate', label: 'Second' },
          ],
        }),
        { status: 200 },
      ),
    );
    await expect(client.readAgentModels!('agent-one')).rejects.toThrow('invalid model catalog');
  } finally {
    fetch.mockRestore();
  }
});
