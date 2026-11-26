import {AgentSession} from './migration_agent_session.mjs';
import {fixture,artifacts} from './migration_agent_test_fixture.mjs';
import {RetainedAgentJourney} from './migration_agent_journey.mjs';

export function sessionFixture() {
  const f=fixture();
  const journal={events:[],closed:false,fail:null,
    async append(event,resources) {
      this.events.push(structuredClone({event,resources}));
      if(this.fail===event.type) throw Error('journal_injected_failure');
    },summary(){return {records:this.events.length};},async close(){this.closed=true;}};
  const api=Object.assign(f.api,{rebinds:[],rebind(origin){this.rebinds.push(origin);},
    async close(){return {joined:0,failed:0};}});
  const observer={bindReader(binding){
      const version=binding.artifact.image===artifacts.old.image?'old':'new';
      f.switchReader(version,Number(binding.podUid.split('-').at(-1)));
    },reader:()=>f.adapter.reader(),runtimeFor:({agentId})=>f.adapter.runtimeFor({agentId})};
  const provider={prepareTurn:args=>f.adapter.prepareTurn(args),releaseTurn:args=>f.adapter.releaseTurn(args),
    oracle:async args=>({...await f.adapter.oracle(args),released:true})};
  const cleanups=[];
  const options={artifacts,api,observer,provider,journal,owner:'fixture-owner',
    journeyFactory:opts=>new RetainedAgentJourney({...opts,now:()=>f.state.clock,
      sleep:async ms=>{f.state.clock+=ms;},uuid:()=>String(++f.state.counter),timeout:2000}),
    cleanupFactory:()=>({async run(resources){cleanups.push(resources);return {outcome:'PUBLIC_CLEANUP_OBSERVED'};}})};
  const session=new AgentSession(options);
  return {...f,journal,api,observer,provider,cleanups,options,session};
}

export function stageCommand(stage) {
  const index=['old-write','new-read-old-write','new-write','old-read-new-write'].indexOf(stage);
  const version=index===0 || index===3?'old':'new',incarnation=index===0?0:index===3?2:1;
  const podUid='api-'+incarnation;
  return {stage,origin:'http://127.0.0.1:'+(14000+incarnation),
    reader:{artifact:artifacts[version],podName:'api',podUid},
    boot:index===2?null:{schema:'cma-migration-boot-observation-v1',failed_boots:0,
      start_resource_version:'1',selected_uid:podUid,attempted_uids:[podUid],ready_uids:[podUid],
      observations:[{type:'ADDED',uid:podUid,name:'api',resource_version:'2',
        image:artifacts[version].image,phase:'Running',restart_count:0,ready:true}]}};
}
