import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { conversationFromTranscript, foldAgentEvent, foldLiveEvent } from './conversation';
import { MessageStream } from './MessageStream';
import { attachmentMarker } from './attachments';
import type { ConversationState } from './model';

const reference = `cmaeg1.ev.1.${'sealed_event-reference_'.repeat(150)}`;
const request = '我正在尝试从telegram上和你对话，你有什么功能';
const input = `${request}\n\nevent_ref: ${reference}`;
const timestamp = '2026-10-30T00:00:00Z';

function emptyConversation() {
  return conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] });
}

function durableConversation(content: string, role: 'user' | 'assistant' = 'user') {
  return foldAgentEvent(emptyConversation(), {
    seq: 1,
    appendedAt: timestamp,
    kind: { type: role === 'user' ? 'owner_message' : 'agent_message', content },
  });
}

const sources: readonly [string, () => ConversationState][] = [
  ['durable events', () => durableConversation(input)],
  [
    'live events',
    () =>
      [
        { type: 'TEXT_MESSAGE_START', messageId: 'owner', role: 'user' },
        { type: 'TEXT_MESSAGE_CONTENT', messageId: 'owner', delta: input },
        { type: 'TEXT_MESSAGE_END', messageId: 'owner' },
      ].reduce(
        (state, event) =>
          foldLiveEvent(state, { ...event, runId: 'run', threadId: 'agent' }, 'agent'),
        emptyConversation(),
      ),
  ],
  [
    'retained transcripts',
    () =>
      conversationFromTranscript({
        agentId: 'agent',
        throughSeq: 2,
        turns: [
          {
            turnSequence: 1,
            startedAt: timestamp,
            ownerMessage: input,
            agentMessages: [],
            messages: [{ sequence: 2, role: 'user', content: input, appendedAt: timestamp }],
            outcome: { type: 'completed' },
          },
        ],
      }),
  ],
];

it.each(sources)('hides and excludes the event reference from copies of %s', async (_, source) => {
  const actor = userEvent.setup();
  const copy = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue();
  const conversation = source();
  const { container } = render(<MessageStream conversation={conversation} onApproval={vi.fn()} />);
  expect(screen.getByText(request)).toBeVisible();
  expect(container).not.toHaveTextContent('event_ref:');
  expect(container).not.toHaveTextContent(reference);
  await actor.click(screen.getByRole('button', { name: 'Copy content' }));
  expect(copy).toHaveBeenCalledWith(request);
  expect(conversation.messages[0]?.content).toBe(input);
});

it.each([
  'Please explain event_ref: and how it works.',
  'Example\n\nevent_ref: ordinary-reference',
  'Example\n\nevent_ref: cmaeg1.ev.1.too_short',
  `${input}\nPlease explain this reference.`,
  `Example\n\nevent_ref: ${reference}\n`,
  `Example\n\nevent_ref: ${reference.replace('.ev.', '.ch.')}`,
  `Example\n\nevent_ref: ${reference.replace('.1.', '.256.')}`,
  `Example\n\nevent_ref: ${reference.replace('.1.', '.01.')}`,
])('keeps ordinary text and malformed or quoted references visible (case %#)', (content) => {
  const { container } = render(
    <MessageStream conversation={durableConversation(content)} onApproval={vi.fn()} />,
  );
  expect(container.querySelector('.message__body > p')?.textContent).toBe(content);
});

it('keeps event references in assistant explanations and copied replies', async () => {
  const actor = userEvent.setup();
  const copy = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue();
  const { container } = render(
    <MessageStream conversation={durableConversation(input, 'assistant')} onApproval={vi.fn()} />,
  );
  expect(container).toHaveTextContent(reference);
  await actor.click(screen.getByRole('button', { name: 'Copy content' }));
  expect(copy).toHaveBeenCalledWith(input);
});

it('renders attachment cards when an event reference follows the manifest', () => {
  const content = `${request}${attachmentMarker}${JSON.stringify([
    {
      name: 'notes.txt',
      path: '/home/attachments/agent/notes.txt',
      media_type: 'text/plain',
      size_bytes: 16,
    },
  ])}\n\nevent_ref: ${reference}`;
  const { container } = render(
    <MessageStream conversation={durableConversation(content)} onApproval={vi.fn()} />,
  );
  expect(screen.getByText(request)).toBeVisible();
  expect(screen.getByRole('list', { name: 'Attached files' })).toHaveTextContent('notes.txt');
  expect(container).not.toHaveTextContent(reference);
});
