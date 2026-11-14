import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { agentClient } from '../test/agent';
import { SessionSettings } from './SessionSettings';

it('paints saved settings without a model-catalog read and cancels a closed picker read', async () => {
  const readModels = vi.fn<(id: string, options?: { signal?: AbortSignal }) => Promise<never>>(
    () => new Promise<never>(() => undefined),
  );
  const client = agentClient({
    readAgentModels: readModels,
    readAgentSettings: async () => ({
      model: 'gpt-6',
      reasoning_effort: 'high',
      permission_mode: 'autonomous',
    }),
    updateAgentSettings: vi.fn(),
  });
  render(
    <CmaClientProvider client={client}>
      <SessionSettings agentId="agt_1" active runtimeReady busy={false} />
    </CmaClientProvider>,
  );
  const settings = await screen.findByRole('button', { name: /^Agent settings:/ });
  expect(settings).toHaveTextContent('gpt-6');
  expect(readModels).not.toHaveBeenCalled();
  fireEvent.click(settings);
  await waitFor(() => expect(readModels).toHaveBeenCalledOnce());
  const signal = readModels.mock.calls[0]?.[1]?.signal;
  expect(signal?.aborted).toBe(false);
  fireEvent.click(settings);
  expect(signal?.aborted).toBe(true);
});
