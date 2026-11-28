import { useEffect } from 'react';
import type { MetricsClient, Selection } from './model';
import type { LatestMetricsClient } from './latest';
import type { UsageReportOwner } from './reportOwner';
import { bindCacheOwner, entryFor, notify, reportKey } from './reportCache';
import { startReportLoad } from './reportLoad';
import { viewUnavailable } from './viewCapability';

/** Two one-shot admissions, not hidden poll loops. Selected work retains priority. */
export function useWarmUsageRanges(
  api: MetricsClient,
  selection: Selection,
  admin: boolean,
  owner: UsageReportOwner,
  active: boolean,
  dailyPreviewOnly = false,
) {
  useEffect(() => {
    if (!active || !(api as LatestMetricsClient).latest || selection.range === 'custom') return;
    let cancelled = false;
    let release: (() => void) | undefined;
    bindCacheOwner(api, owner.cacheKey);
    const selected = entryFor(api, reportKey(selection, admin, owner.cacheKey), owner.cacheKey);
    void (async () => {
      // A direct long-range entry can use the server's already warmed daily
      // snapshot even when this browser has no committed local report yet.
      const ranges = dailyPreviewOnly
        ? (['24h'] as const)
        : selection.range === '7d' || selection.range === '30d'
          ? (['24h', '7d', '30d'] as const)
          : (['7d', '30d'] as const);
      for (const range of ranges) {
        if (
          cancelled ||
          document.visibilityState === 'hidden' ||
          viewUnavailable(api, owner.cacheKey, admin, 'latest')
        )
          return;
        if (range === selection.range) continue;
        const wanted = { ...selection, range };
        const entry = entryFor(api, reportKey(wanted, admin, owner.cacheKey), owner.cacheKey);
        // A recent attempt includes admission. Reopening must not admit again
        // merely because a background report has not completed yet.
        if (
          entry.inFlight ||
          entry.authorityBlocked ||
          entry.stopped ||
          entry.retryAt > Date.now() ||
          (entry.attempted && Date.now() - entry.lastStarted < 300_000)
        )
          continue;
        release = startReportLoad(
          api,
          wanted,
          admin,
          owner.cacheKey,
          entry,
          true,
          false,
          owner.principal,
          true,
        );
        try {
          await entry.inFlight;
        } catch {
          // The report loader records/refuses the failure. Stop this optional
          // warming sequence instead of causing more admissions under pressure.
          return;
        } finally {
          release();
          release = undefined;
        }
        if (!cancelled) notify(selected);
      }
    })();
    return () => {
      cancelled = true;
      release?.();
    };
  }, [api, selection, admin, owner, active, dailyPreviewOnly]);
}
