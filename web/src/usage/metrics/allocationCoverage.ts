import type { UsageMetricRow } from '@cma/client';

type Coverage = 'missing' | 'unverified' | 'complete';

export function allocationCoverage(value: UsageMetricRow['allocated_period']): {
  readonly coverage: Coverage;
  readonly projected: boolean;
} {
  return {
    coverage:
      BigInt(value.unknown_seconds) > 0n ? 'missing' : value.complete ? 'complete' : 'unverified',
    projected: BigInt(value.projected_tail_seconds) > 0n,
  };
}
