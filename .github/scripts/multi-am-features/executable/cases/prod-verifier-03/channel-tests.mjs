import {Blocked} from '../../runner.mjs';
import {define, acquire, openBrowser, egRequest, invoke, visible, goto, peer, segment,
  same, digest, uiReceipt, assertServingRequest} from './helpers.mjs';
import {channelReceipt, openListedChannel, openChannelNode, channelNodeName} from './channel-browser.mjs';
import {assertActivityPage, observeActivityStream, activityWait} from './activity-observer.mjs';

const prefix = '/api/event-gateway';
const text = value => typeof value === 'string' && value.length > 0;
const sourceSummary = 'Configuration check completed. External delivery was not tested.';
const unknownSummary = 'Test result is unknown. Check channel activity before sending another test.';
const countKeys = ['turns', 'effects', 'deliveries', 'ingress'];

export function validateNodeTestFixture(lease) {
  const e = lease.expected;
  if (![lease.recordId, lease.channelId, lease.channelName, e?.ownerSubject,
    e?.producerNodeName, e?.consumerNodeName, e?.consumerId].every(text)
    || e.consumerId === 'default' || !Number.isSafeInteger(e.sourceIndex) || e.sourceIndex < 0
    || !['channel', 'consumer'].every(kind => text(e[kind]?.agentId) && text(e[kind]?.firstText)
      && e[kind].firstText.length <= 200 && /^l[1-5]$/.test(e[kind]?.level ?? ''))
    || e.channel.firstText === e.consumer.firstText)
    throw new Blocked('node_test_distinct_source_consumer_fixture_required');
  for (const method of ['request', 'counters', 'nodeTestCaptures', 'quiesce', 'drain'])
    if (typeof lease[method] !== 'function') throw new Blocked('node_test_primitive_' + method.toLowerCase());
  return e;
}

export function assertNodeTestCounts(ctx, before, after, turns) {
  for (const value of [before, after]) ctx.assert(value && countKeys.every(key =>
    Number.isSafeInteger(value[key]) && value[key] >= 0), 'node_test_scoped_counters');
  ctx.assert(after.turns === before.turns + turns && after.effects === before.effects + turns
    && after.deliveries === before.deliveries && after.ingress === before.ingress,
  'node_test_exact_effects_without_platform_ingress_or_delivery');
}

// This fixture uses a Custom source with a retained authentication reference.
// Check the actual diagnostic scope and every independently specified check,
// including the deliberately unavailable external-delivery check.
export function assertSourceCheck(ctx, body, sourceIndex) {
  ctx.assert(body?.source_index === sourceIndex && body.platform === 'custom'
    && body.scope === 'configuration_only' && body.external_delivery_tested === false,
  'node_source_configuration_scope');
  const expected = [
    {id: 'configuration', label: 'Saved configuration', status: 'passed',
      detail: 'The saved channel definition passes schema and policy validation.'},
    {id: 'credential', label: 'Authentication configuration', status: 'passed',
      detail: 'The configured authentication references are present; caller credentials were not tested.'},
    {id: 'external_delivery', label: 'External delivery', status: 'unavailable',
      detail: 'Send a message from the configured platform to verify incoming delivery. This check sends no external messages.'},
  ];
  ctx.assert(same(body.checks, expected), 'node_source_exact_saved_configuration_checks');
}

// Guard all EG mutations for the lifetime of this case. Only one bodyless POST
// for the active UI action may pass. Unknown-outcome injection discards the real
// accepted response, never replaces the server response with a synthetic one.
export async function guardNodeTests(ctx, session) {
  const pending = new Set(), failures = [], operations = new Set();
  let active, forwarded = 0;
  const matcher = url => url.origin === session.origin && url.pathname.startsWith(prefix + '/');
  const dispatch = async route => {
    const request = route.request();
    if (['GET', 'HEAD', 'OPTIONS'].includes(request.method())) return route.fallback();
    const task = active, errors = []; let response, settled = false, result;
    try {
      ctx.assert(task && !task.cancelled && ++task.attempts === 1, 'node_test_single_mutation_attempt');
      ctx.assert(request.method() === 'POST' && request.url() === new URL(prefix + task.path, session.origin).href
        && (request.postData() === null || request.postData() === ''), 'node_test_exact_bodyless_request');
      const headers = await request.allHeaders();
      ctx.assert(!headers['idempotency-key'], 'node_test_no_invented_replay_key');
      forwarded++;
      response = await route.fetch({maxRetries: 0, maxRedirects: 0,
        timeout: Math.max(1, ctx.remainingMs?.() ?? 30000)});
      ctx.assert(response.status() === 200, 'node_test_real_response_status');
      const requestId = response.headers()['x-request-id'];
      ctx.assert(text(requestId), 'node_test_real_request_id');
      result = {requestId, body: await response.json(), dropped: task.drop === true};
      if (task.drop) await route.abort('failed');
      else await route.fulfill({response});
      settled = true;
    } catch (error) {errors.push(error);}
    if (!settled) try {await route.abort('failed');} catch (error) {errors.push(error);}
    if (response) try {await response.dispose();} catch (error) {errors.push(error);}
    if (errors.length) {
      const error = new AggregateError(errors, 'node_test_mutation_failed');
      failures.push(error); result = {error};
    }
    task?.resolve(result);
  };
  const handler = route => {
    const work = dispatch(route); pending.add(work);
    return work.finally(() => pending.delete(work));
  };
  await session.page.route(matcher, handler);
  ctx.defer('remove_node_test_guard', async () => {
    const errors = [];
    try {await session.page.unroute(matcher, handler);} catch (error) {errors.push(error);}
    const results = await Promise.allSettled([...pending]);
    errors.push(...results.filter(result => result.status === 'rejected').map(result => result.reason), ...failures);
    if (errors.length) throw new AggregateError(errors, 'node_test_guard_cleanup_failed');
  });
  return {forwarded: () => forwarded, check: () => {if (failures.length) throw failures[0];},
    async run({name, path, drop = false}, action) {
      ctx.assert(!active && !operations.has(name) && !ctx.signal?.aborted, 'node_test_operation_once');
      operations.add(name);
      let resolve, reject; const completed = new Promise((done, fail) => {resolve = done; reject = fail;});
      const task = {name, path, drop, resolve, attempts: 0}; active = task;
      const cancel = () => {task.cancelled = true; reject(Error('node_test_cancelled_or_expired'));};
      const timer = setTimeout(cancel, Math.max(1, ctx.remainingMs?.() ?? 30000));
      ctx.signal?.addEventListener('abort', cancel, {once: true});
      try {
        const [, result] = await Promise.all([Promise.resolve().then(action), completed]);
        if (result?.error) throw result.error;
        await invoke(session, 'networkIdle');
        if (failures.length) throw failures[0];
        const receipt = channelReceipt(ctx, await invoke(session, 'egEvidence'), result.requestId, 'POST');
        await ctx.observe('node_test_eg_request', async () => ({schema: 'multi-eg-request-v1', receipt}));
        return {...result, receipt};
      } finally {
        clearTimeout(timer); ctx.signal?.removeEventListener('abort', cancel);
        task.cancelled = true; active = undefined;
      }
    }};
}

export function assertNodeTestTurn(ctx, captured, result, expected) {
  ctx.assert(captured?.eg_request_id === result.requestId && captured.channel_id === expected.channelId
    && captured.consumer_id === expected.consumerId && captured.agent_id === expected.agentId
    && captured.binding_id === expected.bindingId && text(captured.conversation_id) && text(captured.response_id),
  'node_test_diagnostic_request_identity_join');
  const stamps = [result.receipt.started_at, captured.started_at, captured.accepted_at,
    captured.completed_at, result.receipt.ended_at].map(Date.parse);
  ctx.assert(stamps.every(Number.isFinite) && stamps.every((stamp, index) => !index || stamp >= stamps[index - 1]),
    'node_test_real_acceptance_completion_window');
  ctx.assert(Number.isSafeInteger(result.body?.accepted_after_ms) && result.body.accepted_after_ms >= 0
    && result.body.level === expected.level && result.body.first_text === expected.firstText
    && captured.first_text === expected.firstText, 'node_test_acceptance_and_first_text');
  const requests = captured.amRequests;
  ctx.assert(Array.isArray(requests) && requests.length > 0, 'node_test_actual_am_receipts');
  const ids = new Set();
  for (const request of requests) {
    assertServingRequest(ctx, request);
    const proof = request.owner_proof;
    ctx.assert(!ids.has(request.request_id) && request.agent_id_sha256 === digest(expected.agentId)
      && Number.isInteger(request.status) && Number.isFinite(request.elapsed_ms) && request.elapsed_ms >= 0
      && proof?.agent_sha256 === request.agent_id_sha256 && proof.request_id === request.request_id
      && proof.ingress === request.am && ['a', 'b'].includes(proof.owner) && proof.verdict === 'PASS'
      && Number.isSafeInteger(proof.owner_epoch) && proof.owner_epoch > 0
      && /^[a-f0-9]{64}$/.test(proof.owner_instance_sha256 ?? '')
      && (proof.owner === proof.ingress ? ['peer_read', 'explicit_ingress'].includes(proof.kind)
          && proof.owner_instance_sha256 === proof.ingress_instance_sha256
        : proof.kind === 'non_owner' && proof.owner_instance_sha256 !== proof.ingress_instance_sha256),
    'node_test_actual_am_owner_join');
    ids.add(request.request_id);
  }
  ctx.assert(requests.some(request => request.request_id === captured.admission_request_id
    && request.method === 'POST' && [200, 202].includes(request.status)
    && request.owner_proof.kind === 'non_owner' && request.owner_proof.owner !== request.am),
  'node_test_actual_non_owner_admission');
}

export function findNodeTestActivity(ctx, snapshot, turn) {
  assertActivityPage(ctx, snapshot);
  const matches = snapshot.items.flatMap(item => item.consumers).filter(consumer =>
    consumer.response_id === turn.response_id && consumer.consumer_id === turn.consumer_id
    && consumer.agent_id === turn.agent_id && consumer.conversation_id === turn.conversation_id);
  if (matches.length === 0) throw new Blocked('node_test_unknown_activity_reconciliation_missing');
  ctx.assert(matches.length === 1 && matches[0].state === 'completed'
    && Number.isFinite(Date.parse(matches[0].accepted_at))
    && Number.isFinite(Date.parse(matches[0].completed_at)) && matches[0].deliveries.length === 0,
  'node_test_activity_completed_without_external_delivery');
  return matches[0];
}

export const channelTestsCase = define(27, 'browser', ['browser', 'user-fixtures', 'eg_browser'], async ctx => {
  let lease, expected, session, guard, channel, baseline, source, stream, unknown, activity;
  const results = [], turns = [];
  const path = () => '/v1/channels/' + segment(lease.channelId);
  const panel = () => session.page.getByRole('region', {name: 'Node test', exact: true});
  const select = async name => {
    const dialog = await openChannelNode(ctx, session.page, name);
    await dialog.getByRole('navigation', {name: 'Configuration sections', exact: true})
      .getByRole('button', {name: 'Tests', exact: true}).click();
    await visible(ctx, panel(), 'node_test_section_open');
  };
  const run = async (kind, drop = false) => {
    const consumer = kind === 'consumer', e = consumer ? expected.consumer : expected.channel;
    const consumerId = consumer ? expected.consumerId : 'default';
    const bindingId = consumer ? channel.definition.routing.consumers.find(c => c.id === consumerId).provider.binding_id
      : channel.provider.binding_id;
    await select(consumer ? expected.consumerNodeName : channelNodeName(lease.channelName));
    await visible(ctx, panel().getByText(consumer
      ? 'Starts a short test turn with this consumer’s saved settings. It does not send a message to the external platform.'
      : 'Starts a short test turn with the default consumer. It does not test platform ingress or external message delivery.',
    {exact: true}), 'node_test_scope_visible');
    const before = await invoke(lease, 'counters');
    const result = await guard.run({name: drop ? 'unknown' : kind,
      path: path() + (consumer ? '/consumers/' + segment(consumerId) : '') + '/test', drop},
    () => panel().getByRole('button', {name: 'Send test event', exact: true}).click());
    await visible(ctx, panel().getByText(drop ? unknownSummary : e.firstText, {exact: true}),
      drop ? 'node_test_unknown_requires_activity' : 'node_test_first_text_rendered');
    assertNodeTestCounts(ctx, before, await invoke(lease, 'quiesce'), 1);
    const captures = await invoke(lease, 'nodeTestCaptures');
    ctx.assert(Array.isArray(captures) && captures.length === turns.length + 1, 'node_test_one_new_admission');
    ctx.assert(turns.every(previous => captures.some(capture => same(capture, previous))),
      'node_test_prior_captures_immutable');
    const matching = captures.filter(capture => capture.eg_request_id === result.requestId);
    ctx.assert(matching.length === 1, 'node_test_one_capture_for_request');
    const turn = matching[0];
    assertNodeTestTurn(ctx, turn, result, {...e, channelId: lease.channelId, consumerId, bindingId});
    ctx.assert(turns.every(previous => previous.response_id !== turn.response_id
      && previous.admission_request_id !== turn.admission_request_id), 'node_test_distinct_admissions');
    // The initiating event here is the real EG diagnostic HTTP request. It is
    // explicitly distinguished from a platform event; no platform ID is minted.
    await ctx.observe('node_test_am_evidence', async () => ({schema: 'multi-am-eg-evidence-v1',
      event_kind: 'node_test', event_identity: 'eg_request_id', event_sha256: digest(result.requestId),
      response_sha256: digest(turn.response_id), secrets_redacted: true,
      requests: structuredClone(turn.amRequests), eg_receipts: [structuredClone(result.receipt)]}));
    results.push(result); turns.push(structuredClone(turn)); return turn;
  };
  await ctx.step('step_1', async () => {
    lease = await acquire(ctx, 'channel_tests'); expected = validateNodeTestFixture(lease);
    channel = await egRequest(ctx, lease, {path: path()});
    ctx.assert(channel.channel_id === lease.channelId && channel.record_id === lease.recordId
      && channel.definition?.name === lease.channelName && channel.definition.owner?.subject === expected.ownerSubject
      && channel.definition.sources?.[expected.sourceIndex]?.type === 'custom'
      && channel.provider?.level === expected.channel.level && text(channel.provider.binding_id)
      && channel.definition.routing?.consumers?.some(c => c.id === expected.consumerId && c.pending !== true
        && c.provider?.level === expected.consumer.level && text(c.provider.binding_id)),
    'node_test_owned_saved_source_and_consumer');
    baseline = await invoke(lease, 'counters');
    const initial = await invoke(lease, 'nodeTestCaptures');
    ctx.assert(Array.isArray(initial) && initial.length === 0, 'node_test_fresh_diagnostic_capture');
    session = await openBrowser(ctx, 'user', {channelTests: true});
    for (const method of ['egEvidence', 'switchEG', 'networkIdle'])
      if (typeof session[method] !== 'function') throw new Blocked('node_test_actual_eg_browser_required');
    ctx.assert(session.identity?.subject === expected.ownerSubject, 'node_test_actual_owner_session');
    guard = await guardNodeTests(ctx, session);
    const url = new URL(prefix + path() + '/activity/stream', session.origin).href;
    stream = await observeActivityStream(ctx, session.page, url, value => assertActivityPage(ctx, value));
    await openListedChannel(ctx, session, channel, () => goto(session.page, '/event-gateway'));
    await select(expected.producerNodeName);
    await visible(ctx, panel().getByText('Checks the saved configuration and credential references. To verify incoming delivery, send a message from this platform.',
      {exact: true}), 'node_source_configuration_scope_visible');
    source = await guard.run({name: 'source', path: path() + '/sources/' + expected.sourceIndex + '/test'},
      () => panel().getByRole('button', {name: 'Check configuration', exact: true}).click());
    assertSourceCheck(ctx, source.body, expected.sourceIndex);
    await visible(ctx, panel().getByText(sourceSummary, {exact: true}), 'node_source_no_delivery_claim');
    for (const check of source.body.checks) {
      const item = panel().getByRole('listitem').filter({has: session.page.getByText(check.label, {exact: true})});
      await visible(ctx, item.getByText(`${check.status === 'passed' ? 'Passed' : 'Unavailable'} · ${check.detail}`,
        {exact: true}), 'node_source_rendered_check_detail');
    }
    assertNodeTestCounts(ctx, baseline, await invoke(lease, 'quiesce'), 0);
    ctx.assert((await invoke(lease, 'nodeTestCaptures')).length === 0, 'node_source_starts_no_am_work');
    await run('channel');
    await session.switchEG(peer(source.receipt.replica));
    await run('consumer');
    ctx.assert(results[1].receipt.replica !== source.receipt.replica, 'node_test_actual_peer_eg');
    ctx.assert(turns[0].conversation_id !== turns[1].conversation_id, 'node_test_consumer_partition_isolation');
  });
  await ctx.step('step_2', async () => {
    // Results remain attached to their selected node; returning to a producer
    // must not show a consumer's first-text result or cause another POST.
    await select(expected.producerNodeName);
    await visible(ctx, panel().getByText(sourceSummary, {exact: true}), 'node_source_result_retained_separately');
    ctx.assert(await panel().getByText(expected.consumer.firstText, {exact: true}).count() === 0,
      'node_source_not_consumer_result');
    for (const [kind, name] of [['channel', channelNodeName(lease.channelName)], ['consumer', expected.consumerNodeName]]) {
      await select(name); await visible(ctx, panel().getByText(expected[kind].firstText, {exact: true}),
        'node_test_selected_first_text_retained');
    }
    unknown = await run('channel', true);
    // Reconnect the real Activity stream after the unknown result. A fresh
    // snapshot, rather than a fixture verdict, must identify the admitted turn.
    const next = stream.frames.length;
    await session.switchEG(peer(results.at(-1).receipt.replica));
    await openListedChannel(ctx, session, channel, () => goto(session.page, '/event-gateway'));
    activity = await stream.wait(frame => frame.index >= next, 'node_test_activity_readback_missing');
    const receipts = await activityWait(ctx, async () => {
      const values = await invoke(session, 'egEvidence');
      return values.some(value => value.request_id === activity.connection.requestId) ? values : null;
    }, 'node_test_activity_serving_receipt_missing');
    const receipt = channelReceipt(ctx, receipts, activity.connection.requestId);
    ctx.assert(receipt.replica !== results.at(-1).receipt.replica, 'node_test_activity_actual_peer_read');
    await ctx.observe('node_test_activity_read', async () => ({schema: 'multi-eg-request-v1', receipt}));
    await session.page.getByRole('button', {name: 'Activity', exact: true}).click();
    const activityPanel = session.page.getByRole('dialog', {name: 'Channel settings', exact: true})
      .locator('section[aria-label="Activity"] .channel-activity');
    await visible(ctx, activityPanel, 'node_test_activity_inspected');
    await visible(ctx, activityPanel.getByRole('status').filter({hasText: /^Live activity$/}),
      'node_test_activity_live');
    guard.check(); ctx.assert(guard.forwarded() === 4, 'node_test_unknown_not_resubmitted');
    const finality = await invoke(lease, 'drain');
    ctx.assert(finality.pending === 0 && finality.unknown === 0 && finality.lateEffects === 0, 'node_test_work_drained');
    assertNodeTestCounts(ctx, baseline, await invoke(lease, 'counters'), 3);
    const finalCaptures = await invoke(lease, 'nodeTestCaptures');
    ctx.assert(Array.isArray(finalCaptures) && finalCaptures.length === 3
      && turns.every(turn => finalCaptures.some(capture => same(capture, turn))),
    'node_test_readback_no_new_admission');
    const saved = await egRequest(ctx, lease, {path: path(), replica: receipt.replica});
    ctx.assert(saved.version === channel.version && same(saved.definition, channel.definition),
      'node_test_diagnostics_preserve_saved_configuration');
    await ctx.observe('node_test_reconciliation', async () => ({scope: 'diagnostic_test',
      response_sha256: digest(unknown.response_id), activity_sha256: digest(activity.page),
      browser_posts: guard.forwarded(), pending: finality.pending, unknown: finality.unknown,
      late_effects: finality.lateEffects, matching_response_count: activity.page.items.flatMap(item =>
        item.consumers).filter(c => c.response_id === unknown.response_id).length}));
    // Current test_turn.rs does not retain an Activity event. Absence remains
    // BLOCKED; a provider capture cannot stand in for the user-facing readback.
    await uiReceipt(ctx, session.page);
    findNodeTestActivity(ctx, activity.page, unknown);
    await visible(ctx, activityPanel.getByText(unknown.agent_id, {exact: true}), 'node_test_activity_agent_visible');
    await visible(ctx, activityPanel.getByText(unknown.conversation_id, {exact: true}), 'node_test_activity_conversation_visible');
  });
  await ctx.step('expected_result', async () => {
    guard.check(); assertSourceCheck(ctx, source.body, expected.sourceIndex);
    assertNodeTestCounts(ctx, baseline, await invoke(lease, 'quiesce'), 3);
    findNodeTestActivity(ctx, activity.page, unknown);
    ctx.assert(guard.forwarded() === 4, 'node_test_no_replay_after_reconciliation');
    const saved = await egRequest(ctx, lease, {path: path(), replica: peer(ctx.am)});
    ctx.assert(saved.version === channel.version && same(saved.definition, channel.definition),
      'node_test_diagnostics_preserve_saved_configuration');
    await uiReceipt(ctx, session.page);
  });
});
