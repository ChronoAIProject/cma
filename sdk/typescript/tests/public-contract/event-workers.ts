import type { EventWorkerProfileInput } from '../../src/event-worker-profiles.js';
import type { Dispatch } from './context.js';

export const eventWorkerDispatch: Record<string, Dispatch> = {
  'POST /api/v1/event-worker-profiles': ({ client, body, command }) =>
    client.eventWorkerProfiles.create(body, command),
  'GET /api/v1/event-worker-profiles': ({ client, query }) =>
    client.eventWorkerProfiles.list({
      after: query.get('after') ?? undefined,
      limit: query.has('limit') ? Number(query.get('limit')) : undefined,
    }),
  'GET /api/v1/event-worker-profiles/{id}': ({ client, param }) =>
    client.eventWorkerProfiles.get(param('id')),
  'GET /api/v1/event-worker-profiles/{id}/revisions/{revision}': ({ client, param }) =>
    client.eventWorkerProfiles.revision(param('id'), Number(param('revision'))),
  'PUT /api/v1/event-worker-profiles/{id}': ({ client, param, body }) => {
    const input = body as { expected_revision: number; source: EventWorkerProfileInput };
    return client.eventWorkerProfiles.update(param('id'), input.expected_revision, input.source);
  },
  'DELETE /api/v1/event-worker-profiles/{id}': ({ client, param, query }) =>
    client.eventWorkerProfiles.retire(param('id'), Number(query.get('expected_revision'))),
};
