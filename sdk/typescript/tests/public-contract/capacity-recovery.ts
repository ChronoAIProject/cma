import type { Dispatch } from './context.js';
const base = '/api/v1/agents/{id}/capacity-recovery';
export const capacityRecoveryDispatch: Record<string, Dispatch> = {
  [`GET ${base}/dry-run`]: ({ client, param }) =>
    client.agents.capacityRecovery(param('id')).dryRun(),
  [`GET ${base}`]: ({ client, param }) => client.agents.capacityRecovery(param('id')).status(),
  [`POST ${base}/status`]: ({ client, param, body }) =>
    client.agents.capacityRecovery(param('id')).requestStatus(body),
  [`POST ${base}/admit`]: ({ client, param, body }) =>
    client.agents.capacityRecovery(param('id')).admit(body),
  [`POST ${base}/resume`]: ({ client, param, body }) =>
    client.agents.capacityRecovery(param('id')).resume(body),
};
