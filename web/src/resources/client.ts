import type { Cma } from '@chronoai/cma-sdk';

/** Public SDK surfaces used for organization and shared resource views. */
export type ResourceViewsClient = Pick<
  Cma,
  'agents' | 'resourceAccess' | 'sandboxes' | 'workspaces'
> & { readonly agentEventsV2?: Pick<Cma['agentEventsV2'], 'head'> };
