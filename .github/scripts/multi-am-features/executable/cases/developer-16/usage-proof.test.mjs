import test from 'node:test';
import assert from 'node:assert/strict';
import {selectReport,refreshIdentity} from './usage-proof.mjs';
const report=(id,seconds=86400)=>({report_id:id,status:'ready',report_complete:true,summary:{sandbox_count:2},scope:{principal:'fixture-user',admin:false},expires_at:Date.now()/1000+600,query:{from:100,to:100+seconds,filters:{owner:null,runtime_version:null,statuses:[],sandbox_id:null,agent_id:null}}});
const response=body=>({status:200,path:'/api/v1/usage/metrics/view',body});
const page=id=>({status:200,path:`/api/v1/usage/metrics/reports/${id}/sandboxes`,body:{report_id:id,items:[]}});
const wanted={principal:'fixture-user',range:'24h'};
test('a later background warm report cannot replace the inventory report',()=>{
 const seen=[response({report:report('visible')}),page('visible'),response({report:report('warm','604800')})];
 assert.equal(selectReport(seen,wanted)?.report.report_id,'visible');
});
test('pending, foreign principal, expired, wrong window and admin reports never satisfy completion',()=>{
 for(const patch of [{status:'pending'},{report_complete:false},{summary:null},{scope:{principal:'other',admin:false}},{scope:{principal:'fixture-user',admin:true}},{expires_at:1},{query:{from:0,to:3600,filters:{}}}]){
  assert.equal(selectReport([response({...report('bad'),...patch}),page('bad')],wanted),null);
 }
});
test('filters and inventory identity are mandatory for custom selection',()=>{
 const custom={principal:'fixture-user',range:'custom',from:100,to:86500,sandboxId:'sandbox',agentId:'agent'};
 const r=report('custom');r.query.filters.sandbox_id='sandbox';r.query.filters.agent_id='agent';
 assert.equal(selectReport([response(r),page('other')],custom),null);
 assert.equal(selectReport([response(r),page('custom')],custom)?.report.report_id,'custom');
 r.query.filters.agent_id='other';assert.equal(selectReport([response(r),page('custom')],custom),null);
});
test('refresh accepts one immutable job and refuses a loop or unresolved job',()=>{
 const admitted=id=>({method:'POST',status:202,path:'/api/v1/usage/metrics/reports',body:{report_id:id}});
 assert.equal(refreshIdentity([admitted('one'),admitted('one')],'one'),true);
 assert.equal(refreshIdentity([admitted('one'),admitted('two')],'two'),false);
 assert.equal(refreshIdentity([response({refresh:{state:'pending',report_id:'one'}})],'other'),false);
});
test('refresh cannot reuse an observation captured before the refresh intent',()=>{
 const seen=[{...response(report('old')),sequence:1},{...page('old'),sequence:2}];
 assert.equal(selectReport(seen,wanted,2),null);
 seen.push({...response(report('new')),sequence:3},{...page('new'),sequence:4});
 assert.equal(selectReport(seen,wanted,2)?.report.report_id,'new');
});
test('custom search is matched after server normalization',()=>{
 const r=report('searched');r.query.filters.search='sandbox name';
 const selected={...wanted,search:' Sandbox Name '};
 assert.equal(selectReport([response(r),page('searched')],selected)?.report.report_id,'searched');
 assert.equal(selectReport([response(r),page('searched')],{...wanted,search:'another'}),null);
});

test('a fresh observation of the same cached report may retain its already rendered inventory',()=>{
 const seen=[{...response(report('same')),sequence:1},{...page('same'),sequence:2}];
 assert.equal(selectReport(seen,wanted,2),null);
 seen.push({...response(report('same')),sequence:3});
 assert.equal(selectReport(seen,wanted,2)?.report.report_id,'same');
});
test('malformed or unfiltered report query cannot satisfy completion',()=>{
 for(const query of [null,{}, {from:100,to:86500}])
  assert.equal(selectReport([response({...report('bad'),query}),page('bad')],wanted),null);
});
