import {artifact,hash,identity,requireValue,same} from './migration_agent_observations.mjs';
import {RetainedAgentJourney,STAGES} from './migration_agent_journey.mjs';
import {AgentCleanup} from './migration_agent_cleanup.mjs';
import {LocalAgentApi,localOrigin} from './migration_agent_http.mjs';
import {LocalProviderControl,nativeAgentAdapter} from './migration_agent_adapter.mjs';
import {NativeObserver} from './migration_agent_kubernetes.mjs';
import {PrivateAgentJournal} from './migration_agent_journal.mjs';

const safeError=error=>/^[a-z0-9_]{1,100}$/.test(error?.message ?? '')
  ? error.message : 'session_transport_or_evidence_failed';
const publicStage=result=>({schema:result.schema,stage:result.stage,artifact:result.artifact,
  reader_sha256:hash(result.reader),agents:result.agents,outcome:result.outcome,
  persisted_collection_coverage:result.persisted_collection_coverage,
  full_reader_writer_gate:result.full_reader_writer_gate});
export function exactKeys(value,required,optional=[]) {
  requireValue(value && typeof value==='object' && !Array.isArray(value)
    && required.every(k=>Object.hasOwn(value,k))
    && Object.keys(value).every(k=>required.includes(k)||optional.includes(k)),'session_command_shape_invalid');
}

export function bootBinding(stage,binding,boot,expected,last) {
  exactKeys(binding,['artifact','podName','podUid'],['localImageAlias']);
  requireValue(same(artifact(binding.artifact),expected) && identity(binding.podName)
    && identity(binding.podUid),'session_reader_identity_mismatch');
  const image=binding.localImageAlias ?? expected.image;
  requireValue(image===expected.image || /^cma-matrix-(old|new)-[a-f0-9]{12}(?::latest)?$/.test(image),
    'session_reader_alias_invalid');
  if(stage==='new-write') {
    requireValue(boot===null && same(binding,last),'session_new_write_reader_changed');
    return;
  }
  requireValue(boot?.schema==='cma-migration-boot-observation-v1' && boot.failed_boots===0
    && typeof boot.start_resource_version==='string' && boot.start_resource_version.length>0
    && boot.selected_uid===binding.podUid && (!last || last.podUid!==binding.podUid)
    && Array.isArray(boot.observations) && boot.observations.length>0 && boot.observations.length<=2048
    && Array.isArray(boot.attempted_uids) && Array.isArray(boot.ready_uids),'session_clean_boot_required');
  const added=new Set(),ready=new Set(),attempted=new Set();
  for(const row of boot.observations) {
    requireValue(identity(row.uid) && identity(row.name) && row.image===image
      && row.restart_count===0 && typeof row.ready==='boolean'
      && typeof row.resource_version==='string' && row.resource_version.length>0
      && ['ADDED','MODIFIED','DELETED'].includes(row.type)
      && !['Failed','Succeeded'].includes(row.phase),'session_boot_observation_invalid');
    if(row.type==='ADDED') added.add(row.uid);
    else requireValue(added.has(row.uid),'session_boot_creation_missing');
    attempted.add(row.uid);
    if(row.ready) {
      requireValue(row.phase==='Running','session_boot_ready_invalid');ready.add(row.uid);
    }
    if(row.type==='DELETED') requireValue(ready.has(row.uid),'session_failed_boot_observed');
    if(row.uid===binding.podUid) requireValue(row.name===binding.podName,'session_boot_pod_changed');
  }
  requireValue(ready.has(binding.podUid) && same([...attempted].sort(),boot.attempted_uids)
    && same([...ready].sort(),boot.ready_uids) && same(boot.ready_uids,boot.attempted_uids),
  'session_boot_set_mismatch');
}

export class AgentSession {
  #artifacts; #api; #observer; #provider; #journal; #journey; #cleanup;
  #index=0; #failed=false; #active=false; #finished=false; #last=null; #stages=[];
  #controller=new AbortController();
  constructor({artifacts,api,observer,provider,journal,owner,journeyFactory=opts=>new RetainedAgentJourney(opts),
    cleanupFactory=opts=>new AgentCleanup(opts)}) {
    this.#artifacts=Object.fromEntries(['old','new','runtime'].map(k=>[k,artifact(artifacts?.[k])]));
    requireValue(this.#artifacts.old.image!==this.#artifacts.new.image,'session_distinct_readers_required');
    requireValue(identity(owner) && typeof journal?.append==='function'
      && typeof journal.close==='function','session_journal_required');
    this.#api=api;this.#observer=observer;this.#provider=provider;this.#journal=journal;
    const record=async event=>this.#journal.append(event,this.#journey?.resources() ?? []);
    this.#journey=journeyFactory({artifacts:this.#artifacts,api,signal:this.#controller.signal,
      adapter:nativeAgentAdapter({api,observer,provider}),observe:record});
    this.#cleanup=cleanupFactory({api,owner,record});
  }
  resources() {return this.#journey.resources();}
  async interrupt() {
    this.#failed=true;this.#controller.abort();
    return this.#api.close();
  }
  async stage(command) {
    requireValue(!this.#finished && !this.#failed && !this.#active,'session_stage_refused');
    this.#active=true;
    try {
      exactKeys(command,['stage','origin','reader','boot']);
      requireValue(command.stage===STAGES[this.#index],'session_stage_order_invalid');
      const expected=this.#artifacts[this.#index===0 || this.#index===3 ? 'old' : 'new'];
      localOrigin(command.origin);
      bootBinding(command.stage,command.reader,command.boot,expected,this.#last);
      await this.#journal.append({type:'session_stage_binding',binding:command},this.resources());
      this.#api.rebind(command.origin);this.#observer.bindReader(command.reader);
      const result=await this.#journey.run(command.stage);
      this.#last=structuredClone(command.reader);this.#index++;this.#stages.push(publicStage(result));
      await this.#journal.append({type:'session_stage_result',result},this.resources());
      return {schema:'migration-agent-session-stage-v1',stage:command.stage,result:publicStage(result),
        resources_sha256:hash(this.resources()),journal:this.#journal.summary()};
    } catch(error) {
      this.#failed=true;
      await this.#journal.append({type:'session_failed',reason:safeError(error)},this.resources());
      throw error;
    } finally {this.#active=false;}
  }
  async finish(reason='explicit_finish') {
    requireValue(!this.#active && !this.#finished,'session_finish_once_required');
    requireValue(/^[a-z_]{1,100}$/.test(reason),'session_finish_reason_invalid');
    this.#finished=true;
    const failures=[];let streams,cleanup;
    try {
      try {
        streams=await this.#api.close();
        if(streams?.joined!==0 || streams?.failed!==0) failures.push('session_stream_finality_failed');
      } catch(error) {failures.push(safeError(error));}
      // Release only admitted case IDs. A release merely unblocks a synthetic
      // provider response; failures remain visible and do not trigger replay.
      for(const resource of this.resources()) for(const turn of resource.turns ?? []) {
        try {
          const state=await this.#provider.oracle({caseId:turn.case_id});
          if(state.released!==true) await this.#provider.releaseTurn({caseId:turn.case_id});
        } catch(error) {failures.push(safeError(error));}
      }
      cleanup=await this.#cleanup.run(this.resources());
      const complete=this.#index===STAGES.length && !this.#failed;
      const result={schema:'migration-agent-session-final-v1',reason,
        outcome:complete && cleanup.outcome==='PUBLIC_CLEANUP_OBSERVED' && failures.length===0
          ? 'SESSION_OBSERVED' : 'INCOMPLETE',
        stages:this.#stages,cleanup,streams,failures,
        persisted_collection_coverage:'NOT_PROVEN',full_reader_writer_gate:'NOT_PROVEN'};
      await this.#journal.append({type:'session_finished',result},this.resources());
      return {...result,journal:this.#journal.summary()};
    } finally {await this.#journal.close();}
  }
}

export async function createSession(config) {
  exactKeys(config,['artifacts','directory','origin','bearer','owner','providerOrigin','providerToken',
    'kubeconfig','context','server','namespaceUid']);
  exactKeys(config.artifacts,['old','new','runtime']);
  for(const value of Object.values(config.artifacts)) artifact(value);
  localOrigin(config.origin);localOrigin(config.providerOrigin);
  const journal=await PrivateAgentJournal.create(config.directory,[config.bearer,config.providerToken]);
  let api,session;
  try {
    const record=async event=>journal.append(event,session?.resources() ?? []);
    api=new LocalAgentApi({origin:config.origin,bearer:config.bearer,observe:record});
    const observer=new NativeObserver({kubeconfig:config.kubeconfig,context:config.context,
      server:config.server,namespaceUid:config.namespaceUid,record});
    await observer.initialize(config.directory);
    const provider=new LocalProviderControl({origin:config.providerOrigin,token:config.providerToken});
    session=new AgentSession({artifacts:config.artifacts,api,observer,provider,journal,owner:config.owner});
    return session;
  } catch(error) {
    try {if(api) await api.close();}
    finally {await journal.close();}
    throw error;
  }
}
