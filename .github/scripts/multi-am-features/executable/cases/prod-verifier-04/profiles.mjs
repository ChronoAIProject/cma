import {caseOf, check, cleanupRequest, equal, key, otherAM, pages, request, segment, take} from './common.mjs';

const kinds = ['agent', 'workspace', 'sandbox'];
const base = kind => '/api/v1/' + kind + '-profiles';
const at = (kind, id) => base(kind) + '/' + segment(id);
const market = (kind, id = '') => '/api/v1/marketplace/' + kind + '-profiles' + (id ? '/' + segment(id) : '');
const ids = [3,4,5,6,7,8,9,10,11,36,37,38,39,40,41,44,45,46,47,48,53,54,55,56,57,58,59,60,61,72,73,74,75,76,77,78,79,80];

function contents(kind, graph, bounds) {
  if (kind === 'agent') return {thinking_effort: 'low', permission_level: 'guarded',
    skills: [], system_prompt: 'Synthetic API coverage fixture.'};
  if (kind === 'workspace') return {location: {type: 'directory', name: 'api-coverage'},
    name: 'api-coverage', setup_commands: [], agent_profiles: [{id: graph.agent.doc.id, revision: 1}]};
  return {cpu_millis: bounds.min_cpu_millis, memory_mib: bounds.min_memory_mib,
    nyxid_services: [], agent_provider: 'codex', model_access: 'chronoai_managed',
    max_workspaces: 1, max_agents_per_workspace: 1,
    workspace_profiles: [{id: graph.workspace.doc.id, revision: 1}],
    software_install_commands: [], environment_variables: []};
}

async function createProfile(ctx, kind, body) {
  const createKey = key();
  let doc, retired = false;
  // Register custody before dispatch. An attributed failure can still carry the
  // accepted response. Uncertain no-receipt creates remain an explicit cleanup
  // failure for parent namespace teardown, never a fresh-key retry.
  let dispatched = false;
  ctx.defer('retire_' + kind + '_profile', async () => {
    if (!dispatched || retired) return;
    check(doc?.id, 'profile_create_custody_unknown');
    const result = await cleanupRequest(ctx, 'POST', at(kind, doc.id) + '/retire', undefined, {key: retireKey});
    check(result.id === doc.id && typeof result.retired_at === 'string', 'profile_cleanup_not_retired');
    retired = true;
  });
  const retireKey = key();
  try {
    dispatched = true;
    doc = (await request(ctx, 'POST', base(kind), body, {key: createKey, allowed: [201]})).body;
  } catch (error) { doc = error.response?.body; throw error; }
  ctx.assert(typeof doc?.id === 'string' && doc.draft_version === 1 && doc.visibility === 'private', 'profile_created_private');
  ctx.assert(equal(doc.draft, body.content) && doc.name === body.name, 'profile_create_content');
  const record = {kind, body, createKey, doc, revisions: [], retireKey,
    async retire() {
      if (retired) return doc;
      doc = (await request(ctx, 'POST', at(kind, doc.id) + '/retire', undefined, {key: retireKey})).body;
      record.doc = doc;
      ctx.assert(typeof doc.retired_at === 'string', 'profile_retirement_recorded');
      retired = true;
      return doc;
    }};
  return record;
}

async function publish(ctx, record) {
  const response = await request(ctx, 'POST', at(record.kind, record.doc.id) + '/revisions',
    {draft_version: record.doc.draft_version}, {key: key(), allowed: [201]});
  const revision = response.body;
  ctx.assert(revision.profile_id === record.doc.id && revision.revision === record.revisions.length + 1,
    'profile_revision_identity');
  ctx.assert(equal(revision.content, record.doc.draft) && /^[a-f0-9]{64}$/.test(revision.digest), 'profile_revision_content');
  record.revisions.push(revision);
  return response;
}

async function update(ctx, record, suffix = ' changed') {
  const body = {draft_version: record.doc.draft_version, name: record.doc.name + suffix,
    description: 'Changed synthetic description', content: structuredClone(record.doc.draft)};
  if (record.kind === 'agent') body.content.system_prompt += suffix;
  if (record.kind === 'workspace') body.content.name += '-changed';
  if (record.kind === 'sandbox') body.content.max_agents_per_workspace += 1;
  const response = await request(ctx, 'PUT', at(record.kind, record.doc.id), body, {key: key()});
  ctx.assert(response.body.draft_version === record.doc.draft_version + 1
    && response.body.name === body.name && equal(response.body.draft, body.content), 'profile_changed_draft');
  record.doc = response.body;
  return response;
}

async function visibility(ctx, record, value) {
  const response = await request(ctx, 'PUT', at(record.kind, record.doc.id) + '/visibility',
    {visibility: value}, {key: key()});
  ctx.assert(response.body.id === record.doc.id && response.body.visibility === value, 'profile_visibility');
  record.doc = response.body;
  return response;
}

async function fork(ctx, record) {
  let doc, dispatched = false;
  const forkKey = key(), retireKey = key();
  ctx.defer('retire_profile_fork', async () => {
    if (!dispatched) return;
    check(doc?.id, 'profile_fork_custody_unknown');
    const retired = await cleanupRequest(ctx, 'POST', at(record.kind, doc.id) + '/retire', undefined, {key: retireKey});
    check(retired.id === doc.id && typeof retired.retired_at === 'string', 'fork_cleanup_not_retired');
  });
  const options = {key: forkKey, allowed: [201]};
  let response;
  try {
    dispatched = true;
    response = await request(ctx, 'POST', market(record.kind, record.doc.id) + '/fork', {revision: 1}, options);
    doc = response.body;
  } catch (error) { doc = error.response?.body; throw error; }
  ctx.assert(doc.id !== record.doc.id && doc.visibility === 'private'
    && equal(doc.forked_from, {id: record.doc.id, revision: 1}), 'fork_lineage_private');
  ctx.assert(equal(doc.draft, record.revisions[0].content), 'fork_pins_published_content');
  const replay = await request(ctx, 'POST', market(record.kind, record.doc.id) + '/fork', {revision: 1},
    {...options, am: otherAM(ctx.am)});
  ctx.assert(equal(replay.body, doc), 'fork_idempotent_across_am');
  return response;
}

async function graph(ctx) {
  const bounds = (await request(ctx, 'GET', '/api/v1/profile-authoring/bounds')).body;
  ctx.assert(Number.isSafeInteger(bounds.min_cpu_millis) && bounds.min_cpu_millis > 0
    && Number.isSafeInteger(bounds.min_memory_mib) && bounds.min_memory_mib > 0, 'profile_capacity_bounds');
  const result = {};
  for (const kind of kinds) {
    result[kind] = await createProfile(ctx, kind, {name: 'API coverage ' + kind + ' ' + key(),
      description: 'Disposable synthetic coverage', content: contents(kind, result, bounds)});
    await publish(ctx, result[kind]);
  }
  return result;
}

async function eventWorker(ctx, profiles) {
  const source = {name: 'API event worker ' + key(), description: 'Synthetic coverage',
    ...Object.fromEntries(kinds.map(kind => [kind + '_profile', {id: profiles[kind].doc.id, revision: 1}]))};
  let doc, dispatched = false, retired = false;
  const createKey = key();
  ctx.defer('retire_event_worker', async () => {
    if (!dispatched || retired) return;
    check(doc?.id, 'event_worker_create_custody_unknown');
    // Read current revision without replaying a failed update or delete.
    const current = await cleanupRequest(ctx, 'GET', '/api/v1/event-worker-profiles/' + segment(doc.id));
    if (current.retired_at) return;
    const result = await cleanupRequest(ctx, 'DELETE', '/api/v1/event-worker-profiles/' + segment(doc.id)
      + '?expected_revision=' + current.revision);
    check(result.id === doc.id && typeof result.retired_at === 'string', 'event_worker_cleanup_not_retired');
  });
  try {
    dispatched = true;
    doc = (await request(ctx, 'POST', '/api/v1/event-worker-profiles', source, {key: createKey, allowed: [201]})).body;
  } catch (error) { doc = error.response?.body; throw error; }
  ctx.assert(doc.revision === 1 && equal(doc.source, source), 'event_worker_source');
  const result = {doc, source, createKey, initial: structuredClone(doc),
    async update() {
      const changed = {...source, name: source.name + ' changed'};
      const response = await request(ctx, 'PUT', '/api/v1/event-worker-profiles/' + segment(doc.id),
        {expected_revision: doc.revision, source: changed});
      ctx.assert(response.body.id === doc.id && response.body.revision === doc.revision + 1
        && equal(response.body.source, changed), 'event_worker_updated_revision');
      doc = result.doc = response.body;
      return response;
    },
    async retire() {
      const response = await request(ctx, 'DELETE', '/api/v1/event-worker-profiles/' + segment(doc.id)
        + '?expected_revision=' + doc.revision);
      ctx.assert(response.body.id === doc.id && typeof response.body.retired_at === 'string', 'event_worker_retired');
      doc = result.doc = response.body;
      retired = true;
      return response;
    }};
  return result;
}

async function isolation(ctx, record) {
  const denied = await request(ctx, 'GET', at(record.kind, record.doc.id), undefined,
    {actor: 'outsider', am: otherAM(ctx.am), allowed: [403, 404]});
  ctx.assert(['forbidden', 'profile_not_found'].includes(denied.body.code), 'private_profile_denied');
  const visible = await pages(ctx, base(record.kind), {actor: 'outsider', am: otherAM(ctx.am)});
  ctx.assert(!visible.some(item => item.id === record.doc.id), 'profile_owner_list_isolation');
}

async function immutable(ctx, record) {
  for (const original of record.revisions) {
    const readback = await request(ctx, 'GET', at(record.kind, record.doc.id) + '/revisions/' + original.revision,
      undefined, {am: otherAM(ctx.am)});
    ctx.assert(equal(readback.body, original), 'published_revision_immutable_across_am');
  }
}

// Exercise the whole original family journey before the exact operation. These
// are actual public requests, not fixture-provided boolean verdicts.
async function lifecycle(ctx) {
  const profiles = await graph(ctx);
  for (const record of Object.values(profiles)) {
    const replay = await request(ctx, 'POST', base(record.kind), record.body,
      {key: record.createKey, allowed: [201], am: otherAM(ctx.am)});
    ctx.assert(replay.body.id === record.doc.id && equal(replay.body.draft, record.body.content), 'profile_same_create_identity');
    const collision = await request(ctx, 'POST', base(record.kind), {...record.body, name: record.body.name + ' collision'},
      {key: record.createKey, allowed: [409], am: otherAM(ctx.am)});
    ctx.assert(collision.body.code === 'idempotency_key_reused', 'profile_changed_intent_refused');
    const list = await pages(ctx, base(record.kind));
    ctx.assert(list.some(item => item.id === record.doc.id), 'created_profile_listed');
    const get = await request(ctx, 'GET', at(record.kind, record.doc.id), undefined, {am: otherAM(ctx.am)});
    ctx.assert(get.body.id === record.doc.id && equal(get.body.draft, record.doc.draft), 'profile_cross_am_read');
    await isolation(ctx, record);
    const previous = record.doc.draft_version;
    await update(ctx, record);
    const stale = await request(ctx, 'PUT', at(record.kind, record.doc.id),
      {...record.body, draft_version: previous}, {key: key(), allowed: [409]});
    ctx.assert(stale.body.code === 'profile_version_conflict', 'profile_stale_revision_refused');
    await immutable(ctx, record);
    await publish(ctx, record);
    const revisions = await pages(ctx, at(record.kind, record.doc.id) + '/revisions');
    ctx.assert(revisions.length === 2 && revisions.every(item => item.profile_id === record.doc.id), 'profile_revision_history');
    await visibility(ctx, record, 'public');
    const entry = await request(ctx, 'GET', market(record.kind, record.doc.id), undefined, {actor: 'outsider', am: otherAM(ctx.am)});
    ctx.assert(entry.body.summary.id === record.doc.id && entry.body.revision.profile_id === record.doc.id
      && equal(entry.body.revision, record.revisions[1]), 'marketplace_published_snapshot');
    await fork(ctx, record);
    await visibility(ctx, record, 'private');
    const hidden = await request(ctx, 'GET', market(record.kind, record.doc.id), undefined, {actor: 'outsider', allowed: [404]});
    ctx.assert(hidden.body.code === 'profile_not_found', 'private_marketplace_hidden');
  }
  const worker = await eventWorker(ctx, profiles);
  const replay = await request(ctx, 'POST', '/api/v1/event-worker-profiles', worker.source,
    {key: worker.createKey, am: otherAM(ctx.am), allowed: [201]});
  ctx.assert(equal(replay.body, worker.doc), 'event_worker_idempotent_create');
  const list = await request(ctx, 'GET', '/api/v1/event-worker-profiles?limit=50');
  ctx.assert(list.body.items.some(item => item.id === worker.doc.id), 'event_worker_listed');
  await worker.update();
  const old = await request(ctx, 'GET', '/api/v1/event-worker-profiles/' + segment(worker.doc.id) + '/revisions/1',
    undefined, {am: otherAM(ctx.am)});
  ctx.assert(equal(old.body, worker.initial), 'event_worker_revision_immutable');
  const denied = await request(ctx, 'PUT', '/api/v1/event-worker-profiles/' + segment(worker.doc.id),
    {expected_revision: worker.doc.revision, source: worker.source}, {actor: 'outsider', allowed: [403,404]});
  ctx.assert(['event_worker_profile_forbidden', 'event_worker_profile_not_found'].includes(denied.body.code), 'event_worker_owner_boundary');
  await worker.retire();
  for (const kind of [...kinds].reverse()) await profiles[kind].retire();
  return profiles;
}

async function exact(ctx, row) {
  const [method, template] = row.entry_point.split(' ');
  const profiles = await graph(ctx);
  const kind = kinds.find(value => template.includes('/' + value + '-profiles')) ?? 'agent';
  const record = profiles[kind];
  let response, path = template.replace('{kind}', kind + '-profiles').replace('{id}', segment(record.doc.id)).replace('{revision}', '1');
  if (template.includes('event-worker-profiles')) {
    const worker = await eventWorker(ctx, profiles);
    path = template.replace('{id}', segment(worker.doc.id)).replace('{revision}', '1');
    if (method === 'POST') response = await request(ctx, method, path, worker.source,
      {key: worker.createKey, am: otherAM(ctx.am), allowed: [201]});
    else if (method === 'PUT') response = await worker.update();
    else if (method === 'DELETE') response = await worker.retire();
    else response = await request(ctx, method, path);
    ctx.assert(template.endsWith('event-worker-profiles') && method === 'GET'
      ? response.body.items.some(item => item.id === worker.doc.id)
      : response.body.id === worker.doc.id, 'exact_event_worker_identity');
    return {profiles, worker, response};
  }
  if (template.includes('/marketplace/')) {
    await visibility(ctx, record, 'public');
    if (method === 'POST') response = await fork(ctx, record);
    else {
      response = await request(ctx, method, path);
      ctx.assert(template.endsWith('{kind}')
        ? response.body.items.some(item => item.id === record.doc.id)
        : response.body.summary.id === record.doc.id && equal(response.body.revision, record.revisions[0]), 'exact_marketplace_identity');
    }
  } else if (template.includes('/profile-authoring/')) {
    const fixture = await take(ctx, 'profile-authoring');
    if (method === 'POST') {
      response = await request(ctx, method, path, fixture.repositoryRequest);
      ctx.assert(response.body.url === fixture.repositoryRequest.url && response.body.reference === fixture.repositoryRequest.reference
        && response.body.commit_sha === fixture.repositoryCommit && /^[a-f0-9]{40}$/.test(response.body.commit_sha), 'repository_exact_commit');
    } else {
      response = await request(ctx, method, path);
      ctx.assert(Array.isArray(response.body) && equal(response.body, fixture.services), 'authoring_service_catalog');
    }
  } else if (method === 'POST' && !template.includes('{id}')) {
    response = await request(ctx, method, path, record.body, {key: record.createKey, am: otherAM(ctx.am), allowed: [201]});
    ctx.assert(response.body.id === record.doc.id, 'exact_profile_create_receipt');
  } else if (method === 'PUT' && template.endsWith('/visibility')) response = await visibility(ctx, record, 'public');
  else if (method === 'PUT') response = await update(ctx, record);
  else if (method === 'POST' && template.endsWith('/retire')) response = {body: await record.retire(), status: 200};
  else if (method === 'POST') response = await publish(ctx, record);
  else {
    response = await request(ctx, method, path);
    if (template.endsWith('/revisions')) ctx.assert(response.body.items.some(value => equal(value, record.revisions[0])), 'exact_revision_list');
    else if (template.endsWith('{revision}')) ctx.assert(equal(response.body, record.revisions[0]), 'exact_revision_read');
    else if (template.endsWith('{id}')) ctx.assert(response.body.id === record.doc.id && equal(response.body.draft, record.doc.draft), 'exact_profile_read');
    else ctx.assert(response.body.items.some(value => value.id === record.doc.id), 'exact_profile_list');
  }
  return {profiles, response};
}

async function retainedAfterRestart(ctx) {
  if (ctx.phase !== 'after_restart') return;
  const retained = ctx.runState.profileHistory;
  ctx.assert(Array.isArray(retained) && retained.length === 3, 'before_restart_profile_history_required');
  for (const record of retained) {
    const replay = await request(ctx, 'POST', base(record.kind), record.body,
      {key: record.createKey, allowed: [201]});
    ctx.assert(replay.body.id === record.id && equal(replay.body.draft, record.body.content), 'original_profile_receipt_survives_restart');
    for (const revision of record.revisions) {
      const readback = await request(ctx, 'GET', at(record.kind, record.id) + '/revisions/' + revision.revision,
        undefined, {am: otherAM(ctx.am)});
      ctx.assert(equal(readback.body, revision), 'original_profile_revision_survives_restart');
    }
  }
}

export const profileCases = ids.map(number => caseOf(number, 'global',
  [47,48].includes(number) ? ['api-fixtures'] : [], async ctx => {
    let history, focus;
    await ctx.step('step_1', async () => {
      await retainedAfterRestart(ctx);
      history = await lifecycle(ctx);
      if (ctx.phase === 'before_restart') ctx.runState.profileHistory = Object.values(history).map(record => ({
        kind: record.kind, id: record.doc.id, body: record.body, createKey: record.createKey,
        revisions: structuredClone(record.revisions),
      }));
    });
    await ctx.step('step_2', async () => { focus = await exact(ctx, ctx.row); });
    await ctx.step('step_3', async () => {
      for (const record of Object.values(focus.profiles)) {
        await immutable(ctx, record);
        if (!record.doc.retired_at) {
          await visibility(ctx, record, 'private');
          await isolation(ctx, record);
        }
      }
      if (focus.worker) {
        const readback = await request(ctx, 'GET', '/api/v1/event-worker-profiles/' + segment(focus.worker.doc.id),
          undefined, {am: otherAM(ctx.am)});
        ctx.assert(equal(readback.body, focus.worker.doc), 'event_worker_cross_am_readback');
      }
    });
    await ctx.step('expected_result', async () => {
      for (const record of Object.values(history)) ctx.assert(typeof record.doc.retired_at === 'string', 'family_retired');
      for (const record of Object.values(focus.profiles)) {
        await immutable(ctx, record);
        await record.retire();
      }
      await ctx.observe('profile_contract_complete', async () => ({scope: 'global', agent_owner: null,
        profile_kinds: kinds, immutable_revisions: 6, same_intent_replayed: true}));
    });
  }));
