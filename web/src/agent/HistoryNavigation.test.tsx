import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { HistoryNavigation } from './HistoryNavigation';
import { conversationFromWindow } from './historyWindow';
import { historyPage, statePage } from './historyWindow.fixture';
import { cursor } from './transport/controller.fixture';

it('replaces older pages while leaving the live conversation and cursor intact', async () => {
  const conversation = conversationFromWindow(historyPage());
  const client = {
    history: vi
      .fn()
      .mockResolvedValueOnce(historyPage(898, 900))
      .mockResolvedValueOnce(historyPage(896, 898)),
    state: vi.fn().mockResolvedValue(statePage()),
  };
  const authorize = vi.fn(async () => {});
  render(
    <HistoryNavigation
      client={client}
      agentId="agt_1"
      conversation={conversation}
      authorize={authorize}
      onApproval={vi.fn()}
    />,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Earlier activity' }));
  await screen.findByText('Reply 900');
  expect(client.history).toHaveBeenLastCalledWith(
    'agt_1',
    expect.objectContaining({ throughCursor: cursor(900), limit: 200 }),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Earlier' }));
  await screen.findByText('Reply 898');
  expect(screen.queryByText('Reply 900')).toBeNull();
  expect(conversation.agentCursor).toBe(cursor(902));
  expect(conversation.messages.at(-1)?.content).toBe('Reply 902');
  expect(authorize).toHaveBeenCalledTimes(4);
});

it('does not expose a fetched window after its final authorization fails', async () => {
  const client = { history: vi.fn().mockResolvedValue(historyPage(898, 900)), state: vi.fn() };
  const authorize = vi
    .fn()
    .mockResolvedValueOnce(undefined)
    .mockRejectedValueOnce(new Error('Agent access changed.'));
  render(
    <HistoryNavigation
      client={client}
      agentId="agt_1"
      conversation={conversationFromWindow(historyPage())}
      authorize={authorize}
      onApproval={vi.fn()}
    />,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Earlier activity' }));
  await screen.findByRole('alert');
  expect(screen.queryByText('Reply 900')).toBeNull();
});

it('binds approval navigation to its captured head and hides it on a scope change', async () => {
  const first = { ...statePage(), next_approval_cursor: statePage().approvals[0]!.cursor };
  const client = {
    history: vi.fn(),
    state: vi
      .fn()
      .mockResolvedValueOnce(first)
      .mockResolvedValueOnce({ ...first, approvals: [], next_approval_cursor: null }),
  };
  const conversation = { ...conversationFromWindow(historyPage()), approvalsTruncated: true };
  const view = render(
    <HistoryNavigation
      client={client}
      agentId="agt_1"
      scope="alice"
      conversation={conversation}
      onApproval={vi.fn()}
    />,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Browse pending approvals' }));
  await screen.findByText('Original request');
  fireEvent.click(screen.getByRole('button', { name: 'Next approvals' }));
  await waitFor(() => expect(client.state).toHaveBeenCalledTimes(2));
  expect(client.state).toHaveBeenLastCalledWith(
    'agt_1',
    expect.objectContaining({ throughCursor: cursor(902), afterApprovalCursor: cursor(6) }),
  );
  view.rerender(
    <HistoryNavigation
      client={client}
      agentId="agt_1"
      scope="bob"
      conversation={conversation}
      onApproval={vi.fn()}
    />,
  );
  expect(screen.queryByRole('dialog')).toBeNull();
});
