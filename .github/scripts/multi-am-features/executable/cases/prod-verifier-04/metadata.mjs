import {caseOf, equal, hash, otherAM, readOtherAM, request, segment, take} from './common.mjs';

const numbers = [12,18,19,20,21,22,23];
function operation(number, fixture) {
  const root = '/api/v1/agents/' + segment(fixture.agentId);
  const paths = {
    12: '/artifacts/' + segment(fixture.artifact.id),
    18: '/deliverable-references',
    19: '/messages/' + segment(fixture.feedback.message_id) + '/feedback',
    20: '/provenance', 21: '/provider-associations', 22: '/source', 23: '/terminal',
  };
  return {method: number === 18 ? 'POST' : 'GET', path: root + paths[number],
    ...(number === 18 ? {body: {kind: 'agent_artifact', id: fixture.artifact.id}} : {}),
    allowed: number === 23 ? [501] : [200], agentId: fixture.agentId};
}
export function assertMetadata(ctx, number, body, fixture) {
  if (number === 12) {
    ctx.assert(body.agent_id === fixture.agentId && body.id === fixture.artifact.id
      && body.sha256 === hash(fixture.artifact.content) && body.sha256 === fixture.artifact.sha256,
    'artifact_identity_content_hash');
    ctx.assert(body.size_bytes === Buffer.byteLength(fixture.artifact.content) && body.name === fixture.artifact.name
      && body.media_type === fixture.artifact.media_type, 'artifact_content_metadata');
  } else if (number === 18) {
    ctx.assert(body.reference?.agent_id === fixture.agentId
      && equal(body.reference.resource, {kind: 'agent_artifact', id: fixture.artifact.id})
      && body.reference.sha256 === fixture.artifact.sha256
      && body.reference.size_bytes === Buffer.byteLength(fixture.artifact.content), 'deliverable_exact_lineage');
    ctx.assert(typeof body.marker === 'string' && body.marker.length > 0, 'deliverable_marker_present');
  } else if (number === 19) {
    ctx.assert(body.agent_id === fixture.agentId && body.viewer_id === fixture.owner
      && body.message_id === fixture.feedback.message_id, 'feedback_owner_message_join');
    for (const field of ['vote','version','reasons','comment','request_id']) {
      ctx.assert(equal(body[field], fixture.feedback[field]), 'feedback_' + field);
    }
  } else if (number === 20) {
    ctx.assert(body.agent_id === fixture.agentId && body.created_at === fixture.created_at
      && body.creation_origin === fixture.provenance.creation_origin
      && equal(body.details, fixture.provenance.details), 'provenance_creation_identity');
  } else if (number === 21) {
    ctx.assert(body.agent_id === fixture.agentId && body.owner === fixture.owner
      && body.agent_created_at === fixture.created_at && equal(body.items, fixture.associations)
      && body.next == null, 'provider_association_exact_lineage');
    ctx.assert(body.items.length > 0 && body.items.every(item => item.sandbox_id === fixture.sandboxId
      && item.workspace_id === fixture.workspaceId && item.worktree_id === fixture.worktreeId), 'provider_hierarchy_join');
  } else if (number === 22) {
    ctx.assert(body.agent_id === fixture.agentId && equal(body.origin, fixture.source.origin)
      && equal(body.trigger, fixture.source.trigger), 'source_original_lineage');
  } else ctx.assert(body.code === 'capability_unsupported' && body.capability === 'terminal'
    && !('ticket' in body) && !('url' in body), 'terminal_documented_refusal');
}
export const metadataCases = numbers.map(number => caseOf(number, 'agent', ['api-fixtures'], async ctx => {
  let fixture, baseline, first;
  await ctx.step('step_1', async () => {
    fixture = await take(ctx, 'agent-metadata');
    baseline = await ctx.observe('metadata_initial_facts', () => fixture.readNativeFacts());
    ctx.assert(baseline.agent_id === fixture.agentId, 'metadata_fixture_identity');
    for (const member of numbers) {
      const response = await ctx.request({actor: 'user', ...operation(member, fixture)});
      assertMetadata(ctx, member, response.body, fixture);
    }
    const worktree = await request(ctx, 'GET', '/api/v1/worktrees/' + segment(fixture.worktreeId));
    ctx.assert(worktree.body.id === fixture.worktreeId && worktree.body.workspace_id === fixture.workspaceId,
      'metadata_worktree_lineage');
  });
  await ctx.step('step_2', async () => {
    first = await ctx.request({actor: 'user', ...operation(number, fixture)});
    assertMetadata(ctx, number, first.body, fixture);
  });
  await ctx.step('step_3', async () => {
    for (const member of numbers) {
      const denied = await ctx.request({am: otherAM(ctx.am), ...operation(member, fixture),
        actor: 'outsider', allowed: [403,404]});
      ctx.assert(typeof denied.body.code === 'string' && !('agent_id' in denied.body)
        && !('reference' in denied.body), 'metadata_outsider_denied');
    }
    const peer = await readOtherAM(ctx, fixture, {actor: 'user', ...operation(number, fixture)}, first);
    assertMetadata(ctx, number, peer.body, fixture);
    ctx.assert(equal(peer.body, first.body), 'metadata_cross_am_durable_view');
  });
  await ctx.step('expected_result', async () => {
    const final = await ctx.observe('metadata_final_facts', () => fixture.readNativeFacts());
    ctx.assert(equal(final, baseline), 'metadata_reads_do_not_mutate_runtime');
    // deliverable-references describes an existing resource and returns its
    // deterministic marker; the current service does not admit a new turn.
    const finalReference = await ctx.request({actor: 'user', am: otherAM(ctx.am), ...operation(18, fixture)});
    assertMetadata(ctx, 18, finalReference.body, fixture);
  });
}));
