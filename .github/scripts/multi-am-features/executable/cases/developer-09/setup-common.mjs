import {createHash, randomUUID} from 'node:crypto';
import {Blocked} from '../../runner.mjs';
export const check = (condition, code) => {if (!condition) throw Error(code);};
export const requireSetup = (condition, code) => {if (!condition) throw new Blocked(code);};
export const sha = value => createHash('sha256').update(JSON.stringify(value)).digest('hex');
export const key = () => 'admin09-' + randomUUID();
const leases = new WeakMap();
export const once = operation => {
  let promise;
  return () => promise ??= Promise.resolve().then(operation);
};
export async function cleanupSteps(ctx, steps) {
  const failures = [];
  for (const [stage, operation] of steps) {
    try { await operation(); }
    catch (cause) {
      failures.push(new Error(stage, {cause}));
      try {
        await ctx.observe('admin09_cleanup_failure', async () => ({stage,
          code: /^[a-z][a-z0-9_]{0,100}$/.test(cause?.message ?? '') ? cause.message : 'cleanup_operation_failed'}));
      } catch (observationError) {
        failures.push(new Error(stage + '_observation', {cause: observationError}));
      }
    }
  }
  if (failures.length) throw new AggregateError(failures, 'admin09_cleanup_failed');
}
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
  const state = {ctx, active: true};
  const release = once(async () => {
    state.active = false;
    const receipt = await lease.release();
    check(receipt?.registered === true && receipt.parent_cleanup_barrier === true, 'admin09_cleanup_barrier_missing');
    await ctx.observe('admin09_parent_custody', async () => ({registered: true, parent_cleanup_barrier: true,
      final_database_cleanup: 'PARENT_MUST_CONFIRM_BEFORE_CAMPAIGN_PASS'}));
  });
  // Register the release as soon as the parent returns a lease. A malformed
  // lease must not strand a reservation acquired before validation completed.
  if (typeof lease?.release === 'function') ctx.defer('admin09_parent_lease_partial', release);
  requireSetup(lease?.exclusive === true && lease.parent_cleanup_barrier === true
    && ['release', 'retainIntent', 'retainDocument'].every(method => typeof lease[method] === 'function'), 'admin09_exclusive_custody_required');
  const owned = {release, retainIntent: intent => lease.retainIntent(intent), retainDocument: value => lease.retainDocument(value)};
  leases.set(owned, state);
  return owned;
}
// A nested recipe may borrow custody only from this invocation's live lease.
// It gets no release method: the outer recipe restores grants before unlocking.
export function borrowCustody(ctx, lease) {
  const state = leases.get(lease);
  requireSetup(state?.ctx === ctx && state.active, 'admin09_active_parent_lease_required');
  return {retainIntent: intent => {check(state.active, 'admin09_parent_lease_released'); return lease.retainIntent(intent);},
    retainDocument: value => {check(state.active, 'admin09_parent_lease_released'); return lease.retainDocument(value);}};
}
export async function request(ctx, input) {
  const response = await ctx.request(input);
  await ctx.observe('admin09_setup_request', async () => ({method: input.method, path_sha256: sha(input.path),
    status: response.status, request_id: response.timing.request_id, am: response.timing.am}));
  return response.body;
}
