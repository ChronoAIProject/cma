import type { Cma } from '@chronoai/cma-sdk';

/** Public capabilities used by bridge management; credentials remain write-only. */
export type BridgeViewsClient = Pick<
  Cma,
  | 'bridges'
  | 'resourceAccess'
  | 'agents'
  | 'agentProfiles'
  | 'workspaceProfiles'
  | 'sandboxProfiles'
>;
