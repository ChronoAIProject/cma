import type { Dispatch } from './context.js';

export const usageDispatch: Record<string, Dispatch> = {
  'GET /api/v1/usage': ({ client }) => client.usage.summary(),
  'GET /api/v1/usage/documents': ({ client }) => client.usage.documents(),
  'GET /api/v1/usage/sandboxes': ({ client, options }) => client.usage.sandboxes(options),
  'GET /api/v1/usage/sandboxes/{id}': ({ client, param }) => client.usage.sandbox(param('id')),
  'GET /api/v1/usage/sandboxes/{id}/events': ({ client, param, options }) =>
    client.usage.events(param('id'), options),
  'GET /api/v1/admin/usage': ({ client, options }) => client.usage.adminSummary(options),
  'GET /api/v1/admin/usage/sandboxes': ({ client, options }) =>
    client.usage.adminSandboxes(options),
};
