import type { Dispatch } from './context.js';

export const botDispatch: Record<string, Dispatch> = {
  'POST /api/v1/bot-setups/advance': ({ client, body }) => client.botSetups.advance(body),
  'POST /api/v1/bot-setups/actions': ({ client, body }) => client.botSetups.act(body),
  'PUT /api/v1/gateway/interaction-records/{record_id}': ({ client, param, body }) =>
    client.gatewayInteractions.put(param('record_id'), body),
  'GET /api/v1/gateway/interaction-records/{record_id}': ({ client, param }) =>
    client.gatewayInteractions.get(param('record_id')),
  'DELETE /api/v1/gateway/interaction-records/{record_id}': ({ client, param, query }) =>
    client.gatewayInteractions.delete(param('record_id'), Number(query.get('expected_version'))),
  'POST /api/v1/bots': ({ client, body, command }) => client.bots.create(body, command),
  'GET /api/v1/bots': ({ client, query }) =>
    client.bots.list({
      after: query.get('after') ?? undefined,
      limit: query.has('limit') ? Number(query.get('limit')) : undefined,
    }),
  'GET /api/v1/bots/nyxid-bots': ({ client }) => client.bots.available(),
  'GET /api/v1/bots/{bot_id}': ({ client, param }) => client.bots.get(param('bot_id')),
  'POST /api/v1/bots/{bot_id}/test': ({ client, param }) => client.bots.test(param('bot_id')),
  'POST /api/v1/bots/{bot_id}/disable': ({ client, param }) => client.bots.disable(param('bot_id')),
  'POST /api/v1/bots/{bot_id}/enable': ({ client, param }) => client.bots.enable(param('bot_id')),
  'POST /api/v1/bots/{bot_id}/rotate-keys': ({ client, param }) =>
    client.bots.rotateKeys(param('bot_id')),
  'DELETE /api/v1/bots/{bot_id}': ({ client, param }) => client.bots.delete(param('bot_id')),
};
