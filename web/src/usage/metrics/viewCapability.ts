import { ApiError } from '@cma/client';
import type { MetricsClient } from './model';

type ViewRoute = 'initial' | 'latest';
const DISABLED_TTL_MS = 30_000;
const disabled = new WeakMap<MetricsClient, { owner: string; until: Map<string, number> }>();

function isUsageViewDisabled(error: unknown): boolean {
  return error instanceof ApiError && error.status === 501 && error.code === 'report_view_disabled';
}

export function viewUnavailable(
  api: MetricsClient,
  owner: string,
  admin: boolean,
  route: ViewRoute,
): boolean {
  const state = disabled.get(api);
  return state?.owner === owner && (state.until.get(`${admin}/${route}`) ?? 0) > Date.now();
}

function rememberDisabledView(api: MetricsClient, owner: string, admin: boolean, route: ViewRoute) {
  let state = disabled.get(api);
  if (state?.owner !== owner) {
    state = { owner, until: new Map() };
    disabled.set(api, state);
  }
  state.until.set(`${admin}/${route}`, Date.now() + DISABLED_TTL_MS);
}

export function clearDisabledViews(api: MetricsClient, owner: string) {
  if (disabled.get(api)?.owner === owner) disabled.delete(api);
}

export async function observeUsageView<T>(
  api: MetricsClient,
  owner: string,
  signal: AbortSignal,
  admin: boolean,
  route: ViewRoute,
  load: () => Promise<T>,
) {
  try {
    return await load();
  } catch (error) {
    if (!signal.aborted && isUsageViewDisabled(error))
      rememberDisabledView(api, owner, admin, route);
    throw error;
  }
}
