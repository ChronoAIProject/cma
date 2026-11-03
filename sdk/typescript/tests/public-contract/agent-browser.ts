import type { Dispatch } from './context.js';

export const agentBrowserDispatch: Record<string, Dispatch> = {
  'GET /api/v1/agents/{id}/browser': ({ client, param, query }) =>
    client.agentBrowser.read(param('id'), {
      runtime_incarnation: query.get('runtime_incarnation') ?? '',
      observer: query.get('observer') ?? undefined,
      context_id: query.get('context_id') ?? undefined,
    }),
  'PUT /api/v1/agents/{id}/browser': ({ client, param, body }) => {
    const request = body as { runtime_incarnation: string; enabled: boolean };
    return client.agentBrowser.configure(param('id'), request.runtime_incarnation, request.enabled);
  },
  'DELETE /api/v1/agents/{id}/browser': ({ client, param, query }) =>
    client.agentBrowser.release(param('id'), {
      runtime_incarnation: query.get('runtime_incarnation') ?? '',
      observer: query.get('observer') ?? undefined,
      context_id: query.get('context_id') ?? undefined,
    }),
};
