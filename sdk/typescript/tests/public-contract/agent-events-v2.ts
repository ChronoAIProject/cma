import type { Dispatch } from './context.js';

export const agentEventsV2Dispatch: Record<string, Dispatch> = {
  'GET /api/v2/agents/{agent_id}/events/head': ({ client, param }) =>
    client.agentEventsV2.head(param('agent_id')),
  'GET /api/v2/ui/transport': ({ client }) => client.agentEventsV2.readTransportEligibility(),
  'GET /api/v2/agents/{agent_id}/events/snapshot': ({ client, param, query, fixture }) =>
    client.agentEventsV2.snapshot(param('agent_id'), {
      toolOutput: new Headers(fixture.request.headers).get('prefer') === 'cma-tool-output',
      limit: query.has('limit') ? Number(query.get('limit')) : undefined,
      pageCursor: query.getAll('page_cursor').length === 1 ? query.get('page_cursor')! : undefined,
    }),
  'GET /api/v2/agents/{agent_id}/events': async ({ client, param, query, fixture }) => {
    const events: unknown[] = [];
    await client.agentEventsV2.events(param('agent_id'), {
      toolOutput: new Headers(fixture.request.headers).get('prefer') === 'cma-tool-output',
      afterCursor:
        query.getAll('after_cursor').length === 1 ? query.get('after_cursor')! : undefined,
      lastEventId: Object.entries(fixture.request.headers).find(
        ([name]) => name.toLowerCase() === 'last-event-id',
      )?.[1],
      follow: query.has('follow') ? query.get('follow') !== 'false' : undefined,
      onEvent: (frame) => {
        events.push(frame.data);
      },
    });
    return events;
  },
};
