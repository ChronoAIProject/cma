import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp,writeFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {createHash} from 'node:crypto';
import {createParentBrowserLease, installParentBrowserLease} from './parent-browser.mjs';

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(),'c275-browser-lease-'));
  t.after(() => rm(root,{recursive:true,force:true}));
  const source = 'a'.repeat(40),image='repo@sha256:'+'b'.repeat(64);
  const assets = {'index.html':'<p>frozen</p>','build-info.json':JSON.stringify({revision:source,version:'bound',local:false})};
  const files=[];
  for (const [path,bytes] of Object.entries(assets)) {
    await writeFile(join(root,path),bytes);
    files.push({path,sha256:createHash('sha256').update(bytes).digest('hex')});
  }
  let launches=0,closes=0;
  const args={config:{scope:'LOCAL_DISPOSABLE_TWO_AM',database_disposable:true,single_matrix_writer:true,
    expected_api_image:image,compiled_web:{source,image:'sha256:'+'c'.repeat(64),build_version:'bound',asset_root:root,files}},
    actors:{user:{subject:'user-subject',sessionOrigin:'https://api.localhost.direct',
      storageState:{cookies:[{name:'cma_session',value:'private'}],origins:[]}}},
    adapter:{async topology(){return {ams:Object.fromEntries(['a','b'].map((am,i)=>[am,{ready:true,image,
      origin:'http://127.0.0.1:'+(10001+i),pod_uid:am,instance_id:am+'-instance',container_id:am+'-container'}]))};}},
    resolveAgent:async()=>null,async launchBrowser(){launches++;return {newContext(){},async close(){closes++;}};}};
  return {args,root,counts:()=>({launches,closes})};
}

test('serves only frozen assets, SPA paths and no API; releases browser and server once',async t=>{
  const f=await fixture(t),owned=await createParentBrowserLease(f.args);t.after(owned.close);
  assert.equal(await (await fetch(owned.lease.webOrigin+'/agents/example')).text(),'<p>frozen</p>');
  assert.equal((await fetch(owned.lease.webOrigin+'/auth/session')).status,404);
  assert.equal((await fetch(owned.lease.webOrigin+'/missing.js')).status,404);
  assert.equal((await fetch(owned.lease.webOrigin+'/',{method:'POST'})).status,405);
  await writeFile(join(f.root,'index.html'),'<p>changed</p>');
  assert.equal(await (await fetch(owned.lease.webOrigin+'/')).text(),'<p>frozen</p>');
  await owned.close();await owned.close();
  assert.deepEqual(f.counts(),{launches:1,closes:1});
  await assert.rejects(fetch(owned.lease.webOrigin+'/'));
});

test('changed asset refuses before browser launch',async t=>{
  const f=await fixture(t);await writeFile(join(f.root,'index.html'),'changed');
  await assert.rejects(createParentBrowserLease(f.args),/asset_hash/);
  assert.equal(f.counts().launches,0);
});

test('bearer cannot masquerade as cookie session',async t=>{
  const f=await fixture(t);f.args.actors.user.bearer='private-token';
  await assert.rejects(createParentBrowserLease(f.args),/exact_cookie_actor/);
  assert.equal(f.counts().launches,0);
});

test('wrong AM image refuses before browser launch',async t=>{
  const f=await fixture(t),original=f.args.adapter.topology;
  f.args.adapter.topology=async()=>{const value=await original();value.ams.b.image='wrong';return value;};
  await assert.rejects(createParentBrowserLease(f.args),/ready_exact_am/);
  assert.equal(f.counts().launches,0);
});

test('browser close failure still closes static listener and remains failure',async t=>{
  const f=await fixture(t);
  f.args.launchBrowser=async()=>({newContext(){},close(){throw Error('browser-close-failed');}});
  const owned=await createParentBrowserLease(f.args);
  await assert.rejects(owned.close(),error=>error instanceof AggregateError
    && error.errors[0].message==='browser-close-failed');
  await assert.rejects(owned.close(), /browser_parent_release_failed/);
  await assert.rejects(fetch(owned.lease.webOrigin+'/'));
});

test('parent installer preserves services and binds actual actor/artifact primitives once', async t => {
  const f = await fixture(t), previous = {primitives: {custody: {}}}, services = {executable: previous};
  const owned = await installParentBrowserLease({...f.args, services}); t.after(owned.close);
  assert.equal(services.executable.browserLease.actors, f.args.actors);
  assert.equal(services.executable.primitives.compiledWeb.source, f.args.config.compiled_web.source);
  assert.deepEqual(services.executable.primitives.identities, {user: {actor: 'user', subject: 'user-subject'}});
  assert.equal(services.executable.primitives.custody, previous.primitives.custody);
  await assert.rejects(installParentBrowserLease({...f.args, services}), /already_installed/);
  assert.equal(f.counts().launches, 1);
  await owned.close(); assert.equal(services.executable, previous);
});
