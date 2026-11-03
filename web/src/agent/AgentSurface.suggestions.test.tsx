import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { connectionClient } from '../connections/client';
import { accessRequests } from './accessRequests';
import { AgentSurface } from './AgentSurface';
import { useAgentController } from './controller';
import { accepted, harness, item, json, page } from './transport/controller.fixture';

beforeEach(() => {
  sessionStorage.clear();
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'alice',
    ready: true,
    checked_at: '2026-11-03T00:00:00Z',
    services: [],
  });
  vi.spyOn(accessRequests, 'list').mockResolvedValue([]);
});
afterEach(() => vi.restoreAllMocks());

it('recovers a double-clicked suggestion with its original request while retaining the manual draft', async () => {
  const fixture = harness();
  fixture.snapshot.mockResolvedValue(
    json(
      page(
        [
          item(1, { type: 'turn_started', turn_sequence: 1 }),
          item(2, { type: 'message', role: 'user', content: 'Compare PostgreSQL and SQLite.' }),
          item(3, {
            type: 'message',
            role: 'assistant',
            content: 'PostgreSQL supports concurrent writers; SQLite suits local storage.',
          }),
          item(4, { type: 'turn_completed', turn_sequence: 1 }),
        ],
        4,
      ),
    ),
  );
  function Conversation() {
    const controller = useAgentController(fixture.client, 'agt_1', { viewerId: 'alice' });
    return (
      <CmaClientProvider client={fixture.client}>
        <AgentSurface
          agentId="agt_1"
          viewerId="alice"
          lifecycleClient={fixture.client}
          controller={controller}
          detailsOpen={false}
          onDetailsOpenChange={() => {}}
          onNavigate={() => {}}
        />
      </CmaClientProvider>
    );
  }
  render(<Conversation />);
  const field = await screen.findByRole('textbox', { name: 'Message' });
  fireEvent.change(field, { target: { value: 'Keep the budget limit.' } });
  const suggestion = await screen.findByRole('button', {
    name: 'Add explicit comparison criteria and explain the trade-offs.',
  });
  fixture.post.mockRejectedValueOnce(new TypeError('Response lost'));
  fireEvent.click(suggestion);
  fireEvent.click(suggestion);
  await screen.findByText('Awaiting confirmation');
  expect(field).toHaveValue('Keep the budget limit.');
  expect(suggestion).toBeDisabled();
  const original = fixture.post.mock.calls[0]![0]!;
  fixture.post.mockResolvedValueOnce(accepted());
  fireEvent.click(screen.getByRole('button', { name: 'Recover pending request' }));
  await waitFor(() =>
    expect(screen.queryByRole('region', { name: 'Next you can ask' })).not.toBeInTheDocument(),
  );
  expect(fixture.post.mock.calls[1]![0]!.body).toBe(original.body);
  expect(new Headers(fixture.post.mock.calls[1]![0]!.headers).get('Idempotency-Key')).toBe(
    new Headers(original.headers).get('Idempotency-Key'),
  );
  await act(async () => {
    fixture.durable(5, { type: 'turn_started', turn_sequence: 5 });
    fixture.durable(
      6,
      {
        type: 'message',
        role: 'user',
        content: 'Add explicit comparison criteria and explain the trade-offs.',
      },
      null,
      'cmd_1',
    );
  });
  const messages = screen
    .getAllByRole('article', { name: 'You' })
    .filter((message) =>
      within(message).queryByText('Add explicit comparison criteria and explain the trade-offs.'),
    );
  expect(messages).toHaveLength(1);
  expect(field).toHaveValue('Keep the budget limit.');
});
