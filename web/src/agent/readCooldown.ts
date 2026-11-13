import type { CmaWebClient } from '../api/client';
import { detailReadSource } from '../api/detailReads';
import { isIdentityChanged } from './transport/identity';

interface Cooldown {
  readonly at: number;
  readonly check?: () => void;
}
interface Scope {
  readonly viewer?: string;
  readonly entries: Map<string, Cooldown>;
  overflow?: Cooldown;
}
const deadlines = new WeakMap<CmaWebClient, Scope>();
const MAX_DEADLINES = 64;
function scopeFor(client: CmaWebClient, viewer?: string) {
  const source = detailReadSource(client);
  let scope = deadlines.get(source);
  if (!scope || scope.viewer !== viewer) {
    scope = { viewer, entries: new Map() };
    deadlines.set(source, scope);
  }
  for (const [id, value] of scope.entries) if (value.at <= Date.now()) scope.entries.delete(id);
  if (scope.overflow && scope.overflow.at <= Date.now()) scope.overflow = undefined;
  return scope;
}

/** Retry timing only: resource values and authority never survive through this map. */
export function readCooldown(client: CmaWebClient, viewer: string | undefined, agent?: string) {
  if (!agent) return 0;
  const scope = scopeFor(client, viewer);
  const value = scope.entries.get(agent) ?? scope.overflow;
  if (!value) return 0;
  try {
    value.check?.();
  } catch (error) {
    scope.entries.clear();
    scope.overflow = undefined;
    if (!isIdentityChanged(error)) console.warn('Agent read retry identity could not be checked');
    return 0;
  }
  return value.at;
}

export function rememberReadCooldown(
  client: CmaWebClient,
  viewer: string | undefined,
  agent: string,
  at: number,
) {
  const scope = scopeFor(client, viewer);
  scope.entries.delete(agent);
  if (at <= Date.now()) return;
  let check: (() => void) | undefined;
  try {
    check = client.agentTransport?.identityFence();
  } catch {
    console.warn('Agent read retry identity could not be captured');
    return;
  }
  const value = { at, check };
  if (scope.entries.size < MAX_DEADLINES) scope.entries.set(agent, value);
  // A bounded overflow floor delays unknown Agents conservatively instead of
  // evicting a live deadline and allowing a remount to bypass server pacing.
  else if (!scope.overflow || scope.overflow.at < at) scope.overflow = value;
}
