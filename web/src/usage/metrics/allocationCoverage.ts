import type { UsageMetricRow } from '@cma/client';
import { humanDuration } from './humanDuration';

type Coverage = 'missing' | 'unverified' | 'complete';
type PeriodCoverage = Pick<
  UsageMetricRow['allocated_period'],
  'complete' | 'unknown_seconds' | 'projected_tail_seconds'
>;
export type AllocationCoverageEvidence = Pick<PeriodCoverage, 'complete'> &
  Partial<Omit<PeriodCoverage, 'complete'>>;

export function allocationCoverage(value: AllocationCoverageEvidence) {
  const coverage: Coverage =
    value.unknown_seconds !== undefined && BigInt(value.unknown_seconds) > 0n
      ? 'missing'
      : value.complete
        ? 'complete'
        : 'unverified';
  const projected =
    value.projected_tail_seconds === undefined
      ? undefined
      : BigInt(value.projected_tail_seconds) > 0n;
  const missingDuration =
    value.unknown_seconds === undefined
      ? 'Unavailable'
      : coverage === 'unverified'
        ? 'None confirmed'
        : humanDuration(value.unknown_seconds);
  return {
    coverage,
    projected,
    label:
      coverage === 'missing'
        ? 'Missing intervals confirmed'
        : coverage === 'unverified'
          ? 'Coverage unverified'
          : 'Coverage verified',
    missingDuration,
    description:
      coverage === 'missing'
        ? `Confirmed missing duration: ${missingDuration}. Missing intervals are excluded from these totals; this does not mean zero usage.`
        : coverage === 'unverified'
          ? value.unknown_seconds === undefined
            ? 'Missing-interval durations are unavailable for this scope.'
            : 'No missing interval has been established.'
          : 'Allocation history is verified for this scope.',
    projection:
      value.projected_tail_seconds === undefined
        ? 'Projection details are unavailable for this scope.'
        : projected
          ? `Projection from last known state: ${humanDuration(value.projected_tail_seconds)} (estimated).`
          : undefined,
  };
}

export function combineAllocationCoverage(values: readonly PeriodCoverage[]): PeriodCoverage {
  return {
    complete: values.length > 0 && values.every((value) => value.complete),
    unknown_seconds: values
      .reduce((total, value) => total + BigInt(value.unknown_seconds), 0n)
      .toString(),
    projected_tail_seconds: values
      .reduce((total, value) => total + BigInt(value.projected_tail_seconds), 0n)
      .toString(),
  };
}
