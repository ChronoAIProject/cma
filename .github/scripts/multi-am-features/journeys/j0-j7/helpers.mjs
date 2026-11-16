import {createHash, randomUUID} from 'node:crypto';
import {Blocked} from '../../executable/runner.mjs';

export const check = (value, code) => { if (!value) throw Error(code); };
export const hash = value => createHash('sha256').update(String(value)).digest('hex');
export const unique = label => `mfunc01-${label}-${randomUUID()}`;
export const opposite = am => am === 'a' ? 'b' : 'a';

export function actorFor(identity) {
  check(identity?.actor, 'fresh_identity_actor_missing');
  return identity.actor;
}

export async function request(ctx, identity, options) {
  const {proofState, lifecycle = false, ...input} = options;
  const response = await ctx.request({actor: actorFor(identity), ...input});
  if (proofState) {
    check(response?.timing?.request_id, 'mutation_request_id_missing');
    proofState.push({request_id: response.timing.request_id, am: response.timing.am,
      method: input.method, path: input.path, status: response.status, lifecycle,
      ...(input.path === '/api/v1/agents' ? {operation_id: response.body?.id} : {})});
  }
  return response;
}

export {requireOwnerProof} from './owner-proof.mjs';
export {observeBrowser} from './browser-proof.mjs';

export async function waitFor(ctx, operation, predicate, code, {intervalMs = 2000, attempts = 90} = {}) {
  let last;
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    last = await operation();
    if (predicate(last)) return last;
    if (attempt + 1 < attempts) {
      const remaining = typeof ctx.remainingMs === 'function' ? ctx.remainingMs() : intervalMs;
      if (remaining < intervalMs) break;
      await new Promise(resolve => setTimeout(resolve, intervalMs));
    }
  }
  throw Error(code);
}

export function operationId(response) {
  const value = response?.body?.id;
  check(typeof value === 'string' && value.length > 0, 'create_receipt_missing');
  return value;
}

export function requireBrowser(ctx) {
  const browser = ctx.fixture('browser');
  if (typeof browser?.open !== 'function' || typeof browser?.evidence !== 'function') {
    throw new Blocked('fresh_identity_browser_fixture_unavailable');
  }
  return browser;
}
