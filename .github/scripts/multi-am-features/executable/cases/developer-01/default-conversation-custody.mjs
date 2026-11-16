import {randomUUID} from 'node:crypto';
import {check, required, equal, id, other, request} from './common.mjs';
import {poll} from './enforcement.mjs';

// Conversation creation deliberately omits all overrides. Its one request may
// allocate a default Sandbox, so retain intent before dispatch even without ACK.
export async function defaultConversationCustody(ctx, config, owner, defaults) {
  defaults = structuredClone(defaults);
  const lease = await config.reserve({purpose: 'admin01-default-conversation', caseId: ctx.row.case_id});
  let attempted = false, unresolved = false, rootId, agentId, cleanupJob;
  const creation = {actor: 'user', method: 'POST', path: '/api/v1/conversations', body: {}, key: randomUUID()};
  async function readRoot() {
    const current = (await request(ctx, 'GET', '/api/v1/sandboxes/' + id(rootId), undefined,
      {actor: 'user', am: other(ctx.am)})).body;
    check(current.id === rootId && current.owner === owner && equal(current.profile, defaults.sandbox_profile),
      'admin_default_conversation_cleanup_identity');
    return current;
  }
  const cleanup = () => cleanupJob ??= (async () => {
    const errors = [];
    if (rootId) {
      try {
        const current = await readRoot();
        if (!['retiring', 'retired'].includes(current.status)) {
          try {await request(ctx, 'DELETE', '/api/v1/sandboxes/' + id(rootId), undefined,
            {actor: 'user', am: other(ctx.am), key: creation.key + '-retire', allowed: [202]});}
          catch (error) {errors.push(error);}
        }
        await poll(readRoot, value => value.status === 'retired', 'admin_default_conversation_retirement_deadline',
          config.timeout_ms ?? 120000);
        const final = await ctx.observe('admin_default_conversation_native_finality', () => config.finality({sandboxId: rootId}));
        check(final?.sandbox_id === rootId && final.children_remaining === 0 && final.pending_operations === 0
          && final.cleanup_complete === true, 'admin_default_conversation_native_unresolved');
      } catch (error) {errors.push(error);}
    }
    if (unresolved) errors.push(Error('admin_default_conversation_create_unknown'));
    if (errors.length) throw new AggregateError(errors, 'admin_default_conversation_cleanup_failed');
    const released = await lease.release();
    check(released?.released === true, 'admin_default_conversation_lease_release');
  })();
  ctx.defer('admin_default_conversation_partial_cleanup', cleanup);
  required(lease?.parent_cleanup_barrier === true
    && ['retainIntent', 'retainResource', 'release'].every(name => typeof lease[name] === 'function'),
  'admin_default_conversation_parent_custody_required');
  return {creation: structuredClone(creation), cleanup,
    creationStatePath: body => {
      check(body.object === 'conversation' && body.id === agentId, 'admin_default_conversation_read_identity');
      return '/api/v1/agents/' + id(agentId);
    },
    async custody(intent, dispatch) {
      check(!attempted && !cleanupJob && equal(intent, creation), 'admin_default_conversation_intent_replayed');
      check(dispatch?.actor === 'user' && ['a', 'b'].includes(dispatch.am), 'admin_default_conversation_dispatch');
      attempted = true;
      await lease.retainIntent({...creation, ...dispatch, kind: 'conversation', defaults});
      unresolved = true;
      let captured = false;
      return {cleanup, async capture(response) {
        check(!captured, 'admin_default_conversation_receipt_replayed'); captured = true;
        const body = response?.body;
        if (typeof body?.metadata?.sandbox === 'string') rootId = id(body.metadata.sandbox);
        if (typeof body?.id === 'string') agentId = id(body.id);
        // Keep local cleanup custody even when retaining the response fails.
        check(response.status === 200 && body.object === 'conversation' && rootId && agentId,
          'admin_default_conversation_receipt');
        await lease.retainResource({kind: 'sandbox', id: rootId, actor: 'user', key: creation.key,
          profile: defaults.sandbox_profile, agentId});
        check(body.metadata.agent_profile === defaults.agent_profile.id && body.metadata.workspace,
          'admin_default_conversation_metadata');
        unresolved = false;
      }};
    }};
}
