#!/usr/bin/env python3
"""One dry-run/approval/CAS operation at a time; never an automatic stage rollout."""
import argparse
import datetime as dt
import json
import os
from pathlib import Path
import subprocess
import sys
from zoneinfo import ZoneInfo

import operations as ops
import rules as g
import checkpoints

ROUTE = 'reviewed-release-controller'


def write_window(instant=None):
    instant = instant or dt.datetime.now(ZoneInfo('Asia/Singapore'))
    local = instant.astimezone(ZoneInfo('Asia/Singapore'))
    g.require(local.hour != 2, 'SGT_write_blackout')
    boundary = local.replace(hour=2, minute=0, second=0, microsecond=0)
    if local < boundary:
        g.require((boundary - local).total_seconds() > 120, 'SGT_blackout_margin')


def policy(approval=None, context=None):
    env = dict(os.environ, TL_HOME='/Users/shining/code/work/tech-lead')
    args = [g.TEAM, 'policy', 'show', 'ChronoAIProject/cma']
    if approval is not None:
        args = [g.TEAM, 'policy-check', 'deploy', '--approval', approval, '--context', context]
    result = subprocess.run(args, env=env, capture_output=True, timeout=30)
    g.require(result.returncode == 0, 'deployment_policy_refused')
    if approval is not None:
        g.require(result.stdout.decode().strip() == 'ALLOWED once via ' + ROUTE,
                  'approval_did_not_select_reviewed_controller')
    else:
        value = json.loads(result.stdout)
        g.require(value.get('DEPLOYMENT_ROUTE') == ROUTE and
                  value.get('PRODUCTION_DEPLOY') == 'allow' and value.get('SKIP_CI') == 'allow',
                  'current_policy_does_not_allow_controller')


def controller_code():
    return g.hash_bytes(b''.join(Path(__file__).with_name(name).read_bytes()
                                for name in ('controller.py', 'operations.py', 'rules.py',
                                             'checkpoints.py')))


def context_for(plan):
    content = {k: v for k, v in plan.items() if k != 'approval_context'}
    return (f'cma-scale/{plan["release"]["source"]}/{plan["operation"]["stage"]}/'
            f'{plan["operation"]["id"]}/{g.hash_json(content)[:16]}')


def stable_pair(name):
    obj = g.get('Deployment', name)
    g.require(obj is not None, 'stable_pair_required')
    checkpoints.stable(obj, 2, distinct=True)


def checkpoint(obj, op, release=None):
    if op['kind'] == 'Deployment':
        for name in g.TARGETS:
            g.require(g.get('HorizontalPodAutoscaler', name) is None,
                      'remove_both_HPAs_before_Deployment_change')
        g.deployment_checkpoint(obj, op)
        phase = op.get('a2_phase')
        next_count = op.get('changes', {}).get('deployment', {}).get('replicas')
        if phase in ('pause-legacy', 'prepare-template', 'stop-legacy') or op.get('abort_before_stop'):
            checkpoints.stable(obj, 1, current_template=False)
        elif next_count is not None and 0 < next_count < obj['spec'].get('replicas', 1):
            checkpoints.stable(obj, obj['spec']['replicas'])
            if release is not None:
                role = 'am' if op['name'] == 'cma-agent-manager' else 'eg'
                capacity = release.get('survivor_capacity', {}).get(role, {})
                timestamp = capacity.get('observed_at')
                g.require(isinstance(timestamp, str), 'fresh_survivor_capacity_required')
                observed = dt.datetime.fromisoformat(timestamp.replace('Z', '+00:00'))
                g.require(observed.tzinfo is not None, 'survivor_capacity_timezone_required')
                age = (dt.datetime.now(dt.timezone.utc) - observed).total_seconds()
                minimum = capacity.get('minimum_safe_replicas')
                g.require(0 <= age <= 120 and capacity.get('qualified') is True
                          and type(minimum) is int and 1 <= minimum <= next_count,
                          'fresh_survivor_capacity_required')
        if op.get('requires_zero') or phase == 'unpause-zero' or op.get('a2_cutover_start'):
            _, pods = checkpoints.cohort(obj)
            g.require(not pods, 'owned_or_selected_process_still_exists')
        if op.get('abort_before_stop'):
            selector = 'app.kubernetes.io/name=' + op['name']
            pods = g.command(g.K + ['get', 'pods', '-l', selector, '-o', 'json'])['items']
            g.require(len(pods) == 1 and not pods[0]['metadata'].get('deletionTimestamp') and
                      any(x.get('uid') == op['original_rs_uid'] and x.get('controller')
                          for x in pods[0]['metadata'].get('ownerReferences', [])),
                      'abort_original_process_changed')
    elif op['action'] == 'create':
        stable_pair(op['name'])
        if op['kind'] == 'HorizontalPodAutoscaler':
            for name in g.TARGETS:
                if name != op['name']:
                    stable_pair(name)
    if release is not None and (op['kind'] == 'Deployment' or op['action'] == 'create') \
            and not op.get('abort_before_stop'):
        target = obj if op['kind'] == 'Deployment' else g.get('Deployment', op['name'])
        role = 'am' if op['name'] == 'cma-agent-manager' else 'eg'
        configured = next(c['image'] for c in target['spec']['template']['spec']['containers']
                          if c['name'] == g.TARGETS[op['name']])
        permitted = {release['images'][role]}
        if op.get('emergency_quiesce') or op.get('requires_zero') or 'rollback' in op['id'] \
                or op.get('a2_phase') == 'unpause-zero':
            permitted.add(ops.recovery_image(release, role))
        g.require(configured in permitted, 'live_image_outside_selected_release')


def payload(current, op, dry_run=False):
    if op['action'] == 'create':
        g.require(current is None, 'create_conflict')
        manifest = op['manifest']
        g.require(manifest['apiVersion'] == g.KINDS[op['kind']][1] and
                  manifest['kind'] == op['kind'] and
                  manifest['metadata']['name'] == op['name'] and
                  manifest['metadata']['namespace'] == g.NAMESPACE, 'manifest_identity')
        g.transform({'spec': {}}, {**op, 'changes': {'spec': manifest['spec']}})
        cmd = g.K + ['create', '--validate=strict', '-f', '-', '-o', 'json']
        if dry_run:
            cmd += ['--dry-run=server']
        return cmd, g.canon(manifest), manifest['spec'], []
    g.require(current is not None, 'object_missing')
    if op['action'] == 'delete':
        g.require(all(current['spec'].get('behavior', {}).get(d, {}).get('selectPolicy')
                      == 'Disabled' for d in ('scaleUp', 'scaleDown')),
                  'freeze_both_HPA_directions_first')
        cmd, body = g.delete_request(current, op, dry_run)
        return cmd, body, None, []
    expected, old = ops.transformed(current, op)
    cmd = g.K + ['patch', g.KINDS[op['kind']][0], op['name'], '--type=json',
                 '--patch-file=/dev/stdin', '-o', 'json']
    if dry_run:
        cmd += ['--dry-run=server']
    return cmd, g.canon(g.patch_for(current, expected)), expected['spec'], old


def dry_run(current, op):
    cmd, body, expected, old = payload(current, op, True)
    observed = g.command(cmd, body)
    if op['action'] != 'delete':
        g.require(observed['spec'] == expected, 'server_default_or_admission_change')
    return expected, old


def plan(args):
    g.require(not Path(args.out).exists(), 'plan_exists')
    release = ops.release(args.release)
    op = ops.build(args.action, release, args.created, args.pause_plan, args.prepare_plan)
    g.validate_operation(op)
    current = g.get(op['kind'], op['name'])
    if op.get('expected_before'):
        g.same(current, op['expected_before'])
    checkpoint(current, op, release)
    expected, old = dry_run(current, op)
    result = dict(version=1, at=g.now(), route=ROUTE, action=args.action, release=release,
                  operation=op, controller_code=controller_code(),
                  before=g.binding(current) if current else None,
                  before_replicas=current['spec'].get('replicas') if current else None,
                  expected_spec_sha256=g.hash_json(expected) if expected is not None else None,
                  previous_fields=old, server_dry_run='PASS')
    result['approval_context'] = context_for(result)
    with open(args.out, 'x') as stream:
        json.dump(result, stream, indent=2)
        stream.write('\n')
    print(json.dumps({'plan': args.out, 'action': args.action,
                      'source': release['source'], 'dry_run': 'PASS',
                      'approval_context': result['approval_context']}))


def check_same(current, plan):
    if plan['before'] is None:
        g.require(current is None, 'object_created_after_plan')
    else:
        g.same(current, plan['before'])


def apply(plan_path, approval):
    value = g.load(plan_path)
    op = value['operation']
    g.require(value.get('version') == 1 and value.get('route') == ROUTE and
              value.get('controller_code') == controller_code(), 'wrong_controller_or_stale_plan')
    g.require(value.get('approval_context') == context_for(value), 'plan_changed_after_approval')
    g.require(isinstance(approval, str) and approval.strip(), 'operation_approval_required')
    g.validate_operation(op)
    output = Path(plan_path).with_suffix('.result.json')
    g.require(not output.exists(), 'previous_attempt_reconcile_before_retry')
    current = g.get(op['kind'], op['name'])
    check_same(current, value)
    checkpoint(current, op, value['release'])
    expected, _ = dry_run(current, op)
    g.require((g.hash_json(expected) if expected is not None else None)
              == value['expected_spec_sha256'], 'dry_run_changed_after_plan')
    write_window()
    policy()
    policy(approval, value['approval_context'])
    # Status, HPA ownership and CAS state are rechecked after the approval call.
    fresh = g.get(op['kind'], op['name'])
    check_same(fresh, value)
    checkpoint(fresh, op, value['release'])
    cmd, body, expected, _ = payload(fresh, op)
    policy()
    write_window()
    result = dict(at=g.now(), state='WRITE_ATTEMPTED', source=value['release']['source'],
                  operation=op, before=value['before'], approval_context=value['approval_context'])
    with open(output, 'x') as stream:
        json.dump(result, stream, indent=2)
        stream.flush()
        os.fsync(stream.fileno())
    try:
        write_window()
        response = g.command(cmd, body)
        after = g.get(op['kind'], op['name'])
        g.require(after is None if op['action'] == 'delete' else
                  after is not None and after['spec'] == expected and
                  after['metadata']['uid'] == (fresh['metadata']['uid'] if fresh else
                                                response['metadata']['uid']),
                  'write_result_uncertain_reconcile_no_retry')
        result.update(state='APPLIED', completed_at=g.now(), after=g.binding(after) if after else None)
    except Exception as error:
        result.update(state='UNKNOWN', error=type(error).__name__, completed_at=g.now())
        g.save(output, result)
        raise
    g.save(output, result)
    print(json.dumps({'result': str(output), 'state': 'APPLIED', 'operation': op['id']}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    p = sub.add_parser('plan')
    p.add_argument('--action', required=True)
    p.add_argument('--release', required=True)
    p.add_argument('--out', required=True)
    p.add_argument('--created')
    p.add_argument('--pause-plan')
    p.add_argument('--prepare-plan')
    p = sub.add_parser('apply')
    p.add_argument('--plan', required=True)
    p.add_argument('--approval', required=True)
    args = parser.parse_args()
    if args.command == 'plan':
        plan(args)
    else:
        apply(args.plan, args.approval)


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, ValueError, KeyError, OSError, subprocess.SubprocessError) as error:
        # No raw Kubernetes stderr, full specs, environment values or Secret data.
        print('HOLD: ' + (str(error) if isinstance(error, RuntimeError)
                          else type(error).__name__), file=sys.stderr)
        raise SystemExit(1)
