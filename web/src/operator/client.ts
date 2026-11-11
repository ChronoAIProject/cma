import type {
  Administration,
  Cma,
  MarketplaceListOptions,
  MarketplaceSummary,
  ProfileKind,
  ResourcePage,
} from '@chronoai/cma-sdk';
import { measuredUsageReader } from '../usage/metrics/readMeasuredUsage';
import type { ReadMeasuredUsage } from '../usage/metrics/useMeasuredUsage';
import type { MetricsClient } from '../usage/metrics/model';

export interface OperatorClient {
  readonly readMeasuredUsage: ReadMeasuredUsage;
  readonly adminUsageMetrics: MetricsClient;
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
    readMeasuredUsage: measuredUsageReader(client.auth),
    usage: client.usage,
    adminUsageMetrics: client.adminUsageMetrics,
    admin: client.admin,
    listPublicProfiles: (kind, options) => client.marketplace[kind].list(options),
  };
}
