import { ApiError, type Bot, type BotAdmission, type Cma } from '@chronoai/cma-sdk';

export interface BotClient {
  readonly eventWorkerProfiles?: Cma['eventWorkerProfiles'];
  readonly bots: Pick<
    Cma['bots'],
    | 'list'
    | 'get'
    | 'create'
    | 'available'
    | 'test'
    | 'disable'
    | 'enable'
    | 'rotateKeys'
    | 'delete'
  >;
}
export const stateLabels: Record<Bot['state'], string> = {
  creating: 'Creating',
  active: 'Active',
  disabled: 'Disabled',
  failed: 'Needs attention',
  deleting: 'Deleting',
  deleted: 'Deleted',
};
const steps: Record<string, string> = {
  admitted: 'Waiting to start',
  queued: 'Waiting to start',
  verify_external_bot: 'Checking your Telegram bot',
  mint_agent_key: 'Preparing Agent access',
  mint_route_key: 'Preparing channel access',
  keys_minted: 'Access prepared',
  create_channel: 'Connecting the Agent provider',
  set_callback: 'Setting the callback',
  create_route: 'Creating the message route',
  attach_route: 'Connecting the message route',
  recover_channel_for_deletion: 'Finding the connection',
  delete_channel: 'Removing the connection',
  discover_owned_keys: 'Finding Bot access',
  delete_routes: 'Removing message routes',
  revoke_owned_key: 'Revoking Bot access',
  verify_cleanup: 'Checking cleanup',
  cleanup_pending: 'Waiting for cleanup',
  read_channel: 'Checking the connection',
  configure_admission: 'Updating message access',
  dependency_retry: 'Waiting for a service to recover',
};
export const creationSteps = [
  ['verify_external_bot', 'Check Telegram bot'],
  ['mint_agent_key', 'Prepare Agent access'],
  ['mint_route_key', 'Prepare channel access'],
  ['create_channel', 'Connect Agent provider'],
  ['set_callback', 'Set callback'],
  ['create_route', 'Create message route'],
  ['attach_route', 'Connect message route'],
] as const;
export function creationStepIndex(bot: Bot): number | undefined {
  if (bot.operation?.operation !== 'create') return undefined;
  if (bot.operation.step === 'admitted' || bot.operation.step === 'queued') return -1;
  if (bot.operation.step === 'keys_minted') return 2.5;
  const index = creationSteps.findIndex(([step]) => step === bot.operation?.step);
  return index < 0 ? undefined : index;
}
export function operationLabel(bot: Bot): string | undefined {
  if (bot.operation === null) return undefined;
  return steps[bot.operation.step] ?? 'Updating the Bot connection';
}
export function problem(error: unknown): string {
  return error instanceof ApiError
    ? error.message
    : 'The request could not be confirmed. Refresh to check its status.';
}
export function denied(error: unknown): boolean {
  return error instanceof ApiError && [401, 403, 404].includes(error.status);
}
export function definiteRefusal(error: unknown): boolean {
  return error instanceof ApiError && [400, 401, 403, 404, 409, 422, 429].includes(error.status);
}
export type AdmissionChoice = '' | 'private' | 'allowlist' | 'mention';
export function admissionFor(choice: AdmissionChoice, senders: string): BotAdmission | undefined {
  if (choice === '') return undefined;
  const ids = senders
    .split(',')
    .map((id) => id.trim())
    .filter(Boolean);
  if (
    choice === 'allowlist' &&
    (ids.length === 0 ||
      ids.length > 512 ||
      new Set(ids).size !== ids.length ||
      ids.some((id) => new TextEncoder().encode(id).length > 512 || /\p{Cc}/u.test(id)))
  )
    return undefined;
  return {
    type: 'scoped',
    senders: choice === 'allowlist' ? { type: 'allowlist', ids } : { type: 'open' },
    chats: { type: 'open' },
    groups: choice === 'mention' ? 'mention' : 'deny',
  };
}
