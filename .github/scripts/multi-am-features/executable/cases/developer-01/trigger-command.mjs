import {setup, settingsSession, request, read, required, P, id, equal, sameSettings} from './common.mjs';
import {launchIdentity, launchInventory, launchPair, allPending, isPaused, isReady,
  readyResources, publishedTrigger} from './trigger-launch-proof.mjs';
import {triggerAuditRecords, triggerAuditJoin} from './trigger-command-audit.mjs';

export async function triggerCommandScenario(ctx, action, {poll, ownedIntent}) {
  const f = await setup(ctx, 'trigger_commands'), intent = f.launchIntent;
  required(['pause', 'resume'].includes(action) && typeof f.triggerId === 'string' && typeof f.publicId === 'string'
    && typeof f.adminSubject === 'string' && f.adminSubject.trim() && typeof f.custody === 'function'
    && intent?.actor === 'user' && intent.method === 'POST' && typeof intent.key === 'string' && intent.key.trim()
    && intent.path === '/api/v1/triggers/public/' + id(f.publicId) + '/launches'
    && equal(intent.body, {start_if_ready: true, choice: 'new'}), 'admin_trigger_command_fixture_missing');
  id(f.triggerId);
  const state = await settingsSession(ctx, f), priorAuditIds = new Set();
  let admitted, command, cleanupResource;
  // Reuse original-intent response custody and join the same cleanup attempt
  // from the case and runner. Unknown cleanup cannot be retried after restore.
  const scoped = {...f, custody: async (...args) => {
    const custody = await f.custody(...args);
    required(typeof custody?.cleanup === 'function', 'admin_trigger_command_cleanup_missing');
    let job; cleanupResource = () => job ??= Promise.resolve().then(() => custody.cleanup());
    return {...custody, cleanup: cleanupResource};
  }};
  const createPaused = async () => {
    const response = await ownedIntent(ctx, scoped, intent, {actor: 'user', am: ctx.am, allowed: [201, 202], requireCapture: true});
    admitted = structuredClone(response.body); launchIdentity(ctx, f, admitted);
    ctx.assert(admitted.state === 'resolving' && allPending(admitted), 'admin_trigger_initial_launch');
    const pair = await poll(async () => {
      const current = await launchPair(ctx, f, admitted);
      ctx.assert(current.every(({launch}) => allPending(launch)), 'admin_trigger_paused_resources_changed');
      ctx.assert(current.every(({launch}) => launch.blocked_reason == null || isPaused(launch)), 'admin_trigger_wrong_block');
      return current;
    }, rows => rows.every(({launch}) => isPaused(launch)), 'admin_trigger_pause_timeout');
    ctx.assert(pair.every(({launch}) => isPaused(launch) && allPending(launch)), 'admin_trigger_pause_unproven');
    await ctx.observe('admin_trigger_command_paused_launch', async () => pair);
    for (const am of ['a', 'b']) await launchInventory(ctx, f, am, [admitted.id]);
  };
  const commandPolicy = async () => {
    for (const am of ['a', 'b']) {
      const current = await read(ctx, P + '/settings', am);
      ctx.assert(current.revision === command.body.revision && sameSettings(current.settings, command.body.settings),
        'admin_trigger_command_peer');
    }
  };
  await ctx.step('step_1', async () => {
    await publishedTrigger(ctx, f);
    for (const am of ['a', 'b']) await launchInventory(ctx, f, am);
    const initial = {...state.original.settings.triggers, enabled: true, orchestration_paused: action === 'resume'};
    if (!equal(initial, state.original.settings.triggers)) await state.patch('triggers', initial);
    if (action === 'resume') await createPaused();
    const before = await read(ctx, P + '/settings');
    ctx.assert(equal(before.settings.triggers, initial), 'admin_trigger_command_initial_policy');
    for (const am of ['a', 'b']) {
      const rows = await triggerAuditRecords(ctx, f.adminSubject, am);
      for (const row of rows) priorAuditIds.add(row.id);
    }
    const desired = {...before.settings, triggers: {...before.settings.triggers, orchestration_paused: action === 'pause'}};
    state.own(desired);
    try {command = await request(ctx, 'POST', P + '/triggers/' + action);}
    catch (error) {
      if (error.response) {
        try {await ctx.observe('admin_trigger_uncertain_command_receipt', async () => error.response);}
        catch (captureError) {throw new AggregateError([error, captureError], 'admin_trigger_command_capture_failed');}
      }
      throw error;
    }
    await ctx.observe('admin_trigger_command_receipt', async () => command);
    ctx.assert(command.status === 200 && command.timing?.am === ctx.am && command.timing.request_id?.trim(),
      'admin_trigger_command_ingress');
    ctx.assert(command.body?.settings && Number.isSafeInteger(command.body.revision) && command.body.revision > before.revision
      && sameSettings(command.body.settings, desired), 'admin_trigger_command_receipt_policy');
    await commandPolicy();
  });
  await ctx.step('step_2', async () => {
    if (action === 'pause') await createPaused();
    else {
      const completed = await poll(() => launchPair(ctx, f, admitted),
        rows => rows.every(({launch}) => isReady(launch)), 'admin_trigger_resume_timeout');
      readyResources(ctx, completed);
      await ctx.observe('admin_trigger_command_completed_launch', async () => completed);
    }
    await commandPolicy();
    for (const am of ['a', 'b']) await launchInventory(ctx, f, am, [admitted.id]);
    await triggerAuditJoin(ctx, f, command, action, priorAuditIds);
    await cleanupResource();
    // A pause proof must cancel the original blocked Launch before restoring a
    // policy that could resume it. Completed Launch cleanup retires its roots.
    for (const am of ['a', 'b']) {
      const final = (await request(ctx, 'GET', '/api/v1/launches/' + id(admitted.id), undefined, {actor: 'user', am})).body;
      launchIdentity(ctx, f, final, admitted);
      ctx.assert(action === 'pause' ? final.state === 'cancelled' && allPending(final) : isReady(final),
        'admin_trigger_command_cleanup_state');
    }
    await state.restore();
    ctx.assert(sameSettings((await read(ctx, P + '/settings')).settings, state.original.settings),
      'admin_trigger_command_restore');
  });
  await ctx.step('expected_result', async () => {
    for (const am of ['a', 'b']) {
      ctx.assert(sameSettings((await read(ctx, P + '/settings', am)).settings, state.original.settings),
        'admin_trigger_command_restore');
      await launchInventory(ctx, f, am, [admitted.id]);
    }
    await triggerAuditJoin(ctx, f, command, action, priorAuditIds);
  });
}
