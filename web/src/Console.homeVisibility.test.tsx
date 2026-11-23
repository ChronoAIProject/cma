import { act, render, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { CmaClientProvider } from './api/context';
import type { CmaWebClient } from './api/client';
import type { ReplyPreview } from './api/replyPreview';
import { agentClient } from './test/agent';
import { Console } from './Console';
import { ServiceBody } from './services/ServiceBody';

it('aborts Home previews in a hidden portal and rejects late replies before returning', async () => {
  const reads: { signal: AbortSignal; resolve: (value: ReplyPreview) => void }[] = [];
  const readAgentReplyPreview = vi.fn<NonNullable<CmaWebClient['readAgentReplyPreview']>>(
    (_id, options) =>
      new Promise<ReplyPreview>((resolve) => reads.push({ signal: options!.signal!, resolve })),
  );
  const client = agentClient({ readAgentReplyPreview });
  const user = {
    id: 'owner',
    handle: 'Owner',
    visitor: false,
    capabilities: ['use_own_sandboxes', 'create_sandbox'] as const,
  };
  const onNavigate = vi.fn();
  const draw = (visible: boolean) => (
    <CmaClientProvider client={client}>
      <ServiceBody visible={visible}>
        <Console
          user={user}
          route={{ kind: 'agent_manager' }}
          visible={visible}
          authEpoch={7}
          onCreate={vi.fn()}
          onNavigate={onNavigate}
          onSignOut={vi.fn()}
        />
      </ServiceBody>
    </CmaClientProvider>
  );
  const view = render(draw(true));
  await waitFor(() => expect(reads).toHaveLength(1));
  expect(document.hidden).toBe(false);
  view.rerender(draw(false));
  expect(view.container.querySelector('.service-shell__body')).toHaveAttribute('hidden');
  expect(reads.every(({ signal }) => signal.aborted)).toBe(true);
  const reply = {
    kind: 'ready' as const,
    agentId: 'agt_1',
    sourceThroughSeq: 1,
    reply: {
      text: 'Late hidden reply',
      appendedAt: '2026-01-01T00:00:00Z',
      messageSequence: 1,
      turnSequence: 1,
      truncated: false,
    },
  };
  await act(async () => reads[0]!.resolve(reply));
  expect(screen.queryByText('Late hidden reply')).not.toBeInTheDocument();
  view.rerender(draw(false));
  expect(reads).toHaveLength(1);
  view.rerender(draw(true));
  await waitFor(() => expect(reads).toHaveLength(2));
  await act(async () =>
    reads[1]!.resolve({ ...reply, reply: { ...reply.reply, text: 'Fresh reply' } }),
  );
  expect(await screen.findByText('Fresh reply')).toBeVisible();
  view.unmount();
  expect(reads.every(({ signal }) => signal.aborted)).toBe(true);
});
