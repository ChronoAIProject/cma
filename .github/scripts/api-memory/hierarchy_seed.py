"""Two dormant synthetic records and their exact-67b durable provider choice.

No exporter, provider, credentials, runtime binding, Agent or event history.
The actual API's decoder and owner hierarchy GETs must accept both records before
measurement. This module does not claim a Rust-codec roundtrip before that check.
"""
import hashlib
import json

import pair
import repeat

SANDBOX = 'sbx_allocator_read'
WORKSPACE = 'wks_allocator_read'
SUBJECT = 'synthetic-allocator-reader'
SANDBOX_ROUTE = '/api/v1/sandboxes/' + SANDBOX
WORKSPACE_ROUTE = '/api/v1/workspaces/' + WORKSPACE
DEPLOYMENT = 'cma-memory-rca'


def runtime_route():
    # runtime_routing.rs::key uses provider_finality.rs::provider_request_digest.
    identity = json.dumps([DEPLOYMENT, SANDBOX, 1], separators=(',', ':')).encode()
    return {'_id': hashlib.sha256(b'cma-provider-effect-v1\0' + identity).hexdigest(),
            'document_schema_version': {'$numberLong': '1'}, 'sandbox_id': SANDBOX,
            'generation': {'$numberLong': '1'}, 'deployment': DEPLOYMENT, 'backend': 'agent-sandbox'}


def documents():
    at = '2026-11-14T17:00:00Z'
    work = {'ready_at': at, 'next_fence': 1, 'attempts': 0}
    sandbox = {
        'id': SANDBOX, 'owner': SUBJECT, 'name': 'allocator-read',
        'origin': {'type': 'manual'},
        'profile': {'id': 'sbp_allocator_read', 'revision': 1, 'digest': 'a'*64,
                    'content': {'cpu_millis': 1000, 'memory_mib': 2048,
                                'nyxid_services': [], 'agent_provider': 'codex',
                                'model_access': 'chronoai_managed', 'max_workspaces': 4,
                                'max_agents_per_workspace': 4, 'workspace_profiles': [],
                                'software_install_commands': [], 'environment_variables': []}},
        'status': 'failed', 'failure': {'class': 'synthetic_fixture', 'detail': 'Dormant local read fixture'},
        'credential': {'reaches': [], 'key_generation': 0, 'delivered': False,
                       'revoke_debt': [], 'sign_in': 'not_required'},
        'work': dict(work, step={'kind': 'none'}),
        'limits': {'max_workspaces': 4, 'max_agents_per_workspace': 4},
        'last_activity_at': at, 'created_at': at, 'updated_at': at, 'revision': 1,
    }
    workspace = {
        'id': WORKSPACE, 'sandbox_id': SANDBOX, 'owner': SUBJECT,
        'origin': {'type': 'manual'},
        'profile': {'id': 'wkp_allocator_read', 'revision': 1, 'digest': 'd'*64,
                    'content': {'location': {'type': 'directory', 'name': 'allocator-read'},
                                'name': 'allocator-read', 'setup_commands': [], 'agent_profiles': []}},
        'name': 'allocator-read', 'path': '/home/cma/allocator-read', 'source': {'type': 'empty'},
        'status': 'ready', 'work': dict(work, step='none'), 'next_worktree_number': 1,
        'created_at': at, 'ready_at': at, 'revision': 1,
    }
    common = {'document_schema_version': {'$numberLong': '1'}, 'owner': SUBJECT,
              'name': 'allocator-read', 'origin_type': 'manual', 'origin_launch_id': None,
              'origin_operation_id': None, 'work_pending': False, 'work_ready_at': at,
              'work_lease_expires_at': None, 'revision': {'$numberLong': '1'}}
    parent = dict(common, _id=SANDBOX, admission_name=None, admission_fingerprint=None,
                  status='failed', runtime_version=None, updated_at=at,
                  retired_at=None, purge_after=None, last_seq={'$numberLong': '0'}, record=sandbox)
    child = dict(common, _id=WORKSPACE, sandbox_id=SANDBOX, origin_index=None,
                 status='ready', removed_at=None, next_worktree_number={'$numberLong': '1'}, record=workspace)
    return {'sandboxes': [parent], 'workspaces': [child], 'runtime_backend_routes': [runtime_route()]}


def validate_body(route, body):
    if route not in (SANDBOX_ROUTE, WORKSPACE_ROUTE):
        return
    value = json.loads(body)
    expected = SANDBOX if route == SANDBOX_ROUTE else WORKSPACE
    if value.get('id') != expected:
        raise ValueError('fixture response identity mismatch')
    if route == SANDBOX_ROUTE:
        if (value.get('status') != 'failed' or value.get('workspace_count') != 1
                or value.get('agent_count') != 0 or value.get('worktree_count') != 0
                or [w.get('id') for w in value.get('workspaces', [])] != [WORKSPACE]):
            raise ValueError('sandbox hierarchy fixture changed')
    elif (value.get('sandbox_id') != SANDBOX or value.get('status') != 'ready'
          or value.get('worktrees') != [] or value.get('agents') != []):
        raise ValueError('workspace hierarchy fixture changed')

class Fixture:
    subject = SUBJECT

    def __init__(self):
        self.databases = []
        self.payload = json.dumps(documents(), sort_keys=True, separators=(',', ':'))
        self.digest = hashlib.sha256(self.payload.encode()).hexdigest()

    def seed(self, mongo, database):
        # Role/user creation already created the dedicated database, but no application records.
        script = r'''
const input=JSON.parse(require('fs').readFileSync(0,'utf8'));
if(!/^cma_memory_rca_[a-z0-9_]+$/.test(input.database))throw Error('scope');
const target=db.getSiblingDB(input.database);
const records=EJSON.parse(input.records,{relaxed:false});
for(const name of ['sandboxes','workspaces','worktrees','agents','agent_log','runtime_backend_routes'])
 if(target.getCollection(name).countDocuments({})!==0)throw Error('nonempty fixture');
for(const [name,rows] of Object.entries(records)) target.getCollection(name).insertMany(rows);
print('seeded');
'''
        result = pair.docker('exec', '-i', mongo, 'mongosh', '--quiet', '--eval', script,
                             input=json.dumps({'database': database, 'records': self.payload}))
        if result.strip() != 'seeded':
            raise RuntimeError('fixture seed not acknowledged')
        self.databases.append(database)

    def check(self, configs, mongo):
        routes = [SANDBOX_ROUTE, WORKSPACE_ROUTE]
        checks = {}
        for kind, config in configs.items():
            checks[kind] = {}
            for route in routes:
                body = None
                try:
                    body = repeat.get(config['origin'], route)
                    validate_body(route, body)
                except Exception as error:
                    error.safe_details = {'boundary': 'fixture_read', 'variant': kind, 'route': route,
                                          'failure_type': type(error).__name__,
                                          'http_status': getattr(error, 'status', 200 if body is not None else None),
                                          'body_bytes': getattr(error, 'body_bytes', len(body) if body is not None else None)}
                    raise
                checks[kind][route] = {'http_200': True, 'identity_and_hierarchy_match': True, 'bytes': len(body)}
        script = r'''
const input=JSON.parse(require('fs').readFileSync(0,'utf8'));
const result=[];
for(const name of input.databases){
 const target=db.getSiblingDB(name);
 const counts=Object.fromEntries(['sandboxes','workspaces','worktrees','agents','agent_log','runtime_backend_routes'].map(k=>[k,target[k].countDocuments({})]));
 const pending=['sandboxes','workspaces'].reduce((n,k)=>n+target[k].countDocuments({work_pending:true}),0);
 const parent=target.sandboxes.findOne({});const child=target.workspaces.findOne({});
 const route=EJSON.parse(input.route,{relaxed:false});
 const routeMatches=target.runtime_backend_routes.countDocuments(route)===1;
 const initialized=target.runtime_backend_routes.countDocuments({_id:'provider-routing-initialized:'+route.deployment,
   deployment:route.deployment,document_schema_version:Long(1)})===1;
 result.push({counts,pending_work:pending,parent_status:parent.status,workspace_status:child.status,
   no_runtime:parent.record.runtime==null,no_credentials:parent.record.credential.current==null&&parent.record.credential.pending==null,
   parent_revision:Number(parent.revision),workspace_revision:Number(child.revision),
   provider_choice_matches:routeMatches,provider_routing_initialized:initialized});
}
print(JSON.stringify(result));
'''
        states = json.loads(pair.docker('exec', '-i', mongo, 'mongosh', '--quiet', '--eval', script,
                                       input=json.dumps({'databases': self.databases, 'route': json.dumps(runtime_route())})))
        expected = {'sandboxes': 1, 'workspaces': 1, 'worktrees': 0, 'agents': 0, 'agent_log': 0,
                    'runtime_backend_routes': 2}
        if len(states) != 2 or any(s['counts'] != expected or s['pending_work'] or s['parent_status'] != 'failed'
                                 or s['workspace_status'] != 'ready' or not s['no_runtime'] or not s['no_credentials']
                                 or s['parent_revision'] != 1 or s['workspace_revision'] != 1
                                 or not s['provider_choice_matches'] or not s['provider_routing_initialized'] for s in states):
            raise RuntimeError('fixture acquired lifecycle work or changed')
        return {'fixture_sha256': self.digest, 'reads': checks, 'states': states,
                'limitation': 'Dormant failed parent and ready workspace; no live runtime, Agents or provider traffic.'}
