import {createHash, randomUUID} from 'node:crypto';
import {Blocked} from '../../runner.mjs';
export const check = (condition, code) => {if (!condition) throw Error(code);};
export const requireSetup = (condition, code) => {if (!condition) throw new Blocked(code);};
export const sha = value => createHash('sha256').update(JSON.stringify(value)).digest('hex');
export const key = () => 'admin09-' + randomUUID();
export async function wait(read, done, code, limit = 180000) {
  const until = Date.now() + limit;
  for (;;) {const value = await read(); if (done(value)) return value;
    check(Date.now() < until, code); await new Promise(resolve => setTimeout(resolve, 2000));}
}
export async function reserve(ctx, primitives, purpose) {
  const db = primitives.disposableDatabase;
  requireSetup(db?.scope === 'LOCAL_DISPOSABLE_TWO_AM' && db.database_disposable === true
    && typeof db.reserve === 'function', 'admin09_disposable_parent_required');
  const lease = await db.reserve({purpose, caseId: ctx.row.case_id, exclusive: true});
  let released = false;
  const release = async () => {
    if (released) return; const receipt = await lease.release();
    check(receipt?.registered === true && receipt.parent_cleanup_barrier === true, 'admin09_cleanup_barrier_missing');
    released = true;
  };
  // Register the release as soon as the parent returns a lease. A malformed
  // lease must not strand a reservation acquired before validation completed.
  if (typeof lease?.release === 'function') ctx.defer('admin09_parent_lease_partial', release);
  requireSetup(lease?.exclusive === true && lease.parent_cleanup_barrier === true
    && ['release', 'retainIntent', 'retainDocument'].every(method => typeof lease[method] === 'function'), 'admin09_exclusive_custody_required');
  ctx.defer('admin09_parent_lease', release);
  return {...lease, release};
}
export async function request(ctx, input) {
  const response = await ctx.request(input);
  await ctx.observe('admin09_setup_request', async () => ({method: input.method, path_sha256: sha(input.path),
    status: response.status, request_id: response.timing.request_id, am: response.timing.am}));
  return response.body;
}
