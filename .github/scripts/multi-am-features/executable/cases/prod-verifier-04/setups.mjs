import {Blocked} from '../../runner.mjs';
import {check, hash, key, otherAM, request, segment} from './common.mjs';

function requireLocal(primitives) {
  if (primitives?.disposableDatabase?.scope !== 'LOCAL_DISPOSABLE_TWO_AM'
    || primitives.disposableDatabase.database_disposable !== true) throw new Blocked('api_local_disposable_database_required');
}

// Public preparation belongs here; only isolated identities and cleanup custody
// come from the parent. The parent must prove disposal before campaign PASS.
export async function prepareDocuments(ctx, primitives) {
  requireLocal(primitives);
  const database = primitives.disposableDatabase, identities = primitives.identities;
  for (const actor of ['user','recipient','outsider']) {
    if (identities?.[actor]?.actor !== actor || typeof identities[actor].subject !== 'string'
      || !identities[actor].subject) throw new Blocked('api_document_identity_required');
  }
  if (new Set(Object.values(identities).filter(value => ['user','recipient','outsider'].includes(value.actor))
    .map(value => value.subject)).size !== 3) throw new Blocked('api_document_distinct_actors_required');
  if (typeof database.reserve !== 'function') throw new Blocked('api_document_reservation_required');
  const lease = await database.reserve({purpose:'api-documents',caseId:ctx.row.case_id});
  let released = false;
  const cleanup = async () => {
    if (released) return;
    check(typeof lease?.release === 'function', 'api_document_lease_release_required');
    const receipt = await lease.release();
    check(receipt?.registered === true && receipt.parent_cleanup_barrier === true, 'api_document_custody_unconfirmed');
    released = true;
    await ctx.observe('api_document_parent_custody', async () => ({registered:true,parent_cleanup_barrier:true,
      final_database_cleanup:'PARENT_MUST_CONFIRM_BEFORE_CAMPAIGN_PASS'}));
  };
  ctx.defer('api_document_setup_cleanup',cleanup);
  if (lease?.parent_cleanup_barrier !== true || typeof lease.retainIntent !== 'function'
    || typeof lease.retainDocument !== 'function') throw new Blocked('api_document_cleanup_barrier_required');
  const title = 'API87 synthetic document ' + key();
  const versions = [title+'\n\nFirst committed source.\n',title+'\n\nChanged committed source.\n'];
  const create = {actor:'user',am:ctx.am,method:'POST',path:'/api/v1/documents',key:key(),
    body:{title,markdown:versions[0]},allowed:[201]};
  await lease.retainIntent(create);
  let doc;
  try { doc = (await ctx.request(create)).body; }
  catch(error) {
    if (error.response?.body?.id) await lease.retainDocument(error.response.body.id);
    throw error;
  }
  check(typeof doc.id === 'string', 'api_document_creation_identity');
  await lease.retainDocument(doc.id);
  check(doc.owner_subject === identities.user.subject && doc.access === 'owner'
    && doc.sha256 === hash(versions[0]) && doc.source_revision === 1, 'api_document_create_content');
  const path = '/api/v1/documents/' + segment(doc.id);
  const revisions = [{revision:1,title,markdown:versions[0]}];
  doc = (await request(ctx,'PATCH',path,{expected_revision:doc.revision,markdown:versions[1]},
    {key:key(),am:otherAM(ctx.am)})).body;
  check(doc.sha256 === hash(versions[1]) && doc.source_revision === 2, 'api_document_changed_source');
  revisions.push({revision:2,title,markdown:versions[1]});
  doc = (await request(ctx,'PUT',path+'/shares/'+segment(identities.recipient.subject),
    {expected_revision:doc.revision,permission:'comment'},{key:key()})).body;
  const comments = [];
  for (const actor of ['user','recipient']) {
    const body = 'API87 '+actor+' synthetic comment';
    const result = (await request(ctx,'POST',path+'/comments',{body,document_revision:2},
      {actor,key:key(),allowed:[201],am:actor==='user'?ctx.am:otherAM(ctx.am)})).body;
    check(result.comment?.author_subject === identities[actor].subject && result.comment.body === body
      && result.comment.document_revision === 2, 'api_document_comment_attribution');
    comments.push(result.comment);
  }
  return {documentId:doc.id,owner:identities.user.subject,revisions,comments,cleanup};
}

export async function prepareProfileAuthoring(ctx, primitives) {
  requireLocal(primitives);
  // Expected commit/catalog originate in the parent's exact-image fixture
  // configuration, never in the API response that the case subsequently reads.
  const value = primitives.profileAuthoring;
  if (value?.source !== 'PARENT_PINNED_SYNTHETIC_CONFIGURATION'
    || typeof value.repositoryRequest?.url !== 'string'
    || !/^[a-f0-9]{40}$/.test(value.repositoryCommit ?? '') || !Array.isArray(value.services)) {
    throw new Blocked('api_profile_authoring_configuration_required');
  }
  const url = new URL(value.repositoryRequest.url);
  check(!url.username && !url.password && !url.search && !url.hash, 'api_repository_url_must_not_contain_credentials');
  return {repositoryRequest:structuredClone(value.repositoryRequest),repositoryCommit:value.repositoryCommit,
    services:structuredClone(value.services),async cleanup() {}};
}

function requireHierarchyPrimitives(primitives) {
  const value = primitives.userResources;
  if (value?.scope !== 'LOCAL_DISPOSABLE_TWO_AM' || value.database_disposable !== true
    || value.single_matrix_writer !== true || typeof value.reserve !== 'function'
    || typeof value.finality !== 'function' || typeof value.readNativeFacts !== 'function'
    || typeof value.waitForOwner !== 'function') throw new Blocked('api_hierarchy_parent_primitives_required');
  for (const field of ['defaults','sandbox_profile','workspace_profile','agent_profile']) {
    if (value[field] === undefined) throw new Blocked('api_hierarchy_' + field + '_required');
  }
  return value;
}

// Create the hierarchy through the public API. The parent supplies only
// disposable custody and independent lifecycle/finality observations; the case
// still verifies the returned nested parent joins and native effects.
export async function prepareHierarchy(ctx, primitives) {
  const parent = requireHierarchyPrimitives(primitives);
  const lease = await parent.reserve({purpose:'api-hierarchy',caseId:ctx.row.case_id});
  if (lease?.parent_cleanup_barrier !== true || typeof lease.retainIntent !== 'function'
    || typeof lease.retainResource !== 'function') throw new Blocked('api_hierarchy_cleanup_barrier_required');
  const resources = new Map(), intents = new Map();
  let released = false, cleanupPromise;
  const cleanup = async () => {
    if (released) return;
    if (cleanupPromise) return cleanupPromise;
    cleanupPromise = (async () => {
      const failures = [];
      try {
        const receipt = await lease.release();
        check(receipt?.released === true || receipt?.registered === true, 'api_hierarchy_release');
      } catch (error) { failures.push(error); }
      released = true;
      if (failures.length) throw new AggregateError(failures, 'api_hierarchy_cleanup_failed');
    })();
    return cleanupPromise;
  };
  ctx.defer('api_hierarchy_cleanup', cleanup);
  const cleanupResource = async (targetCtx, kind, id) => {
    const resource = resources.get(kind);
    check(resource?.id === id, 'api_hierarchy_cleanup_identity');
    const path = kind === 'sandbox' ? '/api/v1/sandboxes/' + segment(id)
      : kind === 'workspace' ? '/api/v1/workspaces/' + segment(id)
        : '/api/v1/agents/' + segment(id);
    const terminal = kind === 'sandbox' ? ['retired'] : kind === 'workspace' ? ['removed'] : ['deleted'];
    const route = kind === 'agent' ? {agentId:id} : {};
    let current = await request(targetCtx, 'GET', path, undefined,
      {...route, am:otherAM(targetCtx.am), allowed:[200]});
    check(current.body?.id === id, 'api_hierarchy_cleanup_read_identity');
    if (!terminal.includes(current.body.status)) {
      await request(targetCtx, 'DELETE', path, undefined,
        {...route, key:key(), am:otherAM(targetCtx.am), allowed:[200,202,204]});
    }
    for (let count = 0; count < 120; count += 1) {
      current = await request(targetCtx, 'GET', path, undefined,
        {...route, am:otherAM(targetCtx.am), allowed:[200]});
      check(current.body?.id === id, 'api_hierarchy_cleanup_read_identity');
      if (terminal.includes(current.body.status)) break;
      if (count === 119) throw Error('api_hierarchy_cleanup_deadline');
    }
    resource.cleaned = true;
    if (kind === 'sandbox') {
      const finality = await parent.finality({sandboxId:id});
      check(finality?.sandbox_id === id && finality.cleanup_complete === true
        && finality.children_remaining === 0 && finality.pending_operations === 0,
      'api_hierarchy_finality');
    }
  }
  return {
    defaults:structuredClone(parent.defaults),
    sandboxRequest:{sandbox_profile:structuredClone(parent.sandbox_profile),name:'api-hierarchy-sandbox-' + key()},
    workspaceRequest:{workspace_profile:structuredClone(parent.workspace_profile),name:'api-hierarchy-workspace-' + key()},
    agentRequest:{agent_profile:structuredClone(parent.agent_profile)},
    retainIntent:intent => lease.retainIntent(intent),
    retainResource:async entry => {resources.set(entry.kind,{...entry});return lease.retainResource(entry);},
    cleanup,cleanupResource,waitForOwner:id => parent.waitForOwner(id),
    readNativeFacts:() => parent.readNativeFacts(),
  };
}

// Remaining named setups are supplied by the real parent fixture adapter; this
// directory never forwards an opaque run(case) or returns a verdict oracle.
export const setups = {documents:prepareDocuments,'profile-authoring':prepareProfileAuthoring,hierarchy:prepareHierarchy};
export const requiredPrimitives = {
  documents:['disposableDatabase','identities.user','identities.recipient','identities.outsider'],
  'profile-authoring':['disposableDatabase','profileAuthoring'],
  hierarchy:['userResources.scope','userResources.reserve','userResources.retainIntent',
    'userResources.retainResource','userResources.finality','userResources.readNativeFacts','userResources.waitForOwner',
    'userResources.defaults','userResources.sandbox_profile','userResources.workspace_profile',
    'userResources.agent_profile'],
};
