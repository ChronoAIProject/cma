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

CONTEXT = 'gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke'
NAMESPACE = 'chrono-bot'
DEPLOYMENT = 'cma-agent-manager'
REPOSITORY = 'us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/cma-api'
LOCK = Path('/Users/shining/code/work/.tl-worktrees/cma-cma-release/api/target/m4-20261115/u2-controller.lock')
TEAM = '/Users/shining/.claude/skills/shining-tech-lead/scripts/team.sh'
KUBECTL = '/opt/homebrew/share/google-cloud-sdk/bin/kubectl'
ROUTE = 'reviewed-release-controller'
IDENTITY = {'CHRONO_AGENT_MANAGER_POD_NAME': 'metadata.name',
            'CHRONO_AGENT_MANAGER_POD_NAMESPACE': 'metadata.namespace',
            'CHRONO_AGENT_MANAGER_POD_UID': 'metadata.uid'}
REHEARSAL_CHECKS = ('two_replicas', 'image_roll', 'create_reply', 'drain_wake', 'hpa_2_3_2')


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


def read_json(path):
    # Real files only: no /dev/fd, FIFO, symlink, or shell process substitution.
    p = Path(path)
    require(not p.is_symlink() and stat.S_ISREG(p.stat().st_mode), 'regular_JSON_file_required')
    require(not str(p.resolve()).startswith('/dev/'), 'regular_JSON_file_required')
    return json.loads(p.read_bytes())


def bound(ref):
    require(isinstance(ref, dict) and set(ref) == {'path', 'sha256'}, 'unbound_evidence')
    value = read_json(ref['path'])
    require(digest(Path(ref['path']).read_bytes()) == ref['sha256'], 'evidence_hash_changed')
    return value


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


def template_gate(production, rehearsal, exceptions):
    required_template(rehearsal)
    rows = differences(production['spec']['template'], rehearsal['spec']['template'])
    require(isinstance(exceptions, list), 'exceptions_must_be_list')
    require(len({r['path'] for r in exceptions}) == len(exceptions), 'duplicate_exception')
    expected = []
    for rule in exceptions:
        require(set(rule) == {'path', 'production_present', 'rehearsal_present',
                              'production', 'rehearsal', 'reason'}, 'exact_exception_required')
        require(exception_path(rule['path']) and bool(rule['reason'].strip()),
                'behavior_difference_cannot_be_excepted:' + rule['path'])
        expected.append({k: v for k, v in rule.items() if k != 'reason'})
    require(sorted(rows, key=lambda r: r['path']) == sorted(expected, key=lambda r: r['path']),
            'rehearsal_production_template_diff_not_exactly_reviewed')
    return rows


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
    rows = template_gate(planned, rehearsal, packet['rehearsal']['exceptions'])
    require(receipt.get('status') == 'PASS' and receipt.get('source') == candidate['source']
            and receipt.get('image') == image
            and receipt.get('template_sha256') == hash_json(rehearsal['spec']['template'])
            and all(receipt.get('checks', {}).get(c) == 'PASS' for c in REHEARSAL_CHECKS),
            'exact_image_runtime_rehearsal_not_passed')
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


class IO:
    def run(self, argv, timeout=30, env=None):
        return subprocess.run(argv, capture_output=True, text=True, timeout=timeout, env=env)

    def k(self, args):
        return self.run([KUBECTL, '--context', CONTEXT, '-n', NAMESPACE, '--request-timeout=20s'] + args)

    def get(self, args):
        result = self.k(['get'] + args + ['-o', 'json'])
        require(result.returncode == 0, 'production_read_failed')
        return json.loads(result.stdout)

    def policy(self, approval, context):
        return self.run([TEAM, 'policy-check', 'deploy', '--approval', approval, '--context', context],
                        env=dict(os.environ, TL_HOME='/Users/shining/code/work/tech-lead'))


def dispatch_once(io, approval, context, patch_path, journal, window, clock=time.time):
    require(not journal.exists(), 'dispatch_record_exists_no_replay')
    window_guard(window, clock)
    # Durable before policy/dispatch. Any interruption here requires inspection, never retry.
    write_json(journal, {'state': 'ARMED_NO_RETRY', 'approval': approval, 'context': context,
                         'patch_sha256': digest(patch_path.read_bytes()), 'at_epoch': clock()})
    try:
        result = io.policy(approval, context)
        require(result.returncode == 0 and result.stdout.strip() == 'ALLOWED once via ' + ROUTE,
                'policy_check_refused')
    except Exception:
        write_json(journal, {'state': 'POLICY_FAILED_NO_DISPATCH', 'context': context})
        raise
    # No cached receipt, shell, kubectl read, or other external command in this interval.
    # Recheck time in-process in case policy-check crossed the write window.
    window_guard(window, clock)
    try:
        result = io.k(['patch', 'deployment', DEPLOYMENT, '--type=json',
                       '--patch-file', str(patch_path), '-o', 'json'])
    except BaseException as error:
        write_json(journal, {'state': 'UNKNOWN_NO_REPLAY', 'context': context,
                             'error_type': type(error).__name__})
        raise
    write_json(journal, {'state': 'ACK' if result.returncode == 0 else 'UNKNOWN_NO_REPLAY',
                         'context': context, 'exit_code': result.returncode,
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
        write_json(patch_path, image_patch(current, packet['candidate']['image']))
        dry = io.k(['patch', 'deployment', DEPLOYMENT, '--type=json', '--patch-file', str(patch_path),
                    '--dry-run=server', '-o', 'json'])
        require(dry.returncode == 0, 'server_dry_run_failed')
        dry_object = json.loads(dry.stdout)
        require(dry_object['metadata']['uid'] == current['metadata']['uid']
                and dry_object['spec'] == planned['spec'], 'dry_run_has_non_image_change')
        write_json(out / 'dry-run.json', dry_object)
        # ResourceVersion CAS in the saved patch refuses changes after the snapshot.
        dispatch_once(io, approval, context, patch_path, journal, packet['write_window'])
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
