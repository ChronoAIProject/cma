import {setTimeout as delay} from 'node:timers/promises';
import {caseOf, check, equal, key, otherAM, readOtherAM, request, segment, take} from './common.mjs';

async function ready(ctx, path, status, agentId) {
  for (let count = 0; count < 90; count++) {
    const value = (await request(ctx, 'GET', path, undefined, {agentId})).body;
    ctx.assert(!['failed','deleted','removed','retired'].includes(value.status), 'hierarchy_lifecycle_not_failed');
    if (value.status === status) return value;
    await delay(1000);
  }
  throw Error('hierarchy_ready_deadline');
}
export const hierarchyCases = [33,62,63,81,82].map(number => caseOf(number, 'agent', ['api-fixtures'], async ctx => {
  let fixture, defaults, sandbox, workspace, agent, worktree, baseline;
  const created = {}, intents = {}, receipts = {};
  async function create(kind, path, body) {
    const originalKey = key();
    intents[kind] = {method: 'POST', path, body: structuredClone(body), key: originalKey, allowed: [201]};
    let dispatched = false;
    ctx.defer('cleanup_created_' + kind, async () => {
      if (!dispatched) return;
      check(created[kind]?.id, 'hierarchy_creation_custody_unknown');
      await fixture.cleanupResource(ctx, kind, created[kind].id);
    });
    let response;
    try {
      dispatched = true;
      await fixture.retainIntent({kind, actor:'user', am:ctx.am, ...intents[kind]});
      response = await ctx.request({actor: 'user', am: ctx.am, ...intents[kind]});
      created[kind] = response.body;
      receipts[kind] = response;
      await fixture.retainResource({kind, id:created[kind]?.id, key:originalKey});
    } catch (error) {
      created[kind] = error.response?.body;
      if (created[kind]?.id) await fixture.retainResource({kind, id:created[kind].id, key:originalKey});
      throw error;
    }
    ctx.assert(typeof response.body.id === 'string' && response.body.id.length > 0, 'hierarchy_created_identity');
    return response.body;
  }
  async function replay(kind) {
    const response = await ctx.request({actor: 'user', am: otherAM(ctx.am), ...intents[kind]});
    ctx.assert(response.body.id === created[kind].id, 'hierarchy_original_intent_replayed');
    return response.body;
  }
  await ctx.step('step_1', async () => {
    fixture = await take(ctx, 'hierarchy');
    baseline = await fixture.readNativeFacts();
    defaults = (await request(ctx, 'GET', '/api/v1/defaults')).body;
    ctx.assert(equal(defaults, fixture.defaults), 'hierarchy_current_defaults');
    // Empty parent profile child lists and explicit quotas are setup requirements
    // so each request is the sole admission of its intended resource.
    sandbox = await create('sandbox', '/api/v1/sandboxes', fixture.sandboxRequest);
    ctx.assert(equal(sandbox.profile, fixture.sandboxRequest.sandbox_profile), 'sandbox_profile_pin');
    sandbox = await ready(ctx, '/api/v1/sandboxes/' + segment(sandbox.id), 'active');
    workspace = await create('workspace', '/api/v1/sandboxes/' + segment(sandbox.id) + '/workspaces', fixture.workspaceRequest);
    ctx.assert(workspace.sandbox_id === sandbox.id && equal(workspace.profile, fixture.workspaceRequest.workspace_profile), 'workspace_parent_pin');
    workspace = await ready(ctx, '/api/v1/workspaces/' + segment(workspace.id), 'ready');
    agent = await create('agent', '/api/v1/workspaces/' + segment(workspace.id) + '/agents', fixture.agentRequest);
    ctx.assert(agent.sandbox.id === sandbox.id && agent.workspace.id === workspace.id && typeof agent.worktree.id === 'string',
      'agent_created_parent_lineage');
    // Before first acquisition no owner exists. The first read waits on the
    // fixture's independently observed Ready ownership, then every Agent read
    // uses ctx.request with agentId and the shared non-owner router.
    await fixture.waitForOwner(agent.id);
    const active = await ready(ctx, '/api/v1/agents/' + segment(agent.id), 'ready', agent.id);
    ctx.assert(active.id === agent.id && active.sandbox.id === sandbox.id && active.workspace.id === workspace.id,
      'agent_ready_parent_lineage');
    worktree = (await request(ctx, 'GET', '/api/v1/worktrees/' + segment(agent.worktree.id), undefined,
      {agentId:agent.id})).body;
    ctx.assert(worktree.id === agent.worktree.id && worktree.sandbox_id === sandbox.id
      && worktree.workspace_id === workspace.id && worktree.status === 'ready', 'worktree_ready_lineage');
    for (const kind of ['sandbox','workspace','agent']) await replay(kind);
  });
  await ctx.step('step_2', async () => {
    if (number === 33) ctx.assert(equal((await request(ctx, 'GET', '/api/v1/defaults')).body, defaults), 'exact_defaults_read');
    else if (number === 82) {
      const reply = await request(ctx, 'GET', '/api/v1/worktrees/' + segment(worktree.id), undefined,
        {agentId:agent.id});
      ctx.assert(equal(reply.body, worktree), 'exact_worktree_read');
    } else await replay(({62:'sandbox',63:'workspace',81:'agent'})[number]);
  });
  await ctx.step('step_3', async () => {
    for (const [path, agentId] of [
      ['/api/v1/sandboxes/' + segment(sandbox.id), undefined],
      ['/api/v1/workspaces/' + segment(workspace.id), undefined],
      ['/api/v1/agents/' + segment(agent.id), agent.id],
      ['/api/v1/worktrees/' + segment(worktree.id), agent.id],
    ]) {
      const denied = await request(ctx, 'GET', path, undefined, {actor: 'outsider', am: otherAM(ctx.am), agentId, allowed: [403,404]});
      ctx.assert(typeof denied.body.code === 'string' && !('id' in denied.body), 'hierarchy_other_owner_denied');
    }
    const invalidParent = await request(ctx, 'POST', '/api/v1/workspaces/' + segment(workspace.id) + '/agents', fixture.agentRequest,
      {actor: 'outsider', key: key(), allowed: [403,404]});
    ctx.assert(typeof invalidParent.body.code === 'string', 'hierarchy_foreign_parent_admission_refused');
    const peer = await readOtherAM(ctx, fixture, {method: 'GET', path: '/api/v1/agents/' + segment(agent.id),
      agentId: agent.id, allowed: [200]}, receipts.agent);
    ctx.assert(peer.body.id === agent.id && peer.body.sandbox.id === sandbox.id
      && peer.body.workspace.id === workspace.id && peer.body.worktree.id === worktree.id, 'hierarchy_cross_am_readback');
  });
  await ctx.step('expected_result', async () => {
    const facts = await ctx.observe('hierarchy_native_admissions', () => fixture.readNativeFacts());
    for (const [kind, id] of [['sandbox',sandbox.id],['workspace',workspace.id],['agent',agent.id]]) {
      const added = facts[kind + '_ids'].filter(value => !baseline[kind + '_ids'].includes(value));
      ctx.assert(equal(added, [id]), 'hierarchy_one_' + kind);
    }
    ctx.assert(facts.agent_preparations === baseline.agent_preparations + 1, 'hierarchy_no_native_preparation_replay');
    ctx.assert(equal((await request(ctx, 'GET', '/api/v1/worktrees/' + segment(worktree.id), undefined,
      {agentId:agent.id, am: otherAM(ctx.am)})).body, worktree), 'hierarchy_preserved_worktree');
  });
}));
