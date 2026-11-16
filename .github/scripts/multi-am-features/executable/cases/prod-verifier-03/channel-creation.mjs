import {randomBytes} from 'node:crypto';
import {define, acquire, openBrowser, invoke, visible, goto, peer, same, digest, uiReceipt, segment} from './helpers.mjs';
import {Blocked} from '../../runner.mjs';
import {channelRead, channelReceipt, openListedChannel} from './channel-browser.mjs';

const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const hash = /^sha256:[0-9a-f]{64}$/;
const pick = async (page, label, name) => {
  await page.getByRole('button', {name: label, exact: true}).click();
  await page.getByRole('menuitemradio', {name, exact: true}).click();
};

export function channelDraft(lease, capabilities) {
  const draft = lease.draft;
  return {schema_version: 1, name: draft.name,
    profile: {schema_version: 1, metadata: {name: draft.profileName, description: draft.description,
      owner: {kind: 'human', subject: lease.expected.ownerSubject}, provider: draft.provider},
    model: draft.model, permission: draft.permission,
    context: {skills: [], system_prompt: draft.systemPrompt, mcp_servers: []}},
    provider: {slug: draft.provider, target: {type: 'profile'}},
    sources: [{type: 'custom', auth: {type: 'hmac', key_id: draft.hmacKeyId}, prompt_template: draft.promptTemplate}],
    partition: {version: 1, key: 'conversation_and_sender'},
    reply: {auto_final: draft.autoFinal, ack_after_ms: draft.ackAfterMs, sinks: [{type: 'sync'}]},
    ...(capabilities.relay_authoring?.custom_durable_execution_enabled === true
      && capabilities.relay_authoring.response_recovery_providers?.includes(draft.provider)
      ? {durable_custom_execution: true} : {})};
}

export function assertCreateIntent(ctx, body, key, desired, credentialHashes) {
  ctx.assert(uuid.test(body.record_id ?? '') && uuid.test(key ?? '') && key !== body.record_id,
    'channel_browser_generated_record_and_key');
  const {record_id, credentials, ...configuration} = body;
  ctx.assert(same(configuration, desired), 'channel_create_exact_ui_configuration');
  ctx.assert(credentials && same(Object.keys(credentials).sort(), ['agent_key', 'hmac_secrets'])
    && typeof credentials.agent_key === 'string' && credentials.agent_key.length > 0
    && 'sha256:' + digest(credentials.agent_key) === credentialHashes.agent
    && credentials.hmac_secrets?.length === 1
    && same(Object.keys(credentials.hmac_secrets[0]).sort(), ['key_id', 'secret'])
    && credentials.hmac_secrets[0].key_id === desired.sources[0].auth.key_id
    && typeof credentials.hmac_secrets[0].secret === 'string'
    && 'sha256:' + digest(credentials.hmac_secrets[0].secret) === credentialHashes.hmac,
  'channel_create_exact_private_credentials');
}

export function assertCreatedChannel(ctx, record, intent, credentialHashes) {
  const d = record.definition, expected = intent.configuration;
  ctx.assert(record.record_id === intent.recordId && typeof record.channel_id === 'string' && record.channel_id
    && Number.isSafeInteger(record.version) && record.version > 0 && d?.creation?.ready === true
    && hash.test(d.creation.fingerprint ?? ''), 'channel_created_durable_identity');
  ctx.assert(d.name === expected.name && same(d.owner, expected.profile.metadata.owner)
    && same(d.profile, expected.profile),
  'channel_created_exact_consumer');
  ctx.assert(record.provider?.slug === expected.provider.slug && typeof record.provider.binding_id === 'string'
    && record.provider.binding_id.length > 0 && d.provider?.binding_id === record.provider.binding_id
    && same(d.provider.target === undefined ? {type: 'profile'} : d.provider.target, expected.provider.target),
  'channel_created_provider_binding');
  ctx.assert(d.sources?.length === 1 && d.sources[0].type === 'custom'
    && same(d.sources[0].auth, expected.sources[0].auth)
    && d.sources[0].prompt_template === expected.sources[0].prompt_template
    && d.partition?.version === expected.partition.version && d.partition?.key === expected.partition.key
    && d.reply?.auto_final === expected.reply.auto_final && d.reply?.ack_after_ms === expected.reply.ack_after_ms
    && same(d.reply.sinks, expected.reply.sinks)
    && Boolean(d.durable_custom_execution) === Boolean(expected.durable_custom_execution),
  'channel_created_producer_reply_policy');
  ctx.assert(same(d.secrets, {agent_key_hash: credentialHashes.agent}) && !d.credentials && !record.credentials,
    'channel_created_private_values_not_exposed');
}

export function assertRecoveredChannel(ctx, recovered, accepted) {
  // GET projects honoured/refused as empty arrays; POST returns the provider's
  // bind report. Compare saved identity/configuration rather than those reports.
  ctx.assert(recovered.record_id === accepted.record_id && recovered.channel_id === accepted.channel_id
    && recovered.version === accepted.version && same(recovered.definition, accepted.definition)
    && ['slug', 'binding_id', 'level'].every(key => recovered.provider?.[key] === accepted.provider?.[key]),
  'channel_recovered_same_version_binding_credentials');
}

// The UI generates record/key itself. Cleanup acquires the original credential-
// bearing intent before its first dispatch. The actual accepted response is
// read privately, then dropped at the browser boundary by default. Ordinary
// creation cases can forward that same response; no POST is replayed.
export async function armChannelCreation(ctx, page, lease, desired, credentialHashes, {loseResponse = true} = {}) {
  let attempts = 0, resolve;
  const completed = new Promise(done => {resolve = done;});
  const pending = new Set(), failures = [];
  const signal = ctx.signal;
  const cancelled = () => {if (!attempts) resolve({error: signal.reason ?? Error('channel_create_cancelled')});};
  signal?.addEventListener('abort', cancelled, {once: true});
  if (signal?.aborted) cancelled();
  const matcher = url => url.origin === new URL(page.url()).origin && url.pathname.startsWith('/api/event-gateway/');
  const dispatch = async route => {
    const request = route.request(), method = request.method();
    if (['GET', 'HEAD', 'OPTIONS'].includes(method)) return route.fallback();
    const attempt = ++attempts, errors = []; let outcome, fulfilled = false;
    try {
      ctx.assert(attempt === 1 && method === 'POST'
        && new URL(request.url()).pathname === '/api/event-gateway/v1/channels', 'channel_create_unexpected_mutation');
      const body = request.postDataJSON(), key = (await request.allHeaders())['idempotency-key'];
      assertCreateIntent(ctx, body, key, desired, credentialHashes);
      const custody = await invoke(lease, 'retainChannelIntent', {recordId: body.record_id, key, body});
      ctx.assert(custody?.retained === true && custody.record_sha256 === digest(body.record_id)
        && custody.key_sha256 === digest(key) && custody.body_sha256 === digest(body), 'channel_create_cleanup_custody');
      const intent = {recordId: body.record_id, key, body_sha256: digest(body), configuration: desired};
      await ctx.observe('channel_create_retained_intent', async () => ({record_sha256: digest(intent.recordId),
        key_sha256: digest(key), body_sha256: intent.body_sha256}));
      const remaining = ctx.remainingMs?.() ?? 60000;
      ctx.assert(!signal?.aborted && remaining > 0, 'channel_create_cancelled');
      const response = await route.fetch({maxRetries: 0, maxRedirects: 0, timeout: remaining});
      try {
        ctx.assert(response.status() === 201, 'channel_create_accepted_once');
        const record = await response.json(), requestId = response.headers()['x-request-id'];
        assertCreatedChannel(ctx, record, intent, credentialHashes);
        ctx.assert(Array.isArray(record.provider.refused) && record.provider.refused.length === 0,
          'channel_create_provider_configuration_honoured');
        ctx.assert(typeof requestId === 'string' && requestId.length > 0, 'channel_create_actual_request_id');
        outcome = {record, intent, requestId};
        if (!loseResponse) {await route.fulfill({response}); fulfilled = true;}
      } catch (error) {errors.push(error);}
      try {await response.dispose();} catch (error) {errors.push(error);}
    } catch (error) {errors.push(error);}
    if (!fulfilled) try {await route.abort('failed');} catch (error) {errors.push(error);}
    if (errors.length) {
      const error = errors.length === 1 ? errors[0] : new AggregateError(errors, 'channel_creation_fault_failed');
      failures.push(error); outcome = {error};
    }
    if (attempt === 1) resolve(outcome);
  };
  const handler = route => {
    const work = dispatch(route); pending.add(work);
    return work.finally(() => pending.delete(work));
  };
  await page.route(matcher, handler);
  ctx.defer('remove_channel_create_route', async () => {
    signal?.removeEventListener('abort', cancelled);
    const errors = [];
    try {await page.unroute(matcher, handler);} catch (error) {errors.push(error);}
    const drained = await Promise.allSettled([...pending]);
    errors.push(...drained.filter(r => r.status === 'rejected').map(r => r.reason), ...failures);
    if (errors.length) throw new AggregateError(errors, 'channel_creation_route_failed');
  });
  return {attempts: () => attempts, async result() {
    const value = await completed; if (value.error) throw value.error; return value;
  }};
}

async function configure(ctx, session, lease) {
  const page = session.page, draft = lease.draft;
  const capability = await channelRead(ctx, session, '/v1/channel-capabilities', () => goto(page, '/event-gateway'));
  const desired = channelDraft(lease, capability.body);
  ctx.assert(draft.name && draft.profileName && draft.provider && draft.description && draft.systemPrompt
    && draft.promptTemplate && draft.hmacKeyId && Array.isArray(draft.permission.service_scope)
    && draft.permission.service_scope.length > 0 && draft.permission.service_scope.length <= 64
    && ['guarded', 'autonomous'].includes(draft.permission.action_mode)
    && typeof draft.autoFinal === 'boolean' && Number.isSafeInteger(draft.ackAfterMs) && draft.ackAfterMs >= 0,
  'channel_create_configured_fixture');
  await page.getByRole('button', {name: 'New channel', exact: true}).click();
  await visible(ctx, page.getByRole('heading', {name: 'New channel', exact: true}), 'channel_create_real_form');
  const fill = (label, value) => page.getByLabel(label, {exact: true}).fill(String(value));
  await fill('Channel name', draft.name);
  await pick(page, 'Producer 1 connection type', 'Custom event');
  await pick(page, 'Authentication method', 'HMAC signature');
  await fill('HMAC key ID', draft.hmacKeyId); await fill('Prompt template', draft.promptTemplate);
  await fill('Provider', draft.provider);
  await pick(page, 'Agent target', 'Create an agent from these settings');
  await fill('Agent configuration name', draft.profileName); await fill('Description', draft.description);
  await fill('Profile provider', draft.provider); await fill('Owner subject', lease.expected.ownerSubject);
  await pick(page, 'Owner type', 'human');
  await fill('Model', draft.model.model); await pick(page, 'Thinking effort', draft.model.thinking_effort);
  await pick(page, 'Model access', draft.model.access); await fill('Model service', draft.model.service);
  await pick(page, 'Model API', draft.model.api_type);
  await pick(page, 'Permission mode', draft.permission.action_mode === 'guarded' ? 'Ask before sensitive actions' : 'Autonomous');
  const remove = page.getByRole('button', {name: /^Remove Service scope \d+$/});
  while (await remove.count()) await remove.last().click();
  for (let i = 0; i < draft.permission.service_scope.length; i++) {
    await page.getByRole('button', {name: 'Add Service scope', exact: true}).click();
    await fill('Service scope ' + (i + 1), draft.permission.service_scope[i]);
  }
  await fill('System prompt', draft.systemPrompt);
  await pick(page, 'Group incoming messages by', 'Each sender in each conversation');
  await page.getByRole('checkbox', {name: 'Send final answers automatically', exact: true}).setChecked(draft.autoFinal);
  await fill('Acknowledgement delay (ms)', draft.ackAfterMs); await pick(page, 'Destination 1 type', 'Synchronous response');
  const providerField = page.getByLabel('Provider agent key', {exact: true});
  ctx.assert(await providerField.getAttribute('type') === 'password', 'channel_provider_credential_masked');
  const credentialHashes = {agent: (await invoke(lease, 'fillProviderKey', providerField)).sha256};
  ctx.assert(hash.test(credentialHashes.agent ?? ''), 'channel_provider_credential_hash_required');
  await page.getByRole('checkbox', {name: 'Replace HMAC credentials', exact: true}).check();
  await page.getByRole('button', {name: 'Add HMAC credential', exact: true}).click();
  await fill('HMAC 1 key ID', draft.hmacKeyId);
  const secret = randomBytes(32).toString('hex'); credentialHashes.hmac = 'sha256:' + digest(secret);
  const secretField = page.getByLabel('HMAC 1 secret', {exact: true});
  ctx.assert(await secretField.getAttribute('type') === 'password', 'channel_hmac_credential_masked');
  await secretField.fill(secret);
  ctx.assert(await page.getByLabel('Channel name', {exact: true}).inputValue() === draft.name
    && await page.getByRole('button', {name: 'Create channel', exact: true}).isEnabled(), 'channel_create_ready');
  return {desired, credentialHashes};
}

async function prepare(ctx, scenario) {
  const lease = await acquire(ctx, scenario);
  const session = await openBrowser(ctx, 'user', {channelCreation: true});
  for (const method of ['egEvidence', 'switchEG', 'networkIdle'])
    if (typeof session[method] !== 'function') throw new Blocked('channel_creation_actual_eg_browser_required');
  const configured = await configure(ctx, session, lease);
  const submission = await armChannelCreation(ctx, session.page, lease, configured.desired, configured.credentialHashes);
  return {lease, session, submission, ...configured};
}

async function submit(ctx, state) {
  const {session, submission} = state, page = session.page;
  await page.getByRole('button', {name: 'Create channel', exact: true}).click();
  const accepted = await submission.result();
  await visible(ctx, page.getByRole('button', {name: 'Retry same request', exact: true}), 'channel_uncertain_retry_visible');
  await visible(ctx, page.getByRole('alert'), 'channel_uncertain_result_explicit');
  ctx.assert(await page.getByLabel('Channel name', {exact: true}).isDisabled()
    && await page.getByLabel('Channel name', {exact: true}).inputValue() === state.desired.name,
  'channel_uncertain_draft_locked');
  await invoke(session, 'networkIdle');
  const receipt = channelReceipt(ctx, await invoke(session, 'egEvidence'), accepted.requestId, 'POST', 201);
  await ctx.observe('channel_browser_create_eg_receipt', async () => ({schema: 'multi-eg-request-v1', receipt}));
  Object.assign(state, {accepted, receipt});
}

async function recover(ctx, state) {
  const {session, accepted, credentialHashes, receipt} = state, page = session.page;
  await session.switchEG(peer(receipt.replica));
  const recovered = await channelRead(ctx, session, '/v1/channel-records/' + segment(accepted.intent.recordId),
    () => page.getByRole('button', {name: 'Check original channel', exact: true}).click());
  ctx.assert(recovered.receipt.replica !== receipt.replica
    && recovered.receipt.serving.instance_id !== receipt.serving.instance_id, 'channel_recovered_other_actual_replica');
  assertCreatedChannel(ctx, recovered.body, accepted.intent, credentialHashes);
  assertRecoveredChannel(ctx, recovered.body, accepted.record);
  await visible(ctx, page.getByRole('alert').filter({hasText:
    'The original record was found. Retry the same request to confirm completion.'}), 'channel_recovery_keeps_uncertainty_explicit');
  ctx.assert(await page.getByLabel('Channel name', {exact: true}).isDisabled()
    && await page.getByLabel('Channel name', {exact: true}).inputValue() === state.desired.name
    && await page.getByRole('button', {name: 'Retry same request', exact: true}).isEnabled(),
  'channel_recovery_retains_original_locked_draft');
  // Only after reconciling the unchanged record do we reload the peer UI. This
  // intentionally discards React's pending form; it never retries or edits it.
  const selected = await openListedChannel(ctx, session, recovered.body, () => goto(page, '/event-gateway'));
  ctx.assert(selected.receipt.replica === recovered.receipt.replica, 'channel_reload_actual_peer_replica');
  assertCreatedChannel(ctx, selected.body, accepted.intent, credentialHashes);
  assertRecoveredChannel(ctx, selected.body, accepted.record);
}

async function finality(ctx, state) {
  const {lease, accepted, submission, session} = state;
  await invoke(session, 'networkIdle');
  const records = await invoke(lease, 'channelCreationRecords', {recordId: accepted.intent.recordId, key: accepted.intent.key});
  ctx.assert(submission.attempts() === 1 && records.createAttempts === 1
    && same(records.recordIds, [accepted.intent.recordId]) && same(records.channelIds, [accepted.record.channel_id])
    && same(records.bindingIds, [accepted.record.provider.binding_id])
    && records.retained_body_sha256 === accepted.intent.body_sha256 && records.retained_key_sha256 === digest(accepted.intent.key),
  'channel_create_exactly_one_retained_allocation');
  ctx.assert(records.providerTurns === 0 && records.deliveries === 0 && records.pending === 0 && records.unknown === 0,
    'channel_create_no_event_or_unresolved_provider_work');
  await ctx.observe('channel_create_finality', async () => ({record_sha256: digest(accepted.intent.recordId),
    channel_sha256: digest(accepted.record.channel_id), binding_sha256: digest(accepted.record.provider.binding_id),
    version: accepted.record.version, create_attempts: records.createAttempts, response_lost: true}));
  await uiReceipt(ctx, session.page);
}

export const channelCreateCase = {...define(20, 'browser', ['browser', 'user-fixtures', 'eg_browser'], async ctx => {
  let state;
  await ctx.step('step_1', async () => {state = await prepare(ctx, 'channel_create');});
  await ctx.step('step_2', async () => {await submit(ctx, state); await recover(ctx, state);});
  await ctx.step('expected_result', async () => {await finality(ctx, state);});
}), evidence_scope: 'eg_only'};

export const channelRecoveryCase = {...define(21, 'browser', ['browser', 'user-fixtures', 'eg_browser'], async ctx => {
  let state;
  await ctx.step('step_1', async () => {state = await prepare(ctx, 'channel_create_uncertain'); await submit(ctx, state);});
  await ctx.step('step_2', async () => {await recover(ctx, state);});
  await ctx.step('expected_result', async () => {await finality(ctx, state);});
}), evidence_scope: 'eg_only'};

export {configure as configureChannelForm};
