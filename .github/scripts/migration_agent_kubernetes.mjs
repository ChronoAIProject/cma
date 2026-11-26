import {execFile} from 'node:child_process';
import {open,realpath} from 'node:fs/promises';
import {constants} from 'node:fs';
import {dirname,resolve} from 'node:path';
import {promisify} from 'node:util';
import {artifact,hash,identity,requireValue} from './migration_agent_observations.mjs';

const execute=promisify(execFile),namespace='cma-migration';
const dnsName=value=>typeof value==='string' && /^[a-z0-9](?:[a-z0-9.-]{0,251}[a-z0-9])?$/.test(value);

async function configFingerprint(path) {
  const file=await open(path,constants.O_RDONLY|constants.O_NOFOLLOW|constants.O_NONBLOCK);
  try {
    const stat=await file.stat();
    requireValue(stat.isFile() && stat.uid===process.getuid() && !(stat.mode&0o077)
      && stat.size>0 && stat.size<=65536, 'private_kubeconfig_required');
    return hash((await file.readFile()).toString('base64'));
  } finally {await file.close();}
}

function meta(value) {
  const m=value?.metadata;
  requireValue(m && dnsName(m.name) && typeof m.uid==='string' && m.uid.length>0
    && m.namespace===namespace && !m.deletionTimestamp, 'native_metadata_invalid');
  return {name:m.name,uid:m.uid,namespace:m.namespace,
    ownerReferences:(m.ownerReferences ?? []).map(o=>({uid:o.uid,controller:o.controller}))};
}

function podProjection(value) {
  requireValue(Array.isArray(value.spec?.containers) && Array.isArray(value.status?.containerStatuses),
    'pod_shape_invalid');
  return {metadata:meta(value),spec:{containers:value.spec.containers.map(c=>({name:c.name,image:c.image})),
    volumes:(value.spec.volumes ?? []).filter(v=>v.persistentVolumeClaim).map(v=>({
      persistentVolumeClaim:{claimName:v.persistentVolumeClaim.claimName}}))},
  status:{phase:value.status.phase,conditions:(value.status.conditions ?? []).map(c=>({type:c.type,status:c.status})),
    containerStatuses:value.status.containerStatuses.map(s=>({name:s.name,ready:s.ready,restartCount:s.restartCount,
      containerID:s.containerID,imageID:s.imageID,state:{running:s.state?.running ? {startedAt:s.state.running.startedAt} : undefined}}))}};
}

export class NativeObserver {
  #config; #context; #server; #namespaceUid; #fingerprint; #command; #record; #binding;
  #initialized=false; #initializing=false;
  constructor({kubeconfig,context,server,namespaceUid,command,record}) {
    requireValue(/^kind-cma-matrix-[a-f0-9]{10}$/.test(context ?? ''), 'owned_kind_context_required');
    requireValue(typeof server==='string' && /^https:\/\/127\.0\.0\.1:[1-9][0-9]{0,4}$/.test(server)
      && Number(new URL(server).port)<=65535, 'local_kubernetes_server_required');
    requireValue(typeof namespaceUid==='string' && /^[a-zA-Z0-9-]{1,128}$/.test(namespaceUid)
      && typeof record==='function', 'namespace_and_observer_required');
    this.#config=resolve(kubeconfig);this.#context=context;this.#server=server;
    this.#namespaceUid=namespaceUid;this.#record=record;
    this.#command=command ?? (async args=>{
      try {return (await execute('kubectl',args,{timeout:20000,killSignal:'SIGKILL',maxBuffer:4*1024*1024})).stdout;}
      catch {throw new Error('kubernetes_read_failed');}
    });
  }
  async initialize(campaignDirectory) {
    requireValue(!this.#initializing,'observer_initialization_active');
    this.#initialized=false;this.#initializing=true;
    try {
      requireValue(await realpath(campaignDirectory)===dirname(this.#config)
        && await realpath(this.#config)===this.#config, 'campaign_kubeconfig_required');
      this.#fingerprint=await configFingerprint(this.#config);
      const config=await this.#call(['config','view','--minify','-o','json']);
      const cluster=config.clusters?.[0]?.cluster,user=config.users?.[0]?.user;
      requireValue(config['current-context']===this.#context && config.contexts?.length===1
        && config.contexts[0].name===this.#context && config.clusters?.length===1
        && config.clusters[0].cluster?.server===this.#server
        && config.contexts[0].context?.cluster===config.clusters[0].name
        && config.users?.length===1 && config.contexts[0].context?.user===config.users[0].name
        && cluster?.['certificate-authority-data'] && Object.keys(cluster).every(k=>
          ['server','certificate-authority-data'].includes(k))
        && user?.['client-certificate-data'] && user?.['client-key-data']
        && Object.keys(user).every(k=>['client-certificate-data','client-key-data'].includes(k)),
      'kubeconfig_binding_invalid');
      await this.#namespace();
      this.#initialized=true;
    } finally {this.#initializing=false;}
  }
  async #call(args) {
    requireValue(this.#fingerprint && await configFingerprint(this.#config)===this.#fingerprint,
      'kubeconfig_changed');
    let raw;
    try {raw=await this.#command(['--kubeconfig',this.#config,'--context',this.#context,
      '--request-timeout=15s',...args]);}
    catch {throw new Error('kubernetes_read_failed');}
    requireValue(typeof raw==='string' && Buffer.byteLength(raw)<=4*1024*1024,'kubernetes_output_bound');
    let value;
    try {value=JSON.parse(raw);} catch {throw new Error('kubernetes_json_invalid');}
    return value;
  }
  async #namespace() {
    const value=await this.#call(['get','namespace',namespace,'-o','json']);
    requireValue(value.kind==='Namespace' && value.metadata?.name===namespace
      && value.metadata.uid===this.#namespaceUid && !value.metadata.deletionTimestamp,
    'namespace_identity_changed');
  }
  async #object(resource,name) {
    requireValue(['pods','persistentvolumeclaims'].includes(resource) && dnsName(name),'native_read_invalid');
    const value=await this.#call(['-n',namespace,'get',resource,name,'-o','json']);
    requireValue(meta(value).name===name,'native_name_changed');
    return value;
  }
  async #list(resource) {
    requireValue(['pods','sandboxes.agents.x-k8s.io'].includes(resource),'native_list_invalid');
    const prefix=resource==='pods' ? '/api/v1' : '/apis/agents.x-k8s.io/v1beta1';
    const kind=resource==='pods' ? 'pods' : 'sandboxes';
    const value=await this.#call(['get','--raw',`${prefix}/namespaces/${namespace}/${kind}?limit=200`]);
    requireValue(value.metadata && typeof value.metadata.resourceVersion==='string'
      && value.metadata.resourceVersion.length>0 && Array.isArray(value.items)
      && value.items.length<=200 && !value.metadata.continue,
      'native_list_incomplete');
    return value.items;
  }
  bindReader({artifact:expected,podName,podUid,localImageAlias}) {
    requireValue(dnsName(podName) && typeof podUid==='string' && podUid.length>0,'serving_pod_binding_required');
    this.#binding={...artifact(expected),name:podName,uid:podUid,local_image_alias:localImageAlias};
  }
  async reader() {
    requireValue(this.#initialized,'observer_not_initialized');
    requireValue(this.#binding,'serving_reader_missing');
    await this.#namespace();
    const raw=await this.#object('pods',this.#binding.name);
    requireValue(raw.metadata.uid===this.#binding.uid,'serving_pod_replaced');
    await this.#namespace();
    const result={...artifact(this.#binding),local_image_alias:this.#binding.local_image_alias,pod:podProjection(raw)};
    await this.#record({type:'reader_resource_observed',value:result});return result;
  }
  async runtimeFor({agentId,sandboxId}) {
    requireValue(this.#initialized,'observer_not_initialized');
    requireValue(identity(agentId) && identity(sandboxId),'agent_sandbox_binding_required');
    await this.#namespace();
    const natives=(await this.#list('sandboxes.agents.x-k8s.io')).filter(s=>
      s.metadata?.annotations?.['cma/sandbox']===sandboxId && !s.metadata.deletionTimestamp);
    requireValue(natives.length===1,'native_sandbox_join_ambiguous');
    const native=natives[0],m=meta(native);
    const pods=(await this.#list('pods')).filter(p=>!p.metadata?.deletionTimestamp
      && p.metadata?.ownerReferences?.some(o=>o.uid===m.uid && o.controller===true));
    requireValue(pods.length===1,'native_pod_join_ambiguous');
    const pod=podProjection(pods[0]);
    const claims=pod.spec.volumes.map(v=>v.persistentVolumeClaim.claimName);
    requireValue(claims.length>0 && claims.length<=8 && claims.every(dnsName)
      && new Set(claims).size===claims.length,'native_claim_set_invalid');
    const pvcs=[];
    for (const name of claims) {
      const raw=await this.#object('persistentvolumeclaims',name);
      pvcs.push({metadata:meta(raw),spec:{volumeName:raw.spec?.volumeName},status:{phase:raw.status?.phase}});
    }
    await this.#namespace();
    const result={agent_id:agentId,sandbox_id:sandboxId,pod,pvcs,sandbox:{metadata:{...m,annotations:{
      'cma/sandbox':native.metadata.annotations['cma/sandbox'],
      'cma/home-identity':native.metadata.annotations['cma/home-identity']}}}};
    await this.#record({type:'native_resource_observed',value:result});return result;
  }
}
