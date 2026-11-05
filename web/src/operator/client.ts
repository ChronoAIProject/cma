import type {
  Administration,
  Cma,
  MarketplaceListOptions,
  MarketplaceSummary,
  ProfileKind,
  ResourcePage,
} from '@chronoai/cma-sdk';

export interface OperatorClient {
  readonly usage: Pick<
    Cma['usage'],
    'summary' | 'sandboxes' | 'sandbox' | 'events' | 'adminSummary' | 'adminSandboxes'
  >;
  readonly admin: Pick<
    Administration,
    | 'listSandboxes'
    | 'readSandbox'
    | 'sandboxEvents'
    | 'retireSandbox'
    | 'listProfiles'
    | 'retireProfile'
    | 'readSettings'
    | 'replaceSettings'
    | 'previewAutoPause'
    | 'pauseIdleNow'
    | 'setSandboxAlwaysOn'
    | 'listActivity'
    | 'listWarmPools'
    | 'configureWarmPool'
    | 'resizeWarmPool'
  >;
  listPublicProfiles(
    kind: ProfileKind,
    options?: MarketplaceListOptions,
  ): Promise<ResourcePage<MarketplaceSummary>>;
}

export function createOperatorClient(client: Cma): OperatorClient {
  return {
    usage: client.usage,
    admin: client.admin,
    listPublicProfiles: (kind, options) => client.marketplace[kind].list(options),
  };
}
