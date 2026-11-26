import type { Dispatch } from './context.js';

export const agentReplyPreviewDispatch: Record<string, Dispatch> = {
  'GET /api/v1/agents/{id}/reply-preview': ({ client, param, options }) =>
    client.agents.replyPreview(param('id'), options),
};
