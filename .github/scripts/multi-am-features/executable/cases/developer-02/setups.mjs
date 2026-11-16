import {need,api,poll,key,enc,hash} from './helpers.mjs';

export const scenarioNames = ['navigation','agent-history','agent-feedback','agent-details','agent-usage',
  'agent-skills','agent-stop','agent-delete','agent-files','agent-deliverables','desktop','browser',
  'trigger-draft','trigger-published','trigger-launch','trigger-recovery'];

// A leased record is never silently borrowed for destructive cases. Setup
// creates its own Agent via public lifecycle APIs and retains custody first.
export async function freshAgent(ctx, primitives) {
  const p = primitives['ui-runtime'];
  need(p?.scope === 'LOCAL_DISPOSABLE_TWO_AM' && p.exclusive === true
    && p.principal && /^[a-f0-9]{40}$/.test(p.webSource ?? '')
    && typeof p.runtimeFor === 'function' && typeof p.finality === 'function',
  'ui_runtime_primitives_missing');
  const intent = key();
  const record = {local:true,run_owned:true,principal:p.principal,webSource:p.webSource,
    intent, agentName:'coverage02-' + intent, createdByRun:true};
  let createAttempted = false, unknown = false, complete, cleanupPromise, retirementStarted = false;
  async function cleanup() {
    if (!createAttempted) return {complete:true,owned_only:true};
    if (unknown || !record.operationId) throw Error('ui_create_outcome_unresolved');
    const operation = await api(ctx,'/api/v1/agent-lifecycle/' + enc(record.operationId),{am:ctx.am});
    record.agentId ??= operation.body.agent_id;
    if (!record.agentId) throw Error('ui_cleanup_agent_unresolved');
    if (!record.sandboxId) record.sandboxId = (await api(ctx,'/api/v1/agents/' + enc(record.agentId),
      {agentId:record.agentId})).body.sandbox?.id;
    if (!record.sandboxId) throw Error('ui_cleanup_sandbox_unresolved');
    const path = '/api/v1/sandboxes/' + enc(record.sandboxId);
    const sandbox = (await api(ctx,path,{agentId:record.agentId})).body;
    ctx.assert(sandbox.owner === p.principal && sandbox.id === record.sandboxId,'ui_cleanup_owner');
    // An uncertain retirement is retained; do not issue a replacement command.
    if (!retirementStarted && !['retiring','retired'].includes(sandbox.status)) {
      retirementStarted = true;
      await api(ctx,path,{agentId:record.agentId,method:'DELETE',key:intent+'-cleanup',allowed:[200,202,204]});
    }
    await poll(ctx,path,b => b.id === record.sandboxId && b.status === 'retired',{am:ctx.am});
    const observation = await p.finality({agentId:record.agentId,sandboxId:record.sandboxId});
    ctx.assert(observation.agent_id === record.agentId && observation.sandbox_id === record.sandboxId
      && observation.children_remaining === 0 && observation.cleanup_complete === true,'ui_cleanup_finality');
    complete = {complete:true,owned_only:true,agent_sha256:hash(record.agentId),sandbox_sha256:hash(record.sandboxId)};
    return complete;
  }
  record.cleanup = () => complete ? Promise.resolve(complete) : (cleanupPromise ??= cleanup());
  ctx.defer('cleanup_partial_ui_setup',record.cleanup);
  const retain = r => {if (typeof r?.body?.id === 'string') record.operationId = r.body.id;};
  createAttempted = true;
  try {
    const r = await api(ctx,'/api/v1/agents',{am:ctx.am,method:'POST',key:intent,
      body:{name:record.agentName,machine_included:false,desktop:false},allowed:[202]});
    retain(r); ctx.assert(Boolean(record.operationId),'ui_setup_operation_receipt');
  } catch (error) {retain(error.response); unknown = !record.operationId; throw error;}
  await poll(ctx,'/api/v1/agent-lifecycle/' + enc(record.operationId),b => {
    ctx.assert(b.id === record.operationId && !['failed','waiting_for_connection'].includes(b.status),'ui_setup_lifecycle');
    record.agentId = b.agent_id; return b.status === 'succeeded' && Boolean(b.agent_id);
  },{am:ctx.am});
  const detail = await poll(ctx,'/api/v1/agents/' + enc(record.agentId),b => {
    ctx.assert(b.id === record.agentId && !['failed','deleted'].includes(b.status),'ui_setup_agent');
    return b.status === 'ready' && b.sandbox?.status === 'active';
  },{agentId:record.agentId});
  record.sandboxId = detail.sandbox.id;
  const runtime = await p.runtimeFor({agentId:record.agentId});
  ctx.assert(runtime.agent_id === record.agentId && runtime.ready === true
    && runtime.image === p.runtimeImage && Boolean(runtime.pod_uid),'ui_setup_exact_runtime');
  await ctx.observe('ui_setup_binding',async () => ({agent_sha256:hash(record.agentId),
    runtime_image:runtime.image,pod_uid_sha256:hash(runtime.pod_uid),web_source:p.webSource}));
  return record;
}

// Complex scenarios require parent-owned low-level provider/runtime fixtures.
// The factory returns data and cleanup, never a case verdict or step runner.
// Missing providers are explicit BLOCKED until the parent installs them.
async function prepared(ctx, primitives, name) {
  const factory = primitives['ui-scenarios']?.[name];
  need(typeof factory === 'function','ui_scenario_unavailable_' + name.replaceAll('-','_'));
  const value = await factory({ctx,freshAgent:() => freshAgent(ctx,primitives)});
  need(value?.local === true && value.run_owned === true && typeof value.cleanup === 'function',
    'ui_scenario_custody_missing');
  return value;
}
export const setups = Object.fromEntries(scenarioNames.map(name => [name,
  (ctx, primitives) => name === 'navigation' ? freshAgent(ctx,primitives) : prepared(ctx,primitives,name)]));
