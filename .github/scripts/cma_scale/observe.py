#!/usr/bin/env python3
"""One bounded metadata observation or local outage calculation; never a watcher."""
import argparse
import datetime as dt
import json
from pathlib import Path
import subprocess
import sys

import checkpoints
import operations
import rules as g

NAMES = {'am': 'cma-agent-manager', 'eg': 'cma-event-gateway'}


def instant(text):
    value = dt.datetime.fromisoformat(text.replace('Z', '+00:00'))
    g.require(value.tzinfo is not None, 'explicit_timezone_required')
    return value


def outage(withdrawn, restored=None, now=None):
    start = instant(withdrawn)
    end = instant(restored) if restored else now or dt.datetime.now(dt.timezone.utc)
    elapsed = (end - start).total_seconds()
    g.require(0 <= elapsed and end <= dt.datetime.now(dt.timezone.utc), 'invalid_observed_interval')
    state = 'BREACHED' if elapsed >= 120 else 'RESTORED' if restored else 'OPEN'
    return {'withdrawn_utc': start.isoformat(), 'restored_utc': restored,
            'elapsed_seconds': elapsed, 'remaining_seconds': max(0, 120 - elapsed),
            'state': state, 'service_evidence_required': True}


def check(role, count, image, paused=False):
    obj = g.get('Deployment', NAMES[role])
    g.require(obj is not None, 'deployment_missing')
    target = next(c for c in obj['spec']['template']['spec']['containers']
                  if c['name'] == g.TARGETS[NAMES[role]])
    g.require(target['image'] == image, 'selected_image_mismatch')
    if count == 0:
        g.require(obj['spec'].get('replicas') == 0, 'desired_count_not_zero')
        sets, pods = checkpoints.cohort(obj)
        g.require(not pods and all(r['spec'].get('replicas', 0) == 0 and
                                  r.get('status', {}).get('replicas', 0) == 0
                                  for r in sets.values()), 'old_or_terminating_process_exists')
    else:
        checkpoints.stable(obj, count, current_template=not paused, distinct=count == 2)
    return {'service': role, 'observed_at': g.now(), 'count': count,
            'kubernetes_checkpoint': 'PASS', 'service_and_socket_authority': 'NOT_ASSESSED'}


def snapshot(release, stage, run_id, start, history=None):
    instant(start)
    observed = g.now()
    rows = []
    deployments = {}
    for role, name in NAMES.items():
        obj = g.get('Deployment', name)
        g.require(obj is not None, 'deployment_missing')
        _, pods = checkpoints.cohort(obj)
        deployments[role] = {**g.binding(obj), 'desired': obj['spec'].get('replicas'),
                             'paused': obj['spec'].get('paused', False),
                             'observed_generation': obj.get('status', {}).get('observedGeneration')}
        for pod in pods:
            states = [s for s in pod.get('status', {}).get('containerStatuses', [])
                      if s['name'] == g.TARGETS[name]]
            g.require(len(states) == 1, 'pending_container_identity_keep_previous_packet')
            status = states[0]
            digest = status.get('imageID', '').split('@')[-1]
            g.require(digest.startswith('sha256:') and len(digest) == 71,
                      'runtime_digest_unknown_keep_previous_packet')
            deleting = bool(pod['metadata'].get('deletionTimestamp'))
            rows.append({'service': role, 'name': pod['metadata']['name'],
                         'uid': pod['metadata']['uid'], 'restart_count': status['restartCount'],
                         'image_digest': digest,
                         'cohort': 'candidate' if release['images'][role].endswith(digest) else 'outgoing',
                         'ready': checkpoints.ready(pod), 'deleting': deleting,
                         'lifecycle': 'terminating' if deleting else 'active'})
    if history:
        previous = g.load(history)
        g.require(previous['context'] == g.CONTEXT and previous['namespace'] == g.NAMESPACE,
                  'history_wrong_cluster')
        identities = {(r['uid'], r['restart_count']) for r in rows}
        for row in previous['pods']:
            g.require(row.get('lifecycle') != 'retired' or
                      (row['uid'], row['restart_count']) not in identities,
                      'retired_source_observed_again')
            if (row['uid'], row['restart_count']) not in identities:
                old = dict(row)
                if old.get('lifecycle') != 'retired':
                    old.update(lifecycle='retired', observed_until_utc=observed,
                               ready=False, deleting=False)
                rows.append(old)
    return {'run_id': run_id, 'stage': stage, 'context': g.CONTEXT,
            'namespace': g.NAMESPACE, 'source_sha': release['source'],
            'start_utc': start, 'end_utc': observed,
            'images': {k: v.split('@')[-1] for k, v in release['images'].items()},
            'pods': rows, 'deployments': deployments,
            'retirement_semantics': 'first observed absent/restarted upper bound; not socket expiry'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    p = sub.add_parser('check')
    p.add_argument('--service', choices=NAMES, required=True)
    p.add_argument('--count', type=int, choices=range(6), required=True)
    p.add_argument('--image', required=True)
    p.add_argument('--paused-template', action='store_true')
    p = sub.add_parser('snapshot')
    p.add_argument('--release', required=True)
    p.add_argument('--stage', choices=('BATCH9', 'S1', 'S2', 'S3'), required=True)
    p.add_argument('--run-id', required=True)
    p.add_argument('--start', required=True)
    p.add_argument('--history')
    p.add_argument('--out', required=True)
    p = sub.add_parser('outage')
    p.add_argument('--withdrawn', required=True)
    p.add_argument('--restored')
    args = parser.parse_args()
    if args.command == 'snapshot':
        g.require(not Path(args.out).exists(), 'snapshot_exists')
        value = snapshot(operations.release(args.release), args.stage, args.run_id,
                         args.start, args.history)
        with open(args.out, 'x') as stream:
            json.dump(value, stream, indent=2)
            stream.write('\n')
        print(json.dumps({'out': args.out, 'pods': len(value['pods']), 'at': value['end_utc']}))
    elif args.command == 'check':
        print(json.dumps(check(args.service, args.count,
                               operations.image(args.image, args.service), args.paused_template)))
    else:
        value = outage(args.withdrawn, args.restored)
        print(json.dumps(value))
        if value['state'] == 'BREACHED':
            raise SystemExit(2)


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, ValueError, KeyError, OSError, subprocess.SubprocessError) as error:
        print('HOLD: ' + (str(error) if isinstance(error, RuntimeError)
                          else type(error).__name__), file=sys.stderr)
        raise SystemExit(1)
