'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const {collect} = require('./migration_api_custody_documents.cjs');

test('API custody uses bounded majority find only', () => {
  const calls=[];
  const result=collect({runCommand(cmd) {calls.push(cmd);return {ok:1,cursor:{id:'0',ns:'matrix.auth_api_bearers',firstBatch:[{_id:'synthetic'}]}};}});
  assert.deepEqual(calls,[{find:'auth_api_bearers',filter:{},sort:{_id:1},limit:5,batchSize:5,
    singleBatch:true,maxTimeMS:2000,readConcern:{level:'majority'}}]);
  assert.equal(result.schema,'cma-migration-api-custody-documents-v1');
  assert.deepEqual(Object.keys(result.collections),['auth_api_bearers']);
});

test('API custody rejects incomplete, foreign and over-bound observations', () => {
  for (const reply of [null,{ok:0},{ok:1},
    {ok:1,cursor:{id:'1',ns:'matrix.auth_api_bearers',firstBatch:[]}},
    {ok:1,cursor:{id:'0',ns:'admin.auth_api_bearers',firstBatch:[]}},
    {ok:1,cursor:{id:'0',ns:'matrix.auth_api_bearers',firstBatch:Array(5).fill({})}},
    {ok:1,cursor:{id:'0',ns:'matrix.auth_api_bearers',firstBatch:{}}}]) {
    assert.throws(()=>collect({runCommand(){return reply;}}),/snapshot/);
  }
});

test('shell closes its connection and redacts transport and size failures', () => {
  const fs=require('node:fs');
  const vm=require('node:vm');
  const script=fs.readFileSync(require.resolve('./migration_api_custody_documents.cjs'),'utf8');
  for(const fault of ['none','transport','oversize']) {
    let closed=false;
    const output=[];
    const context={process:{env:{MATRIX_API_CUSTODY_READ_URI:'synthetic-private-uri'}},
      Mongo:class {
        getDB(name){assert.equal(name,'matrix');return {runCommand(){
          if(fault==='transport')throw Error('synthetic-private-uri');
          return {ok:1,cursor:{id:'0',ns:'matrix.auth_api_bearers',firstBatch:[]}};
        }};}
        close(){closed=true;}
      },
      EJSON:{stringify(value,options){assert.equal(options.relaxed,false);return fault==='oversize'?'x'.repeat(1048577):JSON.stringify(value);}},
      print(value){output.push(value);},quit(code){throw Error('exit:'+code);}};
    if(fault==='none')vm.runInNewContext(script,context);
    else assert.throws(()=>vm.runInNewContext(script,context),/exit:1/);
    assert.equal(closed,true);assert.equal(output.length,1);
    assert.ok(!output[0].includes('synthetic-private-uri'));
    if(fault==='none')assert.equal(JSON.parse(output[0]).schema,'cma-migration-api-custody-documents-v1');
  }
});
