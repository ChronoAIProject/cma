import type { Dispatch } from './context.js';

export const experienceDispatch: Record<string, Dispatch> = {
  'GET /api/v1/agui/capabilities': ({ client }) => client.agui.readCapabilities(),
  'GET /api/v1/agui/agents/{id}/sandbox': ({ client, param }) =>
    client.agui.readSandboxSnapshot(param('id')),
  'GET /api/v1/agui/events': async ({ client, query }) => {
    const events: unknown[] = [];
    await client.agui.replay(query.get('thread_id')!, {
      after: query.has('starting_after') ? Number(query.get('starting_after')) : undefined,
      follow: query.get('follow') !== 'false',
      maxReconnects: 0,
      onEvent: (event) => {
        events.push(event);
      },
    });
    return events;
  },
  'POST /api/v1/agui/run': async ({ client, body }) => {
    const events: unknown[] = [];
    await client.agui.run(body, {
      onEvent: (event) => {
        events.push(event);
      },
    });
    return events;
  },
  'GET /api/v1/conversations/{id}': ({ client, param }) =>
    client.responses.readConversation(param('id')),
  'POST /api/v1/conversations': ({ client, body }) => client.responses.createConversation(body),
  'GET /api/v1/conversations/{id}/items': ({ client, param, options }) =>
    client.responses.listConversationItems(param('id'), options),
  'GET /api/v1/responses/{response_id}': ({ client, param }) =>
    client.responses.read(param('response_id')),
  'DELETE /api/v1/responses/{response_id}': ({ client, param }) =>
    client.responses.delete(param('response_id')),
  'POST /api/v1/responses/{response_id}/cancel': ({ client, param }) =>
    client.responses.cancel(param('response_id')),
  'POST /api/v1/responses': async ({ client, body }) => {
    const events: unknown[] = [];
    await client.responses.create(body, {
      onEvent: (event) => {
        events.push(event);
      },
    });
    return events;
  },
  'GET /api/v1/agui/launches/{id}/events': async ({ client, param, query }) => {
    const events: unknown[] = [];
    await client.launches.stream(param('id'), {
      after: query.has('starting_after') ? Number(query.get('starting_after')) : undefined,
      follow: query.has('follow')
        ? query.get('follow') === 'true'
          ? true
          : query.get('follow') === 'false'
            ? false
            : (query.get('follow') as never)
        : undefined,
      limit: query.has('limit') ? Number(query.get('limit')) : undefined,
      onEvent: (event) => {
        events.push(event);
      },
    });
    return events;
  },
};
