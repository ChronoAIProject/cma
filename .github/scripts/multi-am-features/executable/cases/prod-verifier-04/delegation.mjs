import {caseOf, check, cleanupRequest, equal, key, otherAM, request, segment, take} from './common.mjs';

const variants = ['wrong_audience','wrong_scope','wrong_resource','revoked','expired','provider_unavailable'];
export function assertNoCredentialFields(ctx, body) {
  function inspect(value) {
    if (!value || typeof value !== 'object') return;
    for (const [name, child] of Object.entries(value)) {
      ctx.assert(!/(^|_)(access_token|refresh_token|password|credential|authorization|api_key|binding_key|secret)($|_)/i.test(name),
        'delegation_no_credential_fields');
      inspect(child);
    }
  }
  inspect(body);
}
function assertIdentity(ctx, body, fixture) {
  ctx.assert(body.subject === fixture.subject && body.grant_id === fixture.grantId && body.login_url === null,
    'delegated_identity_current_grant');
  ctx.assert(equal(Object.keys(body).sort(), ['display_name','grant_id','login_url','subject']), 'delegated_identity_safe_projection');
  assertNoCredentialFields(ctx, body);
}
function assertHandoff(ctx, body, fixture, id, state) {
  ctx.assert(body.handoff_id === id && body.launch_id === fixture.connectionRequest.launch_id
    && body.service_id === fixture.connectionRequest.service_id && body.state === state, 'handoff_exact_identity_state');
  assertNoCredentialFields(ctx, body);
}
export const delegationCases = [1,2,64,65].map(number => caseOf(number, 'global', ['api-fixtures'], async ctx => {
  let fixture, baseline, identity, operationBody, delegated, handoff, handoffKey;
  let profile, operationDispatched = false;
  async function bound(path, body, variant = 'valid', am = ctx.am, allowed = [200]) {
    return fixture.withBindingVariant(variant, () => request(ctx, 'POST', path, body, {am, allowed}));
  }
  await ctx.step('step_1', async () => {
    fixture = await take(ctx, 'delegation');
    baseline = await fixture.readNativeFacts();
    identity = await bound('/api/v1/agent-delegations/identity', {event_ref: fixture.eventRef});
    assertIdentity(ctx, identity.body, fixture);
    operationBody = {event_ref: fixture.eventRef, method: 'POST', path: '/api/v1/agent-profiles',
      body: fixture.profileRequest, idempotency_key: key()};
    const retireKey = key();
    ctx.defer('retire_delegated_profile', async () => {
      if (!operationDispatched) return;
      check(profile?.id, 'delegated_profile_custody_unknown');
      const retired = await cleanupRequest(ctx, 'POST', '/api/v1/agent-profiles/' + segment(profile.id) + '/retire',
        undefined, {key: retireKey});
      check(retired.id === profile.id && typeof retired.retired_at === 'string', 'delegated_profile_cleanup');
    });
    try {
      operationDispatched = true;
      delegated = await bound('/api/v1/agent-delegations/operations', operationBody);
      profile = delegated.body.body;
    } catch (error) { profile = error.response?.body?.body; throw error; }
    ctx.assert(delegated.body.status === 201 && profile.owner === fixture.subject
      && equal(profile.draft, fixture.profileRequest.content), 'delegated_operation_inner_admission');
    assertNoCredentialFields(ctx, delegated.body);
    handoffKey = key();
    handoff = (await request(ctx, 'POST', '/api/v1/service-connections', fixture.connectionRequest,
      {key: handoffKey, allowed: [201]})).body;
    ctx.assert(typeof handoff.handoff_id === 'string' && new URL(handoff.hosted_url).origin === fixture.hostedOrigin
      && Number.isFinite(Date.parse(handoff.expires_at)), 'handoff_hosted_safe_metadata');
    assertNoCredentialFields(ctx, handoff);
    const pending = await request(ctx, 'GET', '/api/v1/service-connections/' + segment(handoff.handoff_id));
    assertHandoff(ctx, pending.body, fixture, handoff.handoff_id, 'pending');
    await fixture.setBrokerHandoffState(handoff.handoff_id, 'satisfied');
    const completed = await request(ctx, 'GET', '/api/v1/service-connections/' + segment(handoff.handoff_id),
      undefined, {am: otherAM(ctx.am)});
    assertHandoff(ctx, completed.body, fixture, handoff.handoff_id, 'satisfied');
    for (const variant of variants) {
      const before = await fixture.readNativeFacts();
      const statuses = variant === 'provider_unavailable' ? [503] : [200,400,401,403,404,409];
      const denied = await bound('/api/v1/agent-delegations/operations', {...operationBody, idempotency_key: key()}, variant,
        otherAM(ctx.am), statuses);
      ctx.assert(variant === 'provider_unavailable' ? denied.status === 503
        : denied.status === 200 ? [401,403,404].includes(denied.body.status) : denied.status >= 400,
      'delegation_variant_refused');
      assertNoCredentialFields(ctx, denied.body);
      ctx.assert(equal(await fixture.readNativeFacts(), before), 'denied_delegation_no_effect');
    }
  });
  await ctx.step('step_2', async () => {
    if (number === 1) assertIdentity(ctx, (await bound('/api/v1/agent-delegations/identity', {event_ref: fixture.eventRef})).body, fixture);
    else if (number === 2) {
      const replay = await bound('/api/v1/agent-delegations/operations', operationBody, 'valid', otherAM(ctx.am));
      ctx.assert(equal(replay.body, delegated.body), 'delegated_exact_intent_replay');
    } else if (number === 64) {
      const replay = await request(ctx, 'POST', '/api/v1/service-connections', fixture.connectionRequest,
        {key: handoffKey, allowed: [201], am: otherAM(ctx.am)});
      ctx.assert(equal(replay.body, handoff), 'handoff_exact_intent_replay');
    } else assertHandoff(ctx, (await request(ctx, 'GET', '/api/v1/service-connections/' + segment(handoff.handoff_id))).body,
      fixture, handoff.handoff_id, 'satisfied');
  });
  await ctx.step('step_3', async () => {
    const readback = await request(ctx, 'GET', '/api/v1/agent-profiles/' + segment(profile.id), undefined, {am: otherAM(ctx.am)});
    ctx.assert(readback.body.id === profile.id && equal(readback.body.draft, profile.draft), 'delegated_cross_am_profile_readback');
    const denied = await request(ctx, 'GET', '/api/v1/service-connections/' + segment(handoff.handoff_id), undefined,
      {actor: 'outsider', am: otherAM(ctx.am), allowed: [404]});
    ctx.assert(denied.body.code === 'service_connection_not_found', 'handoff_other_owner_hidden');
    const foreign = await request(ctx, 'POST', '/api/v1/service-connections', fixture.connectionRequest,
      {actor: 'outsider', key: key(), allowed: [403,404]});
    ctx.assert(typeof foreign.body.code === 'string' && !('hosted_url' in foreign.body), 'handoff_foreign_launch_refused');
    const invalid = await bound('/api/v1/agent-delegations/operations', {...operationBody, path: '/api/v1/admin/settings'},
      'valid', ctx.am, [400]);
    ctx.assert(typeof invalid.body.code === 'string', 'delegation_admin_path_refused');
  });
  await ctx.step('expected_result', async () => {
    const facts = await ctx.observe('delegation_final_effect_counts', () => fixture.readNativeFacts());
    ctx.assert(facts.profile_creates === baseline.profile_creates + 1 && facts.handoff_creates === baseline.handoff_creates + 1,
      'delegation_one_profile_one_handoff');
    ctx.assert(equal(facts.credential_scopes, baseline.credential_scopes), 'delegation_no_credential_scope_escalation');
    const final = await request(ctx, 'GET', '/api/v1/service-connections/' + segment(handoff.handoff_id),
      undefined, {am: otherAM(ctx.am)});
    assertHandoff(ctx, final.body, fixture, handoff.handoff_id, 'satisfied');
  });
}));
