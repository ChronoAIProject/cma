import type { Dispatch } from './context.js';

export const agentBrowserDispatch: Record<string, Dispatch> = {
  'GET /api/v1/agents/{id}/browser/control': ({ client, param, query, fixture }) =>
    client.agentBrowserControl.read(param('id'), {
      runtime_incarnation: query.get('runtime_incarnation') ?? '',
      context_id: query.get('context_id') ?? '',
      client_id: query.get('client_id') ?? '',
      client_key: fixture.request.headers['x-cma-browser-client'] ?? '',
      operation_id: query.get('operation_id') ?? undefined,
    }),
  'POST /api/v1/agents/{id}/browser/control': ({ client, param, body, fixture }) =>
    client.agentBrowserControl.change(param('id'), {
      ...(body as import('../../src/agent-browser-control.js').BrowserControlChange),
      client_key: fixture.request.headers['x-cma-browser-client'] ?? '',
    }),
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
