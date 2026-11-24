import {performance} from 'node:perf_hooks';
import {hash,identity,requireValue} from './migration_agent_observations.mjs';

const safeError=error=>/^[a-z0-9_]{1,100}$/.test(error?.message ?? '')
  ? error.message : 'cleanup_transport_or_evidence_failed';
const absent=response=>response.status===404 || response.body?.status==='deleted';

export class AgentCleanup {
  #api; #owner; #record; #now; #sleep; #timeout; #used=false;
  constructor({api,owner,record,timeout=180000,now=()=>performance.now(),
    sleep=ms=>new Promise(resolve=>setTimeout(resolve,ms))}) {
    requireValue(typeof api?.request==='function' && identity(owner) && typeof record==='function'
      && Number.isInteger(timeout) && timeout>=2000 && timeout<=300000,'cleanup_contract_invalid');
    this.#api=api;this.#owner=owner;this.#record=record;this.#timeout=timeout;this.#now=now;this.#sleep=sleep;
  }
  async #poll(read,accept,reason) {
    const deadline=this.#now()+this.#timeout;
    for(;;) {
      const value=await read();
      if(accept(value)) return value;
      requireValue(this.#now()<deadline,reason);
      await this.#sleep(2000);
    }
  }
  async #request(journal,method,path,options) {
    if(method!=='GET') {
      journal.writes.push({method,path,state:'INTENT',key:options.key});
      // Intent must be durable before transmission. A failed observation after
      // admission leaves this intent unresolved; this method never retries it.
      await this.#record({type:'cleanup_write_intent',journal:structuredClone(journal)});
    }
    const response=await this.#api.request('a',method,path,options);
    if(method==='GET') journal.last_reads[path]=response;
    else Object.assign(journal.writes.at(-1),{state:'RESPONSE_OBSERVED',response});
    await this.#record({type:'cleanup_response',method,path,response,journal:structuredClone(journal)});
    return response;
  }
  async #inventory(journal,sandboxId) {
    let cursor;const cursors=new Set(),ids=new Set();
    for(let page=0;page<8;page++) {
      const query=new URLSearchParams({limit:'200',...(cursor ? {cursor} : {})});
      const response=await this.#request(journal,'GET','/api/v1/agents?'+query);
      const body=response.body;
      requireValue(Array.isArray(body?.items) && body.items.length<=200,'cleanup_inventory_invalid');
      for(const item of body.items) {
        requireValue(identity(item?.id) && identity(item.sandbox_id) && typeof item.status==='string'
          && !ids.has(item.id),'cleanup_inventory_item_invalid');
        ids.add(item.id);
        requireValue(item.sandbox_id!==sandboxId || item.status==='deleted','cleanup_sandbox_has_live_agent');
      }
      if(body.next===null || body.next===undefined) return;
      requireValue(typeof body.next==='string' && /^[a-zA-Z0-9_=-]{1,256}$/.test(body.next)
        && !cursors.has(body.next),'cleanup_inventory_cursor_invalid');
      cursor=body.next;cursors.add(cursor);
    }
    throw new Error('cleanup_inventory_incomplete');
  }
  async #one(resource) {
    const journal={key:resource.key,operation_id:resource.operation_id,agent_id:resource.agent_id,
      sandbox_id:resource.lineage?.sandbox_id ?? null,writes:[],last_reads:{},outcome:'UNRESOLVED'};
    try {
      requireValue(identity(resource.key) && typeof resource.name==='string'
        && identity(resource.operation_id),'cleanup_admission_identity_unresolved');
      const operation=await this.#request(journal,'GET','/api/v1/agent-lifecycle/'+resource.operation_id);
      requireValue(operation.body?.id===resource.operation_id && identity(operation.body.agent_id)
        && (!resource.agent_id || resource.agent_id===operation.body.agent_id),'cleanup_lifecycle_identity_mismatch');
      journal.agent_id=operation.body.agent_id;
      const agentPath='/api/v1/agents/'+journal.agent_id;
      const current=await this.#request(journal,'GET',agentPath,{allowed:[200,404]});
      if(current.status!==404) {
        requireValue(current.body?.id===journal.agent_id && identity(current.body.sandbox?.id),
          'cleanup_agent_identity_mismatch');
        requireValue(!journal.sandbox_id || current.body.sandbox.id===journal.sandbox_id,
          'cleanup_sandbox_identity_mismatch');
        journal.sandbox_id=current.body.sandbox.id;
      }
      requireValue(identity(journal.sandbox_id),'cleanup_sandbox_identity_unresolved');
      const sandboxPath='/api/v1/sandboxes/'+journal.sandbox_id;
      const sandbox=await this.#request(journal,'GET',sandboxPath);
      const suffix=' sandbox '+resource.operation_id;
      requireValue(Buffer.byteLength(suffix)<=64 && /^[a-zA-Z0-9_-]+$/.test(resource.name),
        'cleanup_name_identity_invalid');
      const name=resource.name.slice(0,64-Buffer.byteLength(suffix))+suffix;
      requireValue(sandbox.body?.id===journal.sandbox_id && sandbox.body.owner===this.#owner
        && sandbox.body.name===name,'cleanup_sandbox_owner_or_operation_mismatch');
      if(!absent(current)) {
        if(current.body.status!=='stopped') {
          await this.#request(journal,'POST',agentPath+'/stop',{
            body:{},key:resource.key+'-stop',allowed:[200,202,204]});
          await this.#poll(async()=>{
            const value=await this.#request(journal,'GET',agentPath);
            requireValue(value.body?.id===journal.agent_id
              && value.body.sandbox?.id===journal.sandbox_id,'cleanup_agent_identity_changed');
            return value;
          },value=>value.body.status==='stopped','cleanup_stop_timeout');
        }
        await this.#request(journal,'DELETE',agentPath,{key:resource.key+'-delete',allowed:[200,202,204]});
        await this.#poll(async()=>{
          const value=await this.#request(journal,'GET',agentPath,{allowed:[200,404]});
          requireValue(value.status===404 || value.body?.id===journal.agent_id,'cleanup_agent_identity_changed');
          return value;
        },absent,'cleanup_delete_timeout');
      }
      await this.#inventory(journal,journal.sandbox_id);
      if(sandbox.body.status!=='retired') {
        await this.#request(journal,'DELETE',sandboxPath,{key:resource.key+'-retire',allowed:[200,202,204]});
        await this.#poll(async()=>{
          const value=await this.#request(journal,'GET',sandboxPath,{allowed:[200,404]});
          requireValue(value.status===404 || (value.body?.id===journal.sandbox_id
            && value.body.owner===this.#owner),'cleanup_sandbox_identity_changed');
          return value;
        },value=>value.status===404 || value.body.status==='retired','cleanup_retire_timeout');
      }
      journal.outcome='PUBLIC_CLEANUP_OBSERVED';
    } catch(error) {journal.failure=safeError(error);}
    await this.#record({type:'cleanup_resource_finished',journal:structuredClone(journal)});
    return journal;
  }
  async run(resources) {
    requireValue(!this.#used && Array.isArray(resources) && resources.length<=2,'cleanup_once_required');
    this.#used=true;
    requireValue(new Set(resources.map(r=>r?.key)).size===resources.length
      && new Set(resources.map(r=>r?.operation_id).filter(Boolean)).size===resources.filter(r=>r?.operation_id).length
      && new Set(resources.map(r=>r?.agent_id).filter(Boolean)).size===resources.filter(r=>r?.agent_id).length,
    'cleanup_duplicate_resource');
    const journals=[];
    for(const resource of structuredClone(resources)) {
      try {journals.push(await this.#one(resource));}
      catch(error) {journals.push({outcome:'UNRESOLVED',failure:safeError(error),key:resource?.key});}
    }
    return {schema:'migration-agent-public-cleanup-v1',
      outcome:journals.every(row=>row.outcome==='PUBLIC_CLEANUP_OBSERVED') ? 'PUBLIC_CLEANUP_OBSERVED' : 'UNRESOLVED',
      resources:journals.map(row=>({key_sha256:hash(row.key ?? null),agent_sha256:hash(row.agent_id ?? null),
        sandbox_sha256:hash(row.sandbox_id ?? null),outcome:row.outcome,failure:row.failure ?? null,
        writes_observed:row.writes?.filter(w=>w.state==='RESPONSE_OBSERVED').length ?? 0,
        unknown_write:row.writes?.some(w=>w.state==='INTENT') ?? false})),
      infrastructure_teardown:'NOT_PROVEN'};
  }
}
