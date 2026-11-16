import {define, acquire, openBrowser, visible, uiReceipt, goto, invoke, same, digest,
  poll, peer, segment, egRequest} from './helpers.mjs';
import {validServingReceipt} from '../../serving.mjs';
import {validEgReceipt} from '../../evidence-scope.mjs';
import {Blocked} from '../../runner.mjs';

const pick = async (page, label, value) => {
  await page.getByRole('button', {name: label, exact: true}).click();
  await page.getByRole('menuitemradio', {name: value, exact: true}).click();
};
const next = page => page.getByRole('button', {name: 'Continue', exact: true}).click();
const storageKey = (page, owner) => `cma:bot-creation:${JSON.stringify([new URL(page.url()).origin, owner])}`;
const savedIntent = (page, owner) => page.evaluate(key => {
  const raw = sessionStorage.getItem(key);
  return raw === null ? null : JSON.parse(raw);
}, storageKey(page, owner));

export function wizardBody(wizard) {
  return {name: wizard.name, connection_ref: wizard.connection.ref,
    ...(wizard.image.id === 'system-default' ? {} : {image: {id: wizard.image.id}}),
    ...(wizard.repository ? {repository: {url: wizard.repository}} : {}),
    agent: {model: wizard.model, reasoning_effort: wizard.effort,
      permission_mode: wizard.permission, system_prompt: wizard.prompt,
      skills: [{id: wizard.skill.id, version: wizard.skill.version}],
      permissions: {services: [wizard.service.id]}},
    admission: {type: 'scoped', senders: {type: 'allowlist', ids: wizard.senders},
      chats: {type: 'open'}, groups: 'deny'},
    reply: {auto_final: wizard.autoFinal, ack_after_ms: wizard.ackAfterMs}};
}

// Intercept only this page's real POST. Retain cleanup custody before forwarding
// once through the parent's AM proxy, then lose the accepted response at the
// browser boundary. No product response is fabricated and no mutation is retried.
export async function armConsumerCreation(ctx, page, lease, body) {
  let attempts = 0, settled;
  const signal = ctx.signal;
  const pending = new Set(), failures = [];
  const completed = new Promise(resolve => {settled = resolve;});
  const matcher = url => url.origin === new URL(page.url()).origin
    && ['/api/v1/bots', '/api/agent-manager/v1/bots'].includes(url.pathname);
  const aborted = () => {
    if (attempts === 0) settled({error: signal.reason ?? Error('consumer_creation_cancelled')});
  };
  signal?.addEventListener('abort', aborted, {once: true});
  if (signal?.aborted) aborted();
  const dispatch = async route => {
    const request = route.request();
    if (request.method() !== 'POST') return route.fallback();
    const attempt = ++attempts;
    let outcome;
    const errors = [];
    try {
      ctx.assert(attempt === 1, 'consumer_create_duplicate_attempt');
      const actual = request.postDataJSON();
      const key = (await request.allHeaders())['idempotency-key'];
      ctx.assert(typeof key === 'string' && key.length > 0, 'consumer_creation_key_required');
      ctx.assert(same(actual, body), 'consumer_wizard_request_exact');
      const saved = await savedIntent(page, lease.expected.ownerSubject);
      ctx.assert(saved?.version === 1 && saved.intent?.key === key
        && same(saved.intent.body, body), 'consumer_intent_retained_before_dispatch');
      const custody = await invoke(lease, 'retainCreationIntent', {key, body: actual});
      ctx.assert(custody?.retained === true && custody.key_sha256 === digest(key)
        && custody.body_sha256 === digest(actual), 'consumer_creation_cleanup_custody');
      const remaining = ctx.remainingMs?.() ?? 60000;
      ctx.assert(!ctx.signal?.aborted && remaining > 0, 'consumer_creation_cancelled');
      const response = await route.fetch({maxRetries: 0, maxRedirects: 0, timeout: remaining});
      try {
        ctx.assert(response.status() === 201, 'consumer_create_accepted');
        const accepted = await response.json();
        const requestId = response.headers()['x-request-id'];
        ctx.assert(typeof accepted.bot_id === 'string' && accepted.bot_id.length > 0
          && ['creating', 'active'].includes(accepted.state), 'consumer_create_durable_id');
        ctx.assert(typeof requestId === 'string' && requestId.length > 0,
          'consumer_creation_response_request_id');
        outcome = {accepted, requestId, key};
      } catch (error) {errors.push(error);}
      try {await response.dispose();} catch (error) {errors.push(error);}
    } catch (error) {errors.push(error);}
    try {await route.abort('failed');} catch (error) {errors.push(error);}
    if (errors.length) {
      const error = errors.length === 1 ? errors[0] : new AggregateError(errors, 'consumer_creation_fault_failed');
      failures.push(error); outcome = {error};
    }
    if (attempt === 1) settled(outcome);
  };
  const handler = route => {
    const work = dispatch(route); pending.add(work);
    return work.finally(() => pending.delete(work));
  };
  await page.route(matcher, handler);
  ctx.defer('remove_consumer_creation_route', async () => {
    signal?.removeEventListener('abort', aborted);
    await page.unroute(matcher, handler);
    await Promise.all([...pending]);
    if (failures.length) throw new AggregateError(failures, 'consumer_creation_route_failed');
  });
  return {attempts: () => attempts, async result() {
    const outcome = await completed;
    if (outcome.error) throw outcome.error;
    return outcome;
  }};
}

export function creationReceipt(ctx, evidence, requestId) {
  const matches = evidence.requests?.filter(r => r.request_id === requestId && r.method === 'POST') ?? [];
  ctx.assert(matches.length === 1 && matches[0].status === 201 && validServingReceipt(matches[0]),
    'consumer_create_actual_am_receipt');
  return matches[0];
}

export function assertConsumerChannel(ctx, channel, active, body) {
  const definition = channel.definition;
  ctx.assert(channel.channel_id === active.channel.channel_id && channel.record_id === active.channel.record_id
    && definition?.connection_ref === body.connection_ref && definition.name === body.name,
  'consumer_provision_channel_identity');
  ctx.assert(definition.owner?.subject === active.owner && definition.profile?.metadata?.owner?.subject === active.owner,
    'consumer_provision_channel_owner');
  ctx.assert(Array.isArray(definition.sources) && definition.sources.length === 1
    && same(definition.sources[0].admission, body.admission), 'consumer_provision_channel_admission');
  ctx.assert(definition.reply?.auto_final === body.reply.auto_final
    && definition.reply?.ack_after_ms === body.reply.ack_after_ms
    && same(definition.reply?.sinks, [{type: 'source'}]), 'consumer_provision_channel_reply');
}

async function profile(ctx, kind, pin, am) {
  ctx.assert(typeof pin?.id === 'string' && Number.isSafeInteger(pin.revision) && pin.revision > 0,
    'consumer_profile_pin_required');
  const {body} = await ctx.request({path: `/api/v1/${kind}-profiles/${segment(pin.id)}/revisions/${pin.revision}`, am});
  ctx.assert(body.profile_id === pin.id && body.revision === pin.revision, 'consumer_exact_profile_revision');
  return body.content;
}

export const consumerWizardCase = define(48, 'browser', ['browser', 'user-fixtures', 'eg_browser'], async ctx => {
  let lease, session, wizard, body, submission, accepted, ingress, active, channel;
  await ctx.step('step_1', async () => {
    lease = await acquire(ctx, 'consumer_create'); wizard = lease.wizard; body = wizardBody(wizard);
    ctx.assert(wizard.model && wizard.prompt && wizard.senders?.length > 0
      && wizard.service.id && wizard.skill.version && Number.isSafeInteger(wizard.ackAfterMs),
    'consumer_wizard_configured_fixture');
    session = await openBrowser(ctx, 'user', {consumerCreation: true}); const page = session.page;
    if (typeof session.egEvidence !== 'function') throw new Blocked('consumer_actual_eg_browser_binding_required');
    await goto(page, '/bots/new');
    await visible(ctx, page.getByRole('heading', {name: 'Choose an Event Gateway connection', exact: true}),
      'consumer_connection_step');
    const connections = await ctx.request({path: '/api/v1/bots/connections', am: ctx.am});
    ctx.assert(Array.isArray(connections.body) && connections.body.some(c =>
      c.connection_ref === wizard.connection.ref && c.label === wizard.connection.label), 'consumer_ready_connection');
    await page.getByLabel('Consumer name', {exact: true}).fill(wizard.name);
    await pick(page, 'Event Gateway connection', wizard.connection.label); await next(page);
    await visible(ctx, page.getByRole('heading', {name: 'Configure the Agent inline', exact: true}),
      'consumer_agent_step');
    const image = page.getByRole('radio', {name: wizard.image.accessibleName, exact: true});
    await image.check(); ctx.assert(await image.isChecked(), 'consumer_runtime_image_selected');
    if (wizard.repository) await page.getByLabel('GitHub repository workspace (optional)', {exact: true}).fill(wizard.repository);
  });
  await ctx.step('step_2', async () => {
    const page = session.page;
    await page.getByLabel('Model', {exact: true}).fill(wizard.model);
    await pick(page, 'Reasoning effort', wizard.effort);
    await pick(page, 'Permission level', wizard.permission === 'guarded' ? 'Guarded' : 'Autonomous');
    await page.getByLabel('System prompt', {exact: true}).fill(wizard.prompt);
    await page.getByRole('list', {name: 'Services', exact: true}).getByRole('listitem')
      .filter({has: page.getByRole('link', {name: wizard.service.name, exact: true})}).getByRole('checkbox').check();
    await page.getByLabel('Search Ornn skills', {exact: true}).fill(wizard.skill.name);
    await page.getByRole('button', {name: `Add ${wizard.skill.name}`, exact: true}).click();
    await pick(page, `Version for ${wizard.skill.name}`, wizard.skill.version); await next(page);
    await pick(page, 'Admission policy', 'Selected senders in private chats');
    await page.getByLabel('Sender IDs', {exact: true}).fill(wizard.senders.join(', '));
    await page.getByRole('checkbox', {name: 'Send the Agent’s final answer automatically', exact: true})
      .setChecked(wizard.autoFinal);
    await page.getByLabel('Acknowledgement delay (seconds)', {exact: true}).fill(String(wizard.ackAfterMs / 1000));
    await next(page);
    await visible(ctx, page.getByRole('heading', {name: 'Review and create', exact: true}), 'consumer_review_step');
    await visible(ctx, page.getByText(wizard.name, {exact: true}), 'consumer_review_name');
    submission = await armConsumerCreation(ctx, page, lease, body);
    await page.getByRole('button', {name: 'Create consumer', exact: true}).click();
    accepted = await submission.result();
    await visible(ctx, page.getByText(/Creation has not been confirmed\. Recover the same request/),
      'consumer_unknown_completion_visible');
    const saved = await savedIntent(page, lease.expected.ownerSubject);
    ctx.assert(saved?.intent?.key === accepted.key && same(saved.intent.body, body),
      'consumer_unknown_completion_retains_exact_intent');
    const evidence = await invoke(ctx.fixture('browser'), 'evidence', page);
    ingress = creationReceipt(ctx, evidence, accepted.requestId).am;
    await ctx.observe('consumer_creation_intent', async () => ({key_sha256: digest(accepted.key),
      body_sha256: digest(body), bot_sha256: digest(accepted.accepted.bot_id), response_lost: true}));
  });
  await ctx.step('step_3', async () => {
    const botId = accepted.accepted.bot_id;
    active = await poll(ctx, async () => {
      const response = await ctx.request({path: `/api/v1/bots/${segment(botId)}`, am: peer(ingress)});
      ctx.assert(response.body.bot_id === botId, 'consumer_poll_retained_durable_id');
      ctx.assert(!['failed', 'deleted', 'deleting'].includes(response.body.state), 'consumer_provision_failed');
      return response.body;
    }, value => value.state === 'active', 'consumer_provision_timeout');
    ctx.assert(active.name === body.name && active.owner === lease.expected.ownerSubject
      && active.connection_ref === body.connection_ref && active.channel?.channel_id, 'consumer_provision_owner_connection');
    const agent = await profile(ctx, 'agent', active.profile, peer(ingress));
    ctx.assert(agent.model === wizard.model && agent.thinking_effort === wizard.effort
      && agent.permission_level === wizard.permission && agent.system_prompt === wizard.prompt
      && same(agent.skills, [{source: 'ornn', id: wizard.skill.id, version: wizard.skill.version}]),
    'consumer_provision_agent_configuration');
    const sandbox = await profile(ctx, 'sandbox', active.sandbox_profile, peer(ingress));
    ctx.assert(same([...sandbox.nyxid_services].sort(), [...lease.expected.sandboxServices].sort())
      && same(sandbox.runtime_image ?? null, lease.expected.runtimeImage), 'consumer_provision_services_image');
    const workspace = await profile(ctx, 'workspace', active.workspace_profile, peer(ingress));
    ctx.assert(wizard.repository ? workspace.location?.type === 'git_repository'
      && workspace.location.url === wizard.repository : workspace.location?.type === 'directory',
    'consumer_provision_workspace');
    channel = await egRequest(ctx, lease, {path: `/v1/channels/${segment(active.channel.channel_id)}`, replica: peer(ctx.am)});
    assertConsumerChannel(ctx, channel, active, body);
    await goto(session.page, `/bots/${segment(botId)}`);
    await visible(ctx, session.page.getByRole('heading', {name: wizard.name, exact: true}), 'consumer_detail_open');
    await session.page.getByRole('button', {name: 'Open Event Gateway', exact: true}).click();
    const [selectedResponse] = await Promise.all([
      session.page.waitForResponse(response => response.request().method() === 'GET'
        && new URL(response.url()).pathname === `/api/event-gateway/v1/channels/${segment(channel.channel_id)}`),
      session.page.getByRole('tree', {name: 'Channels', exact: true}).getByRole('treeitem')
        .filter({has: session.page.getByText(channel.definition.name, {exact: true})}).getByRole('button').click(),
    ]);
    await visible(ctx, session.page.getByRole('heading', {name: channel.definition.name, exact: true}),
      'consumer_linked_channel_open');
    const requestId = selectedResponse.headers()['x-request-id'];
    ctx.assert(selectedResponse.status() === 200 && typeof requestId === 'string', 'consumer_channel_browser_response');
    const egReceipts = await invoke(session, 'egEvidence');
    const receipts = egReceipts.filter(receipt => receipt.request_id === requestId);
    ctx.assert(receipts.length === 1 && validEgReceipt(receipts[0]), 'consumer_channel_browser_actual_eg_receipt');
    await ctx.observe('consumer_channel_browser_eg_receipt', async () => ({schema: 'multi-eg-request-v1', receipt: receipts[0]}));
  });
  await ctx.step('expected_result', async () => {
    const counts = await invoke(lease, 'creationRecords', {key: accepted.key});
    ctx.assert(submission.attempts() === 1 && counts.botIds?.length === 1 && counts.botIds[0] === active.bot_id
      && counts.channelIds?.length === 1 && counts.channelIds[0] === channel.channel_id,
    'consumer_create_single_allocation');
    ctx.assert(same(counts.profilePins, {agent: active.profile, sandbox: active.sandbox_profile,
      workspace: active.workspace_profile}), 'consumer_created_resource_identity');
    const saved = await savedIntent(session.page, lease.expected.ownerSubject);
    ctx.assert(saved?.intent?.key === accepted.key && same(saved.intent.body, body),
      'consumer_reconciliation_preserves_unknown_intent');
    await uiReceipt(ctx, session.page);
  });
});
