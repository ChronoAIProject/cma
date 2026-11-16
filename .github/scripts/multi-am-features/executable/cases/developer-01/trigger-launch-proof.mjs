import {request, id, required, equal, check} from './common.mjs';

const states = new Set(['draft', 'resolving', 'sandbox_pending', 'workspace_pending', 'agent_pending',
  'message_pending', 'ready', 'completed', 'failed_retryable', 'failed_terminal', 'cancelled']);

export function launchFixture(f) {
  required(typeof f.triggerId === 'string' && typeof f.publicId === 'string'
    && Array.isArray(f.disabledIntents) && f.disabledIntents.length === 2
    && f.launchIntent && typeof f.custody === 'function', 'admin_trigger_launch_fixture_missing');
  id(f.triggerId); id(f.publicId);
  const intents = [...f.disabledIntents, f.launchIntent];
  required(intents.every(i => i?.actor === 'user' && i.method === 'POST'
    && i.path === '/api/v1/triggers/public/' + id(f.publicId) + '/launches'
    && typeof i.key === 'string' && i.key.trim()
    && equal(i.body, {start_if_ready: true, choice: 'new'}))
    && new Set(intents.map(i => i.key)).size === 3, 'admin_trigger_launch_intents');
}

export function launchIdentity(ctx, f, launch, expected) {
  ctx.assert(typeof launch?.id === 'string' && launch.id.trim() && launch.trigger_id === f.triggerId
    && launch.public_id === f.publicId && /^[a-f0-9]{64}$/.test(launch.revision_digest ?? '')
    && states.has(launch.state) && Number.isSafeInteger(launch.revision) && launch.revision > 0,
  'admin_trigger_launch_identity');
  if (expected) ctx.assert(['id', 'trigger_id', 'public_id', 'revision_digest'].every(k => launch[k] === expected[k]),
    'admin_trigger_launch_identity');
}

export async function launchInventory(ctx, f, am, expected = []) {
  const response = await request(ctx, 'GET', '/api/v1/launches?' + new URLSearchParams({trigger: f.triggerId, limit: '100'}),
    undefined, {actor: 'user', am});
  const page = response.body;
  // The case owns a new Trigger and at most one admitted Launch. More pages
  // cannot be silently discarded or counted as the expected empty/single set.
  ctx.assert(Array.isArray(page?.items) && page.items.length <= 100 && page.next_cursor == null,
    'admin_trigger_inventory_incomplete');
  for (const launch of page.items) launchIdentity(ctx, f, launch);
  ctx.assert(equal(page.items.map(l => l.id).sort(), expected.toSorted()), 'admin_trigger_launch_inventory');
  return page.items;
}

export async function launchPair(ctx, f, expected) {
  const result = [];
  for (const am of ['a', 'b']) {
    const response = await request(ctx, 'GET', '/api/v1/launches/' + id(expected.id), undefined, {actor: 'user', am});
    launchIdentity(ctx, f, response.body, expected);
    ctx.assert(!['draft', 'failed_retryable', 'failed_terminal', 'cancelled'].includes(response.body.state)
      && response.body.failure == null, 'admin_trigger_launch_failed');
    result.push({am, launch: response.body});
  }
  return result;
}

export const allPending = launch => ['sandbox', 'workspace', 'agent'].every(k =>
  launch.resolution?.[k]?.state === 'pending' && launch.resolution[k].id == null);
export const isPaused = launch => launch.blocked_reason?.type === 'orchestration_paused';
export const isReady = launch => ['ready', 'completed'].includes(launch.state);

export function readyResources(ctx, pair) {
  for (const {launch} of pair) ctx.assert(isReady(launch) && launch.blocked_reason == null
    && Number.isFinite(Date.parse(launch.ready_at))
    && ['sandbox', 'workspace', 'agent'].every(k => ['created', 'reused'].includes(launch.resolution?.[k]?.state)
      && typeof launch.resolution[k].id === 'string' && launch.resolution[k].id.trim()),
  'admin_trigger_ready_resources');
  ctx.assert(equal(pair[0].launch.resolution, pair[1].launch.resolution), 'admin_trigger_peer_resources');
}

export async function publishedTrigger(ctx, f) {
  const trigger = (await request(ctx, 'GET', '/api/v1/triggers/' + id(f.triggerId), undefined, {actor: 'user'})).body;
  required(trigger?.id === f.triggerId && trigger.public_id === f.publicId && trigger.availability === 'published',
    'admin_trigger_published_fixture_missing');
  const preview = (await request(ctx, 'GET', '/api/v1/triggers/public/' + id(f.publicId), undefined, {actor: 'user'})).body;
  required(preview?.availability === 'published' && preview.always_review === false && preview.first_message === ''
    && Array.isArray(preview.technical?.services) && preview.technical.services.length === 0,
  'admin_trigger_no_review_fixture_missing');
  check(typeof f.cleanup === 'function', 'admin_trigger_cleanup_missing');
}
