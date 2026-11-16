import test from 'node:test';
import assert from 'node:assert/strict';
import {execute, pass} from './test-helpers.mjs';

test('launch flows delete a distinct unpublished draft and retain cancelled launch history',async()=>{
  for(const number of [42,43,67,68]) {
    let state='draft',deleted=false,manual=false;
    const fixture={triggerId:'trg_published',draftTriggerId:'trg_draft',publicId:'pub_one',placementId:'plc_one',
      launchId:'launch_one',staleLaunchId:'launch_absent',otherOwnerTriggerId:'trg_foreign',
      async readNativeFacts(){return {agent_ids:[],active_jobs:0,other_owner_state:{untouched:true}};},
      async withManualRedirects(operation){manual=true;try{return await operation();}finally{manual=false;}}};
    const result=await execute(number,'launches',fixture,async input=>{
      const url=new URL(input.path,'http://localhost');
      if(url.pathname.endsWith('/open/plc_one')) {
        assert.equal(manual,true);
        return {status:303,body:null,headers:{location:'/t/pub_one/plc_one','cache-control':'private, no-store'}};
      }
      if(url.pathname==='/api/v1/launches') return {status:200,body:{items:input.actor==='outsider'?[]:
        [{id:'launch_one',trigger_id:'trg_published',state}],next_cursor:null}};
      if(input.actor==='outsider'||input.path.includes('absent')||input.path.includes('foreign')) return {status:404,body:{code:'not_found'}};
      if(input.method==='DELETE') {
        assert.equal(input.path,'/api/v1/triggers/trg_draft');deleted=true;
        return {status:204,body:null};
      }
      if(url.pathname==='/api/v1/triggers/trg_draft') return {status:deleted?404:200,body:{code:'trigger_not_found'}};
      if(input.method==='POST') state='cancelled';
      return {status:200,body:{id:'launch_one',trigger_id:'trg_published',state}};
    });
    pass(result);assert.equal(deleted,true);assert.equal(state,'cancelled');
  }
});
