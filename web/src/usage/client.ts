import type { Cma } from '@chronoai/cma-sdk';
import type { MetricsClient } from './metrics/model';

/** Account usage deliberately exposes only owner-scoped SDK operations. */
export interface UsageClient {
  readonly usageMetrics: MetricsClient;
  readonly usage: Pick<Cma['usage'], 'events'>;
  readonly agents: Pick<Cma['agents'], 'list'>;
}
