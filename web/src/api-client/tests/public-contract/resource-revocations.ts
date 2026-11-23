import type { Dispatch } from './context.js';
export const revocationDispatch: Record<string, Dispatch> = {
  'POST /api/v1/resource-revocations': ({ client, body, command }) =>
    client.resourceRevocations.request(body, command),
  'GET /api/v1/resource-revocations/{operation_id}': ({ client, param, query }) =>
    client.resourceRevocations.status(
      param('operation_id'),
      query.get('kind') as 'agent' | 'sandbox',
      query.get('id')!,
    ),
};
