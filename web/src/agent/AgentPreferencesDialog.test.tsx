import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { testClient } from '../test/client';
import { AgentPreferencesDialog } from './AgentPreferencesDialog';
import { AgentPreferencesError } from '../api/rest/agentPreferences';
const preferences = {
  title: 'Original',
  system_prompt: 'Be precise.',
  prompt_pending: false,
  prompt_supported: true,
  revision: 3,
};
it('renames an active Agent without reloading its conversation and replays uncertain updates exactly', async () => {
  const update = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('Connection lost'))
    .mockResolvedValueOnce({ ...preferences, title: 'Renamed', revision: 4 });
  const titleUpdated = vi.fn();
  const close = vi.fn();
  render(
    <CmaClientProvider
      client={testClient({
        readAgentPreferences: vi.fn().mockResolvedValue(preferences),
        updateAgentPreferences: update,
      })}
    >
      <AgentPreferencesDialog
        agentId="agent"
        mode="rename"
        currentTitle="Original"
        active
        onTitleUpdated={titleUpdated}
        onClose={close}
      />
    </CmaClientProvider>,
  );
  fireEvent.change(await screen.findByRole('textbox', { name: 'Agent name' }), {
    target: { value: 'Renamed' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Retry change' }));
  await waitFor(() => expect(titleUpdated).toHaveBeenCalledWith('Renamed'));
  expect(update.mock.calls[0]?.[1]).toEqual({ title: 'Renamed', expected_revision: 3 });
  expect(update.mock.calls[1]?.[1]).toEqual(update.mock.calls[0]?.[1]);
  expect(update.mock.calls[1]?.[2].idempotencyKey).toBe(update.mock.calls[0]?.[2].idempotencyKey);
  expect(close).toHaveBeenCalledOnce();
});
it.each([
  {
    supported: false,
    active: false,
    message: 'This runtime does not support system prompt updates yet.',
  },
  {
    supported: true,
    active: true,
    message: 'Wait for the current turn to finish before changing the system prompt.',
  },
])('disables unsafe prompt changes: $message', async ({ supported, active, message }) => {
  const update = vi.fn();
  render(
    <CmaClientProvider
      client={testClient({
        readAgentPreferences: vi
          .fn()
          .mockResolvedValue({ ...preferences, prompt_supported: supported }),
        updateAgentPreferences: update,
      })}
    >
      <AgentPreferencesDialog
        agentId="agent"
        mode="prompt"
        currentTitle="Original"
        active={active}
        onClose={vi.fn()}
      />
    </CmaClientProvider>,
  );
  expect(await screen.findByRole('textbox', { name: 'Agent system prompt' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  expect(screen.getByText(message)).toBeVisible();
  expect(update).not.toHaveBeenCalled();
});
it('retains prompt edits after conflict, requiring explicit preference reload before another save', async () => {
  render(
    <CmaClientProvider
      client={testClient({
        readAgentPreferences: vi.fn().mockResolvedValue(preferences),
        updateAgentPreferences: vi
          .fn()
          .mockRejectedValue(new AgentPreferencesError('Changed in another tab', 409)),
      })}
    >
      <AgentPreferencesDialog
        agentId="agent"
        mode="prompt"
        currentTitle="Original"
        active={false}
        onClose={vi.fn()}
      />
    </CmaClientProvider>,
  );
  fireEvent.change(await screen.findByRole('textbox', { name: 'Agent system prompt' }), {
    target: { value: 'Be kind.' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await screen.findByText('Changed in another tab');
  expect(screen.getByRole('textbox', { name: 'Agent system prompt' })).toHaveValue('Be kind.');
  expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Reload preferences' }));
  await waitFor(() =>
    expect(screen.getByRole('textbox', { name: 'Agent system prompt' })).toHaveValue('Be precise.'),
  );
});
