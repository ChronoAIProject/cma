// @vitest-environment node

import { expect, it } from 'vitest';
import { fixtureReply } from './devFixtures';
import { agentEvents, agentId } from './fixtures/agent';
import { channelActivity, teamChannel } from './fixtures/eventGateway';

it('leaves application modules and pages to Vite', () => {
  for (const path of ['/', '/event-gateway', '/src/api/client.ts', '/@vite/client', '/authority'])
    expect(fixtureReply('GET', path)).toBeUndefined();
});

it('answers the Event Gateway page reads from fixtures', () => {
  expect(fixtureReply('GET', '/auth/session')).toMatchObject({
    status: 200,
    json: { authenticated: true, subject: 'local-owner' },
  });
  expect(fixtureReply('GET', '/api/event-gateway/v1/channels')).toEqual({
    status: 200,
    json: { items: [teamChannel], next_cursor: null },
  });
  const stream = fixtureReply(
    'GET',
    '/api/event-gateway/v1/channels/channel-local/activity/stream',
  );
  expect(stream).toHaveProperty('stream');
  expect((stream as { stream: string }).stream).toContain(JSON.stringify(channelActivity));
});

it('returns a typed 404 for a read without a fixture instead of the HTML page', () => {
  expect(fixtureReply('GET', '/api/v1/unknown')).toEqual({
    status: 404,
    json: { code: 'fixture_missing', detail: 'No local fixture for GET /api/v1/unknown.' },
  });
});

it('replays the Agent conversation from a cursor and keeps transcript sequences aligned', () => {
  const all = fixtureReply('GET', `/api/v1/agents/${agentId}/events`) as {
    json: { items: { seq: number; kind: { type: string; content?: string } }[] };
  };
  expect(all.json.items.map((event) => event.seq)).toEqual(
    agentEvents.map((_, index) => index + 1),
  );
  const later = fixtureReply('GET', `/api/v1/agents/${agentId}/events`, '?after=8') as {
    json: { items: { seq: number }[] };
  };
  expect(later.json.items[0]?.seq).toBe(9);
  const transcript = fixtureReply('GET', `/api/v1/agents/${agentId}/transcript`) as {
    json: { turns: { messages: { sequence: number; content: string }[] }[] };
  };
  for (const message of transcript.json.turns.flatMap((turn) => turn.messages))
    expect(all.json.items[message.sequence - 1]?.kind.content).toBe(message.content);
});

it('answers the POST connection readiness check, which changes nothing', () => {
  expect(fixtureReply('POST', '/auth/connections/check')).toMatchObject({
    status: 200,
    json: { ready: true },
  });
});

it('refuses every write', () => {
  for (const method of ['POST', 'PUT', 'PATCH', 'DELETE'])
    expect(fixtureReply(method, '/api/event-gateway/v1/channels')).toMatchObject({
      status: 501,
      json: { code: 'fixture_read_only' },
    });
});
