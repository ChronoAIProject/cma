// Offline protocol double only. It never supplies actual image/collection proof.
import {RetainedAgentJourney} from './migration_agent_journey.mjs';

export const digest = c => 'sha256:' + c.repeat(64);
export const artifacts = Object.fromEntries(['old','new','runtime'].map((key,i) => [key,{
  source:String(i + 1).repeat(40), image:'example.invalid/test/' + key + '@' + digest(String(i + 1)),
  config:digest(String(i + 4))}]));
const date = '2026-01-01T00:00:00Z';
const metadata = (name,uid) => ({name,uid,namespace:'migration-fixture'});

export function pod(name,uid,container,artifact,containerChar) {
  return {metadata:metadata(name,uid),spec:{containers:[{name:container,image:artifact.image}],
    volumes:[{name:'home',persistentVolumeClaim:{claimName:'home-' + uid}}]},
  status:{phase:'Running',conditions:[{type:'Ready',status:'True'}],containerStatuses:[{
    name:container,ready:true,restartCount:0,containerID:'containerd://' + containerChar.repeat(64),
    imageID:artifact.config,state:{running:{startedAt:date}}}]}};
}

export function fixture() {
  const state = {version:'old',incarnation:0,counter:0,clock:0,reads:0,records:new Map(),turns:new Map(),
    events:[],calls:[],failure:null,readerCalls:0,mutateReader:()=>{},mutateNative:()=>{},mutateTranscript:()=>{},
    mutateResponse:()=>{},mutateLifecycle:()=>{},mutateOracle:()=>{}};
  const api = {
    async request(am,method,path,options) {
      state.calls.push({am,method,path,options});
      if (state.failure === 'request') throw new Error('request_failed');
      let body;
      if (method === 'POST' && path === '/api/v1/agents') {
        const n = state.records.size + 1, id = 'agent-' + n, op = 'operation-' + n;
        body = {id:op};
        const detail = {id,status:'ready',created_at:date,desktop:false,
          sandbox:{id:'sandbox-' + n,status:'active'},workspace:{id:'workspace-' + n},
          worktree:{id:'worktree-' + n,path:'/home/cma/worktree-' + n}};
        state.records.set(id,{id,op,detail,history:{agent_id:id,through_seq:0,turns:[]},responses:[]});
        if (state.failure === 'lost_create') throw new Error('create_response_lost');
        if (state.failure === 'invalid_create') body = {};
      } else if (path.startsWith('/api/v1/agent-lifecycle/')) {
        const record = [...state.records.values()].find(r => path.endsWith('/' + r.op));
        body = {id:record.op,agent_id:record.id,status:'succeeded'};
        state.mutateLifecycle(body);
      } else if (path.startsWith('/api/v1/agents/')) {
        const id = path.split('/')[4], record = state.records.get(id);
        body = structuredClone(path.endsWith('/transcript') ? record.history : record.detail);
        if (path.endsWith('/transcript')) state.mutateTranscript(body);
      } else if (path.startsWith('/api/v2/responses/')) {
        body = structuredClone([...state.records.values()].flatMap(r => r.responses).find(r => path.endsWith('/' + r.id)));
        state.mutateResponse(body);
      } else throw new Error('unexpected_route');
      return {status:method === 'POST' ? 202 : 200,body,timing:{request_id:'synthetic-request-' + ++state.reads}};
    },
    async send(am,id,input,key) {
      state.calls.push({am,method:'SEND',id,input,key});
      if (state.failure === 'lost_send') throw new Error('send_response_lost');
      const turn = state.turns.get(key), record = state.records.get(id);
      const response = {id:'response-' + ++state.counter,conversation:{id},status:'in_progress',output:[]};
      record.responses.push(response);
      Object.assign(turn,{record,response,input,request_count:1,active_streams:1});
      return {responseId:response.id,done:state.failure === 'stream' ? Promise.reject(new Error('stream_failed')) : Promise.resolve(),
        abort() {state.calls.push({method:'ABORT',key});}};
    },
  };
  const adapter = {
    async reader() {
      state.readerCalls += 1;
      const value = {...artifacts[state.version],pod:pod('api','api-' + state.incarnation,'api',
        artifacts[state.version],String(state.incarnation + 1))};
      state.mutateReader(value); return value;
    },
    async runtimeFor({agentId}) {
      const record = state.records.get(agentId), n = agentId.split('-')[1];
      const nativePod = pod('runtime-' + n,'runtime-uid-' + n,'runtime',artifacts.runtime,'9');
      nativePod.metadata.ownerReferences = [{uid:'native-sandbox-' + n,controller:true}];
      const value = {agent_id:agentId,sandbox_id:record.detail.sandbox.id,pod:nativePod,
        sandbox:{metadata:{...metadata('native-' + n,'native-sandbox-' + n),annotations:{
          'cma/sandbox':record.detail.sandbox.id,'cma/home-identity':'home-identity-' + n}}},
        pvcs:[{metadata:metadata('home-runtime-uid-' + n,'pvc-' + n),status:{phase:'Bound'},
          spec:{volumeName:'pv-' + n}}]};
      state.mutateNative(value); return value;
    },
    async prepareTurn({caseId,marker}) {
      state.turns.set(caseId,{case_id:caseId,marker,request_count:0,completed_effect_count:0,active_streams:0});
    },
    async releaseTurn({caseId}) {
      const turn = state.turns.get(caseId), {record,response,marker} = turn;
      if (state.failure === 'release') throw new Error('release_failed');
      turn.completed_effect_count = 1; turn.active_streams = 0;
      response.status = 'completed';
      response.output = [{type:'message',role:'assistant',content:[{type:'output_text',text:marker}]}];
      const history = record.history;
      history.through_seq += 4;
      history.turns.push({turn_sequence:history.turns.length + 1,started_at:date,owner_message:turn.input,
        agent_messages:[marker],messages:[{sequence:history.through_seq,role:'assistant',content:marker,appended_at:date}],
        outcome:{type:'completed'}});
    },
    async oracle({caseId}) {
      const t = state.turns.get(caseId);
      const value = Object.fromEntries(['case_id','request_count','completed_effect_count','active_streams'].map(k => [k,t[k]]));
      state.mutateOracle(value); return value;
    },
  };
  const options = {artifacts,api,adapter,observe:async event => {state.events.push(event);},
    timeout:2000,now:()=>state.clock,sleep:async ms => {state.clock += ms;},uuid:()=>String(++state.counter)};
  const journey = new RetainedAgentJourney(options);
  const switchReader = (version,incarnation) => {state.version=version;state.incarnation=incarnation;};
  return {state,api,adapter,options,journey,switchReader};
}
