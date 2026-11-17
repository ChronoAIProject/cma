"""Narrow CMA scale transformations and Kubernetes checkpoint checks.

No full Deployment spec, unrelated environment value or Secret payload is saved.
The controller owns policy approval, dry-run and the single-write transaction.
"""
import argparse
import copy
import datetime as dt
from decimal import Decimal
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
from zoneinfo import ZoneInfo

os.umask(0o077)
ROOT=Path(__file__).resolve().parent
CONTEXT='gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke'
NAMESPACE='chrono-bot'
K=['kubectl','--context',CONTEXT,'-n',NAMESPACE]
TEAM='/Users/shining/.claude/skills/shining-tech-lead/scripts/team.sh'
TARGETS={'cma-agent-manager':'api','cma-event-gateway':'cmaeg'}
# Retain EG's existing recovery ceiling; its activation template remains max4.
REPLICA_CEILINGS={'cma-agent-manager':6,'cma-event-gateway':5}
KINDS={'Deployment':('deployment','apps/v1','deployments'),'PodDisruptionBudget':('poddisruptionbudget','policy/v1','poddisruptionbudgets'),'HorizontalPodAutoscaler':('horizontalpodautoscaler','autoscaling/v2','horizontalpodautoscalers'),'ResourceQuota':('resourcequota','v1','resourcequotas')}
QUOTA_BEFORE={'requests.cpu':'40','limits.cpu':'80'}
QUOTA_AFTER={'requests.cpu':'64','limits.cpu':'128'}
ENV={'POD_IP','CHRONO_AGENT_MANAGER_REPLICAS_ENABLED','CHRONO_AGENT_MANAGER_RELAY_ADDRESS','CHRONO_AGENT_MANAGER_MAX_OWNED_AGENTS','CMAEG_REPLICAS_ENABLED','CMAEG_REPLICA_ADDRESS'}
SPEC_KEYS={'replicas','paused','strategy','minReadySeconds','progressDeadlineSeconds'}
POD_KEYS={'terminationGracePeriodSeconds','affinity','topologySpreadConstraints'}
CONTAINER_KEYS={'image','readinessProbe','lifecycle'}
canon=lambda x:json.dumps(x,sort_keys=True,separators=(',',':')).encode()
hash_bytes=lambda b:hashlib.sha256(b).hexdigest()
hash_json=lambda x:hash_bytes(canon(x))
now=lambda:dt.datetime.now(dt.timezone.utc).isoformat()
def load(p):return json.loads(Path(p).read_text())
def save(p,x):Path(p).write_text(json.dumps(x,indent=2)+'\n')
def require(ok,reason):
 if not ok:raise RuntimeError(reason)
def command(args,payload=None):
 r=subprocess.run(args,input=payload,capture_output=True,timeout=50)
 require(r.returncode==0,'command_failed_exit_'+str(r.returncode))
 return json.loads(r.stdout) if r.stdout.strip() else None

def get(kind,name):
 return command(K+['get',KINDS[kind][0],name,'--ignore-not-found','-o','json','--request-timeout=30s'])
def binding(obj):
 return {'uid_sha256':hash_json(obj['metadata']['uid']),'generation':obj['metadata'].get('generation'),'spec_sha256':hash_json(obj['spec'])}
def same(obj,b):
 require(obj is not None,'object_missing')
 require(binding(obj)==b,'identity_generation_or_full_spec_drift')
def cpu(value):
 require(isinstance(value,str) and re.fullmatch(r'[0-9]+(?:\.[0-9]+)?[mun]?',value) is not None,'invalid_CPU_quantity')
 suffix=value[-1:];return Decimal(value[:-1] if suffix in ('m','u','n') else value)*{'m':Decimal('.001'),'u':Decimal('.000001'),'n':Decimal('.000000001')}.get(suffix,Decimal(1))
def quota_cpu(spec, expected):
 return all(cpu(spec.get('hard',{}).get(k))==cpu(v) for k,v in expected.items())
def validate_operation(op):
 require(op.get('stage') in ('S1','S2','S3'),'stage_not_allowed')
 require(op.get('namespace')==NAMESPACE and op.get('context')==CONTEXT,'wrong_cluster_or_namespace')
 if op.get('kind')=='ResourceQuota':
  require(op.get('stage')=='S3' and op.get('name')=='sandbox-quota' and op.get('action')=='patch','quota_scope')
  require(op.get('id') in ('s3-quota-raise','s3-quota-restore'),'quota_action')
  return
 require(op.get('kind') in KINDS and op.get('name') in TARGETS,'wrong_resource')
 require(op.get('action') in ('patch','create','delete'),'wrong_action')
 if op['stage']=='S1':require(op['name']=='cma-agent-manager','S1_scope')
 if op['stage']=='S2':require(op['name']=='cma-event-gateway','S2_scope')
 if op['action']=='delete':require(op['kind']=='HorizontalPodAutoscaler','only_created_HPA_may_be_deleted')
 if op['kind']=='HorizontalPodAutoscaler':require(op['stage']=='S3','HPA_only_S3')
 if op['action']=='create':require(op['kind'] in ('PodDisruptionBudget','HorizontalPodAutoscaler'),'create_scope')

def merge_topology(existing, desired):
 def keyed(rows):
  require(isinstance(rows,list),'topology_list_required')
  result={}
  for row in rows:
   require(isinstance(row,dict) and isinstance(row.get('topologyKey'),str) and row['topologyKey']
           and row.get('whenUnsatisfiable') in ('DoNotSchedule','ScheduleAnyway'),'topology_identity_required')
   key=(row['topologyKey'],row['whenUnsatisfiable'])
   require(key not in result,'duplicate_topology_identity')
   result[key]=copy.deepcopy(row)
  return result
 merged=keyed(existing)
 for key,row in keyed(desired).items():merged[key]={**merged.get(key,{}),**row}
 return list(merged.values())

def transform(obj,op):
 result=copy.deepcopy(obj);old=[];s=result['spec']
 if op['kind']=='ResourceQuota':
  validate_operation(op)
  require(set(op.get('changes',{}))=={'cpu'} and set(op['changes']['cpu'])==set(QUOTA_AFTER),'quota_CPU_fields_only')
  raising=op['id']=='s3-quota-raise'
  require(quota_cpu(s,QUOTA_BEFORE if raising else QUOTA_AFTER),'quota_before_CPU_drift')
  require(quota_cpu({'hard':op['changes']['cpu']},QUOTA_AFTER if raising else QUOTA_BEFORE),'quota_target_CPU_refused')
  old.append({'path':['spec'],'present':True,'value':copy.deepcopy(s)})
  s['hard'].update(op['changes']['cpu'])
  if not raising:require(s==op.get('quota_before_spec'),'quota_restore_unrelated_spec_drift')
  return result,old
 if op['kind']!='Deployment':
  require(set(op.get('changes',{}))=={'spec'},'nondeployment_spec_only')
  desired=copy.deepcopy(op['changes']['spec'])
  if op['kind']=='PodDisruptionBudget':
   require(desired.get('minAvailable')==1,'PDB_minimum')
   require(desired.get('selector')=={'matchLabels':{'app.kubernetes.io/name':op['name']}},'PDB_selector')
  else:
   require(desired.get('scaleTargetRef')=={'apiVersion':'apps/v1','kind':'Deployment','name':op['name']},'HPA_target')
   require(desired.get('minReplicas',0)>=2 and desired.get('maxReplicas',99)<=REPLICA_CEILINGS[op['name']],'HPA_bounds')
  old.append({'path':['spec'],'present':True,'value':copy.deepcopy(s)})
  result['spec']=desired
  return result,old
 changes=op.get('changes',{})
 require(set(changes)<= {'deployment','pod','container','env','remove'},'unknown_change_group')
 if 'paused' in changes.get('deployment',{}):
  require(type(changes['deployment']['paused']) is bool,'pause_boolean')
  if changes['deployment']['paused'] is False:require(s.get('replicas')==0 or op.get('abort_before_stop') is True,'unpause_only_at_zero')
 if op.get('a2_prepare_paused_template'):require(not ({'paused','replicas'} & set(changes.get('deployment',{}))),'template_prepare_must_not_unpause_or_scale')
 if 'replicas' in changes.get('deployment',{}):
  n=changes['deployment']['replicas'];require(type(n) is int and 0<=n<=REPLICA_CEILINGS[op['name']],'replica_bound')
  require(abs(n-s.get('replicas',1))<=1 or (op.get('a2_cutover_start') is True and s.get('replicas',1)==0 and n==2 and s.get('paused',False) is False) or (op.get('emergency_quiesce') is True and n==0 and op['stage'] in ('S1','S2')),'one_owner_at_a_time_except_cutover_or_approved_quiesce')
  if n>0:
   container=next(c for c in s['template']['spec']['containers'] if c['name']==TARGETS[op['name']])
   flag='CHRONO_AGENT_MANAGER_REPLICAS_ENABLED' if op['name']=='cma-agent-manager' else 'CMAEG_REPLICAS_ENABLED'
   require(any(e.get('name')==flag and e.get('value')=='true' for e in container.get('env',[])),'participating_mode_required_before_start_or_scale')
 def set_keys(target,values,allowed,prefix):
  require(set(values)<=allowed,'nonallowlisted_field')
  for key,value in values.items():
   old.append({'path':prefix+[key],'present':key in target,'value':copy.deepcopy(target.get(key))})
   target[key]=copy.deepcopy(value)
 set_keys(s,changes.get('deployment',{}),SPEC_KEYS,['spec'])
 # DeploymentSpec.paused is omitted by the API when false; match its wire form.
 if changes.get('deployment',{}).get('paused') is False:s.pop('paused',None)
 ps=s['template']['spec'];pv=copy.deepcopy(changes.get('pod',{}))
 if 'topologySpreadConstraints' in pv:
  pv['topologySpreadConstraints']=merge_topology(ps.get('topologySpreadConstraints',[]),pv['topologySpreadConstraints'])
 set_keys(ps,pv,POD_KEYS,['spec','template','spec'])
 indices=[i for i,c in enumerate(ps['containers']) if c['name']==TARGETS[op['name']]]
 require(len(indices)==1,'container_not_unique');i=indices[0];c=ps['containers'][i]
 cv=copy.deepcopy(changes.get('container',{}))
 if 'lifecycle' in cv and op['name']=='cma-event-gateway':require(cv['lifecycle']=={'preStop':{'sleep':{'seconds':30}}},'EG_exec_hook_prohibited')
 if 'lifecycle' in cv:
  prior=c.get('lifecycle',{})
  require(isinstance(prior,dict) and set(prior)<={'preStop','postStart'}
          and isinstance(cv['lifecycle'],dict) and set(cv['lifecycle'])=={'preStop'},'lifecycle_preStop_only')
  cv['lifecycle']={**prior,**cv['lifecycle']}
 if 'image' in cv:
  image_repo='cma-api' if op['name']=='cma-agent-manager' else 'cma-cmaeg'
  require(re.fullmatch(r'us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/'+image_repo+r'@sha256:[0-9a-f]{64}',cv['image']) is not None,'image_not_pinned_or_wrong_service')
 set_keys(c,cv,CONTAINER_KEYS,['spec','template','spec','containers',i])
 env=changes.get('env',[]);require(len({x['name'] for x in env})==len(env),'duplicate_env_change')
 for e in env:
  name=e['name'];allowed_env=ENV-({'CMAEG_REPLICAS_ENABLED','CMAEG_REPLICA_ADDRESS'} if op['name']=='cma-agent-manager' else {'CHRONO_AGENT_MANAGER_REPLICAS_ENABLED','CHRONO_AGENT_MANAGER_RELAY_ADDRESS','CHRONO_AGENT_MANAGER_MAX_OWNED_AGENTS'});require(name in allowed_env,'unapproved_env_name')
  require(set(e)<= {'name','value','valueFrom'},'bad_env_entry')
  if name.endswith('REPLICAS_ENABLED') and not op.get('abort_before_stop'):
   require(e.get('value')=='true','mode_off_is_separate_migration')
   require(obj['spec'].get('replicas',1)==0 or (op.get('a2_prepare_paused_template') is True and obj['spec'].get('paused') is True and obj['spec'].get('replicas')==1),'mode_activation_requires_zero_or_reviewed_paused_template')
  if name=='POD_IP':require(e=={'name':'POD_IP','valueFrom':{'fieldRef':{'apiVersion':'v1','fieldPath':'status.podIP'}}},'POD_IP_source')
  if name in ('CHRONO_AGENT_MANAGER_RELAY_ADDRESS','CMAEG_REPLICA_ADDRESS'):
   port='8080' if name=='CHRONO_AGENT_MANAGER_RELAY_ADDRESS' else '8090'
   require(e.get('value')=='http://$(POD_IP):'+port,'relay_must_target_pod_and_exact_service_port')
  if name=='CHRONO_AGENT_MANAGER_MAX_OWNED_AGENTS':require(str(e.get('value','')).isdigit() and int(e['value'])>0,'owned_capacity_positive')
  entries=c.setdefault('env',[]);matches=[j for j,x in enumerate(entries) if x['name']==name];require(len(matches)<=1,'duplicate_existing_env')
  j=matches[0] if matches else len(entries)
  old.append({'env_name':name,'container':c['name'],'present':bool(matches),'value':copy.deepcopy(entries[j]) if matches else None,'index':j})
  if matches:entries[j]=copy.deepcopy(e)
  else:entries.append(copy.deepcopy(e))
 for address_name in ('CHRONO_AGENT_MANAGER_RELAY_ADDRESS','CMAEG_REPLICA_ADDRESS'):
  if address_name in {e['name'] for e in env}:
   names=[e['name'] for e in c['env']];require(names.index('POD_IP')<names.index(address_name),'downward_API_order')
 for removal in changes.get('remove',[]):
  group,key=removal['group'],removal['key']
  scopes={'deployment':(s,SPEC_KEYS-{'replicas','paused'},['spec']),'pod':(ps,POD_KEYS,['spec','template','spec']),'container':(c,CONTAINER_KEYS-{'image'},['spec','template','spec','containers',i])}
  require(group in scopes,'bad_removal_group');target,allowed,prefix=scopes[group]
  require(key in allowed and key in target,'bad_removal_field')
  old.append({'path':prefix+[key],'present':True,'value':copy.deepcopy(target[key])});del target[key]
 return result,old

def patch_for(current,expected,kind=None):
 if current.get('kind')=='ResourceQuota':
  m=current['metadata'];patch=[{'op':'test','path':'/metadata/uid','value':m['uid']},{'op':'test','path':'/metadata/resourceVersion','value':m['resourceVersion']},{'op':'test','path':'/spec','value':current['spec']}]
  for key in QUOTA_AFTER:patch.append({'op':'replace','path':'/spec/hard/'+key,'value':expected['spec']['hard'][key]})
  return patch
 m=current['metadata'];patch=[{'op':'test','path':'/metadata/uid','value':m['uid']},{'op':'test','path':'/metadata/resourceVersion','value':m['resourceVersion']}]
 # HPA APIs may omit generation. UID/RV/full-spec still provide atomic CAS.
 if (kind or current.get('kind'))!='HorizontalPodAutoscaler' or 'generation' in m:
  require(type(m.get('generation')) is int and m['generation']>0,'valid_generation_required')
  patch.append({'op':'test','path':'/metadata/generation','value':m['generation']})
 return patch+[{'op':'test','path':'/spec','value':current['spec']},{'op':'replace','path':'/spec','value':expected['spec']}]

def template_identity(template):
 value=copy.deepcopy(template);value.get('metadata',{}).get('labels',{}).pop('pod-template-hash',None)
 return hash_json(value)

def deployment_checkpoint(obj,op):
 if op['kind']!='Deployment':return
 s=obj['spec'];changes=op.get('changes',{});phase=op.get('a2_phase')
 if op.get('requires_zero'):require(s.get('replicas')==0,'recovery_requires_zero')
 flags=any(e['name'].endswith('REPLICAS_ENABLED') for e in changes.get('env',[]))
 if phase=='prepare-template':require(op.get('a2_prepare_paused_template') is True and s.get('paused') is True and s.get('replicas')==1,'prepare_requires_paused_one')
 if phase=='pause-legacy':require(s.get('replicas')==1 and not s.get('paused',False),'pause_requires_unpaused_legacy_one')
 if phase=='stop-legacy':require(s.get('paused') is True and s.get('replicas')==1,'stop_requires_paused_one')
 if phase=='unpause-zero':require(s.get('paused') is True and s.get('replicas')==0,'unpause_requires_paused_zero')
 need_zero=(flags and not op.get('a2_prepare_paused_template')) or phase=='unpause-zero' or op.get('a2_cutover_start') is True
 if changes.get('deployment',{}).get('paused') is False:need_zero=True
 if phase in ('pause-legacy','prepare-template','stop-legacy') or op.get('abort_before_stop'):
  require(obj.get('status',{}).get('observedGeneration')==obj['metadata']['generation'],'legacy_controller_status_stale')
  require(obj.get('status',{}).get('readyReplicas')==1 and obj.get('status',{}).get('availableReplicas')==1,'legacy_one_must_be_stable_before_cutover')
 next_count=changes.get('deployment',{}).get('replicas',s.get('replicas',1))
 if 0<next_count<s.get('replicas',1):
  require(obj.get('status',{}).get('observedGeneration')==obj['metadata']['generation'],'downscale_controller_status_stale')
  require(obj.get('status',{}).get('readyReplicas')==s['replicas'] and obj.get('status',{}).get('availableReplicas')==s['replicas'],'downscale_all_owners_must_be_ready')
  selector=','.join(k+'='+v for k,v in s['selector']['matchLabels'].items())
  pods=command(K+['get','pods','-l',selector,'-o','json'])['items']
  require(len(pods)==s['replicas'] and not any(p['metadata'].get('deletionTimestamp') for p in pods),'downscale_wait_for_actual_termination')
  require(all(any(c.get('type')=='Ready' and c.get('status')=='True' for c in p.get('status',{}).get('conditions',[])) for p in pods),'downscale_pod_readiness_drift')
 if op.get('requires_zero'):
  need_zero=True
 if need_zero:
  selector=','.join(k+'='+v for k,v in s['selector']['matchLabels'].items())
  pods=command(K+['get','pods','-l',selector,'-o','json'])
  require(not pods['items'],'no_old_or_terminating_pods_required')
 if op.get('a2_cutover_start') is True or op.get('recover_one') is True:
  require(s.get('replicas')==0 and not s.get('paused',False),'start_requires_unpaused_zero')
  sets=command(K+['get','replicasets','-o','json'])['items']
  owned=[r for r in sets if any(x.get('uid')==obj['metadata']['uid'] and x.get('controller') is True for x in r['metadata'].get('ownerReferences',[]))]
  require(owned and all(r['spec'].get('replicas',1)==0 and r.get('status',{}).get('replicas',0)==0 for r in owned),'all_owned_RS_must_be_zero')
  match=[r for r in owned if template_identity(r['spec']['template'])==template_identity(s['template'])]
  require(len(match)==1,'exact_controller_created_candidate_RS_required')
  require(obj.get('status',{}).get('observedGeneration')==obj['metadata']['generation'],'controller_must_observe_zero_template')

def delete_request(current,op,dry_run=False):
 version=KINDS[op['kind']][1];path=f'/apis/{version}/namespaces/{NAMESPACE}/{KINDS[op["kind"]][2]}/{op["name"]}'
 payload={'apiVersion':'v1','kind':'DeleteOptions','preconditions':{'uid':current['metadata']['uid'],'resourceVersion':current['metadata']['resourceVersion']}}
 if dry_run:payload['dryRun']=['All']
 return K+['delete','--raw',path,'-f','-'],canon(payload)
