import type { Cma } from '@cma/client';

/** Public SDK surfaces used for organization and shared resource views. */
export type ResourceViewsClient = Pick<
  Cma,
  'agents' | 'resourceAccess' | 'sandboxes' | 'workspaces'
> & {
  readonly agentEventsV2?: Pick<Cma['agentEventsV2'], 'head'>;
  readonly resourceRevocations?: Cma['resourceRevocations'];
};
