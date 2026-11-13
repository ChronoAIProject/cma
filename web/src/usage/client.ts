import type { Cma } from '@chronoai/cma-sdk';
import { measuredUsageReader } from './metrics/readMeasuredUsage';
import type { ReadMeasuredUsage } from './metrics/useMeasuredUsage';
import type { MetricsClient } from './metrics/model';

/** Account usage deliberately exposes only owner-scoped SDK operations. */
export interface UsageClient {
  readonly readMeasuredUsage: ReadMeasuredUsage;
  readonly usageMetrics: MetricsClient;
  readonly usage: Pick<Cma['usage'], 'events'>;
  readonly agents: Pick<Cma['agents'], 'list'>;
}

export function createUsageClient(client: Cma): UsageClient {
  return {
    readMeasuredUsage: measuredUsageReader(client.auth),
    usageMetrics: client.usageMetrics,
    usage: client.usage,
    agents: client.agents,
  };
}
