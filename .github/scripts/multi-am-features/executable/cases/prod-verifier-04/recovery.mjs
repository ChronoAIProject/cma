import {setTimeout as delay} from 'node:timers/promises';
import {caseOf, equal, otherAM, readOtherAM, request, segment, take} from './common.mjs';

const route = fixture => '/api/v1/agents/' + segment(fixture.original.agent_id) + '/capacity-recovery';
const options = (ctx, fixture) => ({agentId: fixture.original.agent_id});
function assertStatus(ctx, body, fixture) {
  ctx.assert(body.agent_id === fixture.original.agent_id && typeof body.disposition === 'string'
    && Number.isSafeInteger(body.attempts) && body.attempts >= 0, 'recovery_status_identity');
}
function assertOutcome(ctx, response) {
  if (response.status === 202) ctx.assert(equal(response.body, {outcome: 'unknown'}), 'recovery_unknown_not_success');
  else ctx.assert(response.status === 200 && response.body.outcome === 'recorded'
    && typeof response.body.operation_key === 'string' && response.body.operation_key.length > 0,
  'recovery_recorded_receipt');
}
async function settle(ctx, fixture) {
  for (let attempt = 0; attempt < 90; attempt++) {
    const result = await request(ctx, 'POST', route(fixture) + '/status', fixture.original,
      {...options(ctx, fixture), am: otherAM(ctx.am)});
    assertStatus(ctx, result.body, fixture);
    if (result.body.phase === 'completed') return result.body;
    ctx.assert(result.body.phase !== 'failed' && result.body.phase !== 'aborted', 'recovery_not_failed');
    await delay(1000);
  }
  throw Error('recovery_completion_deadline');
}
export const recoveryCases = [13,14,15,16,17].map(number => caseOf(number, 'agent', ['api-fixtures'], async ctx => {
  let fixture, original, before, receipt, settled;
  await ctx.step('step_1', async () => {
    fixture = await take(ctx, 'capacity-recovery');
    original = structuredClone(fixture.original);
    ctx.assert(typeof original.agent_id === 'string' && typeof original.creation_key === 'string'
      && typeof original.creation_fingerprint === 'string' && Number.isSafeInteger(original.failure_seq)
      && Number.isFinite(Date.parse(original.requested_at)), 'recovery_original_identity_required');
    before = await ctx.observe('recovery_original_facts', () => fixture.readNativeFacts());
    ctx.assert(before.agent_id === original.agent_id && before.ever_ready === false
      && before.status === 'failed' && typeof before.data_digest === 'string', 'recovery_failed_never_ready');
    const dry = await request(ctx, 'GET', route(fixture) + '/dry-run', undefined, options(ctx, fixture));
    ctx.assert(dry.body.agent_id === original.agent_id && Number.isSafeInteger(dry.body.through_seq)
      && dry.body.through_seq >= original.failure_seq && typeof dry.body.disposition === 'string', 'recovery_preflight_lineage');
    const current = await request(ctx, 'GET', route(fixture), undefined, options(ctx, fixture));
    assertStatus(ctx, current.body, fixture);
    // Fault acts at the original admission commit boundary. A missing real
    // control blocks fixture setup; status-only mocks cannot establish this.
    await fixture.makeNextAdmissionOutcomeUnknown();
    const admitted = await request(ctx, 'POST', route(fixture) + '/admit', original,
      {...options(ctx, fixture), allowed: [202]});
    assertOutcome(ctx, admitted);
    const reconciliation = await request(ctx, 'POST', route(fixture) + '/status', original,
      {...options(ctx, fixture), am: otherAM(ctx.am)});
    assertStatus(ctx, reconciliation.body, fixture);
    receipt = await request(ctx, 'POST', route(fixture) + '/resume', original,
      {...options(ctx, fixture), allowed: [200,202]});
    assertOutcome(ctx, receipt);
    settled = await settle(ctx, fixture);
    ctx.assert(typeof settled.operation_key === 'string', 'recovery_settled_operation_key');
  });
  await ctx.step('step_2', async () => {
    const [method, template] = ctx.row.entry_point.split(' ');
    const path = template.replace('{id}', segment(original.agent_id));
    const reply = await request(ctx, method, path, method === 'POST' ? original : undefined,
      {...options(ctx, fixture), allowed: [200,202]});
    if ([14,16].includes(number)) {
      assertOutcome(ctx, reply);
      const final = reply.status === 202 ? await settle(ctx, fixture) : reply.body;
      ctx.assert(final.operation_key === settled.operation_key, 'recovery_exact_intent_replayed');
    } else if (number === 15) ctx.assert(reply.body.agent_id === original.agent_id
      && typeof reply.body.disposition === 'string', 'recovery_exact_dry_run');
    else { assertStatus(ctx, reply.body, fixture); ctx.assert(reply.body.operation_key === settled.operation_key, 'recovery_exact_status'); }
  });
  await ctx.step('step_3', async () => {
    for (const field of ['creation_key','creation_fingerprint','failure_seq','requested_at']) {
      const changed = {...original, [field]: field === 'failure_seq' ? original.failure_seq + 1
        : field === 'requested_at' ? new Date(Date.parse(original.requested_at) + 1000).toISOString()
          : original[field].replace(/^./, original[field][0] === 'a' ? 'b' : 'a')};
      const refusal = await request(ctx, 'POST', route(fixture) + '/resume', changed,
        {...options(ctx, fixture), allowed: [400,409]});
      ctx.assert(['invalid_recovery_request','capacity_recovery_conflict'].includes(refusal.body.code), 'changed_recovery_identity_refused');
    }
    const denied = await request(ctx, 'POST', route(fixture) + '/status', original,
      {...options(ctx, fixture), actor: 'outsider', allowed: [404]});
    ctx.assert(denied.body.code === 'capacity_recovery_unavailable', 'recovery_current_owner_required');
    const peer = await readOtherAM(ctx, fixture, {method: 'POST', path: route(fixture) + '/status', body: original,
      agentId: original.agent_id, allowed: [200]}, receipt);
    ctx.assert(peer.body.operation_key === settled.operation_key && peer.body.phase === 'completed', 'recovery_cross_am_settlement');
  });
  await ctx.step('expected_result', async () => {
    const after = await ctx.observe('recovery_final_facts', () => fixture.readNativeFacts());
    ctx.assert(equal(fixture.original, original) && after.agent_id === original.agent_id
      && after.parent_id === before.parent_id && after.data_digest === before.data_digest, 'recovery_preserves_identity_data');
    ctx.assert(equal(after.agent_ids, before.agent_ids) && after.preparations === before.preparations
      && after.input_effects === before.input_effects && after.attachments === before.attachments + 1,
    'recovery_one_attachment_no_native_replay');
    ctx.assert(after.status === 'ready', 'recovery_original_ready');
  });
}));
