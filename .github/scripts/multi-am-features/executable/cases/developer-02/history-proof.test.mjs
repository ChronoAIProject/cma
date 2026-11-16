import test from 'node:test';
import assert from 'node:assert/strict';
import {completeExport,exportRows,retainedEntries} from './history-proof.mjs';
const transcript={agent_id:'fixture-agent',through_seq:7,turns:[{turn_sequence:1,started_at:'2026-11-16T00:00:00Z',outcome:{type:'interrupted'},messages:[{sequence:2,role:'user',content:'safe request',appended_at:'2026-11-16T00:00:01Z'},{sequence:3,role:'assistant',content:'partial reply',appended_at:'2026-11-16T00:00:02Z'}]},{turn_sequence:5,started_at:'2026-11-16T00:01:00Z',outcome:{type:'completed'},messages:[]}]};
const raw=rows=>rows.map(JSON.stringify).join('\n')+'\n';
test('exact JSONL includes empty turns and terminal outcomes',()=>assert.equal(completeExport(raw(exportRows(transcript)),transcript),true));
test('download refuses omission, duplicate, wrong agent and wrong terminal outcome',()=>{
 const rows=exportRows(transcript);
 for(const bad of [rows.slice(1),[...rows,rows[0]],rows.map(r=>({...r,agent_id:'other'})),rows.map(r=>({...r,outcome:{type:'active'}}))]) assert.equal(completeExport(raw(bad),transcript),false);
});
test('later hydration must preserve earlier entries and cannot duplicate an identity',()=>{
 const next=structuredClone(transcript);next.turns[0].messages.push({sequence:4,role:'assistant',content:'new event'});
 assert.equal(retainedEntries(transcript,next),true);
 next.turns[0].messages.push({...next.turns[0].messages[0]});assert.equal(retainedEntries(transcript,next),false);
 const stale=structuredClone(transcript);stale.turns[0].messages[0].content='overwritten';assert.equal(retainedEntries(transcript,stale),false);
});
