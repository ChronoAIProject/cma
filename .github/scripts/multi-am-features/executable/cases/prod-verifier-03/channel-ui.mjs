import {define, acquire, openBrowser, visible, uiReceipt, egRequest, invoke, peer, segment, same, digest} from './helpers.mjs';

async function select(ctx, page, name) {
  const item = page.getByRole('tree', {name: 'Channels', exact: true}).getByRole('treeitem')
    .filter({has: page.getByText(name, {exact: true})});
  await item.getByRole('button').click();
  await visible(ctx, page.getByRole('heading', {name, exact: true}), 'channel_selected');
}
async function inspect(ctx, page, name) {
  await select(ctx, page, name);
  await page.getByRole('button', {name: `Channel: ${name}`, exact: true}).click();
  await visible(ctx, page.getByLabel('Channel name', {exact: true}), 'channel_inspector_open');
}
const channelPath = lease => `/v1/channels/${segment(lease.channelId)}`;
const matchResponse = (path, method) => response => new URL(response.url()).pathname === `/api/event-gateway${path}`
  && response.request().method() === method;

export const channelUiCases = [
  define(18, 'browser', ['browser', 'eg'], async ctx => {
    let lease, page;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'channel_inventory'); ({page} = await openBrowser(ctx));
      const refreshed = page.waitForResponse(matchResponse('/v1/channels', 'GET'));
      await page.getByRole('button', {name: 'Refresh channels', exact: true}).click();
      ctx.assert((await refreshed).status() === 200, 'channel_refresh_success');
      let pages = 1;
      while (await page.getByRole('button', {name: 'Load more channels', exact: true}).isVisible()) {
        ctx.assert(pages++ < 20, 'channel_ui_page_bound');
        const next = page.waitForResponse(matchResponse('/v1/channels', 'GET'));
        await page.getByRole('button', {name: 'Load more channels', exact: true}).click();
        ctx.assert((await next).status() === 200, 'channel_next_page_success');
      }
      ctx.assert(pages >= 2, 'channel_pagination_exercised');
      const names = await page.getByRole('tree', {name: 'Channels', exact: true}).getByRole('treeitem').allTextContents();
      ctx.assert(lease.expected.channels.every(channel => names.some(name => name.includes(channel.name)))
        && !names.some(name => name.includes(lease.expected.foreignName)), 'channel_owner_inventory_only');
    });
    await ctx.step('step_2', async () => {
      await inspect(ctx, page, lease.channelName);
      const saved = await egRequest(ctx, lease, {path: channelPath(lease), replica: peer(ctx.am)});
      ctx.assert(await page.getByLabel('Channel name', {exact: true}).inputValue() === saved.definition.name,
        'channel_saved_name_visible');
      await visible(ctx, page.getByRole('heading', {name: 'Delivery map'}), 'channel_topology_visible');
      for (const name of lease.expected.nodeNames) await visible(ctx,
        page.getByRole('button', {name, exact: true}), 'channel_saved_topology_node');
      ctx.assert((await page.locator('.channel-map').innerText()).includes(`Version ${saved.version}`),
        'channel_saved_revision_visible');
      await select(ctx, page, lease.expected.missingAddressName);
      await visible(ctx, page.getByLabel('Original channel ID', {exact: true}), 'channel_missing_address_explicit');
    });
    await ctx.step('expected_result', async () => {
      const scan = await invoke(lease, 'scanBrowserForCredentialValues', {page});
      ctx.assert(scan.matches === 0 && scan.scannedInputs === true && scan.scannedText === true,
        'channel_no_secret_rendered');
      await uiReceipt(ctx, page);
    });
  }),
  define(22, 'browser', ['browser', 'user-fixtures'], async ctx => {
    let lease, page, before, changed;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'channel_edit'); ({page} = await openBrowser(ctx));
      before = await egRequest(ctx, lease, {path: channelPath(lease)});
      await inspect(ctx, page, lease.channelName);
      const ack = (before.definition.reply.ack_after_ms ?? 0) + 100;
      await page.getByLabel('Channel name', {exact: true}).fill(`${lease.channelName} renamed`);
      await page.getByLabel('Acknowledgement delay (ms)', {exact: true}).fill(String(ack));
      ctx.assert(await page.getByLabel('Channel name', {exact: true}).inputValue() !== before.definition.name,
        'channel_name_changed');
      ctx.assert(Number(await page.getByLabel('Acknowledgement delay (ms)', {exact: true}).inputValue()) === ack,
        'channel_nonsecret_setting_changed');
    });
    await ctx.step('step_2', async () => {
      const response = page.waitForResponse(matchResponse(channelPath(lease), 'PUT'));
      await page.getByRole('button', {name: 'Save channel', exact: true}).click();
      const savedResponse = await response;
      ctx.assert(savedResponse.status() === 200, 'channel_ui_save_success'); changed = await savedResponse.json();
      ctx.assert(changed.version > before.version && changed.record_id === before.record_id, 'channel_saved_revision_identity');
      await page.reload(); await inspect(ctx, page, changed.definition.name);
      ctx.assert(await page.getByLabel('Channel name', {exact: true}).inputValue() === changed.definition.name,
        'channel_ui_reload_converges');
      const stale = {...lease.updateBody, name: 'must not overwrite', expected_version: before.version};
      const denied = await egRequest(ctx, lease, {method: 'PUT', path: channelPath(lease),
        body: stale, replica: peer(ctx.am), allowed: [409]});
      ctx.assert(denied.code || denied.type, 'channel_stale_revision_typed_refusal');
    });
    await ctx.step('expected_result', async () => {
      for (const replica of [ctx.am, peer(ctx.am)]) {
        const current = await egRequest(ctx, lease, {path: channelPath(lease), replica});
        ctx.assert(current.version === changed.version && current.definition.name === changed.definition.name
          && current.definition.reply.ack_after_ms === (before.definition.reply.ack_after_ms ?? 0) + 100,
        'channel_cross_replica_settings_converge');
      }
      await uiReceipt(ctx, page);
    });
  }),
  define(27, 'browser', ['browser', 'eg'], async ctx => {
    let lease, page, before;
    const results = [];
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'channel_tests'); ({page} = await openBrowser(ctx));
      before = await invoke(lease, 'counters'); await select(ctx, page, lease.channelName);
      for (const [node, path, label] of [
        [lease.expected.producerNodeName, `${channelPath(lease)}/sources/0/test`, 'Check configuration'],
        [`Channel: ${lease.channelName}`, `${channelPath(lease)}/test`, 'Send test event'],
        [lease.expected.consumerNodeName, `${channelPath(lease)}/consumers/default/test`, 'Send test event'],
      ]) {
        await page.getByRole('button', {name: node, exact: true}).click();
        const response = page.waitForResponse(matchResponse(path, 'POST'));
        await page.getByRole('button', {name: label, exact: true}).click();
        const returned = await response;
        ctx.assert(returned.status() === 200, 'node_test_response');
        results.push({source: label === 'Check configuration', body: await returned.json()});
      }
      ctx.assert(results.length === 3, 'all_node_tests_invoked_once');
    });
    await ctx.step('step_2', async () => {
      const source = results.find(result => result.source).body;
      ctx.assert(Array.isArray(source.checks) && source.checks.length > 0
        && source.checks.every(check => ['passed', 'unavailable'].includes(check.status)),
      'source_configuration_checks_explicit');
      for (const result of results.filter(r => !r.source)) {
        ctx.assert(result.body.first_text === lease.expected.firstText, 'node_test_actual_first_text');
      }
      await page.getByRole('button', {name: lease.expected.producerNodeName, exact: true}).click();
      await visible(ctx, page.getByText('Configuration check completed. External delivery was not tested.', {exact: true}),
        'source_check_not_platform_delivery');
      await invoke(lease, 'armFault', {kind: 'drop_test_response', channelId: lease.channelId});
      await page.getByRole('button', {name: `Channel: ${lease.channelName}`, exact: true}).click();
      await page.getByRole('button', {name: 'Send test event', exact: true}).click();
      await visible(ctx, page.getByText('Test result is unknown. Check channel activity before sending another test.', {exact: true}),
        'node_unknown_requires_reconciliation');
      const reconciled = await invoke(lease, 'reconcileTest', {channelId: lease.channelId});
      ctx.assert(reconciled.turnCount === 1 && reconciled.resubmissions === 0, 'node_unknown_readback_only');
    });
    await ctx.step('expected_result', async () => {
      const after = await invoke(lease, 'counters');
      ctx.assert(after.turns === before.turns + 3 && after.deliveries === before.deliveries,
        'node_tests_only_am_turns_no_platform_delivery');
      await ctx.observe('node_test_counts', async () => ({before, after})); await uiReceipt(ctx, page);
    });
  }),
  define(31, 'browser', ['browser', 'eg'], async ctx => {
    let lease, page, before;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'channel_delete'); ({page} = await openBrowser(ctx));
      before = await egRequest(ctx, lease, {path: channelPath(lease)});
      await inspect(ctx, page, lease.channelName);
      await page.getByRole('button', {name: 'Delete channel', exact: true}).click();
      await visible(ctx, page.getByText(`Delete ${lease.channelName}? This disconnects its provider and stops incoming event delivery.`,
        {exact: true}), 'channel_delete_confirmation');
      ctx.assert((await egRequest(ctx, lease, {path: channelPath(lease)})).version === before.version,
        'channel_confirmation_has_no_effect');
    });
    await ctx.step('step_2', async () => {
      const deleted = page.waitForResponse(matchResponse(channelPath(lease), 'DELETE'));
      await page.getByRole('button', {name: 'Confirm delete', exact: true}).click();
      const response = await deleted;
      ctx.assert(response.status() === 204
        && new URL(response.url()).searchParams.get('expected_version') === String(before.version),
      'channel_delete_exact_revision');
      await page.reload();
      ctx.assert(await page.getByRole('tree', {name: 'Channels', exact: true})
        .getByText(lease.channelName, {exact: true}).count() === 0, 'deleted_channel_absent_from_ui');
      for (const replica of [ctx.am, peer(ctx.am)]) {
        const missing = await egRequest(ctx, lease, {path: channelPath(lease), replica, allowed: [404]});
        ctx.assert(!missing.record_id, 'deleted_channel_absent_from_replica');
      }
    });
    await ctx.step('expected_result', async () => {
      const counters = await invoke(lease, 'counters');
      const refused = await invoke(lease, 'platformSend', {variant: 'deleted_channel', replica: peer(ctx.am)});
      ctx.assert(['refused', 'rejected'].includes(refused.outcome), 'deleted_channel_rejects_ingress');
      const after = await invoke(lease, 'quiesce', {timeoutMs: 5000});
      ctx.assert(after.turns === counters.turns && after.effects === counters.effects, 'deleted_channel_no_new_work');
      const unrelated = await egRequest(ctx, lease, {path: `/v1/channels/${segment(lease.expected.unrelatedChannelId)}`});
      ctx.assert(digest(unrelated) === lease.expected.unrelatedChannelDigest, 'delete_preserves_unrelated_channel');
      await uiReceipt(ctx, page);
    });
  }),
];
