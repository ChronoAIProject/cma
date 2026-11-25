import { ApiError, isUsageInitialUnavailable } from '@cma/client';
import { unsupportedLatestRoute } from './latest';
import type { MetricsClient } from './model';
import type { ReportEntry } from './reportCache';

type Capability = 'initial-view' | 'initial-report' | 'latest';
const DISABLED_TTL_MS = 5 * 60_000;
const disabledViews = new WeakMap<MetricsClient, { owner: string; until: Map<string, number> }>();

export function clearViewCapability(api: MetricsClient, owner: string) {
  if (disabledViews.get(api)?.owner === owner) disabledViews.delete(api);
}

function capabilityState(api: MetricsClient, owner: string) {
  let state = disabledViews.get(api);
  if (!state || state.owner !== owner) {
    state = { owner, until: new Map() };
    disabledViews.set(api, state);
  }
  return state.until;
}

export async function loadCompatibleReport({
  api,
  owner,
  admin,
  entry,
  signal,
  background,
  initialMode,
  initial,
  latest,
  resumeInitial,
  legacy,
}: {
  api: MetricsClient;
  owner: string;
  admin: boolean;
  entry: ReportEntry;
  signal: AbortSignal;
  background: boolean;
  initialMode?: Capability;
  initial: () => Promise<boolean>;
  latest: () => Promise<boolean>;
  resumeInitial: () => Promise<void>;
  legacy: () => Promise<void>;
}) {
  const until = capabilityState(api, owner);
  const key = (mode: Capability) => `${admin}:${mode}`;
  const disabled = (mode: Capability) => (until.get(key(mode)) ?? 0) > Date.now();
  const remember = (mode: Capability, error: unknown) => {
    signal.throwIfAborted();
    if (error instanceof ApiError && error.status === 501 && error.code === 'report_view_disabled')
      until.set(key(mode), Date.now() + DISABLED_TTL_MS);
  };
  if (initialMode && (entry.initialPending || !disabled(initialMode))) {
    try {
      if (await initial()) return;
    } catch (error) {
      if (!entry.initialPending) remember(initialMode, error);
      if (background || !isUsageInitialUnavailable(error)) throw error;
      if (entry.initialPending) {
        await resumeInitial();
        return;
      }
    }
  }
  if (entry.latestPending || !disabled('latest')) {
    try {
      if (await latest()) return;
    } catch (error) {
      if (!entry.latestPending) remember('latest', error);
      if (background || !unsupportedLatestRoute(error)) throw error;
    }
  } else if (background) {
    entry.observationStartedAt = 0;
    return;
  }
  signal.throwIfAborted();
  await legacy();
}
