import { createContext, useMemo } from 'react';

export interface UsageReportOwner {
  readonly principal?: string;
  readonly cacheKey: string;
}

export const anonymousReportOwner: UsageReportOwner = { cacheKey: 'unscoped' };
export const UsageReportOwnerContext = createContext(anonymousReportOwner);

/** Subjects are opaque; tuple encoding keeps punctuation out of identity parsing. */
export function useReportOwner(
  principal: string | undefined,
  capabilities: readonly string[],
  epoch: number,
): UsageReportOwner {
  const cacheKey = JSON.stringify([principal, [...capabilities].sort(), epoch]);
  return useMemo(() => ({ principal, cacheKey }), [principal, cacheKey]);
}
