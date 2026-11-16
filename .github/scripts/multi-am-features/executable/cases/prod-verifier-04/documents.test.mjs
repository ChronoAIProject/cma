import test from 'node:test';
import assert from 'node:assert/strict';
import {hash} from './common.mjs';
import {execute, pass} from './test-helpers.mjs';

test('document flows delete only the owned comment and retain exact source history',async()=>{
  for(const number of [34,35]) {
    const revisions=[{revision:1,title:'first',markdown:'# First'},{revision:2,title:'second',markdown:'# Second'}];
    const comments=[{id:'c1',author_subject:'owner',body:'own',document_revision:2,created_at:'2026-11-16'},
      {id:'c2',author_subject:'commenter',body:'other',document_revision:2,created_at:'2026-11-16'}];
    let removed=false,revision=5;
    const receipts=new Map();
    const result=await execute(number,'documents',{documentId:'doc_one',owner:'owner',revisions,comments},async input=>{
      const url=new URL(input.path,'http://localhost');
      if(input.actor==='outsider') return {status:404,body:{code:'document_not_found'}};
      if(input.method==='DELETE') {
        if(receipts.has(input.key)) return {status:200,body:receipts.get(input.key)};
        if(input.body.expected_revision!==revision) return {status:409,body:{code:'document_revision_conflict'}};
        assert.equal(url.pathname,'/api/v1/documents/doc_one/comments/c1');
        removed=true;revision++;
        const body={id:'doc_one',revision};receipts.set(input.key,body);
        return {status:200,body};
      }
      if(url.pathname.endsWith('/source')) {
        const record=revisions.find(r=>r.revision===Number(url.searchParams.get('revision')));
        return {status:200,body:{id:'doc_one',revision:record.revision,markdown:record.markdown}};
      }
      if(url.pathname.endsWith('/revisions')) return {status:200,body:{items:revisions.map(r=>({revision:r.revision,title:r.title,
        sha256:hash(r.markdown),size_bytes:Buffer.byteLength(r.markdown)})),next:null}};
      if(url.pathname.endsWith('/comments')) return {status:200,body:{items:removed?comments.slice(1):comments,next:null}};
      return {status:200,body:{id:'doc_one',revision}};
    });
    pass(result);assert.equal(revision,6);
    assert.ok(result.calls.every(call=>!(call.method==='DELETE' && call.path==='/api/v1/documents/doc_one')));
  }
});
