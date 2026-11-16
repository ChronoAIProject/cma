import {define, acquire, openBrowser, visible, goto, invoke, same, digest, peer, segment, poll, uiReceipt} from './helpers.mjs';
import {armConsumerCreation, creationReceipt} from './consumer-wizard.mjs';
import {cookieGet} from './consumer-inventory.mjs';
import {Blocked} from '../../runner.mjs';

const profileRoot = '/api/v1/agent-profiles';
const canonical = path => path.replace(/^\/api\/agent-manager\//, '/api/');
const draftPath = draft => '/bots/new?setup=' + encodeURIComponent(JSON.stringify(draft));
export const fatherProfileBody = draft => ({name: draft.name,
  description: 'Created from a reviewed CMA Bot Father setup.',
  content: {thinking_effort: 'medium', permission_level: 'guarded', skills: [], system_prompt: draft.instructions}});
export const fatherConsumerBody = (draft, connection) => ({name: draft.name, connection_ref: connection,
  agent: {reasoning_effort: 'medium', permission_mode: 'guarded', system_prompt: draft.instructions,
    skills: [], permissions: {services: []}},
  admission: {type: 'scoped', senders: {type: 'open'}, chats: {type: 'open'}, groups: 'deny'},
  reply: {auto_final: true, ack_after_ms: 2000}});

export async function fatherState(page, owner) {
  return page.evaluate(owner => {
    const suffix = JSON.stringify([location.origin, owner]);
    const read = prefix => {
      const raw = sessionStorage.getItem(prefix + suffix);
      return raw === null ? null : JSON.parse(raw);
    };
    return {profile: read('cma:bot-father:'), consumer: read('cma:bot-creation:')};
  }, owner);
}

export function assertFatherReceipt(ctx, saved, owner, draft, created, published) {
  ctx.assert(saved?.version === 1 && saved.owner === owner && same(saved.draft, draft)
    && typeof saved.createKey === 'string' && saved.createKey.length > 0
    && typeof saved.publishKey === 'string' && saved.publishKey.length > 0
    && saved.createKey !== saved.publishKey, 'father_exact_owner_configuration');
  if (created) ctx.assert(same(saved.document, {id: created.id, draftVersion: created.draft_version}),
    'father_retained_document_identity');
  if (published) ctx.assert(published.profile_id === created.id && same(saved.published,
    {revision: published.revision, digest: published.digest, at: published.published_at}), 'father_retained_publication_identity');
}

// Observe the product's actual two POSTs, preserving their responses. The parent
// takes custody of each exact key/body before any forwarding. No synthetic API
// response, result delegate, profile ID prediction or retry is used here.
export async function armFatherPublication(ctx, page, lease) {
  const owner = lease.expected.ownerSubject, draft = lease.reviewed;
  let settle, created, published;
  const attempts = {create: 0, publish: 0}, pending = new Set(), failures = [];
  const completed = new Promise(resolve => {settle = resolve;});
  const signal = ctx.signal;
  const cancelled = () => settle({error: signal.reason ?? Error('father_publication_cancelled')});
  signal?.addEventListener('abort', cancelled, {once: true}); if (signal?.aborted) cancelled();
  const matcher = url => url.origin === new URL(page.url()).origin
    && (canonical(url.pathname) === profileRoot || /^\/api\/v1\/agent-profiles\/[^/]+\/revisions$/.test(canonical(url.pathname)));
  const dispatch = async route => {
    const request = route.request();
    if (request.method() !== 'POST') return route.fallback();
    const path = canonical(new URL(request.url()).pathname), kind = path === profileRoot ? 'create' : 'publish';
    const errors = []; let response, accepted;
    try {
      ctx.assert(++attempts[kind] === 1, 'father_duplicate_' + kind);
      const body = request.postDataJSON(), key = (await request.allHeaders())['idempotency-key'];
      const saved = (await fatherState(page, owner)).profile;
      assertFatherReceipt(ctx, saved, owner, draft, kind === 'publish' ? created?.body : undefined);
      ctx.assert(key === saved[kind === 'create' ? 'createKey' : 'publishKey'], 'father_retained_key_before_dispatch');
      if (kind === 'create') {
        ctx.assert(saved.document === undefined && same(body, fatherProfileBody(draft)), 'father_reviewed_profile_request');
      } else {
        ctx.assert(created?.body?.id && path === `${profileRoot}/${segment(created.body.id)}/revisions`
          && same(body, {draft_version: created.body.draft_version}), 'father_publish_exact_document');
      }
      const custody = await invoke(lease, 'retainProfileIntent', {kind, path, key, body});
      ctx.assert(custody?.retained === true && custody.key_sha256 === digest(key)
        && custody.body_sha256 === digest(body), 'father_profile_cleanup_custody');
      const remaining = ctx.remainingMs?.() ?? 60000;
      ctx.assert(!signal?.aborted && remaining > 0, 'father_publication_cancelled');
      response = await route.fetch({maxRetries: 0, maxRedirects: 0, timeout: remaining});
      const value = await response.json(), requestId = response.headers()['x-request-id'];
      ctx.assert(response.status() === 201 && typeof requestId === 'string' && requestId,
        'father_actual_accepted_profile_response');
      if (kind === 'create') {
        ctx.assert(typeof value.id === 'string' && value.id && value.owner === owner && value.visibility === 'private'
          && Number.isSafeInteger(value.draft_version) && value.draft_version > 0
          && value.name === draft.name, 'father_private_owned_profile');
        created = {body: value, requestId, key};
      } else {
        ctx.assert(value.profile_id === created.body.id && Number.isSafeInteger(value.revision) && value.revision > 0
          && typeof value.digest === 'string' && value.digest && Number.isFinite(Date.parse(value.published_at)),
        'father_published_revision');
        published = {body: value, requestId, key};
      }
      accepted = true;
      await route.fulfill({response});
    } catch (error) {errors.push(error);}
    if (response) try {await response.dispose();} catch (error) {errors.push(error);}
    if (errors.length) {
      try {await route.abort('failed');} catch (error) {errors.push(error);}
      const error = errors.length === 1 ? errors[0] : new AggregateError(errors, 'father_publication_route_failed');
      failures.push(error); settle({error});
    } else if (accepted && kind === 'publish') settle({created, published});
  };
  const handler = route => {
    const work = dispatch(route); pending.add(work);
    return work.finally(() => pending.delete(work));
  };
  await page.route(matcher, handler);
  ctx.defer('remove_father_publication_routes', async () => {
    signal?.removeEventListener('abort', cancelled);
    const errors = [];
    try {await page.unroute(matcher, handler);} catch (error) {errors.push(error);}
    const drained = await Promise.allSettled([...pending]);
    errors.push(...drained.filter(result => result.status === 'rejected').map(result => result.reason), ...failures);
    if (errors.length) throw new AggregateError(errors, 'father_publication_failed');
  });
  return {attempts: () => ({...attempts}), async result() {
    const result = await completed; if (result.error) throw result.error; return result;
  }};
}

export function assertFatherConfiguration(ctx, value, draft, expectedModel = null) {
  ctx.assert((value.model ?? null) === expectedModel && value.thinking_effort === 'medium' && value.permission_level === 'guarded'
    && value.system_prompt === draft.instructions && same(value.skills, []), 'father_exact_published_configuration');
}

async function identity(ctx, session, subject) {
  const response = await cookieGet(ctx, session, '/auth/session');
  ctx.assert(response.body.authenticated === true && response.body.subject === subject && response.body.visitor !== true
    && response.body.capabilities?.includes('manage_own_bots') && response.body.capabilities?.includes('manage_own_profiles'),
  'father_current_cookie_identity_capabilities');
}

async function choose(page, label, value) {
  await page.getByRole('button', {name: label, exact: true}).click();
  await page.getByRole('menuitemradio', {name: value, exact: true}).click();
}

export const botFatherCase = define(49, 'browser', ['browser', 'user-fixtures', 'eg_browser'], async ctx => {
  let lease, session, publication, result, saved, creation, accepted, active, body;
  await ctx.step('step_1', async () => {
    lease = await acquire(ctx, 'bot_father_profile');
    if (lease.draft?.v !== 1 || !lease.draft.name || !lease.draft.instructions
      || lease.reviewed?.v !== 1 || !lease.reviewed.name || !lease.reviewed.instructions
      || lease.reviewed.instructions === lease.draft.instructions || !lease.connection?.ref || !lease.connection.label
      || !lease.expected.ownerSubject || !lease.expected.otherSubject || !lease.expected.defaultModel
      || lease.expected.ownerSubject === lease.expected.otherSubject) throw new Blocked('father_authentic_draft_fixture_required');
    session = await openBrowser(ctx, 'user', {botFather: true, initialPath: draftPath(lease.draft)});
    for (const method of ['signIn', 'networkIdle']) if (typeof session[method] !== 'function') throw new Blocked('father_real_login_control_required');
    await identity(ctx, session, lease.expected.ownerSubject);
    ctx.assert(same(await fatherState(session.page, lease.expected.ownerSubject), {profile: null, consumer: null}), 'father_clean_owner_fixture');
    await goto(session.page, draftPath(lease.draft));
    await visible(ctx, session.page.getByRole('heading', {name: 'Create your Bot with CMA Bot Father', exact: true}), 'father_draft_resumed');
    ctx.assert(await session.page.getByLabel('Bot name', {exact: true}).inputValue() === lease.draft.name
      && await session.page.getByLabel('Agent instructions', {exact: true}).inputValue() === lease.draft.instructions,
    'father_resumed_exact_suggestion');
    ctx.assert(await session.page.getByRole('button', {name: 'Create my agent profile', exact: true}).isDisabled(), 'father_review_consent_required');
  });
  await ctx.step('step_2', async () => {
    const page = session.page;
    await page.getByLabel('Bot name', {exact: true}).fill(lease.reviewed.name);
    await page.getByLabel('Agent instructions', {exact: true}).fill(lease.reviewed.instructions);
    publication = await armFatherPublication(ctx, page, lease);
    await page.getByRole('checkbox', {name: 'I reviewed these instructions and want to create this profile in my CMA account.', exact: true}).check();
    await page.getByRole('button', {name: 'Create my agent profile', exact: true}).click();
    result = await publication.result();
    await visible(ctx, page.getByRole('heading', {name: 'Choose an Event Gateway connection', exact: true}), 'father_profile_prepared_consumer');
    saved = (await fatherState(page, lease.expected.ownerSubject)).profile;
    assertFatherReceipt(ctx, saved, lease.expected.ownerSubject, lease.reviewed, result.created.body, result.published.body);
    const evidence = await invoke(ctx.fixture('browser'), 'evidence', page);
    const createdReceipt = creationReceipt(ctx, evidence, result.created.requestId);
    const publishedReceipt = creationReceipt(ctx, evidence, result.published.requestId);
    const document = await ctx.request({path: `${profileRoot}/${segment(result.created.body.id)}`, am: peer(createdReceipt.am)});
    ctx.assert(document.body.id === result.created.body.id && document.body.owner === lease.expected.ownerSubject
      && document.body.visibility === 'private' && document.body.name === lease.reviewed.name
      && document.body.draft_version === result.created.body.draft_version && document.timing.am !== createdReceipt.am,
    'father_owned_profile_readback');
    assertFatherConfiguration(ctx, document.body.draft, lease.reviewed);
    const revision = await ctx.request({path: `${profileRoot}/${segment(result.created.body.id)}/revisions/${result.published.body.revision}`,
      am: peer(publishedReceipt.am)});
    ctx.assert(same(revision.body, result.published.body) && revision.timing.am !== publishedReceipt.am,
      'father_immutable_publication_readback');
    assertFatherConfiguration(ctx, revision.body.content, lease.reviewed);
    await goto(page, draftPath(lease.draft));
    await visible(ctx, page.getByRole('heading', {name: 'Choose an Event Gateway connection', exact: true}), 'father_published_receipt_resumed');
    ctx.assert(same((await fatherState(page, lease.expected.ownerSubject)).profile, saved)
      && same(publication.attempts(), {create: 1, publish: 1}), 'father_resume_without_profile_resubmission');
    ctx.assert(await page.getByLabel('Consumer name', {exact: true}).inputValue() === lease.reviewed.name, 'father_consumer_name_carried');
    await choose(page, 'Event Gateway connection', lease.connection.label);
    await page.getByRole('button', {name: 'Continue', exact: true}).click();
    await visible(ctx, page.getByRole('heading', {name: 'Configure the Agent inline', exact: true}), 'father_inline_configuration');
    ctx.assert(await page.getByLabel('Model', {exact: true}).inputValue() === ''
      && await page.getByLabel('System prompt', {exact: true}).inputValue() === lease.reviewed.instructions
      && await page.getByRole('button', {name: 'Reasoning effort', exact: true}).innerText() === 'medium'
      && await page.getByRole('button', {name: 'Permission level', exact: true}).innerText() === 'Guarded',
    'father_consumer_configuration_carried');
    await page.getByRole('button', {name: 'Continue', exact: true}).click();
    await choose(page, 'Admission policy', 'Private chats only');
    await page.getByRole('button', {name: 'Continue', exact: true}).click();
    body = fatherConsumerBody(lease.reviewed, lease.connection.ref);
    creation = await armConsumerCreation(ctx, page, lease, body);
    await page.getByRole('button', {name: 'Create consumer', exact: true}).click(); accepted = await creation.result();
    await visible(ctx, page.getByText(/Creation has not been confirmed\. Recover the same request/), 'father_consumer_intent_retained');
    const ingress = creationReceipt(ctx, await invoke(ctx.fixture('browser'), 'evidence', page), accepted.requestId).am;
    active = await poll(ctx, async () => {
      const read = await ctx.request({path: '/api/v1/bots/' + segment(accepted.accepted.bot_id), am: peer(ingress)});
      ctx.assert(read.timing.am !== ingress && read.body.bot_id === accepted.accepted.bot_id
        && !['failed', 'deleted', 'deleting'].includes(read.body.state), 'father_consumer_provision_failure');
      return read.body;
    }, value => value.state === 'active', 'father_consumer_provision_timeout');
    ctx.assert(active.bot_id === accepted.accepted.bot_id && active.owner === lease.expected.ownerSubject
      && active.name === lease.reviewed.name && active.connection_ref === lease.connection.ref
      && active.channel?.channel_id && active.profile?.id && Number.isSafeInteger(active.profile.revision), 'father_consumer_exact_identity');
    const consumerProfile = await ctx.request({path: `${profileRoot}/${segment(active.profile.id)}/revisions/${active.profile.revision}`, am: peer(ingress)});
    ctx.assert(consumerProfile.body.profile_id === active.profile.id && consumerProfile.body.revision === active.profile.revision,
      'father_consumer_exact_profile_pin');
    assertFatherConfiguration(ctx, consumerProfile.body.content, lease.reviewed, lease.expected.defaultModel);
  });
  await ctx.step('expected_result', async () => {
    const page = session.page, origin = session.origin;
    const retained = await fatherState(page, lease.expected.ownerSubject);
    assertFatherReceipt(ctx, retained.profile, lease.expected.ownerSubject, lease.reviewed, result.created.body, result.published.body);
    ctx.assert(retained.consumer?.intent?.key === accepted.key && same(retained.consumer.intent.body, body), 'father_owner_creation_retained');
    await invoke(session, 'signIn', {actor: 'outsider'}); await goto(page, draftPath(lease.draft));
    ctx.assert(session.page === page && session.origin === origin && new URL(page.url()).origin === origin, 'father_same_browser_context');
    await identity(ctx, session, lease.expected.otherSubject);
    await visible(ctx, page.getByRole('heading', {name: 'Create your Bot with CMA Bot Father', exact: true}), 'father_other_owner_fresh_review');
    ctx.assert(await page.getByLabel('Bot name', {exact: true}).inputValue() === lease.draft.name
      && await page.getByLabel('Agent instructions', {exact: true}).inputValue() === lease.draft.instructions,
    'father_other_owner_unreviewed_suggestion');
    ctx.assert(await page.getByRole('button', {name: 'Create my agent profile', exact: true}).isDisabled()
      && await page.getByRole('button', {name: 'Recover creation', exact: true}).count() === 0
      && await page.getByRole('button', {name: 'Recover profile creation', exact: true}).count() === 0,
    'father_other_owner_cannot_resume');
    ctx.assert(same(await fatherState(page, lease.expected.otherSubject), {profile: null, consumer: null})
      && same(await fatherState(page, lease.expected.ownerSubject), retained), 'father_receipts_remain_owner_scoped');
    for (const am of ['a', 'b']) {
      await session.switchAM(am);
      for (const path of [`${profileRoot}/${segment(result.created.body.id)}`, '/api/v1/bots/' + segment(active.bot_id)]) {
        const refusal = await cookieGet(ctx, session, path, [403, 404]);
        ctx.assert(refusal.receipt.am === am && !refusal.body?.id && !refusal.body?.bot_id, 'father_foreign_private_resources_denied');
      }
    }
    await invoke(session, 'networkIdle');
    ctx.assert(same(publication.attempts(), {create: 1, publish: 1}) && creation.attempts() === 1, 'father_changed_login_no_mutation');
    await invoke(session, 'signIn', {actor: 'user'}); await goto(page, '/bots/new');
    await identity(ctx, session, lease.expected.ownerSubject);
    await visible(ctx, page.getByRole('button', {name: 'Recover creation', exact: true}), 'father_original_owner_can_resume');
    ctx.assert(same(await fatherState(page, lease.expected.ownerSubject), retained), 'father_original_exact_receipts_restored');
    await invoke(session, 'networkIdle');
    const counts = await invoke(lease, 'creationRecords', {key: accepted.key});
    const profiles = await invoke(lease, 'profileRecords', {createKey: saved.createKey, publishKey: saved.publishKey});
    ctx.assert(counts.botIds?.length === 1 && counts.botIds[0] === active.bot_id
      && counts.channelIds?.length === 1 && counts.channelIds[0] === active.channel.channel_id
      && profiles.profileIds?.length === 1 && profiles.profileIds[0] === result.created.body.id
      && same(profiles.revisions, [result.published.body.revision])
      && same(publication.attempts(), {create: 1, publish: 1}) && creation.attempts() === 1,
    'father_one_profile_publication_consumer');
    await uiReceipt(ctx, page);
  });
});
