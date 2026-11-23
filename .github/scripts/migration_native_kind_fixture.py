"""Scripted local infrastructure boundary for offline runner regressions."""
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import secrets

from migration_agent_documents import COLLECTIONS, CONSISTENCY
from migration_kind_manifests import IDENTITY
from migration_native_dependencies import ASSETS
from migration_native_test_fixture import runtime
from test_migration_native_campaign import artifacts
from test_migration_native_readiness import pod


def fixture_config(directory):
    selected = artifacts();selected['runtime'], manifest, config = runtime()
    pinned = json.loads((ASSETS/'manifest.json').read_text())['images']
    images = {**selected, 'mongo': {'image': 'docker.io/library/mongo@sha256:'+'4'*64, 'config': 'sha256:'+'5'*64},
              'identity': {'image': IDENTITY, 'config': 'sha256:'+'6'*64},
              **{name: {key: row[key] for key in ('image', 'config')} for name,row in pinned.items()}}
    directory = Path(directory).resolve()
    archives = {}
    for index, image in enumerate(images.values()):
        path = directory/('archive-'+str(index)+'.tar');path.write_bytes(b'not imported by offline fixture')
        archives[image['image']] = str(path)
    tls = {};pins = {}
    for name in ('certificate', 'private_key'):
        path = directory/name;raw = secrets.token_urlsafe(48).encode();path.write_bytes(raw);path.chmod(0o600)
        tls[name] = str(path);pins[name] = hashlib.sha256(raw).hexdigest()
    inventory = directory/'inventory.json'
    inventory.write_text(json.dumps({'schema': 'cma-reader-writer-source-inventory-v1',
        'old': {'source':selected['old']['source']}, 'new': {'source':selected['new']['source']},
        'collections': [{'name':name} for name in COLLECTIONS]}))
    value = {'schema':'migration-native-input-v1','artifacts':selected,'mongo':images['mongo'],
             'identity':images['identity'], 'node':{'image':'docker.io/kindest/node@sha256:'+'7'*64,
             'config':'sha256:'+'8'*64,'architecture':'amd64'},'docker_context':'fixture',
             'archives':archives,'tls':tls,'inventory':str(inventory)}
    path = directory/'input.json';path.write_text(json.dumps(value));path.chmod(0o600)
    return path, {**value,'images':images,'input_sha256':hashlib.sha256(path.read_bytes()).hexdigest(),
                  'inventory_collections':len(COLLECTIONS)}, pins, (manifest,config)


class FakeForward:
    instances = []
    def __init__(self, argv, remote, record):
        self.argv=argv;self.remote=remote;self.record=record;self.closed=False
        self.port=14000+len(self.instances);self.instances.append(self)
    def start(self): return self
    def pump(self):
        if getattr(self,'broken',False): raise RuntimeError('scripted forward failure')
    def close(self):
        self.closed=True
        return {'processes':{'process_group_cleanup':True},'port':self.port}


class FakeCommands:
    def __init__(self, config, directory, cluster):
        self.config=config;self.directory=Path(directory);self.cluster=cluster;self.calls=[]
        self.node=cluster+'-control-plane';self.node_id='a'*64;self.created=False;self.volume=False
        self.api=None;self.jobs={};self.api_count=0;self.fail=None;self.foreign_node=False
        self.dns={'kind':'ConfigMap','apiVersion':'v1','metadata':{'name':'coredns','namespace':'kube-system',
            'uid':'dns-uid','resourceVersion':'1'},'data':{'Corefile':'.:53 {\n    reload\n}\n'}}
        self.mongo=pod(config['mongo'],'mongo',uid='mongo-uid')
        self.provider=pod(config['identity'],'provider',uid='provider-uid')

    def factory(self, _record, tick):
        self.tick=tick
        return self

    @staticmethod
    def encoded(value): return json.dumps(value).encode()

    def __call__(self, argv, input_text=None, **_options):
        self.calls.append((list(argv),input_text));self.tick()
        if self.fail and self.fail(argv): raise RuntimeError('scripted command failure')
        if argv[0]=='node' and argv[1].endswith('migration_agent_document_join.mjs'):
            value=json.loads(input_text)
            return self.encoded({'schema':'migration-agent-document-join-v1',
                'stage':value['stage'],'artifact':value['selected'],'snapshot_sha256':value['snapshot_sha256'],
                'session_tail_sha256':value['session']['journal']['tail_sha256'],
                'outcome':'JOURNAL_IDENTITIES_JOINED','compatibility':'NOT_PROVEN'})
        if argv[:3]==['docker','context','inspect']:
            return self.encoded([{'Name':'fixture','Endpoints':{'docker':{'Host':'unix:///fixture.sock'}}}])
        if argv[0]=='docker':
            args=argv[3:]
            if args[:2]==['image','inspect']:
                return self.encoded([{'Os':'linux','Architecture':'amd64','Id':self.config['node']['config'],
                                     'RepoDigests':[self.config['node']['image']]}])
            if args[:2]==['ps','-a']:
                if any(v.startswith('volume=') for v in args): return b''
                return (self.node_id+'\n').encode() if self.created else b''
            if args[:2]==['volume','ls']: return b'owned-volume\n' if self.volume else b''
            if args[:2]==['volume','rm']: self.volume=False;return b''
            if args[0]=='inspect':
                return self.encoded([{'Id':self.node_id,'Name':'/'+self.node,'State':{'Running':True},
                    'Config':{'Labels':{'io.x-k8s.kind.cluster':'foreign' if self.foreign_node else self.cluster}},
                    'Mounts':[{'Type':'volume','Name':'owned-volume'}]}])
        if argv[0]=='env':
            if 'create' in argv:
                self.created=True;self.volume=True
                self.directory.joinpath('kubeconfig.private').write_bytes(secrets.token_bytes(32))
                return b''
            if 'delete' in argv: self.created=False;return b''
        if argv[0]!='kubectl': raise AssertionError('unexpected command: '+str(argv))
        args=argv[6:]
        if args[:2]==['config','view']:
            context='kind-'+self.cluster
            return self.encoded({'current-context':context,'clusters':[{'name':context,'cluster':{
                'server':'https://127.0.0.1:16443','certificate-authority-data':'fixture'}}],
                'users':[{'name':context,'user':{'client-certificate-data':'fixture','client-key-data':'fixture'}}],
                'contexts':[{'name':context,'context':{'cluster':context,'user':context}}]})
        if args[0] in ('apply','create','replace'):
            value=json.loads(input_text)
            if args[0]=='replace': self.dns=value;self.dns['metadata']['resourceVersion']='2';return b''
            for item in value.get('items',[value]):
                if item['kind']=='Deployment' and item['metadata']['name']=='api':
                    self.api_count+=1
                    image=item['spec']['template']['spec']['containers'][0]['image']
                    expected=next(v for v in self.config['images'].values() if v['image']==image)
                    self.api=pod(expected,'api',uid='api-'+str(self.api_count))
                if item['kind']=='Job': self.jobs[item['metadata']['name']]=item
            return b''
        if 'wait' in args or 'rollout' in args: return b''
        if '--raw' in args:
            if 'watch=true' in args[-1]:
                return ('\n'.join(json.dumps({'type':kind,'object':self.api}) for kind in ('ADDED','MODIFIED'))).encode()
            return self.encoded({'metadata':{'resourceVersion':'opaque-before'},'items':[self.api] if self.api else []})
        if args[0]=='get' and args[1]=='namespace':
            return self.encoded({'kind':'Namespace','metadata':{'name':'cma-migration','uid':'namespace-uid'}})
        if args[0]=='get' and args[1]=='csinode':
            return self.encoded({'metadata':{'name':self.node,'uid':'csi-uid'},'spec':{'drivers':[
                {'name':'hostpath.csi.k8s.io','nodeID':self.node}]}})
        if args[0]=='-n':
            namespace=args[1];args=args[2:]
            if args[:2]==['get','configmap']: return self.encoded(self.dns)
            if args[:2]==['get','service']:
                return self.encoded({'spec':{'clusterIP':'10.96.0.10' if args[2]=='api-proxy' else '10.96.0.11'}})
            if args[:2]==['get','pod']:
                return self.encoded(self.mongo if args[2]=='mongo' else self.provider)
            if args[:2]==['get','pods']:
                label=args[args.index('-l')+1]
                if label.startswith('job-name='):
                    name=label.split('=')[1];manifest=self.jobs[name]
                    image=manifest['spec']['template']['spec']['containers'][0]['image']
                    expected=next(v for v in self.config['images'].values() if v['image']==image)
                    row=pod(expected,name,uid=name+'-uid');row['status']['phase']='Succeeded'
                    container_name=manifest['spec']['template']['spec']['containers'][0]['name']
                    row['spec']['containers'][0]['name']=container_name
                    row['status']['containerStatuses'][0]['name']=container_name
                    row['status']['containerStatuses'][0]['state']={'terminated':{'exitCode':0}}
                    return self.encoded({'items':[row]})
                name=label.split('=')[1]
                if name=='mongo': row=self.mongo
                elif name=='provider': row=self.provider
                elif name=='csi-hostpathplugin':
                    row=pod(self.config['images']['hostpath'],'hostpath',namespace)
                    for item in ('node-driver-registrar','csi-attacher','csi-provisioner'):
                        extra=pod(self.config['images'][item],item,namespace)
                        row['spec']['containers']+=extra['spec']['containers']
                        row['status']['containerStatuses']+=extra['status']['containerStatuses']
                else:
                    expected=self.config['images'].get(name,self.config['identity'])
                    row=pod(expected,name,namespace)
                return self.encoded({'items':[row]})
            if args[0]=='logs':
                name=args[1].split('/')[1]
                if name=='native-runtime-role': return self.encoded({'createRole':'cma_am_runtime','roles':[],
                    'privileges':[{'resource':{'db':'matrix','collection':'agents'},'actions':['find','update']}]})
                if name.startswith('native-tls-'):
                    return self.encoded({'schema':'migration-native-tls-probe-v1','checks':[
                        {'url':url,'status':200,'verified_tls':True} for url in
                        ('https://api.localhost.direct/livez','https://provider.localhost.direct:8080/health')]})
                if name.startswith('catalog-'):
                    return self.encoded({'schema':'cma-migration-catalog-v1','database':'matrix',
                        'collections':[{'name':'agents','type':'collection','uuid':'same-uuid','options':{},
                                        'indexes':[{'name':'_id_','key':[['_id',1]]}]}]})
                if name.startswith('agent-documents-'):
                    return self.encoded({'schema':'migration-agent-documents-v1','database':'matrix',
                        'consistency':CONSISTENCY,'collections':{name:[] for name in COLLECTIONS},
                        'document_hashes':{name:[] for name in COLLECTIONS},'redactions':[]})
        raise AssertionError('unexpected kubectl command: '+str(args))


class FakeSession:
    def __init__(self, tick=lambda:None): self.tick=tick;self.commands=[]
    def start(self): self.tick()
    def command(self, op, payload):
        self.tick();self.commands.append((op,deepcopy(payload)))
        if op!='stage':return {}
        return {'result':{'schema':'migration-agent-session-stage-v1','stage':payload['stage'],
                         'result':{'artifact':payload['reader']['artifact'],'outcome':'API_JOURNEY_OBSERVED',
                                   'persisted_collection_coverage':'NOT_PROVEN','full_reader_writer_gate':'NOT_PROVEN'},
                         'journal':{'tail_sha256':'d'*64}}}
    def close(self):
        return {'outcome':'SESSION_OBSERVED','exit_code':0,'processes':{'process_group_cleanup':True}}
