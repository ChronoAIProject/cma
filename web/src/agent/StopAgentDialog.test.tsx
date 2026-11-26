import { StrictMode } from 'react';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { agentClient, AGENT_DETAIL } from '../test/agent';
import { StopAgentDialog } from './StopAgentDialog';

it('sends one request in StrictMode and waits for observed stopped status after acceptance', async () => {
  let accept!: () => void;
  const stop = vi.fn().mockImplementation(
    () =>
      new Promise<void>((resolve) => {
        accept = resolve;
      }),
  );
  const read = vi.fn().mockResolvedValue(AGENT_DETAIL);
  const stopped = vi.fn();
  render(
    <StrictMode>
      <StopAgentDialog
        client={agentClient({ stopAgent: stop, readAgent: read })}
        agentId={AGENT_DETAIL.id}
        requestKey="same-stop"
        onChanged={vi.fn()}
        onStopped={stopped}
        onClose={vi.fn()}
      />
    </StrictMode>,
  );
  expect(screen.getByRole('dialog', { name: 'Stop Agent' })).toBeVisible();
  expect(screen.getByText(/Stopping this Agent ends its active work/)).toBeVisible();
  expect(screen.getByText(/conversation history and Worktree are retained/)).toBeVisible();
  expect(screen.getByText(/end only the current turn, use Stop response/)).toBeVisible();
  expect(stop).not.toHaveBeenCalled();
  await userEvent.dblClick(screen.getByRole('button', { name: 'Stop Agent' }));
  await waitFor(() => expect(stop).toHaveBeenCalledOnce());
  await act(async () => accept());
  expect(screen.getByText('Accepted')).toBeVisible();
  expect(screen.queryByText('The Agent has stopped.')).not.toBeInTheDocument();
  expect(stopped).not.toHaveBeenCalled();
  read.mockResolvedValue({ ...AGENT_DETAIL, status: 'stopped' });
  await screen.findByRole('dialog', { name: 'Agent stopped' }, { timeout: 2500 });
  expect(stopped).toHaveBeenCalledOnce();
});
it('retains the stop command key after uncertainty and never treats an inaccessible Agent as stopped', async () => {
  const stop = vi
    .fn()
    .mockRejectedValueOnce(new Error('Lost stop response'))
    .mockResolvedValue(undefined);
  render(
    <StopAgentDialog
      client={agentClient({
        stopAgent: stop,
        readAgent: async () => {
          throw new Error('Access unavailable');
        },
      })}
      agentId="agent-one"
      requestKey="same-stop"
      onChanged={vi.fn()}
      onStopped={vi.fn()}
      onClose={vi.fn()}
    />,
  );
  await userEvent.click(screen.getByRole('button', { name: 'Stop Agent' }));
  await screen.findByText('Lost stop response');
  expect(screen.getByText(/Completion is not yet confirmed/)).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: 'Retry stop request' }));
  await waitFor(() => expect(stop).toHaveBeenCalledTimes(2));
  expect(stop.mock.calls[1]).toEqual(stop.mock.calls[0]);
  expect(screen.queryByRole('dialog', { name: 'Agent stopped' })).not.toBeInTheDocument();
});

it('cancels Stop without a write or a completion claim', async () => {
  const stop = vi.fn();
  const close = vi.fn();
  const stopped = vi.fn();
  render(
    <StopAgentDialog
      client={agentClient({ stopAgent: stop })}
      agentId="agt_1"
      requestKey="cancel-stop"
      onChanged={vi.fn()}
      onStopped={stopped}
      onClose={close}
    />,
  );
  await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(close).toHaveBeenCalledOnce();
  expect(stop).not.toHaveBeenCalled();
  expect(stopped).not.toHaveBeenCalled();
});
