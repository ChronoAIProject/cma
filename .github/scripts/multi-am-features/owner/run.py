#!/usr/bin/env python3
"""Run17 real AM, Mongo, native-runtime and held-provider fixture."""
import argparse
import hashlib
import hmac
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import secrets
import signal
import socket
import ssl
import subprocess
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

from common import Client, utc, write
from configuration_log import configuration_dispatch, configuration_log
from file_fixture import seed_oversize_file
import manifests
import legacy_manifests as old
from identity import ADMIN
from login import login
from prepared_runtime import prepared_runtime_evidence
from publication_evidence import (access_proposal_abort, artifact_publication_abort, artifact_publication_race,
                                  prepared_schema_barrier, prepared_schema_race,
                                  prepared_schema_refusals, publication_evidence,
                                  runtime_loss_evidence, runtime_loss_publication_abort,
                                  share_redemption_publication_abort,
                                  writer_publication_abort, writer_publication_evidence)
from readiness_scaling import drain_owner, observe_never_ready, observe_scale
from retirement_purge import retirement_purge
from runtime_loss import remove_runtime_pod, runtime_inventory_authority
from title_evidence import title_effects, title_intent

ROOT=Path(__file__).resolve().parent
PROVIDER=ROOT.parent/'cma'/'provider.py'
def validate_runtime_sources(ready):
    sources=ready.get('runtime_sources')
    if not isinstance(sources,dict):
        raise ValueError('per_family_runtime_sources_required')
    for family,image in ready['runtime_images'].items():
        if image and not re.fullmatch(r'[0-9a-f]{40}',str(sources.get(family,''))):
            raise ValueError('exact_runtime_source_required_'+family)
    return sources

class Fixture:
    configurationLog = configuration_log
    configurationDispatch = configuration_dispatch
    publicationEvidence = publication_evidence
    artifactPublicationAbort = artifact_publication_abort
    artifactPublicationRace = artifact_publication_race
    writerPublicationAbort = writer_publication_abort
    writerPublicationEvidence = writer_publication_evidence
    runtimeLossPublicationAbort = runtime_loss_publication_abort
    runtimeLossEvidence = runtime_loss_evidence
    shareRedemptionPublicationAbort = share_redemption_publication_abort
    accessProposalAbort = access_proposal_abort
    preparedRuntimeEvidence = prepared_runtime_evidence
    preparedSchemaBarrier = prepared_schema_barrier
    preparedSchemaRace = prepared_schema_race
    preparedSchemaRefusals = prepared_schema_refusals
    retirementPurge = retirement_purge
    removeRuntimePod = remove_runtime_pod
    runtimeInventoryAuthority = runtime_inventory_authority
    seedOversizeFile = seed_oversize_file
    drainOwner = drain_owner
    titleEffects = title_effects
    titleIntent = title_intent

    def __init__(self, ready, output):
        self.ready=ready
        self.client=Client(ready,output)
        self.output=Path(output)
        self.secret=old.credentials()
        self.secret['provider_control']=secrets.token_hex(32)
        self.secret['storage_access']=secrets.token_hex(32)
        self.control_token=secrets.token_hex(32)
        self.origins={}
        self.templates={}
        self.killed={}
        self.drained={}
        self.events=[]
        self.initialization_step=None
        self.provider_observation_lock=threading.Lock()
        self.mongo_observation_lock=threading.Lock()
        self.server=None
        self.thread=None

    def record(self, action, **fields):
        value={'at':utc(),'action':action,**fields}
        self.events.append(value)
        write(self.output/'fixture-events.json',self.events)

    def mark_initialization_step(self, step):
        self.initialization_step=step

    def certificates(self):
        directory=Path(self.ready['tls_directory'])
        cert=directory/'localhost.direct.OPEN.crt'; key=directory/'localhost.direct.OPEN.key'
        self.secret.update(provider_cert=cert.read_text(),provider_key=key.read_text())
        self.tls=ssl.create_default_context()
        self.record('public_test_certificate_selected',certificate_sha256=hashlib.sha256(cert.read_bytes()).hexdigest(),
            provider_origin='https://provider.localhost.direct:8080',private_key_logged=False)

    def mongo(self, script):
        prefix="const admin=db.getSiblingDB('admin');if(!admin.auth(process.env.MONGO_INITDB_ROOT_USERNAME,process.env.MONGO_INITDB_ROOT_PASSWORD))quit(2);const d=db.getSiblingDB('cma_native_am');"
        # Each mongosh lives in the Mongo cgroup. Do not overlap observation
        # shells when HTTP/control handlers ask for ownership concurrently.
        with self.mongo_observation_lock:
            raw=self.client.k(['exec','mongo','--','mongosh','--quiet','mongodb://127.0.0.1:27017/admin',
                '--eval',prefix+script],timeout=20)
        return json.loads(raw)

    def initialize(self):
        self.mark_initialization_step('fixture_certificates')
        self.certificates()
        self.mark_initialization_step('fixture_dependency_build')
        self.ready.setdefault('provider_source', str(PROVIDER))
        dependencies = manifests.dependencies(self.secret, self.ready)
        self.mark_initialization_step('fixture_dependency_create')
        self.client.apply(dependencies)
        self.record('dependencies_created')
        def role_done():
            pod=self.client.kjson(['get','pod','am-role','-o','json'])
            phase=pod.get('status',{}).get('phase')
            if phase=='Failed':raise RuntimeError('exact_image_role_failed')
            return phase=='Succeeded'
        self.mark_initialization_step('runtime_role')
        self.client.wait(role_done,100,'role_not_ready')
        role=json.loads(self.client.k(['logs','am-role','--limit-bytes=524288']))
        write(self.output/'runtime-role.json',role)
        grants={x.get('resource',{}).get('collection') for x in role.get('privileges',[])}
        if not {'cma_agent_manager_members','cma_agent_manager_owners'}<=grants:
            raise RuntimeError('exact_image_role_missing_coordination_grants')
        def primary():
            pod=self.client.kjson(['get','pod','mongo','-o','json'])
            if pod.get('status',{}).get('phase')=='Failed':raise RuntimeError('mongo_pod_failed_before_primary')
            rc,raw=self.client.k(['exec','mongo','--','mongosh','--quiet',
                'mongodb://127.0.0.1:27017/admin?serverSelectionTimeoutMS=1000&connectTimeoutMS=1000',
                '--eval',old.MONGO_READY],timeout=15,check=False)
            return rc==0 and 'PRIMARY_AUTHENTICATED' in raw
        self.mark_initialization_step('authenticated_mongo')
        self.client.wait(primary,120,'authenticated_mongo_primary_not_ready')
        self.client.k(['exec','mongo','--','mongosh','--quiet','mongodb://127.0.0.1:27017/admin',
            '--eval',old.mongo_role_script(role)])
        objects=manifests.managers(self.secret,self.ready)
        self.templates={o['metadata']['name'][-1]:o for o in objects if o['kind']=='StatefulSet'}
        self.client.apply([o for o in objects if o['kind']!='StatefulSet'])
        for am in ['a','b']:
            # Initialize the fresh database once before the second AM joins;
            # the complete journey begins only with both independent AMs Ready.
            self.mark_initialization_step('am_ready_'+am)
            self.client.apply([self.templates[am]])
            self.record('am_created',am=am)
            self.wait_am(am)
            self.record('am_ready',am=am)
        self.browser_cookie=login(self.client,self.origins['a'],public_host='api.localhost.direct',
            mark_stage=self.mark_initialization_step)
        if self.ready.get('fixture') == 'features':
            # The public grant route resolves recipients from the directory populated by sign-in.
            self.recipient_browser_cookie = login(self.client, self.origins['b'], public_host='api.localhost.direct',
                mark_stage=lambda step: self.mark_initialization_step('document_recipient_' + step),
                admin_bearer=self.secret['admin_bearer'], expected_subject=ADMIN)
        # Prove the runtime-visible public API route before a real Agent create.
        self.mark_initialization_step('runtime_public_tls')
        probe="import urllib.request; r=urllib.request.urlopen('https://api.localhost.direct/ready',timeout=10); print(r.status)"
        self.client.wait(lambda:self.client.k(['exec','provider','--','python3','-c',probe],check=False)[0]==0,
            30,'runtime_visible_public_api_unreachable')
        self.record('runtime_visible_public_api_tls_ready',origin='https://api.localhost.direct')
        self.mark_initialization_step('provider_readiness')
        forward=self.client.forward('provider',8080)
        self.provider_origin='https://provider.localhost.direct:'+forward.rsplit(':',1)[1]
        self.client.wait(lambda:self.provider('GET','/health').get('ok'),30,'provider_not_ready')
        self.mark_initialization_step('shared_membership')
        self.client.wait(lambda:all(v['ready'] for v in self.topology()['ams'].values()),60,'two_members_not_ready')
        self.record('two_am_shared_database_ready',topology=self.topology())

    def wait_am(self, am, renew=False):
        name='am-'+am+'-0'
        def pod_ready():
            raw=self.client.k(['get','pod',name,'-o','json','--ignore-not-found'])
            if not raw.strip():return None
            p=json.loads(raw)
            if p.get('status',{}).get('phase')=='Failed':raise RuntimeError('am_failed_'+am)
            return p if any(c.get('type')=='Ready' and c.get('status')=='True' for c in p.get('status',{}).get('conditions',[])) else None
        self.client.wait(pod_ready,150,'am_pod_not_ready_'+am)
        self.origins[am]=self.client.forward(name,8080,renew=renew)
        def ready():
            try:
                if self.client.http(self.origins[am],'GET','/ready')[0]==200:return True
            except (OSError,urllib.error.URLError):pass
            pod=self.client.kjson(['get','pod',name,'-o','json'])
            if pod.get('status',{}).get('phase')=='Failed':raise RuntimeError('am_exited_before_ready_'+am)
            return False
        self.client.wait(ready,150,'am_not_ready_'+am)

    def topology(self, **_):
        raise NotImplementedError('fixture topology must verify ConfigMap bindings')

    def ownerOf(self, agentId=None, agent_id=None, **_):
        identity=agentId or agent_id
        if not isinstance(identity,str) or not 1<=len(identity)<=200:raise ValueError('agent_id_required')
        row=self.mongo("const o=d.cma_agent_manager_owners.findOne({_id:"+json.dumps(identity)+",instance_id:{$type:'string'},$expr:{$gt:['$expires_at','$$NOW']}});const m=o&&d.cma_agent_manager_members.findOne({_id:o.instance_id,draining:false,$expr:{$gt:['$expires_at','$$NOW']}});print(JSON.stringify(o&&m?{agent_id:o._id,instance_id:o.instance_id,epoch:Number(o.epoch),relay_address:m.relay_address,expires_at:o.expires_at}:null));")
        if row is None:return None
        host=urllib.parse.urlsplit(row['relay_address']).hostname
        pods=self.client.kjson(['get','pods','-o','json'])['items']
        for pod in pods:
            if pod['metadata']['name'] in ['am-a-0','am-b-0'] and pod.get('status',{}).get('podIP')==host:
                return {**row,'am':pod['metadata']['name'][-3],'am_id':pod['metadata']['name'][-3],'pod_uid':pod['metadata']['uid']}
        # Keep the last known Pod binding while a dead owner's lease awaits takeover.
        for am,value in self.killed.items():
            if value['instance_id']==row['instance_id']:return {**row,'am':am,'am_id':am,'pod_uid':value['pod_uid']}
        raise RuntimeError('owner_pod_mapping_absent')

    def liveOwnerLease(self, agentId=None):
        if not isinstance(agentId, str) or not re.fullmatch(r'[-A-Za-z0-9_]{1,160}', agentId):
            raise ValueError('owner_lease_agent_required')
        return self.mongo(
            "const row=d.cma_agent_manager_owners.findOne({_id:" + json.dumps(agentId)
            + ",instance_id:{$type:'string'},$expr:{$gt:['$expires_at','$$NOW']}},{_id:1});"
            "print(JSON.stringify(row!==null));")

    def lifecycleSnapshot(self, sandboxId=None, agentIds=None):
        if not isinstance(agentIds, list) or len(agentIds) not in (1, 2) or any(
            not isinstance(value, str) or not re.fullmatch(r'[-A-Za-z0-9_]{1,160}', value)
            for value in [sandboxId, *agentIds]
        ) or len(set(agentIds)) != len(agentIds):
            raise ValueError('lifecycle_snapshot_lineage_required')
        sample = json.dumps({'sandboxId': sandboxId, 'agentIds': agentIds})
        return self.mongo(f'const lifecycleSample = {sample};\n' + (ROOT / 'lifecycle_snapshot.js').read_text())

    def organizationMembership(self, member=None):
        if type(member) is not bool:
            raise ValueError('organization_membership_required')
        origin = self.client.forward('identity', 8080)
        code, body = self.client.http(origin, 'POST', '/fixture/organization-membership',
                                     headers={'Authorization': 'Bearer ' + self.secret['admin_bearer']},
                                     body={'member': member})
        if code != 200 or body != {'member': member}:
            raise RuntimeError('organization_membership_update_unconfirmed')
        return body

    def runtimeFor(self, agentId=None, agent_id=None, **_):
        identity=agentId or agent_id
        live=next((a for a in ['a','b'] if a not in self.killed),None)
        status,detail=self.client.http(self.origins[live],'GET','/api/v1/agents/'+identity,
            headers={'Authorization':'Bearer '+self.secret['bearer']})
        if status!=200:raise RuntimeError('agent_runtime_read_failed')
        sandbox=detail['sandbox']['id']
        members=self.client.kjson(['get','sandboxes.agents.x-k8s.io','-o','json'])['items']
        native=[m for m in members if m['metadata'].get('annotations',{}).get('cma/sandbox')==sandbox]
        if len(native)!=1:raise RuntimeError('sandbox_native_join')
        resource=native[0]
        metadata=resource['metadata']
        uid=metadata['uid']
        generation=metadata.get('annotations',{}).get('cma/runtime-generation','')
        conditions=resource.get('status',{}).get('conditions',[])
        ready=any(condition.get('type')=='Ready' and condition.get('status')=='True'
                  and condition.get('observedGeneration')==metadata.get('generation')
                  for condition in conditions)
        if metadata.get('deletionTimestamp') or resource['spec'].get('operatingMode')=='Suspended':
            provider_status='unavailable'
        else:
            provider_status='ready' if ready else 'starting'
        pods=self.client.kjson(['get','pods','-o','json'])['items']
        pods=[p for p in pods if not p['metadata'].get('deletionTimestamp') and any(o['uid']==uid for o in p['metadata'].get('ownerReferences',[]))]
        if len(pods)!=1:raise RuntimeError('native_pod_join')
        pod=pods[0]; runtime=next(c for c in pod['spec']['containers'] if c['name']=='runtime')
        pvcs=[v['persistentVolumeClaim']['claimName'] for v in pod['spec']['volumes'] if 'persistentVolumeClaim'in v]
        value={'agent_id':identity,'sandbox_id':sandbox,'image':runtime['image'],'pod_name':pod['metadata']['name'],
            'pod_uid':pod['metadata']['uid'],'ready':detail.get('status')=='ready','pvc_names':pvcs,
            'runtime_id':f"agent-sandbox:{metadata['name']}:{uid}",
            'runtime_generation':int(generation) if generation.isdecimal() else None,'provider_status':provider_status,
            'home_identity':native[0]['metadata']['annotations'].get('cma/home-identity'),
            'runtime_security_context':runtime.get('securityContext',{}),
            'pod_security_context':pod['spec'].get('securityContext',{}),
            'runtime_dns_policy':pod['spec'].get('dnsPolicy'),
            'runtime_dns_config':pod['spec'].get('dnsConfig'),
            'declared_runtime_security_context':next((c.get('securityContext',{}) for c in native[0]['spec']['podTemplate']['spec']['containers'] if c['name']=='runtime'),{}),
            'desktop_admission_policy':self.ready.get('desktop_admission_policy'),
            'container_statuses':pod.get('status',{}).get('containerStatuses',[])}
        self.record('runtime_identity',runtime=value)
        return value

    def killAm(self, agentId=None, owner=None, **_):
        actual=self.ownerOf(agentId=agentId)
        if actual is None:raise RuntimeError('agent_owner_absent_before_kill')
        if owner and any(actual.get(k)!=owner.get(k) for k in ['am','epoch','pod_uid']):raise RuntimeError('owner_changed_before_kill')
        return self.stopOwnedAm(actual)

    def stopOwnedAm(self, actual):
        am=actual['am']; name='am-'+am+'-0'
        if am in self.killed:raise RuntimeError('am_already_killed')
        pod=self.client.kjson(['get','pod',name,'-o','json'])
        if pod['metadata']['uid']!=actual['pod_uid']:raise RuntimeError('pod_uid_changed_before_kill')
        cid=pod['status']['containerStatuses'][0]['containerID'].removeprefix('containerd://')
        if not cid or len(cid)!=64:raise RuntimeError('container_identity_missing')
        docker=['docker','exec',self.ready['node_name']]
        self.killed[am]=actual
        self.client.k(['scale', 'statefulset/am-'+am, '--replicas=0'])
        self.client.k(['wait', '--for=delete', 'pod/'+name, '--timeout=90s'], timeout=95)
        # Inspect the owned node's actual task set independently of API deletion.
        tasks=self.client.command(docker+['ctr','-n','k8s.io','tasks','list','-q'],timeout=15).split()
        if cid in tasks:raise RuntimeError('actual_owner_task_still_running')
        forward=self.client.forwards.get((name,8080))
        if forward:self.client.stop(forward[0])
        self.record('owning_am_killed',owner=actual,container_id=cid,actual_container_exited=True)
        return {'killed_am':am,'actual_container_exited':True,'pod_uid':actual['pod_uid']}

    def restartDrained(self, am=None):
        if am not in self.drained:
            raise RuntimeError('owned_drain_required')
        self.stopOwnedAm(self.drained[am])
        result = self.restoreAm(am=am)
        del self.drained[am]
        return result

    def observeScale(self):
        return observe_scale(self)

    def observeNeverReady(self, agentId=None):
        if not isinstance(agentId, str) or not re.fullmatch(r'[A-Za-z0-9_-]{1,128}', agentId):
            raise ValueError('startup_witness_agent_required')
        return observe_never_ready(self, agentId)

    def restoreAm(self, am=None, am_id=None, **_):
        am=am or am_id
        if am not in self.killed:raise RuntimeError('restore_without_owned_kill')
        self.client.k(['scale', 'statefulset/am-'+am, '--replicas=1'])
        self.wait_am(am,renew=True)
        del self.killed[am]
        self.record('am_restored',am=am,pod=self.topology()['ams'][am])
        return {'ready':True,'origin':self.origins[am]}

    def provider(self, method, path, body=None):
        raw=json.dumps(body).encode() if body is not None else None
        port=urllib.parse.urlsplit(self.provider_origin).port
        connection=http.client.HTTPSConnection('provider.localhost.direct',port,context=self.tls,timeout=10)
        # Pin the owned loopback forwarding socket while preserving real SNI,
        # hostname verification and the default public CA root set.
        connection.sock=self.tls.wrap_socket(socket.create_connection(('127.0.0.1',port),timeout=10),
            server_hostname='provider.localhost.direct')
        try:
            connection.request(method,path,body=raw,headers={'Content-Type':'application/json',
                'Authorization':'Bearer '+self.secret['provider_control']})
            response=connection.getresponse();data=response.read(1024*1024+1)
            if len(data)>1024*1024:raise RuntimeError('provider_output_bound')
            if response.status!=200:raise RuntimeError('provider_http_'+str(response.status))
            return json.loads(data)
        finally:connection.close()

    def provider_observation(self, operation, **values):
        path=self.output/'provider-observations.private.jsonl'
        with self.provider_observation_lock:
            with path.open('a') as handle:
                handle.write(json.dumps({'at':utc(),'operation':operation,**values})+'\n')
            path.chmod(0o600)

    def prepareTurn(self, caseId=None, marker=None, **extra):
        self.provider_observation('prepare_turn',input={'case_id':caseId,'marker':marker,**extra})
        value=self.provider('POST','/__ws4/arm',{'case_id':caseId,'marker':marker,
            **{key: extra[key] for key in ('suggestions', 'configuration', 'skill', 'title') if key in extra},
            **({'access_request': extra['accessRequest']} if 'accessRequest' in extra else {})})
        self.provider_observation('prepare_turn_result',snapshot=value)
        return value

    def releaseTurn(self, caseId=None, suggestion=False, title=False):
        if type(suggestion) is not bool or type(title) is not bool or (suggestion and title):
            raise ValueError('release_gate_required')
        self.provider_observation('release_turn',case_id=caseId,suggestion=suggestion,title=title)
        route = '/__ws4/release-title' if title else '/__ws4/release-suggestion' if suggestion else '/__ws4/release'
        value=self.provider('POST',route,{'case_id':caseId})
        self.provider_observation('release_turn_result',snapshot=value)
        return value

    def oracle(self, caseId=None, **_):
        value=self.provider('GET','/__ws4/state?'+urllib.parse.urlencode({'case_id':caseId}))
        self.provider_observation('oracle',snapshot=value)
        return value

    def skillEffects(self, agentId=None, skillId=None):
        if not isinstance(agentId, str) or not 1 <= len(agentId) <= 200:
            raise ValueError('agent_id_required')
        if not isinstance(skillId, str) or not re.fullmatch(r'two-am-(?:retry-)?(?:before|after)-restart', skillId):
            raise ValueError('fixture_skill_required')
        scope = json.dumps({'agentId': agentId, 'skillId': skillId})
        return self.mongo(f'const scope = {scope};\n' + (ROOT / 'skill_evidence.js').read_text())

    def preparedReady(self):
        return self.mongo(
            "const rows=d.native_prepared_sandboxes.find({kind:'reservation',state:'ready',"
            "machine:{$ne:true},'record.ownership':null},{_id:1}).limit(2).toArray();"
            "print(JSON.stringify(rows.map(row=>row._id)));")

    def preparedEvidence(self, agentId=None, sandboxId=None, operationId=None):
        if any(not isinstance(value, str) or not re.fullmatch(r'[A-Za-z0-9_-]{1,128}', value)
               for value in (agentId, sandboxId, operationId)):
            raise ValueError('prepared_lineage_required')
        lineage = json.dumps({'agentId': agentId, 'sandboxId': sandboxId, 'operationId': operationId})
        return self.mongo(f'const lineage = {lineage};\n' + (ROOT / 'prepared_evidence.js').read_text())

    def peerWriteEvidence(self, am=None, requestHash=None, epoch=None, operation=None):
        if (am not in ('a', 'b') or not isinstance(requestHash, str)
                or not re.fullmatch(r'[a-f0-9]{64}', requestHash)
                or type(epoch) is not int or not 0 < epoch < 2**53
                or operation not in ('preferences', 'settings', 'skill_install', 'response_control')):
            raise ValueError('peer_write_evidence_scope')
        raw = self.client.k(['logs', f'am-{am}-0', '--since=20s', '--limit-bytes=1048576'], timeout=5)
        matches = 0
        for entry in raw.splitlines():
            line = re.sub(r'\x1b\[[0-9;]*m', '', entry)
            if 'cma::agent_manager::peer_write:' not in line:
                continue
            fields = dict(re.findall(r'(?<!\S)(request_sha256|operation|epoch)="?([a-z0-9_]+)"?(?=\s|$)', line))
            if fields == {'request_sha256': requestHash, 'operation': operation, 'epoch': str(epoch)}:
                matches += 1
        return {'matches': matches}

    def serve(self):
        fixture=self
        class Handler(BaseHTTPRequestHandler):
            def setup(self):
                super().setup()
                self.connection.settimeout(5)
            def log_message(self,*_):pass
            def do_POST(self):
                try:
                    if not hmac.compare_digest(self.headers.get('Authorization',''),'Bearer '+fixture.control_token):
                        raise ValueError('control_unauthorized')
                    size=int(self.headers.get('Content-Length','0'))
                    if not 0<=size<=8192:raise ValueError('control_body_bound')
                    body=json.loads(self.rfile.read(size))
                    name=self.path.strip('/')
                    allowed=['topology','runtimeFor','ownerOf','killAm','restoreAm','prepareTurn',
                             'releaseTurn','oracle','captureFailure','titleEffects','titleIntent','skillEffects',
                             'drainOwner','restartDrained','peerWriteEvidence','preparedReady','preparedEvidence','preparedRuntimeEvidence',
                             'observeScale','observeNeverReady','liveOwnerLease','lifecycleSnapshot',
                             'organizationMembership','configurationLog','configurationDispatch','publicationEvidence',
                             'retirementPurge','removeRuntimePod','artifactPublicationAbort','artifactPublicationRace','preparedSchemaBarrier',
                             'preparedSchemaRace','preparedSchemaRefusals','writerPublicationAbort','writerPublicationEvidence',
                             'runtimeLossPublicationAbort','runtimeLossEvidence','runtimeInventoryAuthority',
                             'shareRedemptionPublicationAbort','accessProposalAbort','seedOversizeFile']
                    if name not in allowed:raise ValueError('control_route')
                    value=getattr(fixture,name)(**body); status=200
                except Exception as exc:
                    value={'error':type(exc).__name__+':'+str(exc)};status=500
                    fixture.record('adapter_error',route=self.path,error=value['error'])
                raw=json.dumps(value).encode()
                self.send_response(status);self.send_header('Content-Type','application/json')
                self.send_header('Content-Length',str(len(raw)));self.end_headers();self.wfile.write(raw)
        self.server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
        self.server.daemon_threads=False
        self.thread=threading.Thread(target=self.server.serve_forever,daemon=True);self.thread.start()
        return 'http://127.0.0.1:'+str(self.server.server_port)

    def captureFailure(self, reason=None, operationId=None, **_):
        destination=self.output/('diagnosis-'+str(time.time_ns()))
        destination.mkdir(mode=0o700,exist_ok=False)
        result={'started_at':utc(),'reason':reason,'mutating_requests':0,'errors':[]}
        previous_deadline=self.client.deadline
        self.client.deadline=time.monotonic()+90
        try:
            self.capture(destination)
            if isinstance(operationId, str) and re.fullmatch(r'[-A-Za-z0-9_]{1,160}', operationId):
                try:
                    script = f'const operationId = {json.dumps(operationId)};\n'
                    snapshot = self.mongo(script + (ROOT / 'creation_failure_snapshot.js').read_text())
                    write(destination/'creation-failure-snapshot.json', snapshot)
                except Exception as exc:
                    result['errors'].append({'scope': 'creation_snapshot', 'error': type(exc).__name__})
            try:
                database=self.mongo("print(EJSON.stringify({"
                    "agents:d.agents.find({}).limit(10).toArray(),"
                    "commands:d.agent_commands.find({}).limit(100).toArray(),"
                    "log:d.agent_log.find({}).sort({seq:-1}).limit(200).toArray(),"
                    "members:d.cma_agent_manager_members.find({}).limit(10).toArray(),"
                    "owners:d.cma_agent_manager_owners.find({}).limit(100).toArray()}));")
                write(destination/'durable-state.private.json',database)
            except Exception as exc:
                result['errors'].append({'scope':'durable_state','error':type(exc).__name__+':'+str(exc)})
            pod_file=destination/'pods.final.json'
            pods=json.loads(pod_file.read_text()).get('items',[]) if pod_file.exists() else []
            runtime_pods = [pod for pod in pods if any(c.get('name') == 'runtime'
                            for c in pod.get('spec', {}).get('containers', []))]
            if pod_file.exists():
                result['runtime_pod_count'] = len(runtime_pods)
            for pod in runtime_pods:
                name=pod['metadata']['name']
                try:
                    rc,raw=self.client.k(['exec',name,'-c','runtime','--','cat','/proc/net/tcp','/proc/net/tcp6'],check=False)
                    path=destination/(name+'.network.private.log');path.write_text(raw);path.chmod(0o600)
                    if rc:result['errors'].append({'scope':'runtime_network','pod':name,'exit':rc})
                except Exception as exc:
                    result['errors'].append({'scope':'runtime_network','pod':name,'error':type(exc).__name__+':'+str(exc)})
        except Exception as exc:
            result['errors'].append({'scope':'capture','error':type(exc).__name__+':'+str(exc)})
        finally:
            self.client.deadline=previous_deadline
            result['ended_at']=utc()
            result['files']=[p.name for p in sorted(destination.iterdir()) if p.is_file()]
            write(destination/'capture-result.json',result)
        return {'attempted':True,'path':str(destination),'errors':result['errors'],
                'runtime_pod_count': result.get('runtime_pod_count')}

    def capture(self, output=None):
        destination=Path(output) if output is not None else self.output
        pods=[]
        for kind in ['pods','sandboxes.agents.x-k8s.io','pvc','events']:
            try:
                value=self.client.kjson(['get',kind,'-o','json'])
                write(destination/(kind+'.final.json'),value)
                if kind=='pods':pods=value.get('items',[])
            except Exception as exc:self.record('capture_failed',kind=kind,error=str(exc))
        for name in ['am-a-0','am-b-0','mongo','identity','provider','api-proxy','storage']:
            try:
                if name in ['am-a-0','am-b-0']:
                    rc,raw=self.client.k(['logs',name,'--timestamps=true','--limit-bytes=2097152'],check=False)
                    (destination/(name+'.full.private.log')).write_text(raw)
                    (destination/(name+'.private.log')).write_text(''.join(raw.splitlines(keepends=True)[-200:]))
                    matching = next((pod for pod in pods if pod.get('metadata', {}).get('name') == name), {})
                    if any(container.get('restartCount', 0) > 0 for container in
                           matching.get('status', {}).get('containerStatuses', [])):
                        previous_rc, previous = self.client.k(
                            ['logs', name, '--previous', '--tail=100', '--limit-bytes=131072'], check=False)
                        if previous_rc == 0:
                            (destination/(name+'.previous.private.log')).write_text(previous)
                else:
                    rc,raw=self.client.k(['logs',name,'--tail=200','--limit-bytes=131072'],check=False)
                    (destination/(name+'.private.log')).write_text(raw)
                if rc:self.record('capture_failed',kind='logs',pod=name,exit=rc)
            except Exception as exc:self.record('capture_failed',kind='logs',pod=name,error=str(exc))
        for pod in pods:
            if not any(c.get('name')=='runtime' for c in pod.get('spec',{}).get('containers',[])):
                continue
            name=pod['metadata']['name']
            try:
                rc,raw=self.client.k(['logs',name,'-c','runtime','--timestamps=true','--limit-bytes=2097152'],check=False)
                (destination/(name+'.runtime.full.private.log')).write_text(raw)
                if rc:self.record('capture_failed',kind='runtime_logs',pod=name,exit=rc)
            except Exception as exc:self.record('capture_failed',kind='runtime_logs',pod=name,error=str(exc))
