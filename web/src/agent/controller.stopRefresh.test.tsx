import '../test/connectedAccounts';
import { act, render, screen, waitFor, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { useAgentController } from './controller';
import { AgentSurface } from './AgentSurface';
import { CmaClientProvider } from '../api/context';
import { harness, json } from './transport/controller.fixture';
import { AGENT_DETAIL, agentClient } from '../test/agent';
import type { CmaWebClient } from '../api/client';

function View({ client }: { readonly client: CmaWebClient }) {
  const controller = useAgentController(client, 'agt_1');
  return (
    <CmaClientProvider client={client}>
      <button onClick={() => void controller.stop()}>Stop current Agent</button>
      <button onClick={() => void controller.send('Rejected after stopping')}>
        Submit after stop
      </button>
      <AgentSurface
        agentId="agt_1"
        controller={controller}
        detailsOpen={false}
        onDetailsOpenChange={vi.fn()}
        onNavigate={vi.fn()}
      />
    </CmaClientProvider>
  );
}

it('retains the v2 view on Stop, folds terminal events, and displays a rejected subsequent turn', async () => {
  const h = harness();
  let stopped = false;
  const readAgent = vi.spyOn(h.client, 'readAgent').mockImplementation(async () => ({
    ...AGENT_DETAIL,
    status: stopped ? 'stopped' : 'ready',
  }));
  h.client.stopAgent = vi.fn(async () => {
    stopped = true;
  });
  h.post.mockResolvedValue(
    json(
      {
        error: {
          type: 'invalid_request_error',
          code: 'agent_not_ready',
          message: 'Agent is stopped.',
          param: null,
        },
      },
      409,
    ),
  );
  const view = render(<View client={h.client} />);
  await waitFor(() => expect(h.streaming).toBe(true));
  const title = screen.getByRole('heading', { name: 'Agent one' });
  const composer = screen.getByRole('textbox', { name: 'Message' });
  const beforeSnapshots = h.snapshot.mock.calls.length;
  const beforeTranscripts = vi.mocked(h.client.readAgentTranscript).mock.calls.length;
  await act(async () => screen.getByRole('button', { name: 'Stop current Agent' }).click());
  expect(await within(screen.getByRole('banner')).findByText('Agent stopped')).toBeVisible();
  await act(async () => h.durable(1, { type: 'stopped', reason: 'Owner requested stop' }));
  expect(screen.getByRole('heading', { name: 'Agent one' })).toBe(title);
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
  expect(composer).toBeDisabled();
  expect(composer).toHaveAttribute(
    'placeholder',
    'This Agent has stopped and cannot be restarted.',
  );
  expect(readAgent).toHaveBeenCalledTimes(2);
  expect(h.snapshot).toHaveBeenCalledTimes(beforeSnapshots);
  expect(h.client.readAgentTranscript).toHaveBeenCalledTimes(beforeTranscripts);
  expect(h.cancel).not.toHaveBeenCalled();
  await act(async () => screen.getByRole('button', { name: 'Submit after stop' }).click());
  expect(h.post).toHaveBeenCalledOnce();
  await screen.findByText(
    'This Agent has stopped and cannot be restarted. To end only the current turn, use Stop response in the message composer.',
  );
  view.unmount();
});

it('preserves the full reload after legacy Stop', async () => {
  const client = agentClient({ stopAgent: vi.fn(async () => undefined) });
  const transcript = vi.spyOn(client, 'readAgentTranscript');
  const view = render(<View client={client} />);
  await screen.findByRole('heading', { name: 'Agent one' });
  await act(async () => screen.getByRole('button', { name: 'Stop current Agent' }).click());
  await waitFor(() => expect(transcript).toHaveBeenCalledTimes(2));
  expect(client.stopAgent).toHaveBeenCalledOnce();
  view.unmount();
});
