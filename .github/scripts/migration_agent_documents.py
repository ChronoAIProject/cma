"""Bounded raw Agent collection observations; these are not compatibility cells."""
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import re

from migration_catalog import job as catalog_job
from migration_kind_manifests import NAMESPACE
from migration_native_campaign import artifact, digest, require

COLLECTIONS = (
    'agent_command_orders', 'agent_commands', 'agent_creation_provenance', 'agent_lifecycle_operations',
    'agent_log', 'agent_log_admissions', 'agent_log_audits', 'agent_log_evidence', 'agent_log_gates',
    'agent_log_heads', 'agent_log_projections', 'agent_log_publications', 'agent_log_segments', 'agents',
    'cma_agent_manager_members', 'cma_agent_manager_owners',
    'sandbox_admissions', 'sandbox_controls', 'sandboxes', 'workspaces', 'worktrees',
)
MAX_BYTES = 32 * 1024 * 1024
CONSISTENCY = 'majority-per-collection; not a cross-collection transaction'


def infrastructure(objects, password):
    require(isinstance(password, str) and re.fullmatch('[A-Za-z0-9_-]{24,64}', password),
            'campaign_document_password_invalid')
    selected = {(obj['kind'], obj['metadata']['name']): obj for obj in objects}
    init = selected[('Job', 'mongo-init')]
    config = selected[('ConfigMap', 'mongo-init')]
    require(init['metadata'].get('namespace') == NAMESPACE
            and config['metadata'].get('namespace') == NAMESPACE
            and ('ConfigMap', 'mongo-agent-documents-code') not in selected
            and ('ConfigMap', 'mongo-agent-documents-auth') not in selected,
            'campaign_document_infrastructure_duplicate_or_foreign')
    init['spec']['template']['spec']['containers'][0]['env'].append(
        {'name': 'MATRIX_AGENT_READ_PASSWORD', 'value': password})
    privileges = [{'resource': {'db': 'matrix', 'collection': name}, 'actions': ['find', 'killCursors']}
                  for name in COLLECTIONS]
    script = ('const m=db.getSiblingDB("matrix");m.createRole({role:"matrix_agent_reader",privileges:'
              + json.dumps(privileges, separators=(',', ':')) + ',roles:[]});'
              'm.createUser({user:"matrix_agent_reader",pwd:process.env.MATRIX_AGENT_READ_PASSWORD,'
              'roles:[{role:"matrix_agent_reader",db:"matrix"}]})')
    config['data']['init.sh'] += (
        '\nmongosh --host mongo:27017 --username matrix_admin --password "$MATRIX_ADMIN_PASSWORD" '
        '--authenticationDatabase admin --quiet --eval ' + "'" + script + "'\n")
    uri = (f'mongodb://matrix_agent_reader:{password}@mongo:27017/matrix?authSource=matrix'
           '&replicaSet=matrix&serverSelectionTimeoutMS=5000&connectTimeoutMS=5000&socketTimeoutMS=5000')
    for name, data in [('code', {'read.cjs': Path(__file__).with_suffix('.cjs').read_text()}),
                       ('auth', {'MATRIX_AGENT_READ_URI': uri})]:
        objects.append({'apiVersion': 'v1', 'kind': 'ConfigMap',
                        'metadata': {'name': 'mongo-agent-documents-' + name, 'namespace': NAMESPACE},
                        'data': data})
    return objects


def job(image, stage):
    result = deepcopy(catalog_job(NAMESPACE, image, stage))
    result['metadata']['name'] = 'agent-documents-' + stage
    pod = result['spec']['template']['spec']
    pod['containers'][0].update(name='agent-documents', envFrom=[{
        'configMapRef': {'name': 'mongo-agent-documents-auth'}}])
    pod['volumes'][0]['configMap']['name'] = 'mongo-agent-documents-code'
    return result


def validate(raw):
    require(isinstance(raw, str) and len(raw.encode()) <= MAX_BYTES, 'campaign_document_byte_bound')
    value = json.loads(raw)
    require(isinstance(value, dict) and set(value) == {
        'schema', 'database', 'collections', 'consistency', 'document_hashes', 'redactions'}
            and value['schema'] == 'migration-agent-documents-v1' and value['database'] == 'matrix'
            and value['consistency'] == CONSISTENCY and isinstance(value['collections'], dict)
            and set(value['collections']) == set(COLLECTIONS)
            and isinstance(value['document_hashes'], dict)
            and set(value['document_hashes']) == set(COLLECTIONS)
            and isinstance(value['redactions'], list), 'campaign_document_scope_invalid')
    count = 0
    for name, rows in value['collections'].items():
        require(isinstance(rows, list) and len(rows) <= 2048, 'campaign_document_row_bound')
        ids = set()
        for row in rows:
            require(isinstance(row, dict) and '_id' in row, 'campaign_document_id_missing')
            key = json.dumps(row['_id'], sort_keys=True, separators=(',', ':'), allow_nan=False)
            require(key not in ids, 'campaign_document_id_duplicate')
            ids.add(key)
        count += len(rows)
        hashes = value['document_hashes'][name]
        require(isinstance(hashes, list) and len(hashes) == len(rows), 'campaign_document_hash_count')
        for row in hashes:
            require(isinstance(row, dict) and set(row) == {'id_sha256', 'document_sha256'}
                    and all(isinstance(v, str) and re.fullmatch('[a-f0-9]{64}', v) for v in row.values()),
                    'campaign_document_hash_invalid')
    for row in value['redactions']:
        require(isinstance(row, dict) and set(row) == {'path', 'canonical_sha256', 'type'}
                and isinstance(row['path'], list) and len(row['path']) >= 3
                and row['path'][0] in COLLECTIONS and isinstance(row['canonical_sha256'], str)
                and re.fullmatch('[a-f0-9]{64}', row['canonical_sha256'])
                and isinstance(row['type'], str), 'campaign_document_redaction_invalid')
    require(count <= 8192, 'campaign_document_total_bound')
    return value


def capture(campaign, image, stage, selected_artifact, reader, session_result, *, execute_job=None):
    """Write protected synthetic rows privately; retain only counts/hashes publicly.

    The surrounding engine independently verifies reader stability on both
    sides. This collector joins the exact session journal tail, without claiming
    that a nonempty collection was exercised through its application decoder.
    """
    identity = artifact(selected_artifact)
    require(session_result.get('stage') == stage
            and session_result.get('result', {}).get('artifact') == identity,
            'campaign_document_session_mismatch')
    journal = session_result.get('journal', {})
    require(isinstance(journal.get('tail_sha256'), str)
            and re.fullmatch('[a-f0-9]{64}', journal['tail_sha256']),
            'campaign_document_journal_required')
    manifest = job(image, stage)
    path = campaign.output / ('agent-documents-' + stage + '.private.json')
    require(campaign.output.resolve() == campaign.output.absolute()
            and campaign.output.stat().st_uid == os.getuid()
            and not campaign.output.stat().st_mode & 0o077,
            'campaign_document_private_directory_required')
    if path.exists() or path.is_symlink():
        raise FileExistsError('campaign_document_snapshot_exists')
    # create + restartPolicy Never/backoffLimit 0 prevents collector replay.
    if execute_job is not None:
        # The native adapter additionally binds the actual collector Pod UID,
        # imageID, zero restarts and successful termination before reading logs.
        raw = execute_job(manifest)
    else:
        name = manifest['metadata']['name']
        campaign.kubectl(['create', '-f', '-'], input_text=json.dumps(manifest))
        campaign.kubectl(['-n', NAMESPACE, 'wait', '--for=condition=complete',
                          'job/' + name, '--timeout=125s'], timeout=135)
        raw = campaign.kubectl(['-n', NAMESPACE, 'logs', 'job/' + name, '-c', 'agent-documents'], timeout=20)
    value = validate(raw)
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, 'w') as stream:
        stream.write(raw)
        stream.flush()
        os.fsync(stream.fileno())
    return {
        'schema': 'migration-agent-document-observation-v1', 'stage': stage, 'artifact': identity,
        'reader_sha256': digest(reader), 'session_journal': journal,
        'raw_path': path.name, 'raw_sha256': hashlib.sha256(raw.encode()).hexdigest(),
        'collections': {name: len(rows) for name, rows in value['collections'].items()},
        'redacted_fields': len(value['redactions']),
        'raw_policy': 'Original document hashes; credential fields protected before collector logs.',
        'outcome': 'DOCUMENTS_OBSERVED', 'compatibility': 'NOT_PROVEN', 'consistency': CONSISTENCY,
    }
