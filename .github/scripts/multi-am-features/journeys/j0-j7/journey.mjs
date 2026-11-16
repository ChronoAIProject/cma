import {registerFreshIdentity} from './identity.mjs';
import {observeBinding} from './binding.mjs';
import {contextFor} from './context.mjs';
import {verifyBrowserSession} from './browser-proof.mjs';
import {j3} from './turns.mjs';
import {j5, j6} from './transitions.mjs';
import {cleanupAgent} from './cleanup.mjs';
import {actorFor, check, hash, opposite, observeBrowser, operationId, request, requireBrowser, requireOwnerProof, unique, waitFor} from './helpers.mjs';

export const STEPS = Object.freeze([
  'J0_register', 'J1_first_agent_create', 'J2_open', 'J3_send_interrupt',
  'J4_settings_rename', 'J5_stop_pause_resume', 'J6_delete', 'J7_usage_billing',
]);

const ready = value => value?.body?.status === 'ready' && value.body.sandbox?.status === 'active';

async function readAgent(ctx, identity, agentId, am, proofState) {
  return request(ctx, identity, {am, method: 'GET', path: `/api/v1/agents/${agentId}`, agentId, allowed: [200], proofState});
}

async function j0(ctx, state) {
  state.identity = await registerFreshIdentity(ctx);
  state.actor = actorFor(state.identity);
  const browser = requireBrowser(ctx);
  state.browser = await browser.open({actor: state.actor, am: ctx.am});
  ctx.defer('fresh_identity_browser_close', () => state.browser.close());
  const sessionProof = await verifyBrowserSession(ctx, state.browser, state.identity);
  await ctx.observe('fresh_identity_cookie_session', async () => ({
    subject_sha256: sessionProof.subject_sha256,
    session_binding_sha256: sessionProof.session_binding_sha256,
    registration_receipt_sha256: sessionProof.registration_receipt_sha256,
  }));
  const inventory = await request(ctx, state.identity, {method: 'GET', path: '/api/v1/agents', allowed: [200]});
  check(Array.isArray(inventory.body?.items) && inventory.body.items.length === 0,
    'fresh_identity_has_existing_agents');
  ctx.assert(inventory.body.items.length === 0, 'fresh_identity_empty_inventory');
  state.am = ctx.am;
  await ctx.observe('fresh_identity_inventory', async () => ({actor_sha256: hash(state.actor),
    subject_sha256: state.identity.registration.subject_sha256, existing_agents: 0}));
}

async function j1(ctx, state) {
  state.key = unique('first-agent');
  state.name = unique('agent');
  // Custody is registered before the mutation. An accepted response whose
  // attribution is lost remains unresolved and is never replayed with a new key.
  contextFor(ctx).resourcesReleased = false;
  ctx.defer('fresh_agent_cleanup', () => cleanupAgent(ctx, state));
  let receipt;
  state.requestIds = [];
  try {
    receipt = await request(ctx, state.identity, {am: state.am, method: 'POST', path: '/api/v1/agents',
      body: {name: state.name, machine_included: false, desktop: false}, key: state.key, allowed: [202],
      proofState: state.requestIds, lifecycle: true});
  } catch (error) {
    const response = error.response;
    state.operationId = response?.body?.id;
    state.unresolvedCreate = !state.operationId;
    throw error;
  }
  state.operationId = operationId(receipt);
  const lifecycle = await waitFor(ctx, () => request(ctx, state.identity, {am: opposite(state.am), method: 'GET',
    path: `/api/v1/agent-lifecycle/${state.operationId}`, allowed: [200]}),
    value => {
      state.agentId ??= value.body?.agent_id;
      check(!['failed', 'cancelled', 'waiting_for_connection'].includes(value.body?.status), 'first_agent_lifecycle_failed');
      return value.body?.status === 'succeeded' && typeof value.body.agent_id === 'string';
    },
    'first_agent_lifecycle_timeout');
  state.agentId = lifecycle.body.agent_id;
  const detail = await waitFor(ctx, () => readAgent(ctx, state.identity, state.agentId, state.am), ready,
    'first_agent_ready_timeout', {attempts: 120});
  state.sandboxId = detail.body.sandbox.id;
  const sandbox = await request(ctx, state.identity, {am: opposite(state.am), method: 'GET',
    path: `/api/v1/sandboxes/${state.sandboxId}`, allowed: [200]});
  check(detail.body.id === state.agentId && sandbox.body.id === state.sandboxId
    && hash(sandbox.body.owner) === state.identity.registration.subject_sha256,
    'first_agent_owner_mismatch');
  ctx.assert(detail.body.status === 'ready', 'first_agent_ready');
  await requireOwnerProof(ctx, state, 'j1_create_owner');
  await ctx.observe('first_agent_ready', async () => ({agent_sha256: hash(state.agentId),
    sandbox_sha256: hash(state.sandboxId), operation_sha256: hash(state.operationId)}));
}

async function j2(ctx, state) {
  const other = opposite(state.am);
  const detail = await readAgent(ctx, state.identity, state.agentId, other);
  check(detail.body.id === state.agentId && detail.body.status === 'ready', 'open_agent_not_ready');
  const transcript = await request(ctx, state.identity, {am: other, method: 'GET',
    path: `/api/v1/agents/${state.agentId}/transcript`, agentId: state.agentId, allowed: [200]});
  check(transcript.body?.agent_id === state.agentId, 'open_transcript_identity_mismatch');
  const session = state.browser;
  await session.page.goto(new URL(`/agents/${encodeURIComponent(state.agentId)}`, session.origin).href);
  const composer = session.page.getByRole('textbox', {name: 'Message', exact: true});
  check(await composer.isVisible() && await composer.isEnabled(), 'open_agent_composer_unavailable');
  await observeBrowser(ctx, 'open_agent_browser', session, state.identity);
  ctx.assert(await composer.isVisible(), 'open_agent_ui_visible');
}

async function j4(ctx, state) {
  state.requestIds = [];
  const preferences = await request(ctx, state.identity, {method: 'GET', path: `/api/v1/agents/${state.agentId}/preferences`,
    agentId: state.agentId, allowed: [200]});
  const title = unique('renamed');
  const rename = await request(ctx, state.identity, {method: 'PATCH', path: `/api/v1/agents/${state.agentId}/preferences`,
    agentId: state.agentId, body: {expected_revision: preferences.body.revision, title}, key: unique('rename'), allowed: [200], proofState: state.requestIds});
  check(rename.body?.title === title && rename.body.revision > preferences.body.revision, 'rename_not_published');
  const settings = await request(ctx, state.identity, {method: 'GET', path: `/api/v1/agents/${state.agentId}/settings`,
    agentId: state.agentId, allowed: [200]});
  const permissionMode = settings.body.permission_mode === 'guarded' ? 'autonomous' : 'guarded';
  const updated = await request(ctx, state.identity, {method: 'PATCH', path: `/api/v1/agents/${state.agentId}/settings`,
    agentId: state.agentId, body: {model: settings.body.model, reasoning_effort: settings.body.reasoning_effort,
      permission_mode: permissionMode}, key: unique('settings'), allowed: [200], proofState: state.requestIds});
  check(updated.body?.model === settings.body.model && updated.body?.permission_mode === permissionMode
    && updated.body?.pending === false, 'settings_not_confirmed');
  await ctx.observe('settings_rename_result', async () => ({agent_sha256: hash(state.agentId),
    title_sha256: hash(title), preferences_revision: rename.body.revision, settings_model_sha256: hash(updated.body.model)}));
  ctx.assert(rename.body.title === title && updated.body.permission_mode === permissionMode, 'settings_rename_confirmed');
  await requireOwnerProof(ctx, state, 'j4_mutation_owner');
}

async function j7(ctx, state) {
  const usage = await request(ctx, state.identity, {am: opposite(state.am), method: 'GET', path: '/api/v1/usage', allowed: [200]});
  check(usage.body && typeof usage.body === 'object', 'usage_summary_missing');
  const session = state.browser;
  await session.switchAM?.(opposite(state.am));
  await session.page.goto(new URL('/usage', session.origin).href);
  check(await session.page.getByRole('dialog', {name: 'Your usage', exact: true}).isVisible(), 'usage_view_missing');
  check(await session.page.getByLabel('Your usage resources', {exact: true}).isVisible(), 'usage_resources_missing');
  await observeBrowser(ctx, 'usage_billing_browser', session, state.identity);
  ctx.assert(await session.page.getByRole('dialog', {name: 'Your usage', exact: true}).isVisible(), 'usage_billing_confirmed');
  await ctx.observe('usage_billing_result', async () => ({usage_status: usage.status,
    usage_shape: Array.isArray(usage.body?.items) ? 'items' : 'summary',
    billing_basis: 'CMA allocation usage; external NyxID invoices require the registration adapter'}));
}

export async function runJourney(ctx) {
  const parent = contextFor(ctx);
  const binding = await observeBinding(parent.binding, parent.release, parent.adapter, parent.environment);
  await ctx.observe('mfunc01_local_kind_binding', async () => binding);
  const state = {phase: ctx.phase, iteration: ctx.iteration, am: ctx.am};
  const steps = [j0, j1, j2, j3, j4, j5, j6, j7];
  for (let index = 0; index < steps.length; index += 1) {
    await ctx.step(STEPS[index], () => steps[index](ctx, state));
  }
  return {agent_sha256: hash(state.agentId), identity_subject_sha256: state.identity.registration.subject_sha256};
}
