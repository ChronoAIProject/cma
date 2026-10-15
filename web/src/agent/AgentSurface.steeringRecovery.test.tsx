import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { connectionClient } from '../connections/client';
import { accessRequests } from './accessRequests';
import { AgentSurface } from './AgentSurface';
import { useAgentController } from './controller';
import { binding, harness, item, json, page } from './transport/controller.fixture';
import { agentTransportJournal } from './transport/journal';

beforeEach(() => {
  sessionStorage.clear();
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'alice',
    ready: true,
    checked_at: '2026-10-16T00:00:00Z',
    services: [],
  });
  vi.spyOn(accessRequests, 'list').mockResolvedValue([]);
});
afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
});

it.each([false, true])(
  'recovers lost native steering with its exact key/body and merges by command identity (reload: %s)',
  async (reload) => {
    const h = harness();
    const original = [
      item(1, { type: 'turn_started', turn_sequence: 1 }, binding),
      item(2, { type: 'message', role: 'user', content: 'Original user input' }),
    ];
    h.snapshot.mockImplementation(async () => json(page(original, 2)));
    h.control.mockRejectedValueOnce(new TypeError('Steering reply lost'));
    function Agent() {
      const controller = useAgentController(h.client, 'agt_1', { viewerId: 'alice' });
      return (
        <CmaClientProvider client={h.client}>
          <AgentSurface
            agentId="agt_1"
            viewerId="alice"
            lifecycleClient={h.client}
            controller={controller}
            detailsOpen={false}
            onDetailsOpenChange={() => {}}
            onNavigate={() => {}}
          />
        </CmaClientProvider>
      );
    }
    const view = render(<Agent />);
    await waitFor(() => expect(h.streaming).toBe(true));
    const field = screen.getByRole('textbox', { name: 'Message' });
    await userEvent.click(screen.getByRole('button', { name: 'Steering mode: Queue' }));
    await userEvent.click(screen.getByRole('menuitemradio', { name: /Interrupt/ }));
    fireEvent.change(field, { target: { value: 'Adjust the approach' } });
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Interrupt with message' })).toBeEnabled(),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Interrupt with message' }));
    await screen.findByText('Awaiting confirmation');
    expect(field).toHaveValue('Adjust the approach');
    expect(h.post).not.toHaveBeenCalled();
    expect(h.client.interruptAgent).not.toHaveBeenCalled();
    const journal = agentTransportJournal(sessionStorage, 'alice', 'agt_1');
    expect(journal.read()).toBeNull();
    expect(journal.readControl()?.body.command).toEqual({
      type: 'steer',
      text: 'Adjust the approach',
    });
    const request = h.control.mock.calls[0]![1]!;
    if (reload) {
      view.unmount();
      render(<Agent />);
      await screen.findByText('Awaiting confirmation');
      expect(screen.getAllByText('Adjust the approach', { selector: 'p' })).toHaveLength(1);
    }
    fireEvent.click(screen.getByRole('button', { name: 'Recover pending request' }));
    await screen.findByText('Sent');
    expect(screen.queryByText('Awaiting confirmation')).not.toBeInTheDocument();
    expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue('');
    expect(h.control).toHaveBeenCalledTimes(2);
    const replayed = h.control.mock.calls[1]![1]!;
    expect(replayed.body).toBe(request.body);
    expect(new Headers(replayed.headers).get('idempotency-key')).toBe(
      new Headers(request.headers).get('idempotency-key'),
    );
    expect(journal.read()).toBeNull();
    expect(journal.readControl()?.commandId).toBe('cmd_control');
    await act(async () => {
      h.durable(
        3,
        { type: 'message', role: 'user', content: 'Adjust the approach' },
        null,
        'cmd_control',
      );
    });
    await waitFor(() => expect(screen.queryByText('Sent')).not.toBeInTheDocument());
    expect(
      screen
        .getAllByRole('article', { name: 'You' })
        .filter((article) => within(article).queryByText('Adjust the approach')),
    ).toHaveLength(1);
  },
);
