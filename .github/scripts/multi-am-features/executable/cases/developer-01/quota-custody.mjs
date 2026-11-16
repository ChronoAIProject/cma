import {randomUUID} from 'node:crypto';
import {check, required, equal, id, other, request} from './common.mjs';
import {poll} from './enforcement.mjs';

// Subordinate resource custody, not another global settings lease. The parent
// owns durable unresolved intents and the final disposable-database barrier.
export async function quotaCustody(ctx, config, owner, intents) {
  const lease = await config.reserve({purpose: 'admin01-quota-resources', caseId: ctx.row.case_id});
  const entries = new Map(intents.map(intent => [intent.key, {intent, attempted: false, unknown: false}]));
  const roots = new Map();
  let cleanupJob;

  function exact(value, entry) {
    return value?.id === entry.root.id && value.owner === owner
      && value.name === entry.intent.body.name
      && equal(value.profile, entry.intent.body.sandbox_profile);
  }
  async function readRoot(entry) {
    const response = await request(ctx, 'GET', '/api/v1/sandboxes/' + id(entry.root.id), undefined,
      {actor: 'user', am: other(ctx.am)});
    check(exact(response.body, entry), 'admin_quota_cleanup_identity');
    return response.body;
  }
  async function cleanupOnce() {
    const errors = [];
    for (const entry of [...roots.values()].reverse()) {
      try {
        const current = await readRoot(entry);
        if (!['retiring', 'retired'].includes(current.status)) {
          const key = randomUUID();
          // Retain the exact cleanup intent before its sole dispatch. No new
          // key or second DELETE is generated after an ambiguous response.
          await ctx.observe('admin_quota_retirement_intent', async () => ({
            kind: 'sandbox', id: entry.root.id, key, actor: 'user', am: other(ctx.am),
          }));
          try {
            await request(ctx, 'DELETE', '/api/v1/sandboxes/' + id(entry.root.id), undefined,
              {actor: 'user', am: other(ctx.am), key, allowed: [202]});
          } catch (error) {errors.push(error);}
        }
        await poll(() => readRoot(entry), value => value.status === 'retired',
          'admin_quota_retirement_deadline', config.timeout_ms ?? 120000);
        const final = await ctx.observe('admin_quota_native_finality', () =>
          config.finality({sandboxId: entry.root.id}));
        check(final?.sandbox_id === entry.root.id && final.children_remaining === 0
          && final.pending_operations === 0 && final.cleanup_complete === true,
        'admin_quota_native_cleanup_incomplete');
      } catch (error) {errors.push(error);}
    }
    if ([...entries.values()].some(entry => entry.unknown)) errors.push(Error('admin_quota_create_unresolved'));
    if (errors.length) throw new AggregateError(errors, 'admin_quota_cleanup_failed');
    required(typeof lease?.release === 'function', 'admin_quota_parent_release_required');
    const released = await lease.release();
    check(released?.released === true, 'admin_quota_custody_release_failed');
  }
  const cleanup = () => cleanupJob ??= cleanupOnce();
  ctx.defer('admin_quota_partial_cleanup', cleanup);
  required(lease?.parent_cleanup_barrier === true && ['retainIntent', 'retainResource',
    'retainRefusal', 'release'].every(name => typeof lease[name] === 'function'),
  'admin_quota_parent_custody_required');

  return {cleanup, async custody(intent, dispatch) {
    const entry = entries.get(intent?.key);
    check(entry && equal(entry.intent, intent) && !entry.attempted && !cleanupJob,
      'admin_quota_intent_replayed_or_changed');
    check(dispatch?.actor === 'user' && ['a', 'b'].includes(dispatch.am), 'admin_quota_intent_dispatch');
    entry.attempted = true; entry.unknown = true;
    await lease.retainIntent({...intent, kind: 'sandbox', ...dispatch});
    let captured = false;
    return {cleanup, async capture(response) {
      check(!captured, 'admin_quota_response_replayed'); captured = true;
      const body = response?.body;
      if (response?.status >= 200 && response.status < 300 && typeof body?.id === 'string') {
        id(body.id);
        check(!roots.has(body.id), 'admin_quota_duplicate_resource');
        entry.root = {kind: 'sandbox', id: body.id, actor: 'user', name: intent.body.name, key: intent.key};
        // In-process custody precedes the parent write: its failure must not
        // orphan an accepted resource or make a later capture retry it.
        roots.set(body.id, entry);
        await lease.retainResource(entry.root);
        check(response.status === 201 && exact(body, entry), 'admin_quota_create_receipt_identity');
        entry.unknown = false;
      } else if (response?.status === 429 && body?.code === 'sandbox_limit_reached'
        && response.timing?.am === dispatch.am && response.timing.request_id?.trim()) {
        await lease.retainRefusal({key: intent.key, actor: 'user', am: dispatch.am, response});
        entry.unknown = false;
      } else {
        throw Error('admin_quota_create_outcome_unknown');
      }
    }};
  }};
}
