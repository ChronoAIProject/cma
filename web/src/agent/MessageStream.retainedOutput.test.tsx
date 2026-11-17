import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { ApiError, type RetainedOutputPage } from '@cma/client';
import { agentClient } from '../test/agent';
import { MessageStream } from './MessageStream';
import { OwnerMessageStream } from './OwnerMessageStream';
import { conversationFromTranscript } from './conversation';

const at = '2026-11-17T00:00:00Z';
const output = {
  id: 'retained_checkpoint',
  agent_id: 'agent',
  turn_sequence: 3,
  checkpoint_sequence: 5,
  failure_sequence: 6,
  failed_at: at,
  byte_length: 12,
  partial: true,
};
const first: RetainedOutputPage = {
  output,
  offset: 0,
  content: 'Saved ',
  next: 'retained_checkpoint:6',
};
const last: RetainedOutputPage = { output, offset: 6, content: 'prefix', next: null };
function history() {
  return conversationFromTranscript({
    agentId: 'agent',
    throughSeq: 10,
    turns: [
      {
        turnSequence: 3,
        retainedOutput: { available: true },
        startedAt: at,
        ownerMessage: 'Original question',
        agentMessages: [],
        messages: [{ sequence: 4, role: 'user', content: 'Original question', appendedAt: at }],
        outcome: {
          type: 'failed',
          at,
          failure: {
            class: 'agent_log_capacity_exceeded',
            detail: 'This Agent turn failed.',
          },
        },
      },
      {
        turnSequence: 8,
        startedAt: at,
        ownerMessage: 'Later question',
        agentMessages: ['Later answer'],
        messages: [
          { sequence: 9, role: 'user', content: 'Later question', appendedAt: at },
          { sequence: 10, role: 'assistant', content: 'Later answer', appendedAt: at },
        ],
        outcome: { type: 'completed', at },
      },
    ],
  });
}

it('restores retained chunks on reload in the failed turn without changing history, replay cursors or later outcomes', async () => {
  const read = vi
    .fn()
    .mockResolvedValueOnce(first)
    .mockResolvedValueOnce(last)
    .mockResolvedValueOnce(first)
    .mockResolvedValueOnce(last);
  const client = agentClient({ readRetainedOutput: read });
  const conversation = history();
  const original = structuredClone(conversation);
  const draw = () => (
    <OwnerMessageStream
      conversation={conversation}
      onApproval={vi.fn()}
      client={client}
      agentId="agent"
      viewerId="owner"
      authorized
    />
  );
  const mounted = render(draw());
  expect(await screen.findByText('Saved prefix')).toBeInTheDocument();
  const partial = screen.getByText('Retained partial output · Failed turn').closest('article')!;
  expect(within(partial).queryByRole('button', { name: /good|bad|vote/i })).not.toBeInTheDocument();
  expect(
    partial.compareDocumentPosition(screen.getByText('Later question')) &
      Node.DOCUMENT_POSITION_FOLLOWING,
  ).toBeTruthy();
  expect(conversation).toEqual(original);
  expect(read.mock.calls[1]?.[1]).toMatchObject({ cursor: 'retained_checkpoint:6' });
  mounted.unmount();
  render(draw());
  expect(await screen.findByText('Saved prefix')).toBeInTheDocument();
  expect(read).toHaveBeenCalledTimes(4);
  expect(conversation).toEqual(original);
});

it.each(['authorization', 'viewer', 'client'] as const)(
  'does not retrieve owner retained output without current %s',
  async (missing) => {
    const read = vi.fn().mockResolvedValue({ ...first, content: 'Saved prefix', next: null });
    render(
      <OwnerMessageStream
        conversation={history()}
        onApproval={vi.fn()}
        client={missing === 'client' ? undefined : agentClient({ readRetainedOutput: read })}
        agentId="agent"
        viewerId={missing === 'viewer' ? undefined : 'owner'}
        authorized={missing !== 'authorization'}
      />,
    );
    await act(async () => {});
    expect(read).not.toHaveBeenCalled();
    expect(screen.queryByText('Saved prefix')).not.toBeInTheDocument();
    expect(screen.getByText('Original question')).toBeInTheDocument();
  },
);

it('clears owner retained output and ignores pending chunks when authorization becomes uncertain', async () => {
  let resolve!: (page: RetainedOutputPage) => void;
  const read = vi
    .fn()
    .mockResolvedValueOnce(first)
    .mockImplementationOnce(
      () =>
        new Promise<RetainedOutputPage>((done) => {
          resolve = done;
        }),
    );
  const client = agentClient({ readRetainedOutput: read });
  const conversation = history();
  const draw = (authorized: boolean) => (
    <OwnerMessageStream
      conversation={conversation}
      onApproval={vi.fn()}
      client={client}
      agentId="agent"
      viewerId="owner"
      authorized={authorized}
    />
  );
  const view = render(draw(true));
  await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
  expect(screen.getByText('Saved')).toBeInTheDocument();
  view.rerender(draw(false));
  expect(screen.queryByText('Retained partial output · Failed turn')).not.toBeInTheDocument();
  await act(async () => {
    resolve(last);
  });
  expect(screen.queryByText('Saved prefix')).not.toBeInTheDocument();
  expect(read).toHaveBeenCalledTimes(2);
  expect(conversation.turnOutcomes?.['8']).toBe('completed');
});

it('keeps already fetched text during a failed chunk and retries from its exact cursor without recording a run failure', async () => {
  const read = vi
    .fn()
    .mockResolvedValueOnce(first)
    .mockRejectedValueOnce(new ApiError(503, 'Temporary read failure'))
    .mockResolvedValueOnce(last);
  const client = agentClient({ readRetainedOutput: read });
  const conversation = history();
  const outcomes = conversation.turnOutcomes;
  render(
    <MessageStream
      conversation={conversation}
      onApproval={vi.fn()}
      retainedOutput={{ client, agentId: 'agent', viewerId: 'owner' }}
    />,
  );
  await screen.findByRole('button', { name: 'Retry retained output' });
  expect(screen.getByText('Saved')).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Retry retained output' }));
  expect(await screen.findByText('Saved prefix')).toBeInTheDocument();
  expect(read.mock.calls[2]?.[1]).toMatchObject({ cursor: first.next });
  expect(conversation.turnOutcomes).toBe(outcomes);
  expect(conversation.turnOutcomes?.['8']).toBe('completed');
});

it('renders a full 512 KiB chunk and its continuation without truncating retained text', async () => {
  const content = 'x'.repeat(512 * 1024);
  const descriptor = { ...output, byte_length: content.length + 5 };
  const next = `retained_checkpoint:${content.length}`;
  const read = vi
    .fn()
    .mockResolvedValueOnce({ output: descriptor, offset: 0, content, next })
    .mockResolvedValueOnce({
      output: descriptor,
      offset: content.length,
      content: ' tail',
      next: null,
    });
  const client = agentClient({ readRetainedOutput: read });
  render(
    <MessageStream
      conversation={history()}
      onApproval={vi.fn()}
      retainedOutput={{ client, agentId: 'agent', viewerId: 'owner' }}
    />,
  );
  const caption = await screen.findByText('Retained partial output · Failed turn');
  await waitFor(() =>
    expect(caption.closest('article')?.querySelector('.agent-content')?.textContent).toBe(
      content + ' tail',
    ),
  );
  expect(read.mock.calls[1]?.[1]).toMatchObject({ cursor: next });
  expect(read).toHaveBeenCalledTimes(2);
});

it.each(['viewer', 'agent', 'authority'] as const)(
  'discards a pending old-scope chunk when %s changes',
  async (changed) => {
    let resolve!: (page: RetainedOutputPage) => void;
    const read = vi
      .fn()
      .mockResolvedValueOnce(first)
      .mockImplementationOnce(
        () =>
          new Promise<RetainedOutputPage>((done) => {
            resolve = done;
          }),
      )
      .mockResolvedValue({ output: null, offset: 0, content: '', next: null });
    const client = agentClient({ readRetainedOutput: read });
    const conversation = history();
    const authority = {};
    const view = render(
      <MessageStream
        conversation={conversation}
        onApproval={vi.fn()}
        retainedOutput={{ client, agentId: 'agent', viewerId: 'owner', authority }}
      />,
    );
    await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
    view.rerender(
      <MessageStream
        conversation={conversation}
        onApproval={vi.fn()}
        retainedOutput={{
          client,
          agentId: changed === 'agent' ? 'other' : 'agent',
          viewerId: changed === 'viewer' ? 'other' : 'owner',
          authority: changed === 'authority' ? {} : authority,
        }}
      />,
    );
    await act(async () => {
      resolve(last);
    });
    expect(screen.queryByText('Saved prefix')).not.toBeInTheDocument();
    expect(screen.queryByText('Retained partial output · Failed turn')).not.toBeInTheDocument();
  },
);

it('hides fetched retained text on a revoked continuation and preserves the conversation', async () => {
  const read = vi
    .fn()
    .mockResolvedValueOnce(first)
    .mockRejectedValueOnce(new ApiError(404, 'Agent is not visible'));
  const conversation = history();
  const client = agentClient({ readRetainedOutput: read });
  render(
    <MessageStream
      conversation={conversation}
      onApproval={vi.fn()}
      retainedOutput={{ client, agentId: 'agent', viewerId: 'shared', authority: {} }}
    />,
  );
  await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
  await waitFor(() =>
    expect(screen.queryByText('Retained partial output · Failed turn')).not.toBeInTheDocument(),
  );
  expect(screen.getByText('Original question')).toBeInTheDocument();
  expect(conversation.turnOutcomes?.['8']).toBe('completed');
});

it('refuses inconsistent chunks without displaying foreign bytes or discarding saved text', async () => {
  const read = vi
    .fn()
    .mockResolvedValueOnce(first)
    .mockResolvedValueOnce({
      ...last,
      output: { ...output, agent_id: 'foreign' },
      content: 'leaked',
    });
  const client = agentClient({ readRetainedOutput: read });
  render(
    <MessageStream
      conversation={history()}
      onApproval={vi.fn()}
      retainedOutput={{ client, agentId: 'agent', viewerId: 'owner' }}
    />,
  );
  await screen.findByRole('button', { name: 'Retry retained output' });
  expect(screen.getByText('Saved')).toBeInTheDocument();
  expect(screen.queryByText('leaked')).not.toBeInTheDocument();
});
