import type { Dispatch } from './context.js';

export const accountDeletionDispatch: Record<string, Dispatch> = {
  'POST /api/v1/account/deletion': ({ client, command }) =>
    client.account.deletion.request(command),
  'GET /api/v1/account/deletion': ({ client }) => client.account.deletion.status(),
};
