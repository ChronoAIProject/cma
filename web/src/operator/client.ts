import type {
  Administration,
  Cma,
  MarketplaceListOptions,
  MarketplaceSummary,
  ProfileKind,
  ResourcePage,
} from '@chronoai/cma-sdk';

export interface OperatorClient {
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
    admin: client.admin,
    listPublicProfiles: (kind, options) => client.marketplace[kind].list(options),
  };
}
