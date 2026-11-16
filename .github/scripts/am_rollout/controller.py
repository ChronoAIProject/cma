#!/usr/bin/env python3
"""Prepare and dispatch one image-only AM successor; no automatic recovery writes."""
import argparse
import contextlib
import copy
import datetime as dt
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import time
from urllib.parse import urlsplit

CONTEXT = 'gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke'
NAMESPACE = 'chrono-bot'
DEPLOYMENT = 'cma-agent-manager'
REPOSITORY = 'us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/cma-api'
LOCK = Path('/Users/shining/code/work/.tl-worktrees/cma-cma-release/api/target/m4-20261115/u2-controller.lock')
TEAM = '/Users/shining/.claude/skills/shining-tech-lead/scripts/team.sh'
KUBECTL = '/opt/homebrew/share/google-cloud-sdk/bin/kubectl'
ROUTE = 'reviewed-release-controller'
REQUEST_TIMEOUT = 20
REQUEST_RESERVE = 25
IDENTITY = {'CHRONO_AGENT_MANAGER_POD_NAME': 'metadata.name',
            'CHRONO_AGENT_MANAGER_POD_NAMESPACE': 'metadata.namespace',
            'CHRONO_AGENT_MANAGER_POD_UID': 'metadata.uid'}
REHEARSAL_CHECKS = ('two_replicas', 'image_roll', 'create_reply', 'drain_wake', 'hpa_2_3_2')
FIXTURE_NAME = 'cma-rev234-synthetic-opensandbox-key'
KEY_FILE = '/var/secrets/opensandbox-api-key'
FIXTURE_RULE = {
    'kind': 'local_opensandbox_secret_v1',
    'scope': 'LOCAL_APPLICATION_QUALIFICATION_ONLY',
    'volume_path': '/spec/volumes/3',
    'production_volume': {'name': 'opensandbox-api-key', 'csi': {
        'driver': 'secrets-store-gke.csi.k8s.io', 'readOnly': True,
        'volumeAttributes': {'secretProviderClass': 'cma-opensandbox-api-key'}}},
    'local_volume': {'name': 'opensandbox-api-key', 'secret': {
        'secretName': FIXTURE_NAME, 'defaultMode': 288,
        'items': [{'key': 'opensandbox-api-key', 'path': 'opensandbox-api-key'}]}},
    'mount': {'name': 'opensandbox-api-key', 'mountPath': '/var/secrets', 'readOnly': True},
}


class Refused(RuntimeError):
    pass


def require(ok, reason):
    if not ok:
        raise Refused(reason)


def canon(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def hash_json(value):
    return digest(canon(value))


def read_bytes(path):
    # Open once, forbid pseudo-files/symlinks, and check the descriptor before read.
    p = Path(path)
    require(not str(p.resolve()).startswith('/dev/'), 'regular_JSON_file_required')
    require(not p.is_symlink(), 'regular_JSON_file_required')
    fd = os.open(p, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, 'rb') as stream:
        require(stat.S_ISREG(os.fstat(stream.fileno()).st_mode), 'regular_JSON_file_required')
        return stream.read()


def read_json(path):
    return json.loads(read_bytes(path))


def bound(ref):
    require(isinstance(ref, dict) and set(ref) == {'path', 'sha256'}, 'unbound_evidence')
    raw = read_bytes(ref['path'])
    require(digest(raw) == ref['sha256'], 'evidence_hash_changed')
    return json.loads(raw)


def write_json(path, value):
    p = Path(path)
    p.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    temporary = p.with_name(p.name + '.tmp-' + str(os.getpid()))
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'wb') as out:
        out.write(canon(value) + b'\n')
        out.flush()
        os.fsync(out.fileno())
    os.replace(temporary, p)
    fd = os.open(p.parent, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


@contextlib.contextmanager
def writer_lock(path):
    # macOS lockf(1) uses BSD flock(2), not POSIX fcntl.lockf(). One owner, no re-exec.
    # Keep the inode: unlinking it permits another writer to lock a different file.
    fd = os.open(path, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    try:
        require(stat.S_ISREG(os.fstat(fd).st_mode), 'writer_lock_not_regular')
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise Refused('writer_lock_busy') from error
        require(os.fstat(fd).st_ino == os.stat(path).st_ino, 'writer_lock_inode_changed')
        yield
    finally:
        os.close(fd)


def container(deployment):
    rows = deployment['spec']['template']['spec']['containers']
    matches = [(i, row) for i, row in enumerate(rows) if row['name'] == 'api']
    require(len(matches) == 1, 'exact_api_container_required')
    return matches[0]


def required_template(deployment):
    _, api = container(deployment)
    entries = api.get('env', [])
    require(len({e['name'] for e in entries}) == len(entries), 'duplicate_env')
    env = {e['name']: e for e in entries}
    for name, field in IDENTITY.items():
        require(env.get(name) == {'name': name, 'valueFrom': {'fieldRef': {
            'apiVersion': 'v1', 'fieldPath': field}}}, 'pod_identity_missing_or_changed:' + name)
    require(env.get('RUST_MIN_STACK') == {'name': 'RUST_MIN_STACK', 'value': '16777216'},
            'stack_mitigation_missing_or_changed')


def baseline_shape(deployment):
    require(deployment['kind'] == 'Deployment' and deployment['metadata']['name'] == DEPLOYMENT
            and deployment['metadata']['namespace'] == NAMESPACE, 'wrong_AM_target')
    require(deployment['metadata']['annotations']['deployment.kubernetes.io/revision'] == '233',
            'baseline_revision_must_be_233')
    require(deployment['spec']['replicas'] == 2 and not deployment['spec'].get('paused', False),
            'baseline_must_be_active_pair')
    require(deployment['spec']['strategy'] == {'type': 'RollingUpdate', 'rollingUpdate': {
        'maxSurge': 1, 'maxUnavailable': 0}}, 'baseline_strategy_changed')
    require(deployment['spec']['minReadySeconds'] == 30, 'baseline_minReady_changed')
    require(deployment['spec']['template']['spec']['terminationGracePeriodSeconds'] == 735,
            'baseline_grace_changed')
    require(container(deployment)[1]['lifecycle']['preStop']['exec']['command'] == ['/bin/sleep', '30'],
            'baseline_preStop_changed')
    required_template(deployment)


def differences(left, right, path=''):
    """Exact leaf differences; arrays remain ordered (including env expansion order)."""
    if isinstance(left, dict) and isinstance(right, dict):
        rows = []
        for key in sorted(set(left) | set(right)):
            child = path + '/' + key.replace('~', '~0').replace('/', '~1')
            if key not in left or key not in right:
                rows.append({'path': child, 'production_present': key in left,
                             'rehearsal_present': key in right,
                             'production': left.get(key), 'rehearsal': right.get(key)})
            else:
                rows.extend(differences(left[key], right[key], child))
        return rows
    if isinstance(left, list) and isinstance(right, list) and len(left) == len(right):
        return [row for i, (a, b) in enumerate(zip(left, right))
                for row in differences(a, b, path + '/' + str(i))]
    if left != right or type(left) is not type(right):
        return [{'path': path, 'production_present': True, 'rehearsal_present': True,
                 'production': left, 'rehearsal': right}]
    return []


def exception_path(path):
    # Only local placement/reference substitutions, never behavior, identity, stack,
    # readiness, drain hook, grace, images, command, grants, or entire subtrees.
    return bool(re.fullmatch(
        r'/metadata/(?:labels|annotations)/[^/]+'
        r'|/spec/serviceAccount(?:Name)?'
        r'|/spec/containers/\d+/envFrom/\d+/(?:configMapRef|secretRef)/name'
        r'|/spec/volumes/\d+/(?:secret/secretName|configMap/name)'
        r'|/spec/containers/\d+/resources/(?:requests|limits)/(?:cpu|memory)', path))


def fixture_differences(production, rehearsal, rule):
    """One explicit local source pair; never normalize the production template."""
    require(canon(rule) == canon(FIXTURE_RULE), 'unsupported_credential_fixture_rule')
    for deployment, expected in ((production, rule['production_volume']),
                                  (rehearsal, rule['local_volume'])):
        pod = deployment['spec']['template']['spec']
        volumes = pod.get('volumes', [])
        names = [v['name'] for v in volumes]
        require(len(names) == len(set(names)) and len(volumes) > 3
                and canon(volumes[3]) == canon(expected), 'credential_fixture_volume_not_exact')
        security = pod.get('securityContext', {})
        require(all(security.get(k) == 10001 for k in ('runAsUser', 'runAsGroup', 'fsGroup')),
                'credential_fixture_pod_identity_changed')
        _, api = container(deployment)
        mounts = [(row['name'], m) for row in pod.get('containers', []) + pod.get('initContainers', [])
                  for m in row.get('volumeMounts', []) if m.get('name') == 'opensandbox-api-key'
                  or m.get('mountPath') == '/var/secrets']
        require(canon(mounts) == canon([('api', rule['mount'])]), 'credential_fixture_mount_not_exact')
        restricted = api.get('securityContext', {})
        require(restricted.get('runAsNonRoot') is True
                and restricted.get('readOnlyRootFilesystem') is True
                and restricted.get('allowPrivilegeEscalation') is False
                and restricted.get('privileged') is False
                and restricted.get('capabilities') == {'drop': ['ALL']}
                and restricted.get('runAsUser', 10001) == 10001
                and restricted.get('runAsGroup', 10001) == 10001,
                'credential_fixture_container_security_changed')
    return differences(rule['production_volume'], rule['local_volume'], rule['volume_path'])


def template_gate(production, rehearsal, exceptions, credential_rule=None):
    required_template(rehearsal)
    rows = differences(production['spec']['template'], rehearsal['spec']['template'])
    require(isinstance(exceptions, list), 'exceptions_must_be_list')
    require(len({r['path'] for r in exceptions}) == len(exceptions), 'duplicate_exception')
    expected = ([] if credential_rule is None else
                fixture_differences(production, rehearsal, credential_rule))
    for rule in exceptions:
        require(set(rule) == {'path', 'production_present', 'rehearsal_present',
                              'production', 'rehearsal', 'reason'}, 'exact_exception_required')
        require(exception_path(rule['path']) and bool(rule['reason'].strip()),
                'behavior_difference_cannot_be_excepted:' + rule['path'])
        expected.append({k: v for k, v in rule.items() if k != 'reason'})
    require(canon(sorted(rows, key=lambda r: r['path'])) == canon(sorted(expected, key=lambda r: r['path'])),
            'rehearsal_production_template_diff_not_exactly_reviewed')
    return rows


def nonempty(value):
    return isinstance(value, str) and bool(value.strip())


def sha256_value(value):
    return isinstance(value, str) and re.fullmatch(r'[0-9a-f]{64}', value) is not None


def validate_fixture_evidence(packet, rehearsal, receipt, publication, evidence):
    """Admit bound observations, never a projection or an accept-all fake."""
    candidate = packet['candidate']
    require(evidence.get('schema') == 'am-local-credential-fixture-v1'
            and evidence.get('status') == 'PASS', 'credential_fixture_not_executed')
    config = candidate.get('config_digest')
    require(isinstance(config, str) and re.fullmatch(r'sha256:[0-9a-f]{64}', config)
            and publication.get('config_digest') == config, 'credential_fixture_config_unbound')
    for key, expected in (('source', candidate['source']), ('image', candidate['image']),
                          ('config_digest', config),
                          ('template_sha256', hash_json(rehearsal['spec']['template']))):
        require(evidence.get(key) == expected, 'credential_fixture_binding_mismatch:' + key)
    require(nonempty(receipt.get('run_id')) and evidence.get('run_id') == receipt['run_id'],
            'credential_fixture_run_mismatch')
    cluster = evidence.get('cluster', {})
    url = urlsplit(cluster.get('server', ''))
    require(cluster == receipt.get('local_cluster') and nonempty(cluster.get('kube_system_uid'))
            and nonempty(cluster.get('context')) and cluster['context'].startswith('kind-')
            and url.scheme == 'https' and url.hostname in ('127.0.0.1', 'localhost', '::1')
            and url.port is not None and not url.username and not url.password
            and not url.query and not url.fragment and url.path in ('', '/'),
            'credential_fixture_cluster_not_local')
    namespace = evidence.get('namespace', {})
    require(namespace == receipt.get('local_namespace')
            and nonempty(namespace.get('uid')) and nonempty(namespace.get('name'))
            and namespace['name'] != NAMESPACE
            and namespace['name'] == rehearsal['metadata'].get('namespace'),
            'credential_fixture_namespace_mismatch')
    secret = evidence.get('secret', {})
    require(secret.get('name') == FIXTURE_NAME and secret.get('namespace') == namespace['name']
            and nonempty(secret.get('uid')) and nonempty(secret.get('resource_version'))
            and secret.get('type') == 'Opaque' and secret.get('immutable') is True
            and secret.get('data_keys') == ['opensandbox-api-key']
            and secret.get('provenance') == 'generated_for_this_local_run'
            and sha256_value(secret.get('content_sha256'))
            and set(secret) == {'name', 'namespace', 'uid', 'resource_version', 'type', 'immutable',
                                'data_keys', 'provenance', 'content_sha256'},
            'credential_fixture_secret_not_exact')
    require(receipt.get('fixture_secret') == {k: secret[k] for k in
            ('name', 'namespace', 'uid', 'resource_version', 'content_sha256')},
            'credential_fixture_secret_identity_mismatch')
    fake = evidence.get('fake_provider', {})
    origin = urlsplit(fake.get('origin', ''))
    require(fake.get('namespace_uid') == namespace['uid'] and nonempty(fake.get('service_uid'))
            and fake.get('isolated') is True and fake.get('production') is False
            and origin.scheme in ('http', 'https') and origin.hostname is not None
            and origin.hostname.endswith('.' + namespace['name'] + '.svc')
            and not origin.username and not origin.password and not origin.query
            and not origin.fragment and origin.path in ('', '/')
            and fake == receipt.get('fake_provider'), 'credential_fixture_provider_not_isolated')
    identities = receipt.get('candidate_processes', [])
    observed = evidence.get('processes', [])
    require(isinstance(identities, list) and len(identities) >= 3 and isinstance(observed, list),
            'credential_fixture_process_coverage_missing')
    keys = ('pod_uid', 'container_id', 'image_id')
    expected = {p.get('pod_uid'): p for p in identities}
    actual = {p.get('pod_uid'): p for p in observed}
    require(len(expected) == len(identities) and len(actual) == len(observed)
            and set(expected) == set(actual), 'credential_fixture_process_identity_mismatch')
    final = receipt.get('final_pod_uids', [])
    require(isinstance(final, list) and len(final) == len(set(final)) == 2
            and set(final).issubset(expected), 'credential_fixture_final_pair_missing')
    for uid, identity in expected.items():
        process = actual[uid]
        require(set(identity) == set(keys) and all(nonempty(identity[k]) for k in keys)
                and identity['image_id'].endswith(candidate['image'].split('@')[1])
                and {k: process.get(k) for k in keys} == identity
                and process.get('namespace_uid') == namespace['uid']
                and process.get('secret_uid') == secret['uid']
                and process.get('secret_resource_version') == secret['resource_version'],
                'credential_fixture_process_identity_mismatch')
        require(process.get('uid') == process.get('gid') == 10001
                and 10001 in process.get('groups', []), 'credential_fixture_effective_identity')
        file = process.get('file', {})
        require(file.get('path') == KEY_FILE and type(file.get('resolved_mode')) is int
                and file['resolved_mode'] == 288 and type(file.get('resolved_uid')) is int
                and file.get('resolved_uid') in (0, 10001) and file.get('resolved_gid') == 10001
                and file.get('sha256') == secret['content_sha256']
                and all(file.get(k) is True for k in ('readable', 'valid_nonempty_header',
                    'read_only_mount', 'write_refused', 'non_destructive_write_check')),
                'credential_fixture_file_proof_missing')
        effective = process.get('effective_config', {})
        require(effective.get('api_key_file') == KEY_FILE
                and effective.get('sandbox_provider') in ('opensandbox', 'agent-sandbox')
                and isinstance(effective.get('retain_opensandbox'), bool)
                and (effective['sandbox_provider'] == 'opensandbox' or effective['retain_opensandbox'])
                and effective.get('sandbox_base_url') == fake['origin'],
                'credential_fixture_file_read_branch_disabled')
    authentication = evidence.get('authentication', {})
    accepted = authentication.get('am_requests', [])
    require(isinstance(accepted, list) and bool(accepted)
            and len({r.get('request_id') for r in accepted}) == len(accepted)
            and all(nonempty(r.get('request_id')) and r.get('pod_uid') in actual
                    and r.get('caller') == 'actual_am' and r.get('credential_source') == 'startup_file'
                    and type(r.get('status')) is int and 200 <= r['status'] < 300
                    and r.get('fake_service_uid') == fake['service_uid'] for r in accepted),
            'credential_fixture_actual_AM_auth_missing')
    negative = authentication.get('negative_checks', [])
    require(isinstance(negative, list) and len(negative) == 2
            and {r.get('credential') for r in negative} == {'missing', 'wrong'}
            and len({r.get('request_id') for r in negative}) == 2
            and not {r.get('request_id') for r in negative}.intersection(r['request_id'] for r in accepted)
            and all(nonempty(r.get('request_id')) and r.get('status') in (401, 403)
                    and r.get('fake_service_uid') == fake['service_uid'] for r in negative),
            'credential_fixture_negative_auth_missing')


def validate_packet(packet):
    require(packet.get('schema') == 'am-successor-v1', 'wrong_packet_schema')
    require(packet.get('context') == CONTEXT and packet.get('namespace') == NAMESPACE,
            'packet_target_changed')
    baseline = bound(packet['baseline']['deployment'])
    baseline_shape(baseline)
    candidate = packet['candidate']
    require(isinstance(candidate, dict), 'candidate_pending')
    image = candidate['image']
    require(re.fullmatch(re.escape(REPOSITORY) + r'@sha256:[0-9a-f]{64}', image or ''),
            'immutable_API_image_required')
    require(image != container(baseline)[1]['image'], 'candidate_equals_baseline')
    require(re.fullmatch(r'[0-9a-f]{40}', candidate['source'] or ''), 'source_required')
    publication = bound(candidate['publication'])
    require(publication.get('status') == 'PASS' and publication.get('image') == image
            and publication.get('source') == candidate['source']
            and publication.get('exact_config_and_layers_match') is True, 'publication_binding_failed')
    rehearsal = bound(packet['rehearsal']['deployment'])
    receipt = bound(packet['rehearsal']['receipt'])
    planned = copy.deepcopy(baseline)
    container(planned)[1]['image'] = image
    fixture = packet['rehearsal'].get('credential_fixture')
    if fixture is not None:
        require(isinstance(fixture, dict) and set(fixture) == {'rule', 'evidence'},
                'credential_fixture_binding_required')
    rows = template_gate(planned, rehearsal, packet['rehearsal']['exceptions'],
                         fixture['rule'] if fixture is not None else None)
    require(receipt.get('status') == 'PASS' and receipt.get('source') == candidate['source']
            and receipt.get('image') == image
            and receipt.get('template_sha256') == hash_json(rehearsal['spec']['template'])
            and all(receipt.get('checks', {}).get(c) == 'PASS' for c in REHEARSAL_CHECKS),
            'exact_image_runtime_rehearsal_not_passed')
    if fixture is not None:
        validate_fixture_evidence(packet, rehearsal, receipt, publication, bound(fixture['evidence']))
    return baseline, planned, rows


def validate_live(packet, baseline, current, hpa, namespace, pods, endpoints):
    baseline_shape(current)
    require(current['metadata']['uid'] == baseline['metadata']['uid']
            and current['metadata']['generation'] == baseline['metadata']['generation']
            and current['spec'] == baseline['spec'], 'production_changed_since_packet')
    expected_hpa = bound(packet['baseline']['hpa'])
    expected_namespace = bound(packet['baseline']['namespace'])
    require(hpa['metadata']['uid'] == expected_hpa['metadata']['uid']
            and hpa['spec'] == expected_hpa['spec']
            and hpa['spec']['minReplicas'] == hpa['spec']['maxReplicas'] == 2, 'HPA_changed')
    require(namespace['metadata']['uid'] == expected_namespace['metadata']['uid'], 'namespace_changed')
    status = current.get('status', {})
    require(status.get('observedGeneration') == current['metadata']['generation']
            and all(status.get(k) == 2 for k in ('replicas', 'readyReplicas', 'availableReplicas', 'updatedReplicas'))
            and status.get('terminatingReplicas', 0) == 0, 'deployment_not_converged')
    rows = pods['items']
    require(len(rows) == 2, 'exact_two_pods_required')
    uids = set()
    for pod in rows:
        require(not pod['metadata'].get('deletionTimestamp')
                and pod['status']['phase'] == 'Running'
                and any(c['type'] == 'Ready' and c['status'] == 'True' for c in pod['status']['conditions']),
                'pod_not_ready')
        states = [c for c in pod['status']['containerStatuses'] if c['name'] == 'api']
        require(len(states) == 1 and states[0]['ready'] and states[0]['restartCount'] == 0
                and states[0]['state'].get('running') and states[0].get('containerID')
                and states[0]['imageID'].endswith(container(baseline)[1]['image'].split('@')[1]),
                'pod_process_identity_unproved')
        uids.add(pod['metadata']['uid'])
    ready = {e['targetRef']['uid'] for s in endpoints['items'] for e in s.get('endpoints', [])
             if e.get('conditions', {}).get('ready') is True
             and e.get('conditions', {}).get('terminating') is not True}
    require(len(uids) == 2 and uids == ready, 'serving_endpoints_not_exact_pair')


def image_patch(current, candidate):
    index, _ = container(current)
    return [{'op': 'test', 'path': '/metadata/uid', 'value': current['metadata']['uid']},
            {'op': 'test', 'path': '/metadata/resourceVersion', 'value': current['metadata']['resourceVersion']},
            {'op': 'test', 'path': '/metadata/generation', 'value': current['metadata']['generation']},
            {'op': 'test', 'path': '/metadata/annotations/deployment.kubernetes.io~1revision', 'value': '233'},
            {'op': 'test', 'path': '/spec', 'value': current['spec']},
            {'op': 'replace', 'path': f'/spec/template/spec/containers/{index}/image', 'value': candidate}]


def window_guard(window, clock=time.time):
    now = clock()
    require(window['start_epoch'] <= now < window['end_epoch'], 'outside_lead_write_window')
    local = dt.datetime.fromtimestamp(now, dt.timezone(dt.timedelta(hours=8)))
    require(local.hour != 2, 'SGT_write_blackout')


def dispatch_budget(window, clock=time.time):
    now = clock()
    window_guard(window, lambda: now)
    require(window['end_epoch'] - now >= REQUEST_RESERVE, 'insufficient_request_budget')
    local = dt.datetime.fromtimestamp(now, dt.timezone(dt.timedelta(hours=8)))
    boundary = local.replace(hour=2, minute=0, second=0, microsecond=0)
    if boundary > local:
        require((boundary - local).total_seconds() >= REQUEST_RESERVE, 'SGT_blackout_request_budget')


class IO:
    def run(self, argv, timeout=30, env=None):
        return subprocess.run(argv, capture_output=True, text=True, timeout=timeout, env=env)

    def k(self, args, deadline=None):
        timeout = REQUEST_TIMEOUT
        if deadline is not None:
            timeout = min(timeout, deadline - time.time())
            require(timeout > 0, 'absolute_mutation_deadline_elapsed')
        return self.run([KUBECTL, '--context', CONTEXT, '-n', NAMESPACE,
                         f'--request-timeout={timeout:.3f}s'] + args, timeout=timeout)

    def get(self, args):
        result = self.k(['get'] + args + ['-o', 'json'])
        require(result.returncode == 0, 'production_read_failed')
        return json.loads(result.stdout)

    def policy(self, approval, context):
        return self.run([TEAM, 'policy-check', 'deploy', '--approval', approval, '--context', context],
                        env=dict(os.environ, TL_HOME='/Users/shining/code/work/tech-lead'))


def dispatch_once(io, approval, context, patch_path, journal, window, clock=time.time, expected_patch=None):
    require(not journal.exists(), 'dispatch_record_exists_no_replay')
    dispatch_budget(window, clock)
    patch_bytes = expected_patch if expected_patch is not None else read_bytes(patch_path)
    require(read_bytes(patch_path) == patch_bytes, 'patch_changed_since_dry_run_no_dispatch')
    patch_text = patch_bytes.decode('utf-8')
    json.loads(patch_text)
    # Durable before policy/dispatch. Any interruption here requires inspection, never retry.
    intent = {'state': 'ARMED_NO_RETRY', 'approval': approval, 'context': context,
              'patch_sha256': digest(patch_bytes), 'patch': json.loads(patch_text), 'at_epoch': clock()}
    write_json(journal, intent)
    try:
        result = io.policy(approval, context)
        require(result.returncode == 0 and result.stdout.strip() == 'ALLOWED once via ' + ROUTE,
                'policy_check_refused')
    except Exception:
        write_json(journal, {**intent, 'state': 'POLICY_FAILED_NO_DISPATCH'})
        raise
    # No cached receipt, shell, kubectl read, or other external command in this interval.
    # Recheck time in-process in case policy-check crossed the write window.
    require(read_bytes(patch_path) == patch_bytes, 'patch_changed_after_policy_no_dispatch')
    dispatch_budget(window, clock)
    now = clock()
    local = dt.datetime.fromtimestamp(now, dt.timezone(dt.timedelta(hours=8)))
    blackout = local.replace(hour=2, minute=0, second=0, microsecond=0).timestamp()
    deadline = min(now + REQUEST_TIMEOUT, window['end_epoch'] - 5,
                   blackout - 5 if blackout > now else float('inf'))
    intent['deadline_epoch'] = deadline
    try:
        # Use the captured bytes, never reopen a path in the mutation subprocess.
        result = io.k(['patch', 'deployment', DEPLOYMENT, '--type=json',
                       '--patch', patch_text, '-o', 'json'], deadline=deadline)
    except BaseException as error:
        write_json(journal, {**intent, 'state': 'UNKNOWN_NO_REPLAY',
                             'error_type': type(error).__name__})
        raise
    write_json(journal, {**intent, 'state': 'ACK' if result.returncode == 0 else 'UNKNOWN_NO_REPLAY', 'exit_code': result.returncode,
                         'stdout': result.stdout, 'stderr': result.stderr})
    require(result.returncode == 0, 'mutation_result_unknown_readback_only')


def execute(packet, packet_sha, review, approval, out, io=None):
    io = io or IO()
    require(hash_json(packet) == packet_sha, 'packet_digest_changed')
    require(review.get('verdict') == 'GO' and review.get('packet_sha256') == packet_sha
            and review.get('controller_sha256') == digest(Path(__file__).read_bytes()), 'exact_review_GO_required')
    require(packet.get('execution_enabled') is True, 'packet_is_preparation_only')
    baseline, planned, diff = validate_packet(packet)
    context = 'am-rev234/' + packet_sha
    out.mkdir(mode=0o700, parents=True, exist_ok=False)
    with writer_lock(LOCK):
        journal = LOCK.parent / 'am-successor-journal' / (packet_sha + '.json')
        require(not journal.exists(), 'dispatch_record_exists_no_replay')
        window_guard(packet['write_window'])
        current = io.get(['deployment', DEPLOYMENT])
        hpa = io.get(['hpa', DEPLOYMENT])
        namespace = io.get(['namespace', NAMESPACE])
        pods = io.get(['pods', '-l', 'app.kubernetes.io/name=' + DEPLOYMENT])
        endpoints = io.get(['endpointslices', '-l', 'kubernetes.io/service-name=' + DEPLOYMENT])
        validate_live(packet, baseline, current, hpa, namespace, pods, endpoints)
        write_json(out / 'template-diff.json', diff)
        patch_path = out / 'image-patch.json'
        patch = image_patch(current, packet['candidate']['image'])
        write_json(patch_path, patch)
        patch_bytes = canon(patch) + b'\n'
        dry = io.k(['patch', 'deployment', DEPLOYMENT, '--type=json', '--patch', patch_bytes.decode('utf-8'),
                    '--dry-run=server', '-o', 'json'])
        require(dry.returncode == 0, 'server_dry_run_failed')
        dry_object = json.loads(dry.stdout)
        require(dry_object['metadata']['uid'] == current['metadata']['uid']
                and dry_object['spec'] == planned['spec'], 'dry_run_has_non_image_change')
        write_json(out / 'dry-run.json', dry_object)
        # ResourceVersion CAS in the saved patch refuses changes after the snapshot.
        dispatch_once(io, approval, context, patch_path, journal, packet['write_window'],
                      expected_patch=patch_bytes)
        readback = io.get(['deployment', DEPLOYMENT])
        write_json(out / 'readback.json', readback)
        require(readback['metadata']['uid'] == baseline['metadata']['uid']
                and readback['spec'] == planned['spec'], 'readback_mismatch_no_replay')
        write_json(out / 'RESULT.json', {'status': 'IMAGE_PATCH_ACK_READBACK',
                   'rollout_health': 'PENDING_OBSERVER', 'context': context,
                   'production_mutations': 1, 'automatic_recovery': False})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--packet', type=Path, required=True)
    parser.add_argument('--execute', action='store_true')
    parser.add_argument('--packet-sha256')
    parser.add_argument('--review', type=Path)
    parser.add_argument('--approval')
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    packet = read_json(args.packet)
    if args.execute:
        require(args.review and args.approval and args.packet_sha256, 'review_approval_and_packet_hash_required')
        execute(packet, args.packet_sha256, read_json(args.review), args.approval, args.out)
    else:
        baseline, planned, diff = validate_packet(packet)
        args.out.mkdir(mode=0o700, parents=True, exist_ok=False)
        write_json(args.out / 'candidate-deployment.json', planned)
        write_json(args.out / 'template-diff.json', diff)
        write_json(args.out / 'RESULT.json', {'status': 'OFFLINE_PREFLIGHT_PASS',
                   'packet_sha256': hash_json(packet), 'production_calls': 0})
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (Refused, OSError, ValueError, KeyError, TypeError) as exc:
        print('BLOCKED: ' + str(exc), file=sys.stderr)
        sys.exit(2)
