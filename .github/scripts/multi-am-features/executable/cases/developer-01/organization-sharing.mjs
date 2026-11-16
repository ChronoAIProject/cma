import {define, setup, required, roleRequest, browser, evidence, id, equal, other, check} from './common.mjs';
import {access, capabilities, agentRequest} from './access-common.mjs';

export const orgSharing = define(92, 'agent', 3, async ctx => {
  const f = await setup(ctx, 'organization_sharing');
  required(f.targetOrgId && f.keys && f.orgAgentId && typeof f.faults?.withholdNextSuccessfulResponse === 'function',
    'admin_org_sharing_fixture_missing');
  const path = '/api/v1/resource-shares/agent/' + id(f.agentId);
  let created, updated;
  await ctx.step('step_1', async () => {
    const before = (await roleRequest(ctx, f, 'creator', 'GET', path, undefined, {agentId: f.agentId})).body;
    required(!before.items.some(s => s.target_org_id === f.targetOrgId), 'admin_unique_share_target_required');
    // Parent fixture cleanup reconciles the original key if the create response is unknown.
    const custody = await f.shareCustody({path, key: f.keys.create, targetOrgId: f.targetOrgId});
    ctx.defer('admin_share_cleanup', () => custody.cleanup());
    const receipt = (await roleRequest(ctx, f, 'creator', 'POST', path,
      {target_org_id: f.targetOrgId, mode: 'view'}, {key: f.keys.create, agentId: f.agentId})).body;
    created = receipt.share;
    ctx.assert(created.target_org_id === f.targetOrgId && created.mode === 'view', 'admin_share_create_identity');
    const next = (await roleRequest(ctx, f, 'creator', 'PATCH', path + '/' + id(created.id),
      {expected_revision: created.revision, mode: 'org_roles'},
      {key: f.keys.update, agentId: f.agentId, am: other(ctx.am)})).body;
    updated = next.share;
    ctx.assert(updated.id === created.id && updated.revision > created.revision && updated.mode === 'org_roles',
      'admin_share_update_cas');
  });
  await ctx.step('step_2', async () => {
    const org = {...f, agentId: f.orgAgentId};
    capabilities(ctx, await access(ctx, org, 'org_admin'), ['manage_sharing']);
    const orgPath = '/api/v1/resource-shares/agent/' + id(f.orgAgentId);
    for (const method of ['GET', 'POST']) {
      const result = await roleRequest(ctx, org, 'org_admin', method, orgPath,
        method === 'POST' ? {target_org_id: f.targetOrgId, mode: 'view'} : undefined,
        {agentId: org.agentId, key: f.keys.denied + method, allowed: [403]});
      ctx.assert(result.status === 403, 'admin_org_label_granted_personal_sharing');
    }
  });
  await ctx.step('step_3', async () => {
    await roleRequest(ctx, f, 'creator', 'PATCH', path + '/' + id(created.id),
      {expected_revision: created.revision, mode: 'view'}, {agentId: f.agentId, key: f.keys.stale, allowed: [409]});
    ctx.assert(equal((await roleRequest(ctx, f, 'creator', 'GET', path + '/' + id(created.id),
      undefined, {agentId: f.agentId})).body, updated), 'admin_stale_share_changed_value');
    await f.revokeRecipientMembership();
    await agentRequest(ctx, f, 'org_member', 'GET', '', undefined, {allowed: [403, 404]});
    const target = path + '/' + id(created.id) + '?expected_revision=' + updated.revision;
    const fault = await f.faults.withholdNextSuccessfulResponse({method: 'DELETE', path: target.split('?')[0]});
    ctx.defer('admin_share_fault_cleanup', () => fault.cleanup());
    let unknown = false;
    try {
      await roleRequest(ctx, f, 'creator', 'DELETE', target, undefined, {key: f.keys.revoke, agentId: f.agentId});
    } catch (error) {
      const receipt = await fault.receipt();
      if (receipt.forwarded_writes !== 1 || !receipt.response_withheld) throw error;
      unknown = true;
    }
    ctx.assert(unknown, 'admin_share_unknown_outcome_not_exercised');
    const current = (await roleRequest(ctx, f, 'creator', 'GET', path + '/' + id(created.id), undefined,
      {agentId: f.agentId, am: other(ctx.am)})).body;
    ctx.assert(current.revoked_at !== null && current.revision > updated.revision, 'admin_share_revoke_not_durable');
    const receipt = await ctx.observe('admin_share_unknown_receipt', () => fault.receipt());
    ctx.assert(receipt.forwarded_writes === 1 && receipt.following_requests?.[0]?.method === 'GET',
      'admin_share_unknown_replayed');
  });
  await ctx.step('expected_result', async () => {
    const remaining = (await roleRequest(ctx, f, 'creator', 'GET', path, undefined, {agentId: f.agentId})).body;
    ctx.assert(!remaining.items.some(s => s.id === created.id), 'admin_revoked_share_still_active');
  });
});

export const linkServiceBoundary = define(93, 'agent', 3, async ctx => {
  const f = await setup(ctx, 'link_service_boundary');
  required(f.approveRequestId && f.denyRequestId && f.services && f.keys && f.shareOpenLabel
    && typeof f.readCredentialScope === 'function', 'admin_link_service_fixture_missing');
  let ownerSession, linkId, approvedScope;
  await ctx.step('step_1', async () => {
    ownerSession = await browser(ctx, 'creator', ctx.am, f.agentId);
    const p = ownerSession.page;
    await p.goto(new URL('/agents/' + id(f.agentId), ownerSession.origin).href);
    await p.getByRole('button', {name: f.shareOpenLabel, exact: true}).click();
    const dialog = p.getByRole('dialog', {name: 'Share Agent session', exact: true}); await dialog.waitFor();
    // The compiled UI generates/keeps the share token; this test never records it.
    const created = p.waitForResponse(r => new URL(r.url()).pathname.endsWith('/share-links') && r.request().method() === 'POST');
    await dialog.getByRole('button', {name: 'Create link', exact: true}).click();
    const response = await created, receipt = await response.json(); linkId = receipt.id;
    ctx.assert(response.status() === 200 && receipt.agent_id === f.agentId, 'admin_link_create_identity');
    const link = await dialog.getByLabel('Share link 1 URL', {exact: true}).inputValue();
    const recipient = await browser(ctx, 'share_recipient', other(ctx.am), f.agentId);
    const target = new URL(link); target.host = new URL(recipient.origin).host;
    await recipient.page.goto(target.href);
    await recipient.page.getByText(f.transcriptMarker, {exact: false}).waitFor();
    capabilities(ctx, await access(ctx, f, 'share_recipient'), ['view_details'], ['manage_sharing']);
    await dialog.getByRole('button', {name: 'Revoke link', exact: true}).click();
    await dialog.getByRole('button', {name: 'Revoke access', exact: true}).click();
    await dialog.getByText('No active links.', {exact: true}).waitFor();
    await agentRequest(ctx, f, 'share_recipient', 'GET', '', undefined, {allowed: [403, 404]});
    for (const [action, requestId, body] of [['approve', f.approveRequestId, {services: f.services}],
      ['deny', f.denyRequestId, {}]]) {
      const response = await agentRequest(ctx, f, 'creator', 'POST',
        '/access-requests/' + id(requestId) + '/' + action, body, {key: f.keys[action], allowed: [200, 202]});
      ctx.assert(response.body.id === requestId && response.body.agent_id === f.agentId,
        'admin_service_request_identity');
      if (action === 'deny') ctx.assert(response.body.status === 'denied', 'admin_service_request_not_denied');
    }
    approvedScope = await f.readCredentialScope(ctx);
    await evidence(ctx, recipient);
  });
  await ctx.step('step_2', async () => {
    for (const role of ['control_operator', 'org_admin']) {
      for (const suffix of ['/share-links', '/access-requests']) {
        await agentRequest(ctx, f, role, 'GET', suffix, undefined, {allowed: [403]});
      }
      await agentRequest(ctx, f, role, 'DELETE', '/share-links/' + id(linkId) + '?expected_revision=1',
        undefined, {key: f.keys.denied + role + '-link', allowed: [403]});
      for (const [action, requestId, body] of [['approve', f.denyRequestId, {services: f.services}],
        ['deny', f.approveRequestId, {}]]) {
        await agentRequest(ctx, f, role, 'POST', '/access-requests/' + id(requestId) + '/' + action, body,
          {key: f.keys.denied + role + action, allowed: [403]});
      }
    }
    ctx.assert(true, 'admin_nonowner_service_management_refused');
  });
  await ctx.step('step_3', async () => {
    const current = await ctx.observe('admin_credential_scope_after_refusals', () => f.readCredentialScope(ctx));
    ctx.assert(equal(current, approvedScope), 'admin_operator_changed_credential_scope');
    const requests = (await agentRequest(ctx, f, 'creator', 'GET', '/access-requests', undefined,
      {am: other(ctx.am)})).body;
    ctx.assert(requests.items.some(r => r.id === f.denyRequestId && r.status === 'denied'),
      'admin_denied_service_request_changed');
  });
  await ctx.step('expected_result', async () => { await evidence(ctx, ownerSession); });
}, ['admin-fixtures', 'browser']);
