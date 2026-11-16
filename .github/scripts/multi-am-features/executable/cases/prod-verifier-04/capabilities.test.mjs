import test from 'node:test';
import assert from 'node:assert/strict';
import {execute, pass} from './test-helpers.mjs';

test('capability and anonymous telemetry flows reject extra content without recording it',async()=>{
  for(const number of [29,66,83,86,87]) {
    const events=[];let anonymous=false;
    const manifest={
      '/api/v1/conformance':{service:'cma',status:'rebuilding',interfaces:[]},
      '/api/v2':{object:'cma.responses.capabilities',version:2,create:'/api/v2/responses',conversation:'/api/v2/conversations/{conversation_id}',creation:{stream:true,store:true}},
      '/api/v2/ui/task-experience':{object:'cma.ui.task_experience',schema_version:1,capabilities:{desktop_enable:false}},
      '/api/v2/ui/transport':{object:'cma.ui.transport',input:'responses_v2',selection:'new_tabs',snapshot:'/api/v2/agents/{agent_id}/events/snapshot',events:'/api/v2/agents/{agent_id}/events'},
    };
    const fixture={manifest,subjects:{user:'owner',outsider:'other'},readTelemetry:async()=>structuredClone(events),
      async withAnonymousRequests(operation){anonymous=true;try{return await operation();}finally{anonymous=false;}}};
    const result=await execute(number,'capabilities',fixture,async input=>{
      if(input.path==='/api/v1/telemetry/browser') {
        assert.equal(anonymous,true);
        if(Object.keys(input.body).length>1) return {status:422,body:{code:'invalid'}};
        events.push(structuredClone(input.body));return {status:204,body:null};
      }
      if(anonymous) return {status:401,body:{code:'unauthenticated'}};
      const body=structuredClone(manifest[input.path]);
      if(input.path.endsWith('task-experience')) body.account_id=fixture.subjects[input.actor];
      return {status:200,body};
    });
    pass(result);assert.equal(anonymous,false);assert.equal(events.length,number===66?2:1);
  }
});
