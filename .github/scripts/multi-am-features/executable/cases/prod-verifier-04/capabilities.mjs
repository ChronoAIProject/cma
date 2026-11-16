import {caseOf, equal, otherAM, request, take} from './common.mjs';

const paths = ['/api/v1/conformance', '/api/v2', '/api/v2/ui/task-experience', '/api/v2/ui/transport'];
export function assertCapability(ctx, path, body, fixture, actor = 'user') {
  const manifest = fixture.manifest[path];
  ctx.assert(manifest && equal(body, path.endsWith('task-experience')
    ? {...manifest, account_id: fixture.subjects[actor]} : manifest), 'capabilities_exact_image_manifest');
  if (path.endsWith('conformance')) ctx.assert(body.service === 'cma' && Array.isArray(body.interfaces), 'conformance_service');
  else if (path === '/api/v2') {
    ctx.assert(body.object === 'cma.responses.capabilities' && body.version === 2
      && body.create === '/api/v2/responses' && body.conversation === '/api/v2/conversations/{conversation_id}'
      && body.creation.stream === true && body.creation.store === true, 'v2_advertised_contract');
  } else if (path.endsWith('transport')) ctx.assert(body.object === 'cma.ui.transport'
    && body.input === 'responses_v2' && body.selection === 'new_tabs'
    && body.snapshot === '/api/v2/agents/{agent_id}/events/snapshot'
    && body.events === '/api/v2/agents/{agent_id}/events', 'ui_transport_route_contract');
  else ctx.assert(body.object === 'cma.ui.task_experience' && body.schema_version === 1
    && Object.values(body.capabilities).every(value => typeof value === 'boolean'), 'task_experience_identity_capabilities');
}
export const capabilityCases = [29,66,83,86,87].map(number => caseOf(number, 'global', ['api-fixtures'], async ctx => {
  let fixture, before, sent = 0;
  async function telemetry() {
    const reply = await fixture.withAnonymousRequests(() => request(ctx, 'POST', '/api/v1/telemetry/browser',
      {kind: 'react_error'}, {allowed: [204]}));
    ctx.assert(reply.status === 204 && (reply.body == null || reply.body === ''), 'telemetry_empty_receipt');
    sent++;
  }
  await ctx.step('step_1', async () => {
    fixture = await take(ctx, 'capabilities');
    before = await fixture.readTelemetry();
    ctx.assert(Array.isArray(before), 'telemetry_observation_available');
    for (const path of paths) assertCapability(ctx, path, (await request(ctx, 'GET', path)).body, fixture);
    await telemetry();
    for (const path of paths.slice(1)) {
      const denial = await fixture.withAnonymousRequests(() => request(ctx, 'GET', path, undefined, {allowed: [401]}));
      ctx.assert(denial.status === 401 && !('account_id' in (denial.body ?? {})), 'capabilities_authentication_required');
    }
  });
  await ctx.step('step_2', async () => {
    if (number === 66) await telemetry();
    else {
      const path = ctx.row.entry_point.split(' ')[1];
      assertCapability(ctx, path, (await request(ctx, 'GET', path)).body, fixture);
    }
  });
  await ctx.step('step_3', async () => {
    for (const path of paths) {
      assertCapability(ctx, path, (await request(ctx, 'GET', path, undefined, {am: otherAM(ctx.am)})).body, fixture);
      assertCapability(ctx, path, (await request(ctx, 'GET', path, undefined,
        {actor: 'outsider', am: otherAM(ctx.am)})).body, fixture, 'outsider');
    }
    const rejected = await fixture.withAnonymousRequests(() => request(ctx, 'POST', '/api/v1/telemetry/browser',
      {kind: 'react_error', stack: 'synthetic-extra-field'}, {allowed: [422]}));
    ctx.assert(rejected.status === 422, 'telemetry_rejects_content_fields');
  });
  await ctx.step('expected_result', async () => {
    const observed = await ctx.observe('telemetry_content_free_events', () => fixture.readTelemetry());
    const added = observed.slice(before.length);
    ctx.assert(equal(observed.slice(0, before.length), before) && added.length === sent, 'telemetry_exact_event_count');
    ctx.assert(added.every(event => equal(event, {kind: 'react_error'})), 'telemetry_no_identity_or_payload');
  });
}));
