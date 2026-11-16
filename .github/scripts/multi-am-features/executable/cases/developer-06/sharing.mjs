import {
  define,
  setup,
  open,
  api,
  webRequest,
  button,
  pick,
  visible,
  contains,
  absent,
  capture,
  enc,
  hash,
} from './helpers.mjs';
const ap = (id) => `/api/v1/agents/${enc(id)}`;
async function agent(ctx, scenario, actor = 'user') {
  const { page, origin, f } = await setup(ctx, scenario, actor);
  await page.goto(origin + '/agents/' + enc(f.agentId));
  await contains(ctx, page.locator('main'), f.historyMarker, 'shared_agent_history');
  return f;
}
async function links(ctx, b = ctx.state.b) {
  await button(b.page, 'Show Agent details').click();
  await button(b.page, 'Manage share links').click();
  await visible(ctx, b.page.getByRole('dialog', { name: 'Share Agent session' }), 'share_dialog');
}
async function revoke(ctx) {
  const f = ctx.state.f;
  const owner = await open(ctx, 'user', f.agentId);
  await owner.page.goto(owner.origin + '/agents/' + enc(f.agentId));
  await links(ctx, owner);
  const row = owner.page.getByRole('article', { name: 'Share link 1', exact: true });
  await button(row, 'Revoke link').click();
  const r = await capture(owner.page, 'DELETE', ap(f.agentId) + '/share-links/' + enc(f.shareId), () =>
    button(row, 'Revoke access').click(),
  );
  ctx.assert(r.status === 200 && r.body.revoked_at && r.body.id === f.shareId, 'exact_link_revoked');
  return r;
}
async function sharePage(ctx) {
  const { page, origin, f } = await setup(ctx, 'share-grants');
  await page.goto(origin + '/sandboxes/' + enc(f.sandboxId));
  await button(page, 'More actions').click();
  await page.getByRole('menuitem', { name: 'Sharing', exact: true }).click();
  await contains(
    ctx,
    page.getByRole('dialog', { name: 'Sandbox sharing' }),
    'whole Sandbox',
    'sandbox_share_scope',
  );
  return f;
}
export const cases = [
  define('sharing.agent-link', [
    async (ctx) => {
      const f = await agent(ctx, 'share-links');
      ctx.state.variants = [];
      ctx.assert(
        f.linkVariants.length === 6 &&
          new Set(f.linkVariants.map((v) => v.audience + ':' + v.mode)).size === 6,
        'six_audience_permission_variants',
      );
      for (let i = 0; i < f.linkVariants.length; i++) {
        const v = f.linkVariants[i];
        const b = i === 0 ? ctx.state.b : await open(ctx, 'user', v.agentId);
        if (i) await b.page.goto(b.origin + '/agents/' + enc(v.agentId));
        await links(ctx, b);
        await pick(
          b.page,
          'Share audience',
          { public: 'Anyone with the link', organization: 'Organization members', users: 'Specific people' }[
            v.audience
          ],
        );
        if (v.audience === 'organization') await pick(b.page, 'Share organization', f.orgName);
        if (v.audience === 'users') {
          await b.page.getByLabel('Find share recipients', { exact: true }).fill(v.recipientName);
          await button(
            b.page
              .getByRole('list', { name: 'Matching people' })
              .getByRole('listitem')
              .filter({ hasText: v.recipientName }),
            'Add',
          ).click();
        }
        ctx.state.variants.push({ ...v, b });
        ctx.assert(await button(b.page, 'Share audience').isVisible(), 'audience_selected');
      }
    },
    async (ctx) => {
      for (const v of ctx.state.variants) {
        await pick(v.b.page, 'Share permission', v.mode === 'view' ? 'View only' : 'View and write');
        await contains(
          ctx,
          button(v.b.page, 'Share permission'),
          v.mode === 'view' ? 'View only' : 'View and write',
          'permission_selected',
        );
      }
    },
    async (ctx) => {
      for (const v of ctx.state.variants) {
        const r = await capture(v.b.page, 'POST', ap(v.agentId) + '/share-links', () =>
          button(v.b.page, 'Create link').click(),
        );
        ctx.assert(
          [200, 201].includes(r.status) &&
            r.body.agent_id === v.agentId &&
            r.body.mode === v.mode &&
            r.body.audience.kind === v.audience,
          'created_share_exact_scope',
        );
        v.share = r.body;
        await ctx.state.f.registerShare(v.agentId, r.body.id);
        await button(v.b.page, 'Copy link').click();
        v.url = await v.b.page.evaluate(() => navigator.clipboard.readText());
        ctx.assert(
          new URL(v.url).pathname === `/agent-manager/share/${r.body.id}` && new URL(v.url).hash.length > 1,
          'private_link_identity',
        );
      }
    },
    async (ctx) => {
      for (const v of ctx.state.variants) {
        const b = await open(ctx, v.recipientActor, v.agentId);
        await b.page.goto(v.url);
        await contains(ctx, b.page.locator('main'), v.historyMarker, 'intended_recipient_history');
        v.recipient = b;
        if (v.mode === 'view') await absent(ctx, button(b.page, 'Send message'), 'view_only_no_send');
        else
          await visible(
            ctx,
            b.page.getByRole('textbox', { name: 'Message', exact: true }),
            'writer_composer',
          );
      }
    },
    async (ctx) => {
      for (const v of ctx.state.variants) {
        const r = await api(ctx, ap(v.agentId) + '/share-links', { agentId: v.agentId });
        ctx.assert(
          r.body.items.some((l) => l.id === v.share.id && l.mode === v.mode && l.revoked_at === null),
          'share_durable_exact',
        );
        if (v.audience !== 'public') {
          const wrong = await open(ctx, 'unauthorized', v.agentId);
          await wrong.page.goto(v.url);
          await visible(
            ctx,
            wrong.page.getByRole('heading', { name: 'Shared Agent unavailable' }),
            'wrong_audience_denied',
          );
          await absent(
            ctx,
            wrong.page.getByText(v.historyMarker, { exact: true }),
            'no_private_content_wrong_audience',
          );
        }
      }
    },
  ]),
  define('sharing.revoke-agent-link', [
    async (ctx) => {
      await agent(ctx, 'share-revoke', 'outsider');
      await visible(ctx, button(ctx.state.b.page, 'Send message'), 'writer_before_revoke');
    },
    revoke,
    async (ctx) => {
      const { page } = ctx.state.b;
      await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Reply only OK');
      // Submit using the recipient's stale UI once, then refresh current authority.
      await button(page, 'Send message').click();
      await visible(ctx, page.getByRole('alert'), 'revoked_send_visible_refusal');
      await page.reload();
      await absent(
        ctx,
        page.getByText(ctx.state.f.historyMarker, { exact: true }),
        'revoked_reads_not_retained',
      );
    },
    async (ctx) => {
      ctx.assert((await ctx.state.f.effectJournal()).post_revocation_sends === 0, 'revoked_send_no_effect');
      const denied = await api(ctx, ap(ctx.state.f.agentId), {
        actor: 'outsider',
        agentId: ctx.state.f.agentId,
        allowed: [403, 404],
      });
      ctx.assert([403, 404].includes(denied.status), 'revoked_read_denied');
    },
  ]),
  define('sharing.redeem', [
    async (ctx) => {
      const { page, f } = await setup(ctx, 'share-redeem', 'outsider', { requireSession: true });
      await page.goto(f.validShareUrl);
      await contains(ctx, page.locator('main'), f.historyMarker, 'valid_link_opens_agent');
    },
    async (ctx) => {
      const session = await webRequest(ctx, '/auth/session');
      ctx.assert(session.body.subject === ctx.state.f.recipientPrincipal, 'recipient_identity');
    },
    async (ctx) => {
      await absent(ctx, button(ctx.state.b.page, 'Manage share links'), 'recipient_no_reshare');
      await visible(ctx, ctx.state.b.page.getByRole('heading', { level: 1 }), 'shared_surface_visible');
    },
    async (ctx) => {
      for (const name of ['revokedShareUrl', 'wrongAudienceShareUrl']) {
        await ctx.state.b.page.goto(ctx.state.f[name]);
        await visible(
          ctx,
          ctx.state.b.page.getByRole('heading', { name: 'Shared Agent unavailable' }),
          'invalid_share_denied',
        );
      }
    },
    async (ctx) => {
      await absent(
        ctx,
        ctx.state.b.page.getByText(ctx.state.f.historyMarker, { exact: true }),
        'invalid_link_no_private_history',
      );
      const r = await api(ctx, ap(ctx.state.f.agentId) + '/share-links', { agentId: ctx.state.f.agentId });
      ctx.assert(
        r.body.items.some((l) => l.id === ctx.state.f.validShareId && l.accepted_count === 1),
        'valid_share_single_redemption',
      );
    },
  ]),
  define('sharing.sandbox-grant', [
    async (ctx) => {
      const f = await sharePage(ctx);
      await pick(ctx.state.b.page, 'Share with organization', f.orgName);
      await pick(
        ctx.state.b.page,
        'New share access',
        f.mode === 'view' ? 'View only' : 'Use organization roles',
      );
      await visible(ctx, button(ctx.state.b.page, 'Create share'), 'organization_selected');
    },
    async (ctx) => {
      await contains(
        ctx,
        ctx.state.b.page.getByLabel('Sharing exposure preview'),
        'future resources',
        'future_children_disclosed',
      );
      await contains(
        ctx,
        ctx.state.b.page.getByLabel('Sharing exposure preview'),
        'allowed scoped files',
        'file_scope_disclosed',
      );
      await ctx.state.b.page
        .getByRole('checkbox', {
          name: 'I intend to share the whole Sandbox, including its current and future resources.',
          exact: true,
        })
        .check();
    },
    async (ctx) => {
      const f = ctx.state.f;
      const r = await capture(
        ctx.state.b.page,
        'POST',
        `/api/v1/resource-shares/sandbox/${enc(f.sandboxId)}`,
        () => button(ctx.state.b.page, 'Create share').click(),
      );
      ctx.state.share = r.body.share;
      ctx.assert(
        [200, 201].includes(r.status) &&
          r.body.share.target_org_id === f.orgId &&
          r.body.share.mode === f.mode,
        'exact_organization_grant',
      );
    },
    async (ctx) => {
      const f = ctx.state.f;
      const b = await open(ctx, 'outsider', f.agentId);
      ctx.state.recipient = b;
      for (const [route, marker] of [
        [`sandboxes/${f.sandboxId}`, f.sandboxName],
        [`workspaces/${f.workspaceId}`, f.workspaceName],
        [`agents/${f.agentId}`, f.historyMarker],
      ]) {
        await b.page.goto(b.origin + '/' + route);
        await contains(ctx, b.page.locator('main'), marker, 'granted_hierarchy_visible');
      }
    },
    async (ctx) => {
      const f = ctx.state.f;
      const access = await api(ctx, `/api/v1/resource-access/agent/${enc(f.agentId)}`, {
        actor: 'outsider',
        agentId: f.agentId,
      });
      ctx.assert(
        access.body.capabilities.includes('view_artifacts') &&
          hash([...access.body.capabilities].sort()) === hash([...f.expectedCapabilities].sort()),
        'grant_exact_current_capabilities',
      );
    },
  ]),
  define('sharing.update-revoke', [
    async (ctx) => {
      await sharePage(ctx);
      ctx.state.grant = (
        await api(ctx, `/api/v1/resource-shares/sandbox/${enc(ctx.state.f.sandboxId)}`)
      ).body.items.find((s) => s.id === ctx.state.f.shareId);
      ctx.assert(ctx.state.grant?.revoked_at === null, 'existing_grant');
    },
    async (ctx) => {
      const f = ctx.state.f;
      await pick(ctx.state.b.page, `Access for ${f.orgName}`, 'Use organization roles');
      const r = await capture(
        ctx.state.b.page,
        'PATCH',
        `/api/v1/resource-shares/sandbox/${enc(f.sandboxId)}/${enc(f.shareId)}`,
        () => button(ctx.state.b.page, 'Save access').click(),
      );
      ctx.assert(
        r.body.share.mode === 'org_roles' && r.body.share.revision === ctx.state.grant.revision + 1,
        'grant_mode_cas',
      );
    },
    async (ctx) => {
      await button(ctx.state.b.page, 'Revoke').click();
      const f = ctx.state.f;
      const r = await capture(
        ctx.state.b.page,
        'DELETE',
        `/api/v1/resource-shares/sandbox/${enc(f.sandboxId)}/${enc(f.shareId)}`,
        () => button(ctx.state.b.page, 'Revoke grant').click(),
      );
      ctx.assert(r.body.share.revoked_at && r.body.share.id === f.shareId, 'grant_exact_revoked');
    },
    async (ctx) => {
      const f = ctx.state.f;
      const b = await open(ctx, 'outsider', f.agentId);
      await b.page.goto(b.origin + '/agents/' + enc(f.agentId));
      await visible(
        ctx,
        b.page.getByRole('heading', { name: 'Agent unavailable', exact: true }),
        'grant_revoked_surface',
      );
      await absent(ctx, b.page.getByText(f.historyMarker, { exact: true }), 'grant_revoked_content');
      await absent(ctx, button(b.page, 'Send message'), 'grant_revoked_controls');
      const denied = await api(ctx, ap(f.agentId), {
        actor: 'outsider',
        agentId: f.agentId,
        allowed: [403, 404],
      });
      ctx.assert([403, 404].includes(denied.status), 'grant_revoked_read');
    },
    async (ctx) => {
      const f = ctx.state.f;
      const shares = await api(ctx, `/api/v1/resource-shares/sandbox/${enc(f.controlSandboxId)}`);
      ctx.assert(
        shares.body.items.some((s) => s.id === f.controlShareId && s.revoked_at === null),
        'unrelated_grant_intact',
      );
    },
  ]),
  define('sharing.read-only', [
    async (ctx) => {
      const f = await agent(ctx, 'share-read-only', 'outsider');
      for (const [route, label] of [
        [`sandboxes/${f.sandboxId}`, f.sandboxName],
        [`workspaces/${f.workspaceId}`, f.workspaceName],
      ]) {
        await ctx.state.b.page.goto(ctx.state.b.origin + '/' + route);
        await contains(ctx, ctx.state.b.page.locator('main'), label, 'shared_resource_visible');
      }
    },
    async (ctx) => {
      const f = ctx.state.f;
      await ctx.state.b.page.goto(ctx.state.b.origin + '/agents/' + enc(f.agentId));
      await contains(ctx, ctx.state.b.page.locator('main'), f.historyMarker, 'readonly_history');
      await button(ctx.state.b.page, 'Show Agent computer').click();
      await ctx.state.b.page.getByRole('tab', { name: 'Files', exact: true }).click();
      await contains(ctx, ctx.state.b.page.locator('main'), f.scopedFileName, 'scoped_file_visible');
    },
    async (ctx) => {
      for (const name of [
        'Send message',
        'Manage share links',
        'Take control',
        'Save settings',
        'Publish artifact',
      ])
        await absent(ctx, button(ctx.state.b.page, name), 'owner_control_absent');
      const forbidden = await api(ctx, ap(ctx.state.f.agentId) + '/preferences', {
        actor: 'outsider',
        agentId: ctx.state.f.agentId,
        allowed: [403, 404],
      });
      ctx.assert([403, 404].includes(forbidden.status), 'owner_preferences_denied');
    },
    async (ctx) => {
      const access = await api(ctx, `/api/v1/resource-access/agent/${enc(ctx.state.f.agentId)}`, {
        actor: 'outsider',
        agentId: ctx.state.f.agentId,
      });
      ctx.assert(
        access.body.capabilities.includes('view_details') && !access.body.capabilities.includes('interact'),
        'readonly_effective_capabilities',
      );
    },
  ]),
];
