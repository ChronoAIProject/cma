import { useState } from 'react';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ApiError } from '@cma/client';
import { afterEach, expect, it, vi } from 'vitest';
import { UsageAgentPicker } from './UsageAgentPicker';
import { SelectedResourceLabels } from './metrics/selectedResourceLabel';
import { metricsClient } from './metrics/testFixtures';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function fixture() {
  return {
    readMeasuredUsage: vi.fn().mockResolvedValue(false),
    usageMetrics: metricsClient(),
    usage: { events: vi.fn() },
    agents: { list: vi.fn(), read: vi.fn() },
  };
}

it('retains a full title chosen off the first page across pagination and picker remounts', async () => {
  const client = fixture();
  const name = 'Research notes '.repeat(12).trim();
  client.agents.list.mockImplementation(async ({ cursor }) => ({
    items: [
      {
        id: cursor ? 'agt-later' : 'agt-first',
        sandboxId: 'sbx-1',
        name: cursor ? name : 'Other Agent',
      },
    ],
    next: cursor ? null : 'next-page',
  }));
  function Subject({ shown = true }) {
    const [value, onChange] = useState('');
    return (
      <SelectedResourceLabels scope={client}>
        {shown && (
          <UsageAgentPicker
            client={client}
            sandbox="sbx-1"
            value={value}
            onChange={onChange}
            active
          />
        )}
      </SelectedResourceLabels>
    );
  }
  const { rerender } = render(<Subject />);
  await userEvent.click(screen.getByRole('button', { name: 'Agent: All Agents in this Sandbox' }));
  await userEvent.click(await screen.findByRole('button', { name: 'Next Agents' }));
  await waitFor(() =>
    expect(client.agents.list).toHaveBeenLastCalledWith(
      expect.objectContaining({ cursor: 'next-page' }),
    ),
  );
  await userEvent.click(screen.getByRole('button', { name: 'Usage Agent' }));
  await userEvent.click(screen.getByRole('menuitemradio', { name: `${name} (agt-later)` }));
  expect(screen.getByRole('button', { name: `Agent: ${name}` })).toHaveTextContent(name);
  rerender(<Subject shown={false} />);
  expect(screen.queryByRole('button', { name: /^Agent:/ })).not.toBeInTheDocument();
  rerender(<Subject />);
  const selected = screen.getByRole('button', { name: `Agent: ${name}` });
  expect(selected.querySelector('[title]')).toHaveAttribute('title', name);
  await userEvent.click(selected);
  expect(await screen.findByRole('button', { name: 'Usage Agent' })).toHaveTextContent(name);
  expect(client.agents.read).not.toHaveBeenCalled();
});

it.each([
  new ApiError(403, 'Forbidden'),
  new ApiError(404, 'Missing'),
  { id: 'wrong-agent', sandboxId: 'sbx-1', name: 'Private name' },
  { id: 'agt-1', sandboxId: 'another-sandbox', name: 'Private name' },
])('retains an unreadable selection without exposing a mismatched label: %j', async (result) => {
  const client = fixture();
  if (result instanceof Error) client.agents.read.mockRejectedValue(result);
  else client.agents.read.mockResolvedValue(result);
  const change = vi.fn();
  const { rerender } = render(
    <SelectedResourceLabels scope={client}>
      <UsageAgentPicker client={client} sandbox="sbx-1" value="agt-1" active onChange={change} />
    </SelectedResourceLabels>,
  );
  await screen.findByRole('button', { name: 'Agent: Selected Agent unavailable' });
  expect(screen.queryByText('Private name')).not.toBeInTheDocument();
  expect(change).not.toHaveBeenCalled();
  rerender(
    <SelectedResourceLabels scope={client}>
      <p>Picker pending</p>
    </SelectedResourceLabels>,
  );
  rerender(
    <SelectedResourceLabels scope={client}>
      <UsageAgentPicker client={client} sandbox="sbx-1" value="agt-1" active onChange={change} />
    </SelectedResourceLabels>,
  );
  expect(client.agents.read).toHaveBeenCalledOnce();
  client.agents.read.mockResolvedValue({
    id: 'agt-1',
    sandboxId: 'sbx-1',
    name: 'Recovered Agent',
  });
  await userEvent.click(screen.getByRole('button', { name: 'Retry Agent name' }));
  await screen.findByRole('button', { name: 'Agent: Recovered Agent' });
  expect(client.agents.read).toHaveBeenCalledTimes(2);
  expect(client.agents.list).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: 'Clear Agent' }));
  expect(change).toHaveBeenCalledWith('');
});

it('aborts recovery while hidden and rejects late results for an old Sandbox', async () => {
  const client = fixture();
  let finish!: (value: { id: string; sandboxId: string; name: string }) => void;
  client.agents.read.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const props = { client, sandbox: 'sbx-1', value: 'agt-1', onChange: vi.fn() };
  const { rerender } = render(<UsageAgentPicker {...props} active={false} />);
  expect(client.agents.read).not.toHaveBeenCalled();
  rerender(<UsageAgentPicker {...props} active />);
  await waitFor(() => expect(client.agents.read).toHaveBeenCalledOnce());
  const signal = client.agents.read.mock.calls[0]![1].signal as AbortSignal;
  rerender(<UsageAgentPicker {...props} active={false} />);
  expect(signal.aborted).toBe(true);
  client.agents.read.mockResolvedValue({
    id: 'agt-1',
    sandboxId: 'sbx-2',
    name: 'New Sandbox Agent',
  });
  rerender(<UsageAgentPicker {...props} sandbox="sbx-2" active />);
  await screen.findByRole('button', { name: 'Agent: New Sandbox Agent' });
  await act(async () => finish({ id: 'agt-1', sandboxId: 'sbx-1', name: 'Old Sandbox Agent' }));
  expect(screen.queryByText('Old Sandbox Agent')).not.toBeInTheDocument();
  expect(client.agents.read).toHaveBeenCalledTimes(2);
});

it('shows unnamed and same-name choices distinctly without reading details for a choice', async () => {
  const client = fixture();
  client.agents.list.mockResolvedValue({
    items: [
      { id: 'a', sandboxId: 'sbx-1', name: 'Research' },
      { id: 'b', sandboxId: 'sbx-1', name: 'Research' },
      { id: 'c', sandboxId: 'sbx-1', name: ' ' },
    ],
    next: null,
  });
  const change = vi.fn();
  render(<UsageAgentPicker client={client} sandbox="sbx-1" value="" active onChange={change} />);
  await userEvent.click(screen.getByRole('button', { name: 'Agent: All Agents in this Sandbox' }));
  await userEvent.click(await screen.findByRole('button', { name: 'Usage Agent' }));
  expect(screen.getByRole('menuitemradio', { name: 'Research (a)' })).toBeVisible();
  expect(screen.getByRole('menuitemradio', { name: 'Untitled Agent (c)' })).toBeVisible();
  await userEvent.click(screen.getByRole('menuitemradio', { name: 'Research (b)' }));
  expect(change).toHaveBeenCalledWith('b');
  expect(client.agents.read).not.toHaveBeenCalled();
});

it('honors Retry-After and stops after bounded automatic name recovery attempts', async () => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
  const client = fixture();
  client.agents.read.mockRejectedValue(new ApiError(429, 'Busy', undefined, undefined, 5000));
  render(
    <UsageAgentPicker client={client} sandbox="sbx-1" value="agt-1" active onChange={vi.fn()} />,
  );
  await act(async () => {});
  expect(client.agents.read).toHaveBeenCalledOnce();
  await act(async () => {
    await vi.advanceTimersByTimeAsync(4999);
  });
  expect(client.agents.read).toHaveBeenCalledOnce();
  await act(async () => {
    await vi.advanceTimersByTimeAsync(1);
  });
  expect(client.agents.read).toHaveBeenCalledTimes(2);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(10000);
  });
  expect(client.agents.read).toHaveBeenCalledTimes(4);
  expect(screen.getByRole('button', { name: 'Agent: Selected Agent unavailable' })).toBeVisible();
  expect(screen.getByRole('button', { name: 'Waiting to retry' })).toBeDisabled();
  await act(async () => {
    await vi.advanceTimersByTimeAsync(30000);
  });
  expect(client.agents.read).toHaveBeenCalledTimes(4);
  client.agents.read.mockResolvedValue({ id: 'agt-1', sandboxId: 'sbx-1', name: 'Recovered' });
  fireEvent.click(screen.getByRole('button', { name: 'Retry Agent name' }));
  await act(async () => {});
  expect(screen.getByRole('button', { name: 'Agent: Recovered' })).toBeVisible();
  expect(client.agents.read).toHaveBeenCalledTimes(5);
});
