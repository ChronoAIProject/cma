import {define, setup, required, read, roleRequest, browser, evidence, id, equal, other} from './common.mjs';
import {agentRequest, access, capabilities, transcript, digest, markerCount, interaction, revokedSend} from './access-common.mjs';

export const crossOwnerReads = define(80, 'agent', 3, async ctx => {
  const f = await setup(ctx, 'cross_owner_reads');
  required(f.agentId && f.content && f.filePath && f.transcriptMarker && f.ownerSubject !== f.operatorSubject,
    'admin_cross_owner_read_fixture_missing');
  await ctx.step('step_1', async () => {
    const current = await access(ctx, f, 'read_operator');
    capabilities(ctx, current, ['view_details', 'view_history', 'view_files'], ['interact', 'operate_agent']);
    ctx.assert(current.owner?.kind === 'person' && current.owner.is_current_person === false,
      'admin_cross_owner_identity');
  });
  await ctx.step('step_2', async () => {
    for (const am of ['a', 'b']) {
      const detail = (await agentRequest(ctx, f, 'read_operator', 'GET', '', undefined, {am})).body;
      ctx.assert(detail.id === f.agentId, 'admin_cross_owner_agent_identity');
      ctx.assert(markerCount(await transcript(ctx, f, 'read_operator'), f.transcriptMarker) === 1,
        'admin_cross_owner_transcript_marker');
      const tree = (await agentRequest(ctx, f, 'read_operator', 'GET', '/files/tree', undefined, {am})).body;
      ctx.assert(tree.entries.some(row => row.path === f.filePath), 'admin_cross_owner_file_missing');
      const query = '?' + new URLSearchParams({path: f.filePath});
      const file = (await agentRequest(ctx, f, 'read_operator', 'GET', '/files/content' + query, undefined, {am})).body;
      ctx.assert(file.sha256 === digest(f.content)
        && Buffer.from(file.content_base64, 'base64').toString() === f.content, 'admin_cross_owner_file_bytes');
    }
    const s = await browser(ctx, 'read_operator', ctx.am, f.agentId);
    await s.page.goto(new URL(f.fileUiPath, s.origin).href);
    const download = s.page.waitForEvent('download');
    await s.page.getByRole('button', {name: f.downloadLabel, exact: true}).click();
    const value = await download, stream = await value.createReadStream();
    const chunks = []; let size = 0;
    for await (const chunk of stream) {size += chunk.length; ctx.assert(size <= 1048576, 'admin_download_bound'); chunks.push(chunk);}
    ctx.assert(digest(Buffer.concat(chunks)) === digest(f.content), 'admin_cross_owner_download_bytes');
    await evidence(ctx, s);
  });
  await ctx.step('step_3', async () => {
    capabilities(ctx, await access(ctx, f, 'read_operator'), ['view_details'], ['interact', 'resolve_approval']);
    await agentRequest(ctx, f, 'read_operator', 'POST', '/input', f.deniedInput, {key: f.deniedKey, allowed: [403]});
    await agentRequest(ctx, f, 'read_operator', 'POST', '/interrupt', undefined, {allowed: [403]});
    const s = await browser(ctx, 'read_operator', other(ctx.am), f.agentId);
    await s.page.goto(new URL('/agents/' + id(f.agentId), s.origin).href);
    await s.page.getByText(f.transcriptMarker, {exact: false}).waitFor();
    const send = s.page.getByRole('button', {name: 'Send message', exact: true});
    ctx.assert(!(await send.isVisible()) || await send.isDisabled(), 'admin_read_operator_ui_interaction_enabled');
    await evidence(ctx, s);
  });
  await ctx.step('expected_result', async () => {
    await agentRequest(ctx, f, 'outsider', 'GET', '', undefined, {allowed: [403, 404]});
    const history = await transcript(ctx, f, 'creator');
    ctx.assert(markerCount(history, f.deniedMarker) === 0, 'admin_read_operator_created_effect');
  });
}, ['admin-fixtures', 'browser']);

export const crossOwnerInteraction = define(81, 'agent', 3, async ctx => {
  const f = await setup(ctx, 'cross_owner_interaction');
  await ctx.step('step_1', async () => capabilities(ctx, await access(ctx, f, 'control_operator'),
    ['interact', 'resolve_approval']));
  await ctx.step('step_2', async () => interaction(ctx, f, 'control_operator'));
  await ctx.step('step_3', async () => revokedSend(ctx, f, 'control_operator', () => f.revokeOperator()));
  await ctx.step('expected_result', async () => {
    await agentRequest(ctx, f, 'control_operator', 'POST', '/input', f.inputRevoked,
      {key: f.keys.afterRevocation, allowed: [403, 404]});
    ctx.assert(markerCount(await transcript(ctx, f, 'creator'), f.markers.revoked) === 0,
      'admin_revoked_output_published');
  });
});

export const recovery = define(84, 'agent', 3, async ctx => {
  const f = await setup(ctx, 'operator_recovery');
  required(f.agentId && f.workspaceId && f.homeMarkerHash && f.worktreeKey && f.workspaceKey
    && typeof f.readHomeMarker === 'function', 'admin_recovery_fixture_missing');
  let before;
  await ctx.step('step_1', async () => {
    before = (await agentRequest(ctx, f, 'creator', 'GET')).body;
    ctx.assert(before.status === 'failed' && before.id === f.agentId, 'admin_failed_agent_setup');
    const workspace = (await roleRequest(ctx, f, 'creator', 'GET', '/api/v1/workspaces/' + id(f.workspaceId),
      undefined, {agentId: f.agentId})).body;
    ctx.assert(workspace.status === 'failed', 'admin_failed_workspace_setup');
  });
  await ctx.step('step_2', async () => {
    const recovered = await agentRequest(ctx, f, 'control_operator', 'POST', '/recover-worktree', undefined,
      {key: f.worktreeKey, allowed: [202]});
    ctx.assert(recovered.body.id === f.agentId && recovered.body.sandbox.id === before.sandbox.id,
      'admin_recovery_replaced_identity');
    const workspace = await roleRequest(ctx, f, 'control_operator', 'POST',
      '/api/v1/workspaces/' + id(f.workspaceId) + '/recover-prepare', undefined,
      {key: f.workspaceKey, agentId: f.agentId, allowed: [202]});
    ctx.assert(workspace.body.id === f.workspaceId, 'admin_workspace_recovery_replaced_identity');
  });
  await ctx.step('step_3', async () => {
    const terminal = await ctx.observe('admin_recovery_final_state', () => f.waitRecovered(ctx));
    ctx.assert(terminal.agent_id === f.agentId && terminal.workspace_id === f.workspaceId
      && terminal.agent_status === 'ready' && terminal.workspace_status === 'ready', 'admin_recovery_not_ready');
    ctx.assert(await f.readHomeMarker(ctx) === f.homeMarkerHash, 'admin_recovery_lost_data');
    for (const am of ['a', 'b']) {
      await agentRequest(ctx, f, 'control_operator', 'POST', '/recover-worktree', undefined,
        {am, key: f.nonrecoverableKey + am, allowed: [409]});
      await roleRequest(ctx, f, 'control_operator', 'POST', '/api/v1/workspaces/' + id(f.workspaceId) + '/recover-prepare',
        undefined, {am, agentId: f.agentId, key: f.nonrecoverableKey + '-workspace-' + am, allowed: [409]});
    }
  });
  await ctx.step('expected_result', async () => {
    await agentRequest(ctx, f, 'read_operator', 'POST', '/recover-worktree', undefined,
      {key: f.deniedKey, allowed: [403]});
    ctx.assert(await f.readHomeMarker(ctx) === f.homeMarkerHash, 'admin_denied_recovery_changed_home');
  });
});

export const noCustodyEscalation = define(85, 'agent', 3, async ctx => {
  const f = await setup(ctx, 'operator_custody');
  required(f.deniedOperations?.length >= 4 && typeof f.readPublicState === 'function',
    'admin_custody_boundary_fixture_missing');
  let before;
  await ctx.step('step_1', async () => {
    for (const role of ['read_operator', 'control_operator']) {
      capabilities(ctx, await access(ctx, f, role), ['view_details'], ['manage_sharing', 'manage_bridge']);
    }
    before = await ctx.observe('admin_custody_before', () => f.readPublicState(ctx));
  });
  await ctx.step('step_2', async () => {
    required(['sharing', 'service', 'bridge', 'profile'].every(kind => f.deniedOperations.some(r => r.kind === kind)),
      'admin_custody_boundary_omitted');
    for (const role of ['read_operator', 'control_operator']) for (const op of f.deniedOperations) {
      const response = await roleRequest(ctx, f, role, op.method, op.path, op.body,
        {key: op.key + '-' + role, agentId: f.agentId, allowed: [403, 404]});
      ctx.assert(response.status === 403 || response.status === 404, 'admin_operator_custody_admitted');
    }
  });
  await ctx.step('step_3', async () => {
    capabilities(ctx, await access(ctx, f, 'creator'), ['manage_sharing', 'manage_bridge']);
    const after = await ctx.observe('admin_custody_after', () => f.readPublicState(ctx));
    ctx.assert(equal(before, after), 'admin_operator_custody_denial_had_effect');
  });
  await ctx.step('expected_result', async () => {
    const ownerRead = await roleRequest(ctx, f, 'creator', 'GET', '/api/v1/resource-shares/agent/' + id(f.agentId),
      undefined, {agentId: f.agentId});
    ctx.assert(Array.isArray(ownerRead.body.items), 'admin_owner_sharing_boundary_control');
  });
});
