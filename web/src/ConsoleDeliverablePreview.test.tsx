import { fireEvent, render, screen, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { createClient, type AnswerDeliverable } from '@chronoai/cma-sdk';
import { CmaClientProvider } from './api/context';
import { Console } from './Console';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, agentClient } from './test/agent';
import { WIRE_AGENT } from './test/rest';
import './test/connectedAccounts';

const first: AnswerDeliverable = {
  id: `delivery_${'a'.repeat(64)}`,
  reference: {
    agent_id: 'agt_1',
    resource: { kind: 'live_file', path: 'first.txt' },
    name: 'first.txt',
    media_type: 'application/octet-stream',
    size_bytes: 3,
    sha256: 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
  },
  status: 'unchecked',
};
const second: AnswerDeliverable = {
  ...first,
  id: `delivery_${'b'.repeat(64)}`,
  reference: {
    ...first.reference,
    name: 'second.md',
    resource: { kind: 'document', id: `doc_${'b'.repeat(32)}`, revision: 1 },
    media_type: 'text/markdown',
  },
};
const marker = (item: AnswerDeliverable) =>
  `<!--cma-deliverable:v1:${btoa(JSON.stringify(item.reference)).replaceAll('+', '-').replaceAll('/', '_').replaceAll('=', '')}-->`;

it('previews two answer resources and closes without losing the draft or history position', async () => {
  const sdk = createClient({
    baseUrl: 'https://cma.test',
    fetch: async (input) => {
      const path = new URL(String(input)).pathname;
      if (!path.includes('/answers/'))
        return Response.json({
          ...WIRE_AGENT,
          id: 'agt_1',
          workspace: { ...WIRE_AGENT.workspace, id: AGENT_DETAIL.workspace.id },
          sandbox: { ...WIRE_AGENT.sandbox, id: AGENT_DETAIL.sandbox.id },
        });
      const identity = { agent_id: 'agt_1', answer_id: 'event:3', turn_sequence: 1 };
      if (path.endsWith('/download'))
        return new Response('abc', { headers: { 'content-type': 'application/octet-stream' } });
      return Response.json(
        path.endsWith('/deliverables')
          ? { ...identity, items: [first, second] }
          : { ...identity, item: path.endsWith(first.id) ? first : second },
      );
    },
  });
  const turn = AGENT_TRANSCRIPT.turns[0]!;
  const client = agentClient({
    resourceViews: sdk,
    deliverables: sdk.agents,
    readAgentTranscript: async () => ({
      ...AGENT_TRANSCRIPT,
      turns: [
        {
          ...turn,
          messages: turn.messages!.map((message) =>
            message.role === 'assistant'
              ? {
                  ...message,
                  content: `The repository is ready.\n${marker(first)}\n${marker(second)}`,
                }
              : message,
          ),
        },
      ],
    }),
  });
  render(
    <CmaClientProvider client={client}>
      <Console
        user={{ id: 'owner', handle: 'owner', visitor: false, capabilities: ['use_own_sandboxes'] }}
        route={{ kind: 'agent', agentId: 'agt_1' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  const composer = await screen.findByRole('textbox', { name: 'Message' });
  fireEvent.change(composer, { target: { value: 'My unsent draft' } });
  const history = document.querySelector('.agent-surface__conversation')!;
  Object.defineProperties(history, { scrollHeight: { value: 1000 }, clientHeight: { value: 200 } });
  history.scrollTop = 1000;
  fireEvent.scroll(history);
  history.scrollTop = 85;
  fireEvent.scroll(history);
  const firstCard = await screen.findByRole('button', { name: 'Preview first.txt' });
  fireEvent.click(firstCard);
  let preview = await screen.findByRole('complementary', { name: 'Deliverable preview' });
  expect(
    await within(preview).findByRole('textbox', { name: 'Contents of first.txt' }),
  ).toHaveValue('abc');
  expect(firstCard).toHaveAttribute('aria-pressed', 'true');
  const secondCard = screen.getByRole('button', { name: 'Preview second.md' });
  fireEvent.click(secondCard);
  preview = screen.getByRole('complementary', { name: 'Deliverable preview' });
  expect(await within(preview).findByText('abc')).toBeVisible();
  expect(firstCard).toHaveAttribute('aria-pressed', 'false');
  fireEvent.click(within(preview).getByRole('button', { name: 'Close preview' }));
  expect(screen.queryByRole('complementary', { name: 'Deliverable preview' })).toBeNull();
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
  expect(composer).toHaveValue('My unsent draft');
  expect(document.querySelector('.agent-surface__conversation')).toBe(history);
  expect(history.scrollTop).toBe(85);
  expect(secondCard).toHaveFocus();
});
