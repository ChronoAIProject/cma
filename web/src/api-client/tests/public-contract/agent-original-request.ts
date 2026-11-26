import type { Dispatch } from './context.js';

export const agentOriginalRequestDispatch: Record<string, Dispatch> = {
  'GET /api/v1/agents/{id}/original-request': ({ client, param, options }) =>
    client.agents.originalRequest(param('id'), options),
};
