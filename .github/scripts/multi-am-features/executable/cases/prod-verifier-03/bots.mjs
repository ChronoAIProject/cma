import {define, acquire, openBrowser, invoke, visible, uiReceipt, goto, poll, peer, digest, segment, egRequest} from './helpers.mjs';
import {send} from './platform.mjs';

const path = lease => `/api/v1/bots/${segment(lease.botId)}`;
async function bot(ctx, lease, am = ctx.am) {
  const result = await ctx.request({method: 'GET', path: path(lease), am});
  ctx.assert(result.body.bot_id === lease.botId, 'bot_read_exact_identity');
  return result.body;
}

function actionCase(number, action, label) {
  return define(number, 'browser', ['browser', 'eg'], async ctx => {
    let lease, page, before, result;
    const writes = [];
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, `consumer_${action}`);
      before = await bot(ctx, lease);
      ({page} = await openBrowser(ctx, 'user', {agentId: lease.agentId}));
      await goto(page, `/bots/${segment(lease.botId)}`);
      await visible(ctx, page.getByRole('heading', {name: before.name, exact: true}), 'consumer_detail_open');
      page.on('request', request => {
        if (new URL(request.url()).pathname === path(lease) + '/' + action && request.method() === 'POST') writes.push(request);
      });
      await invoke(lease, 'armFault', {kind: 'drop_bot_action_response', action, botId: lease.botId});
      await page.getByRole('button', {name: label, exact: true}).click();
      if (action === 'rotate-keys') await page.getByRole('button', {name: 'Confirm key rotation', exact: true}).click();
      await visible(ctx, page.getByText(/outcome is unconfirmed\. Refresh status to inspect recorded progress/),
        'consumer_unknown_action_explicit');
      ctx.assert(writes.length === 1, 'consumer_one_action_attempt');
    });
    await ctx.step('step_2', async () => {
      result = await poll(ctx, () => bot(ctx, lease, peer(ctx.am)), value => value.operation === null && (
        action === 'test' ? value.last_test && value.last_test.tested_at !== before.last_test?.tested_at
          : action === 'disable' ? value.state === 'disabled'
            : action === 'enable' ? value.state === 'active'
              : value.channel?.version > before.channel?.version), 'consumer_action_settlement_timeout');
      await page.getByRole('button', {name: 'Refresh status', exact: true}).click();
      await invoke(lease, 'browserReadBarrier', {page});
      ctx.assert(writes.length === 1, 'consumer_reconciliation_did_not_resubmit');
      ctx.assert(result.owner === before.owner && result.bot_id === before.bot_id, 'consumer_action_owner_identity');
      if (action === 'test') {
        ctx.assert(result.last_test.first_text === lease.expected.firstText && result.last_test.accepted_after_ms >= 0,
          'consumer_test_actual_first_text');
        const turn = await invoke(lease, 'testTurnReceipt', {botId: lease.botId, testedAt: result.last_test.tested_at});
        ctx.assert(turn.consumerId === lease.botId && turn.agentId === lease.agentId
          && turn.responseId && turn.turnCount === 1, 'consumer_test_actual_am_turn');
        await visible(ctx, page.getByText(lease.expected.firstText, {exact: true}), 'consumer_test_result_visible');
      } else if (action === 'disable') {
        const event = await invoke(lease, 'platformSend', {platform: lease.platform, variant: 'disabled_consumer', replica: peer(ctx.am)});
        ctx.assert(['refused', 'ignored', 'rejected'].includes(event.outcome) && event.turnCount === 0,
          'disabled_consumer_no_new_work');
      } else {
        const turn = await send(ctx, lease, {platform: lease.platform, trustMode: lease.trustMode,
          replica: peer(ctx.am), sender: 'allowed_sender', conversation: 'consumer_action', text: 'synthetic post-action event'});
        ctx.assert(turn.event.consumerId === lease.expected.consumerId, 'consumer_action_event_completes');
        if (action === 'rotate-keys') {
          const credentials = await invoke(lease, 'credentialUseReceipts', {eventId: turn.event.eventId});
          ctx.assert(credentials.activeVersion > lease.expected.originalCredentialVersion
            && credentials.usedVersion === credentials.activeVersion && credentials.oldVersionRejected === true,
          'consumer_rotated_credentials_used');
        }
      }
    });
    await ctx.step('expected_result', async () => {
      const final = await invoke(lease, 'drain', {timeoutMs: 90000});
      ctx.assert(final.pending === 0 && final.unknown === 0 && writes.length === 1, 'consumer_action_finality');
      const scan = await invoke(lease, 'scanBrowserForCredentialValues', {page});
      ctx.assert(scan.matches === 0 && scan.scannedText && scan.scannedInputs, 'consumer_no_credential_disclosure');
      await uiReceipt(ctx, page);
    });
  });
}

export const botCases = [
  actionCase(50, 'test', 'Run synthetic test'), actionCase(51, 'disable', 'Disable consumer'),
  actionCase(52, 'enable', 'Enable consumer'), actionCase(53, 'rotate-keys', 'Rotate keys'),
  define(54, 'browser', ['browser', 'eg'], async ctx => {
    let lease, page, before;
    const writes = [];
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'consumer_delete'); before = await bot(ctx, lease);
      ({page} = await openBrowser(ctx, 'user', {agentId: lease.agentId}));
      await goto(page, `/bots/${segment(lease.botId)}`);
      await page.getByRole('button', {name: 'Delete consumer', exact: true}).click();
      await visible(ctx, page.getByRole('heading', {name: `Delete ${before.name}`, exact: true}), 'consumer_delete_exact_confirmation');
      await page.getByLabel('I understand this Bot connection will be removed.', {exact: true}).check();
      page.on('request', request => {
        if (request.method() === 'DELETE' && new URL(request.url()).pathname === path(lease)) writes.push(request);
      });
      await page.getByRole('button', {name: 'Confirm deletion', exact: true}).click();
      await poll(ctx, async () => writes.length, count => count > 0, 'consumer_delete_request_timeout', 10000);
      ctx.assert(writes.length === 1, 'consumer_delete_requested_once');
    });
    await ctx.step('step_2', async () => {
      await poll(ctx, () => ctx.request({method: 'GET', path: path(lease), am: peer(ctx.am), allowed: [200, 404]}),
        response => response.status === 404 || response.body.state === 'deleted', 'consumer_delete_terminal_timeout');
      await page.getByRole('button', {name: 'Refresh status', exact: true}).click();
      await visible(ctx, page.getByText('Consumer deleted. Its channel and Agent access have been removed.', {exact: true}),
        'consumer_deleted_ui');
      const channel = await egRequest(ctx, lease, {path: `/v1/channels/${segment(before.channel.channel_id)}`,
        replica: peer(ctx.am), allowed: [404]});
      ctx.assert(!channel.record_id, 'consumer_linked_channel_removed');
      const cleanup = await invoke(lease, 'linkedResources');
      ctx.assert(cleanup.consumerAccess === 'revoked' && cleanup.channel === 'deleted'
        && cleanup.existingConversations === 'retained', 'consumer_linked_resources_cleanup');
    });
    await ctx.step('expected_result', async () => {
      const beforeCounts = await invoke(lease, 'counters');
      const rejected = await invoke(lease, 'platformSend', {variant: 'deleted_consumer', replica: ctx.am});
      ctx.assert(['refused', 'rejected', 'ignored'].includes(rejected.outcome), 'deleted_consumer_rejects_work');
      const after = await invoke(lease, 'quiesce', {timeoutMs: 5000});
      ctx.assert(after.turns === beforeCounts.turns && writes.length === 1, 'deleted_consumer_no_new_turn');
      const unrelated = await ctx.request({method: 'GET', path: `/api/v1/bots/${segment(lease.expected.unrelatedBotId)}`,
        am: peer(ctx.am)});
      ctx.assert(digest(unrelated.body) === lease.expected.unrelatedBotDigest, 'consumer_delete_preserves_unrelated_resources');
      await uiReceipt(ctx, page);
    });
  }),
];
