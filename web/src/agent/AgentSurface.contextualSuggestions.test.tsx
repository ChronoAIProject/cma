import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { connectionClient } from '../connections/client';
import { accessRequests } from './accessRequests';
import { AgentSurface } from './AgentSurface';
import { useAgentController } from './controller';
import { harness, item, json, page } from './transport/controller.fixture';

afterEach(() => vi.restoreAllMocks());

it('never reopens answer A when its saved suggestions arrive after another message is accepted', async () => {
  sessionStorage.clear();
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'alice', ready: true, checked_at: '2026-11-03T00:00:00Z', services: [],
  });
  vi.spyOn(accessRequests, 'list').mockResolvedValue([]);
  const fixture = harness();
  let deliver: (value: unknown) => void = () => {};
  const read = vi.fn(() => new Promise((resolve) => { deliver = resolve; }));
  Object.assign(fixture.client, { readSuggestedReplies: read });
  fixture.snapshot.mockResolvedValue(json(page([
    item(1, { type: 'turn_started', turn_sequence: 1 }),
    item(2, { type: 'message', role: 'user', content: 'Compare PostgreSQL and SQLite.' }),
    item(3, { type: 'message', role: 'assistant', content: 'SQLite suits this local-only app.' }),
    item(4, { type: 'turn_completed', turn_sequence: 1 }),
  ], 4)));
  function Conversation() {
    const controller = useAgentController(fixture.client, 'agt_1', { viewerId: 'alice' });
    return <CmaClientProvider client={fixture.client}><AgentSurface
      agentId="agt_1" viewerId="alice" lifecycleClient={fixture.client} controller={controller}
      detailsOpen={false} onDetailsOpenChange={() => {}} onNavigate={() => {}}
    /></CmaClientProvider>;
  }
  render(<Conversation />);
  await screen.findByText('SQLite suits this local-only app.');
  await waitFor(() => expect(read).toHaveBeenCalled());
  expect(screen.queryByRole('region', { name: 'Next you can ask' })).not.toBeInTheDocument();
  const composer = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(composer, { target: { value: 'Use SQLite.' } });
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
  await waitFor(() => expect(fixture.post).toHaveBeenCalledTimes(1));
  await act(async () => deliver({
    status: 'ready',
    set: { id: 'saved-a', viewerId: 'alice', agentId: 'agt_1', answerId: 'event:3',
      scope: { turn_sequence: 1, answer_sequence: 3 },
      source: { kind: 'generated', version: 'contextual-v1' },
      items: [{ id: 'saved-a:0', text: 'Explain SQLite backup options.', rank: 0 }],
    },
  }));
  expect(screen.queryByText('Explain SQLite backup options.')).not.toBeInTheDocument();
  expect(screen.queryByRole('region', { name: 'Next you can ask' })).not.toBeInTheDocument();
  expect(composer).toBeEnabled();
});
