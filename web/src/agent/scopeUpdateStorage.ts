import { validAccessServices } from './accessRequests';

export interface ScopeUpdateIntent {
  readonly key: string;
  readonly services: readonly string[];
  readonly expectedServices: readonly string[];
  readonly requestId?: string;
}
function storageKey(owner: string, agent: string) {
  return `cma:scope-update:${JSON.stringify([location.origin, owner, agent])}`;
}
export function loadScopeUpdate(owner: string | undefined, agent: string) {
  if (!owner) return {};
  try {
    const raw = sessionStorage.getItem(storageKey(owner, agent));
    if (raw === null) return {};
    const saved = JSON.parse(raw) as { version?: number; intent?: ScopeUpdateIntent };
    const intent = saved.intent;
    if (
      saved.version !== 1 ||
      !intent ||
      !/^[a-zA-Z0-9-]{1,200}$/.test(intent.key) ||
      !validAccessServices(intent.services) ||
      !validAccessServices(intent.expectedServices) ||
      (intent.requestId !== undefined &&
        (!intent.requestId || typeof intent.requestId !== 'string'))
    )
      throw new Error();
    return { intent };
  } catch {
    return {
      error:
        'The saved scope update could not be read. Restore browser session storage before continuing.',
    };
  }
}
export function saveScopeUpdate(owner: string, agent: string, intent?: ScopeUpdateIntent) {
  const key = storageKey(owner, agent);
  if (intent) sessionStorage.setItem(key, JSON.stringify({ version: 1, intent }));
  else sessionStorage.removeItem(key);
}
