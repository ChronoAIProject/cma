'use strict';
const assert=require('node:assert/strict');
const test=require('node:test');
const {collect,protect,COLLECTIONS,MAX_ROWS,MAX_TOTAL_ROWS,MAX_BYTES}=require('./migration_agent_documents.cjs');

function fixture(override=()=>null) {
  const calls=[];
  const database={runCommand(request){
    calls.push(request);
    const injected=override(request,calls);if(injected)return injected;
    if(request.killCursors)return {ok:1};
    return {ok:1,cursor:{id:0,ns:'matrix.'+request.find,firstBatch:[]}};
  }};
  return {database,calls};
}

test('all 21 scoped collections are read with majority bounds; emptiness is explicit',()=>{
  const f=fixture(),result=collect(f.database,JSON.stringify);
  assert.deepEqual(Object.keys(result.collections),COLLECTIONS);
  assert.equal(f.calls.length,21);assert.equal(result.database,'matrix');
  assert.match(result.consistency,/not a cross-collection/);
  for(const call of f.calls) {
    assert.deepEqual(call.readConcern,{level:'majority'});
    assert.equal(call.maxTimeMS,2000);assert.equal(call.limit,MAX_ROWS+1);
    assert.equal(call.batchSize,32);assert.deepEqual(call.filter,{});
  }
});

test('multiple cursor batches preserve raw nested BSON-shaped documents',()=>{
  const original={_id:'agent1',document_schema_version:{$numberInt:'3'},record:{nested:[null,{state:'ready'}]}};
  const f=fixture(request=>{
    if(request.find===COLLECTIONS[0])return {ok:1,cursor:{id:99,ns:'matrix.'+request.find,firstBatch:[original]}};
    if(request.getMore)return {ok:1,cursor:{id:0,ns:'matrix.'+request.collection,nextBatch:[{_id:'agent2'}]}};
  });
  const result=collect(f.database,JSON.stringify);
  assert.deepEqual(result.collections[COLLECTIONS[0]],[original,{_id:'agent2'}]);
  assert.equal(f.calls.filter(row=>row.getMore).length,1);
  assert.equal(f.calls.filter(row=>row.killCursors).length,0);
});

test('credential and fencing strings are protected before logs while original document hashes remain',()=>{
  const credential=require('node:crypto').randomBytes(24).toString('hex');
  const row={_id:'agent1',lease_token:credential,record:{credential:{current:credential,ready:true}},
    history:[{authorization:credential}],count:3,missing:null};
  const f=fixture(r=>r.find===COLLECTIONS[0]?{ok:1,cursor:{id:0,ns:'matrix.'+r.find,firstBatch:[row]}}:null);
  const value=collect(f.database,JSON.stringify),saved=value.collections[COLLECTIONS[0]][0];
  assert.equal(JSON.stringify(value).includes(credential),false);
  assert.equal(saved.record.credential.ready,true);assert.equal(saved.lease_token,'[REDACTED]');
  assert.equal(value.redactions.length,3);
  assert.equal(value.document_hashes[COLLECTIONS[0]][0].document_sha256,
    require('node:crypto').createHash('sha256').update(JSON.stringify(row)).digest('hex'));
  const redactions=[];
  assert.deepEqual(protect({_bsontype:'Binary',value:credential},JSON.stringify,redactions,[],true),
    {redacted_bson_type:'Binary'});
  assert.equal(redactions.length,1);
  assert.equal(protect(undefined,JSON.stringify,[]),undefined);
  assert.ok(protect(new Date(),JSON.stringify,[]) instanceof Date);
  assert.deepEqual(protect(Buffer.from(credential),JSON.stringify,[],[],true),{redacted_bson_type:'binary_or_date'});
});

test('invalid cursors, duplicate IDs, missing IDs and server failures reject partial data',()=>{
  for(const cursor of [null,{id:null},{id:0,ns:'foreign.rows',firstBatch:[]},
    {id:0,ns:'matrix.'+COLLECTIONS[0],firstBatch:{}},
    {id:0,ns:'matrix.'+COLLECTIONS[0],firstBatch:[{}]},
    {id:0,ns:'matrix.'+COLLECTIONS[0],firstBatch:[{_id:1},{_id:1}]},
    {id:0,ns:'matrix.'+COLLECTIONS[0],firstBatch:[null]},
    {id:0,ns:'matrix.'+COLLECTIONS[0],firstBatch:[[]]}]) {
    const f=fixture(()=>({ok:1,cursor}));assert.throws(()=>collect(f.database,JSON.stringify));
    assert.equal(f.calls.filter(row=>row.find).length,1);
  }
  assert.throws(()=>collect(fixture(()=>({ok:0})).database,JSON.stringify),/read_failed/);
  assert.throws(()=>collect(fixture().database,null),/serializer_required/);
});

test('live cursor cleanup uses the requested scope even when server returns a foreign namespace',()=>{
  const f=fixture(request=>request.find?{ok:1,cursor:{id:7,ns:'foreign.target',firstBatch:[]}}:null);
  assert.throws(()=>collect(f.database,JSON.stringify),/cursor_invalid/);
  assert.deepEqual(f.calls.at(-1),{killCursors:COLLECTIONS[0],cursors:[7]});
  const g=fixture(request=>request.killCursors?{ok:0}:request.find?
    {ok:1,cursor:{id:7,ns:'foreign.target',firstBatch:[]}}:null);
  assert.throws(()=>collect(g.database,JSON.stringify),/read_failed/);
  const h=fixture(request=>request.find?{ok:1,cursor:{id:8,ns:'matrix.'+request.find,firstBatch:[]}}:
    request.getMore?{ok:1,cursor:null}:null);
  assert.throws(()=>collect(h.database,JSON.stringify),/cursor_invalid/);
  assert.deepEqual(h.calls.at(-1),{killCursors:COLLECTIONS[0],cursors:[8]});
});

test('collection, total, byte and cursor-progress bounds never truncate to a success',()=>{
  const rows=Array.from({length:MAX_ROWS+1},(_,i)=>({_id:i}));
  const f=fixture(r=>({ok:1,cursor:{id:0,ns:'matrix.'+r.find,firstBatch:rows}}));
  assert.throws(()=>collect(f.database,JSON.stringify),/snapshot_bound/);
  const g=fixture(r=>({ok:1,cursor:{id:0,ns:'matrix.'+r.find,firstBatch:rows.slice(0,MAX_ROWS)}}));
  assert.throws(()=>collect(g.database,JSON.stringify),/snapshot_bound/);
  assert.equal(g.calls.length,Math.floor(MAX_TOTAL_ROWS/MAX_ROWS)+1);
  const h=fixture(r=>r.killCursors?{ok:1}:{ok:1,cursor:{id:9,ns:'matrix.'+(r.find??r.collection),
    [r.find?'firstBatch':'nextBatch']:[]}});
  assert.throws(()=>collect(h.database,JSON.stringify),/cursor_invalid/);
  assert.equal(h.calls.filter(r=>r.killCursors).length,1);
  const i=fixture(r=>({ok:1,cursor:{id:0,ns:'matrix.'+r.find,firstBatch:[{_id:1}]}}));
  assert.throws(()=>collect(i.database,value=>value && typeof value==='object'?'x'.repeat(MAX_BYTES+1):'1'),/snapshot_bound/);
  assert.throws(()=>collect(fixture().database,()=> 'x'.repeat(MAX_BYTES+1)),/snapshot_bound/);
});
