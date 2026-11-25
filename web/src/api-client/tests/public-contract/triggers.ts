import type { Dispatch } from './context.js';

export const triggerDispatch: Record<string, Dispatch> = {
  'GET /api/v1/triggers/public/{public_id}/badge.svg': ({ client, param, query, fixture }) =>
    client.triggers.badge(param('public_id'), {
      label: (query.get('label') ?? undefined) as never,
      theme: (query.get('theme') ?? undefined) as never,
      placement: query.get('placement') ?? undefined,
      ifNoneMatch: fixture.request.headers['if-none-match'],
    }),
  'GET /embed/v1.js': ({ client }) => client.triggers.embedScript(),
  'GET /api/v1/triggers': ({ client, options }) => client.triggers.list(options),
  'POST /api/v1/triggers': ({ client, body, command }) => client.triggers.create(body, command),
  'GET /api/v1/triggers/{id}': ({ client, param }) => client.triggers.read(param('id')),
  'PUT /api/v1/triggers/{id}': ({ client, param, body, command }) =>
    client.triggers.save(param('id'), body, command),
  'DELETE /api/v1/triggers/{id}': ({ client, param, command }) =>
    client.triggers.delete(param('id'), command),
  'POST /api/v1/triggers/{id}/publish': ({ client, param, body, command }) =>
    client.triggers.publish(param('id'), body, command),
  'POST /api/v1/triggers/{id}/retire': ({ client, param, command }) =>
    client.triggers.retire(param('id'), command),
  'GET /api/v1/triggers/{id}/revisions': ({ client, param }) =>
    client.triggers.listRevisions(param('id')),
  'GET /api/v1/triggers/{id}/placements': ({ client, param, query }) =>
    client.triggers.listPlacements(param('id'), {
      includeRetired: query.has('include_retired')
        ? query.get('include_retired') === 'true'
          ? true
          : query.get('include_retired') === 'false'
            ? false
            : (query.get('include_retired') as never)
        : undefined,
    }),
  'POST /api/v1/triggers/{id}/placements': ({ client, param, body, command }) =>
    client.triggers.createPlacement(param('id'), body, command),
  'POST /api/v1/triggers/{id}/placements/{placement_id}/retire': ({ client, param, command }) =>
    client.triggers.retirePlacement(param('id'), param('placement_id'), command),
  'GET /api/v1/triggers/{id}/embed': ({ client, param, query }) =>
    client.triggers.embed(param('id'), query.get('placement_id')!),
  'GET /api/v1/triggers/{id}/stats': ({ client, param }) => client.triggers.stats(param('id')),
  'GET /api/v1/triggers/public/{public_id}': ({ client, param }) =>
    client.launches.preview(param('public_id')),
  'POST /api/v1/triggers/public/{public_id}/entry-intent': ({ client, param }) =>
    client.launches.entryIntent(param('public_id')),
  'POST /api/v1/triggers/public/{public_id}/launches': ({ client, param, body, command }) =>
    client.launches.createOrResume(param('public_id'), body, command),
  'POST /api/v2/triggers/public/{public_id}/launches': ({
    client,
    param,
    body,
    command,
    fixture,
  }) =>
    client.launches.createOrResumeBound(param('public_id'), body, {
      ...command,
      expectedSubject: Buffer.from(
        (fixture.request.headers['x-cma-expected-subject'] ?? '').slice(3),
        'base64url',
      ).toString('utf8'),
    }),
  'GET /api/v1/launches': ({ client, options }) => client.launches.list(options),
  'GET /api/v1/launches/{id}': ({ client, param }) => client.launches.read(param('id')),
  'POST /api/v1/launches/{id}/start': ({ client, param, body, command }) =>
    client.launches.start(param('id'), body, command),
  'POST /api/v1/launches/{id}/cancel': ({ client, param, command }) =>
    client.launches.cancel(param('id'), command),
  'GET /api/v1/launches/{id}/requirements': ({ client, param }) =>
    client.launches.requirements(param('id')),
  'GET /api/v1/launches/{id}/events': ({ client, param, options }) =>
    client.launches.events(param('id'), options),
  'POST /api/v1/service-connections': ({ client, body, command }) =>
    client.launches.createConnection(body, command),
  'GET /api/v1/service-connections/{handoff_id}': ({ client, param }) =>
    client.launches.readConnection(param('handoff_id')),
};
