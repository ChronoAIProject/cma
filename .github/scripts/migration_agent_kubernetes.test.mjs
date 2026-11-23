import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp,writeFile,chmod,rm,symlink,realpath} from 'node:fs/promises';
import {join} from 'node:path';
import {NativeObserver} from './migration_agent_kubernetes.mjs';
import {fixture,artifacts,pod} from './migration_agent_test_fixture.mjs';
import {nativeObservation,readerObservation} from './migration_agent_observations.mjs';

async function setup(t,overrides={}) {
  const directory=await realpath(await mkdtemp(join(process.cwd(),'.cma-matrix-observer-')));
  t.after(()=>rm(directory,{recursive:true,force:true}));
  const kubeconfig=join(directory,'kubeconfig.private');
  await writeFile(kubeconfig,'synthetic configuration bytes\n',{mode:0o600});
  const f=fixture();await f.journey.run('old-write');
  const native=await f.adapter.runtimeFor({agentId:'agent-1'});
  for (const resource of [native.sandbox,native.pod,...native.pvcs]) resource.metadata.namespace='cma-migration';
  native.pod.spec.containers[0].env=[{name:'DO_NOT_LOG',value:'sensitive-fixture-marker'}];
  native.pod.spec.volumes.push({name:'sensitive',secret:{secretName:'must-not-read'}});
  const context='kind-cma-matrix-abcdef1234',server='https://127.0.0.1:40000',namespaceUid='namespace-uid';
  const config={'current-context':context,contexts:[{name:context,context:{cluster:context,user:'fixture'}}],
    clusters:[{name:context,cluster:{server,'certificate-authority-data':'DATA+OMITTED'}}],
    users:[{name:'fixture',user:{'client-certificate-data':'DATA+OMITTED','client-key-data':'DATA+OMITTED'}}]};
  const state={commands:[],events:[],config,native,namespaceUid,pods:[native.pod],sandboxes:[native.sandbox],
    mutate:()=>{},apiPod:pod('api','api-uid','api',artifacts.old,'1')};
  state.apiPod.metadata.namespace='cma-migration';
  const command=async args=>{
    state.commands.push(args);
    const rest=args.slice(5);let result;
    if (rest[0]==='config')result=state.config;
    else if (rest[0]==='get' && rest[1]==='namespace')result={kind:'Namespace',metadata:{name:'cma-migration',uid:state.namespaceUid}};
    else if (rest[0]==='get' && rest[1]==='--raw') {
      result={metadata:{resourceVersion:'42'},items:rest[2].includes('/sandboxes?')?state.sandboxes:state.pods};
    } else if (rest[0]==='-n' && rest[2]==='get' && rest[3]==='pods')result=state.apiPod;
    else if (rest[0]==='-n' && rest[2]==='get' && rest[3]==='persistentvolumeclaims')result=state.native.pvcs.find(v=>v.metadata.name===rest[4]);
    else throw new Error('unexpected command '+args.join(' '));
    const copy=structuredClone(result);state.mutate(copy,rest);return JSON.stringify(copy);
  };
  const options={kubeconfig,context,server,namespaceUid,record:async e=>state.events.push(e),command,...overrides};
  const observer=new NativeObserver(options);
  return {state,options,observer,directory,kubeconfig,lineage:f.journey.resources()[0].lineage};
}

test('observer joins exact local namespace, native owner and PVC using GET only',async t=>{
  const f=await setup(t);await f.observer.initialize(f.directory);
  f.observer.bindReader({artifact:artifacts.old,podName:'api',podUid:'api-uid'});
  assert.equal(readerObservation(await f.observer.reader(),artifacts.old).pod.uid,'api-uid');
  const raw=await f.observer.runtimeFor({agentId:'agent-1',sandboxId:'sandbox-1'});
  const observation=nativeObservation(raw,f.lineage,artifacts.runtime);
  assert.equal(observation.home,'home-identity-1');assert.equal(observation.pvcs[0].uid,'pvc-1');
  const log=JSON.stringify(f.state.events);
  assert.equal(log.includes('DO_NOT_LOG'),false);assert.equal(log.includes('sensitive-fixture-marker'),false);
  assert.equal(log.includes('secretName'),false);
  for (const args of f.state.commands) {
    assert.deepEqual(args.slice(0,5),['--kubeconfig',f.kubeconfig,'--context',f.options.context,'--request-timeout=15s']);
    assert.ok(args.includes('get')||args.includes('view'));
    assert.ok(!args.some(a=>['exec','apply','delete','secret','secrets','patch'].includes(a)));
  }
});

test('foreign context/server, unsafe config and replaced kubeconfig fail closed',async t=>{
  const f=await setup(t);
  for (const changes of [{context:'production'},{server:'https://example.invalid:443'},
    {server:'https://127.0.0.1:65536'},{namespaceUid:'bad uid'},{record:null}])
    assert.throws(()=>new NativeObserver({...f.options,...changes}));
  await assert.rejects(f.observer.initialize(process.cwd()),/campaign_kubeconfig/);
  await chmod(f.kubeconfig,0o644);
  await assert.rejects(f.observer.initialize(f.directory),/private_kubeconfig/);
  await chmod(f.kubeconfig,0o600);
  await f.observer.initialize(f.directory);
  await writeFile(f.kubeconfig,'replacement bytes');
  await assert.rejects(f.observer.runtimeFor({agentId:'agent-1',sandboxId:'sandbox-1'}),/kubeconfig_changed/);
  await rm(f.kubeconfig);await symlink(join(f.directory,'missing'),f.kubeconfig);
  await assert.rejects(f.observer.initialize(f.directory));
});

for (const [name,change] of [
  ['exec user',v=>{v.users[0].user.exec={command:'must-not-run'};}],
  ['token file',v=>{v.users[0].user.tokenFile='/private/unrelated';}],
  ['proxy',v=>{v.clusters[0].cluster['proxy-url']='https://external.invalid';}],
  ['insecure TLS',v=>{v.clusters[0].cluster['insecure-skip-tls-verify']=true;}],
  ['wrong current context',v=>{v['current-context']='wrong';}],
  ['wrong context user',v=>{v.contexts[0].context.user='wrong';}],
  ['missing certificate',v=>{delete v.users[0].user['client-key-data'];}],
]) test(name+' rejected before cluster access',async t=>{
  const f=await setup(t);change(f.state.config);
  await assert.rejects(f.observer.initialize(f.directory),/kubeconfig_binding/);
  assert.equal(f.state.commands.length,1);
});

test('uninitialized observer, missing reader and wrong Pod UID are refused',async t=>{
  const f=await setup(t);
  await assert.rejects(f.observer.reader(),/observer_not_initialized/);
  await assert.rejects(f.observer.runtimeFor({agentId:'agent-1',sandboxId:'sandbox-1'}),/observer_not_initialized/);
  await f.observer.initialize(f.directory);
  await assert.rejects(f.observer.reader(),/serving_reader_missing/);
  assert.throws(()=>f.observer.bindReader({artifact:artifacts.old,podName:'../wrong',podUid:'uid'}));
  f.observer.bindReader({artifact:artifacts.old,podName:'api',podUid:'wrong'});
  await assert.rejects(f.observer.reader(),/serving_pod_replaced/);
  f.state.namespaceUid='replaced';
  await assert.rejects(f.observer.reader(),/namespace_identity_changed/);
});

for (const [name,change,reason] of [
  ['missing native Sandbox',s=>{s.sandboxes=[];},'native_sandbox_join'],
  ['duplicate native Sandbox',s=>{s.sandboxes.push(s.sandboxes[0]);},'native_sandbox_join'],
  ['missing native Pod',s=>{s.pods=[];},'native_pod_join'],
  ['duplicate native Pod',s=>{s.pods.push(s.pods[0]);},'native_pod_join'],
  ['partial resource list',s=>{s.mutate=(v,args)=>{if(args[1]==='--raw')v.metadata.continue='more';};},'native_list_incomplete'],
  ['unbound resource list',s=>{s.mutate=(v,args)=>{if(args[1]==='--raw')delete v.metadata;};},'native_list_incomplete'],
  ['foreign namespace',s=>{s.native.sandbox.metadata.namespace='other';},'native_metadata'],
  ['missing Pod status',s=>{delete s.native.pod.status.containerStatuses;},'pod_shape'],
  ['no home claims',s=>{s.native.pod.spec.volumes=[];},'native_claim_set'],
  ['invalid claim',s=>{s.native.pod.spec.volumes[0].persistentVolumeClaim.claimName='../secret';},'native_claim_set'],
  ['duplicate claim',s=>{s.native.pod.spec.volumes.push(s.native.pod.spec.volumes[0]);},'native_claim_set'],
  ['wrong PVC response',s=>{s.mutate=(v,args)=>{if(args[3]==='persistentvolumeclaims')v.metadata.name='other';};},'native_name_changed'],
]) test(name+' prevents native observation',async t=>{
  const f=await setup(t);await f.observer.initialize(f.directory);change(f.state);
  await assert.rejects(f.observer.runtimeFor({agentId:'agent-1',sandboxId:'sandbox-1'}),new RegExp(reason));
  assert.equal(f.state.events.length,0);
});

test('command errors, excessive output and invalid JSON produce safe failures',async t=>{
  for (const [command,reason] of [
    [async()=>{throw new Error('credential text');},'kubernetes_read_failed'],
    [async()=>'{','kubernetes_json_invalid'],
    [async()=>'x'.repeat(4*1024*1024+1),'kubernetes_output_bound'],
  ]) {
    const f=await setup(t,{command});await assert.rejects(f.observer.initialize(f.directory),new RegExp(reason));
  }
});

test('namespace recreation during a snapshot prevents result publication',async t=>{
  const f=await setup(t);await f.observer.initialize(f.directory);
  f.state.mutate=(value,args)=>{if(args[3]==='persistentvolumeclaims')f.state.namespaceUid='replacement';};
  await assert.rejects(f.observer.runtimeFor({agentId:'agent-1',sandboxId:'sandbox-1'}),/namespace_identity/);
  assert.equal(f.state.events.length,0);
  await assert.rejects(f.observer.runtimeFor({agentId:'../bad',sandboxId:'sandbox-1'}),/agent_sandbox_binding/);
});

test('a refused kubeconfig cannot authorize a later resource read',async t=>{
  const f=await setup(t);f.state.config.users[0].user.exec={command:'must-not-run'};
  await assert.rejects(f.observer.initialize(f.directory),/kubeconfig_binding/);
  const count=f.state.commands.length;
  await assert.rejects(f.observer.runtimeFor({agentId:'agent-1',sandboxId:'sandbox-1'}),/observer_not_initialized/);
  f.observer.bindReader({artifact:artifacts.old,podName:'api',podUid:'api-uid'});
  await assert.rejects(f.observer.reader(),/observer_not_initialized/);
  assert.equal(f.state.commands.length,count);
});
