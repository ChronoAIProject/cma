import {caseOf, equal, key, otherAM, pages, request, segment, take} from './common.mjs';

export const launchCases = [42,43,67,68].map(number => caseOf(number, 'global', ['api-fixtures'], async ctx => {
  let fixture, initialFacts, cancellation, cancelKey, deleteKey;
  const launchPath = () => '/api/v1/launches/' + segment(fixture.launchId);
  // Published Triggers cannot be deleted. The deletion fixture is a separate
  // unpublished draft; the public placement belongs to the published Trigger.
  const triggerPath = () => '/api/v1/triggers/' + segment(fixture.draftTriggerId);
  const entryPath = () => '/api/v1/triggers/public/' + segment(fixture.publicId) + '/open/' + segment(fixture.placementId);
  async function open() {
    // The parent transport must disable automatic redirect following and retain
    // safe response headers. This verifies the public API entry, not a UI flow.
    const reply = await fixture.withManualRedirects(() => request(ctx, 'GET', entryPath(), undefined, {allowed: [303]}));
    ctx.assert(reply.headers?.location === '/t/' + fixture.publicId + '/' + fixture.placementId,
      'placement_redirect_exact_target');
    ctx.assert(reply.headers?.['cache-control'] === 'private, no-store', 'placement_redirect_private');
  }
  await ctx.step('step_1', async () => {
    fixture = await take(ctx, 'launches');
    ctx.assert(typeof fixture.draftTriggerId === 'string' && fixture.draftTriggerId !== fixture.triggerId,
      'separate_unpublished_deletion_fixture');
    initialFacts = await fixture.readNativeFacts();
    const list = await pages(ctx, '/api/v1/launches', {nextField: 'next_cursor'});
    ctx.assert(list.some(value => value.id === fixture.launchId && value.trigger_id === fixture.triggerId), 'owned_launch_listed');
    await open();
    const openedFacts = await fixture.readNativeFacts();
    ctx.assert(equal(openedFacts, initialFacts), 'placement_get_does_not_launch_work');
    cancelKey = key(); deleteKey = key();
    cancellation = await request(ctx, 'POST', launchPath() + '/cancel', undefined, {key: cancelKey});
    ctx.assert(cancellation.body.id === fixture.launchId && cancellation.body.trigger_id === fixture.triggerId
      && cancellation.body.state === 'cancelled', 'launch_cancelled_exact_resource');
    if (number !== 67) {
      const removed = await request(ctx, 'DELETE', triggerPath(), undefined, {key: deleteKey, allowed: [204]});
      ctx.assert(removed.status === 204, 'trigger_deletion_admitted');
    }
  });
  await ctx.step('step_2', async () => {
    if (number === 42) {
      const reply = await request(ctx, 'GET', '/api/v1/launches');
      ctx.assert(reply.body.items.some(value => value.id === fixture.launchId && value.state === 'cancelled'), 'exact_launch_list');
    } else if (number === 43) {
      const replay = await request(ctx, 'POST', launchPath() + '/cancel', undefined, {key: cancelKey, am: otherAM(ctx.am)});
      ctx.assert(equal(replay.body, cancellation.body), 'launch_cancel_original_receipt');
    } else if (number === 68) {
      const replay = await request(ctx, 'DELETE', triggerPath(), undefined, {key: deleteKey, allowed: [204], am: otherAM(ctx.am)});
      ctx.assert(replay.status === 204, 'trigger_delete_original_receipt');
    } else {
      await open();
      const removed = await request(ctx, 'DELETE', triggerPath(), undefined, {key: deleteKey, allowed: [204]});
      ctx.assert(removed.status === 204, 'entry_fixture_trigger_deleted');
    }
  });
  await ctx.step('step_3', async () => {
    const denied = await request(ctx, 'POST', launchPath() + '/cancel', undefined,
      {actor: 'outsider', key: key(), allowed: [403,404], am: otherAM(ctx.am)});
    ctx.assert(typeof denied.body.code === 'string', 'launch_other_owner_refused');
    const stale = await request(ctx, 'POST', '/api/v1/launches/' + segment(fixture.staleLaunchId) + '/cancel',
      undefined, {key: key(), allowed: [404]});
    ctx.assert(typeof stale.body.code === 'string', 'stale_launch_refused');
    const staleDelete = await request(ctx, 'DELETE', '/api/v1/triggers/' + segment(fixture.otherOwnerTriggerId),
      undefined, {key: key(), allowed: [403,404]});
    ctx.assert(typeof staleDelete.body.code === 'string', 'other_owner_trigger_not_deleted');
    const list = await pages(ctx, '/api/v1/launches', {actor: 'outsider', am: otherAM(ctx.am), nextField: 'next_cursor'});
    ctx.assert(!list.some(value => value.id === fixture.launchId), 'launch_owner_list_isolation');
  });
  await ctx.step('expected_result', async () => {
    const final = await request(ctx, 'GET', launchPath(), undefined, {am: otherAM(ctx.am)});
    ctx.assert(final.body.id === fixture.launchId && final.body.state === 'cancelled', 'launch_durable_cancellation');
    const removed = await request(ctx, 'GET', triggerPath(), undefined, {allowed: [404], am: otherAM(ctx.am)});
    ctx.assert(typeof removed.body.code === 'string', 'trigger_durable_deletion');
    const facts = await ctx.observe('launch_final_native_facts', () => fixture.readNativeFacts());
    ctx.assert(equal(facts.agent_ids, initialFacts.agent_ids) && facts.active_jobs === 0
      && equal(facts.other_owner_state, initialFacts.other_owner_state), 'launch_no_child_leak_or_foreign_effect');
  });
}));
