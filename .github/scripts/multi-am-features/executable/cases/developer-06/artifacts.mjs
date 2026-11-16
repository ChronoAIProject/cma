import { readFile } from 'node:fs/promises';
import {
  define,
  setup,
  open,
  api,
  button,
  visible,
  contains,
  absent,
  capture,
  enc,
  hash,
} from './helpers.mjs';
const ap = (ctx) => `/api/v1/agents/${enc(ctx.state.f.agentId)}`;
async function enter(ctx, scenario, actor = 'outsider') {
  const { page, origin, f } = await setup(ctx, scenario, actor);
  await page.goto(origin + '/agents/' + enc(f.agentId));
  await button(page, 'Show Agent details').click();
  await button(page, 'Artifacts').click();
  await visible(ctx, page.getByRole('dialog', { name: 'Agent artifacts' }), 'artifact_surface');
}
async function revoke(ctx) {
  const f = ctx.state.f;
  const r = await api(
    ctx,
    ap(ctx) + '/share-links/' + enc(f.shareId) + '?expected_revision=' + f.shareRevision,
    { method: 'DELETE', key: f.revokeKey, agentId: f.agentId },
  );
  ctx.assert(r.body.id === f.shareId && r.body.revoked_at, 'reader_grant_revoked');
}
export const cases = [
  define('sharing.agent-artifacts', [
    (ctx) => enter(ctx, 'share-artifacts'),
    async (ctx) => {
      await button(ctx.state.b.page, 'Refresh artifacts').click();
      await button(ctx.state.b.page, 'Next artifacts').click();
      await visible(ctx, button(ctx.state.b.page, 'Previous artifacts'), 'artifact_page_two');
      await button(ctx.state.b.page, 'Previous artifacts').click();
      await absent(
        ctx,
        ctx.state.b.page.getByText(ctx.state.f.unapprovedFileName, { exact: true }),
        'unapproved_artifact_hidden',
      );
    },
    async (ctx) => {
      const pending = ctx.state.b.page.waitForEvent('download');
      await button(ctx.state.b.page, 'Download ' + ctx.state.f.artifactName).click();
      const download = await pending;
      const data = await readFile(await download.path());
      ctx.assert(
        data.length === ctx.state.f.artifactSize && hash(data) === ctx.state.f.artifactSha256,
        'approved_artifact_exact_bytes',
      );
      await ctx.observe('download_receipt', async () => ({ size: data.length, sha256: hash(data) }));
    },
    async (ctx) => {
      await revoke(ctx);
      await button(ctx.state.b.page, 'Refresh artifacts').click();
      await visible(ctx, ctx.state.b.page.getByRole('alert'), 'artifact_revocation_visible');
      const r = await api(ctx, ap(ctx) + '/artifacts/' + enc(ctx.state.f.artifactId) + '/download', {
        actor: 'outsider',
        agentId: ctx.state.f.agentId,
        allowed: [403, 404],
      });
      ctx.assert([403, 404].includes(r.status), 'revoked_artifact_download_denied');
    },
    async (ctx) => {
      const r = await api(ctx, ap(ctx) + '/artifacts', { agentId: ctx.state.f.agentId });
      ctx.assert(
        r.body.items.every((x) => x.id !== ctx.state.f.unapprovedArtifactId),
        'only_published_artifacts',
      );
      const traffic = await ctx.state.b.traffic.flush();
      const reads = traffic.filter(
        (r) =>
          r.path === ap(ctx) + '/artifacts/' + enc(ctx.state.f.artifactId) + '/download' && r.status === 200,
      );
      ctx.assert(reads.length === 1, 'single_approved_download');
      const read = reads[0];
      const policy = traffic.filter(
        (r) => r.path === `/api/v1/resource-access/agent/${enc(ctx.state.f.agentId)}` && r.status === 200,
      );
      const before = policy.filter((r) => r.observed_ms < read.observed_ms).at(-1),
        after = policy.find((r) => r.observed_ms > read.observed_ms);
      ctx.assert(
        before &&
          after &&
          before.policy_revision === after.policy_revision &&
          before.capabilities.includes('view_artifacts') &&
          after.capabilities.includes('view_artifacts'),
        'artifact_read_fresh_authority',
      );
    },
  ]),
  define('sharing.publish-artifact', [
    async (ctx) => {
      await enter(ctx, 'share-publish-artifact', 'user');
      const f = ctx.state.f;
      const bytes = await readFile(f.artifactPath);
      ctx.assert(
        bytes.length >= 1 && bytes.length <= 1048576 && hash(bytes) === f.artifactSha256,
        'bounded_approved_file',
      );
      await ctx.state.b.page.getByLabel('Artifact file', { exact: true }).setInputFiles(f.artifactPath);
    },
    async (ctx) => {
      await contains(
        ctx,
        ctx.state.b.page.getByRole('dialog', { name: 'Agent artifacts' }),
        'current and future readers',
        'artifact_exposure_disclosure',
      );
      await ctx.state.b.page
        .getByRole('checkbox', {
          name: 'I approve sharing this file with everyone who can read this Agent.',
          exact: true,
        })
        .check();
      ctx.assert(
        await button(ctx.state.b.page, 'Publish artifact').isEnabled(),
        'explicit_publication_consent',
      );
    },
    async (ctx) => {
      const drop = await ctx.state.f.dropNextResponse({
        method: 'POST',
        path: ap(ctx) + '/artifacts',
        afterCommit: true,
      });
      ctx.defer('remove_publication_fault', () => drop.close());
      await button(ctx.state.b.page, 'Publish artifact').click();
      await visible(
        ctx,
        button(ctx.state.b.page, 'Retry artifact publication'),
        'uncertain_publication_retained',
      );
      const r = await capture(ctx.state.b.page, 'POST', ap(ctx) + '/artifacts', () =>
        button(ctx.state.b.page, 'Retry artifact publication').click(),
      );
      ctx.assert(
        [200, 201].includes(r.status) && r.body.sha256 === ctx.state.f.artifactSha256,
        'publication_exact_file',
      );
      ctx.state.artifact = r.body;
    },
    async (ctx) => {
      const f = ctx.state.f;
      const b = await open(ctx, 'outsider', f.agentId);
      await b.page.goto(b.origin + '/agents/' + enc(f.agentId));
      await button(b.page, 'Show Agent details').click();
      await button(b.page, 'Artifacts').click();
      const pending = b.page.waitForEvent('download');
      await button(b.page, 'Download ' + f.artifactName).click();
      const download = await pending;
      ctx.assert(hash(await readFile(await download.path())) === f.artifactSha256, 'reader_published_bytes');
    },
    async (ctx) => {
      const r = await api(ctx, ap(ctx) + '/artifacts', { agentId: ctx.state.f.agentId });
      const matching = r.body.items.filter((a) => a.sha256 === ctx.state.f.artifactSha256);
      ctx.assert(
        matching.length === 1 && matching[0].id === ctx.state.artifact.id,
        'publication_no_duplicate',
      );
      const journal = await ctx.state.f.publicationJournal();
      ctx.assert(
        journal.requests.length === 2 &&
          new Set(journal.requests.map((r) => r.idempotency_key_sha256)).size === 1 &&
          journal.commits === 1,
        'uncertain_publication_original_identity',
      );
    },
  ]),
  define('sharing.channel-history', [
    async (ctx) => {
      const { page, origin, f } = await setup(ctx, 'share-channel');
      await page.goto(origin + '/agents/' + enc(f.agentId));
      await button(page, 'Show Agent details').click();
      const current = await api(ctx, ap(ctx) + '/channel-usage', { agentId: f.agentId });
      ctx.assert(current.body.channel_usage.has_history === true, 'channel_saved_history');
      await contains(ctx, page.locator('main'), f.channelStatusText, 'current_channel_usage');
    },
    async (ctx) => {
      await button(ctx.state.b.page, 'Channel history').click();
      await visible(
        ctx,
        ctx.state.b.page.getByRole('region', { name: 'Channel binding history' }),
        'channel_history_expanded',
      );
    },
    async (ctx) => {
      const page = ctx.state.b.page;
      await page.getByText(`Binding changes for event ${ctx.state.f.eventRevision}`, { exact: true }).click();
      for (const text of ['Actor attribution', 'Before', 'After', ctx.state.f.actorDisplay])
        await contains(
          ctx,
          page.getByRole('dialog', { name: 'Channel history' }),
          text,
          'channel_attribution_visible',
        );
      await button(page, 'Next channel events').click();
      await contains(
        ctx,
        page.getByRole('dialog', { name: 'Channel history' }),
        ctx.state.f.secondPageActor,
        'channel_pagination',
      );
    },
    async (ctx) => {
      const r = await api(ctx, ap(ctx) + '/channel-usage/history', { agentId: ctx.state.f.agentId });
      ctx.assert(
        r.body.items.every((e) => e.actor && Array.isArray(e.before) && Array.isArray(e.after) && e.at),
        'channel_durable_facts',
      );
      const denied = await api(ctx, ap(ctx) + '/channel-usage/history', {
        actor: 'outsider',
        agentId: ctx.state.f.agentId,
        allowed: [403, 404],
      });
      ctx.assert([403, 404].includes(denied.status), 'private_history_authorized');
      const text = await ctx.state.b.page.locator('main').innerText();
      ctx.assert(
        !ctx.state.f.forbiddenCredentialMarkers.some((m) => text.includes(m)),
        'channel_no_credentials',
      );
    },
  ]),
  define('sharing.interact', [
    async (ctx) => {
      const { page, origin, f } = await setup(ctx, 'share-interact', 'outsider');
      await page.goto(origin + '/agents/' + enc(f.agentId));
      await visible(
        ctx,
        page.getByRole('textbox', { name: 'Message', exact: true }),
        'shared_writer_composer',
      );
    },
    async (ctx) => {
      const { page } = ctx.state.b,
        f = ctx.state.f;
      await page.getByRole('textbox', { name: 'Message', exact: true }).fill(f.slowPrompt);
      await button(page, 'Send message').click();
      await visible(ctx, button(page, 'Stop response'), 'shared_active_turn');
      await button(page, 'Stop response').click();
      await visible(ctx, button(page, 'Send message'), 'shared_interrupt_finished');
      await page.locator('input[type="file"]').setInputFiles(f.attachmentPath);
      await page.getByRole('textbox', { name: 'Message', exact: true }).fill(f.approvalPrompt);
      await button(page, 'Send message').click();
      const approval = page.getByRole('region', { name: f.approvalTitle, exact: true });
      await contains(ctx, approval, f.harmlessCommand, 'shared_harmless_approval');
      await button(approval, f.approveLabel).click();
      await contains(ctx, page.locator('main'), f.replyMarker, 'shared_reply');
    },
    async (ctx) => {
      await revoke(ctx);
      await ctx.state.b.page.getByRole('textbox', { name: 'Message', exact: true }).fill('Reply only OK');
      await button(ctx.state.b.page, 'Send message').click();
      await visible(ctx, ctx.state.b.page.getByRole('alert'), 'shared_revoked_send_refused');
    },
    async (ctx) => {
      const journal = await ctx.state.f.effectJournal();
      ctx.assert(
        journal.approvals === 1 &&
          journal.attachments === 1 &&
          journal.interrupted === 1 &&
          journal.post_revocation_sends === 0,
        'shared_interaction_exact_effects',
      );
      for (const name of ['Manage share links', 'Take control', 'Save settings'])
        await absent(ctx, button(ctx.state.b.page, name), 'shared_no_owner_authority');
    },
  ]),
];
