#!/usr/bin/env python3
"""Render reviewable operator inputs. Never contacts a cluster or Google API."""
import argparse
import copy
import json
from pathlib import Path
import re

from policy import (ANNOTATION, CLASS, COHORT, DRIVER, LABEL, digest, policy_body,
                    policy_ref, require, timestamp)

NAME = 'cma-home-snapshot-attach'
FILES = ('policy.py', 'reconcile.py')


def render(source_class, config, image, service_account):
    require(re.fullmatch(r'[a-z][a-z0-9-]{4,61}[a-z0-9]', config['project']), 'invalid_project')
    for value in (config['namespace'], config['region'], config['policy']):
        require(re.fullmatch(r'[a-z][a-z0-9-]{0,61}[a-z0-9]', value), 'invalid_config_name')
    require(re.fullmatch(r'[a-z0-9][a-z0-9./_-]+@sha256:[a-f0-9]{64}', image), 'immutable_image_required')
    require(re.fullmatch(r'[a-z][a-z0-9-]+@' + re.escape(config['project']) + r'\.iam\.gserviceaccount\.com', service_account), 'gsa_project_mismatch')
    require(source_class['metadata']['name'] == 'home-a', 'source_class_mismatch')
    require(source_class['provisioner'] == DRIVER, 'source_driver_mismatch')
    require(source_class['parameters'].get('type') == 'hyperdisk-balanced', 'source_type_mismatch')
    require('labels' not in source_class['parameters'], 'source_labels_require_review')
    require(source_class['parameters'].get('disk-encryption-kms-key') == config['kms_key'], 'source_cmek_mismatch')
    require(source_class['volumeBindingMode'] == 'WaitForFirstConsumer', 'source_binding_mode_mismatch')
    require(source_class['reclaimPolicy'] == 'Delete' and source_class['allowVolumeExpansion'], 'source_lifecycle_mismatch')
    timestamp(config['activation_time'])
    sc = {key: copy.deepcopy(source_class[key]) for key in
          ('apiVersion', 'kind', 'provisioner', 'parameters', 'allowVolumeExpansion', 'reclaimPolicy', 'volumeBindingMode')}
    for key in ('mountOptions', 'allowedTopologies'):
        if key in source_class:
            sc[key] = copy.deepcopy(source_class[key])
    sc['metadata'] = {'name': CLASS, 'annotations': {ANNOTATION: policy_ref(config)}}
    sc['parameters']['labels'] = f'{LABEL}={COHORT}'
    data = {name: (Path(__file__).parent / name).read_text() for name in FILES}
    data['config.json'] = json.dumps(config, sort_keys=True)
    code_name = NAME + '-' + digest(data)[:12]
    ns = config['namespace']
    sa = {'apiVersion': 'v1', 'kind': 'ServiceAccount', 'metadata': {
        'name': NAME, 'namespace': ns, 'annotations': {'iam.gke.io/gcp-service-account': service_account}}}
    role = {'apiVersion': 'rbac.authorization.k8s.io/v1', 'kind': 'Role',
            'metadata': {'name': NAME, 'namespace': ns},
            'rules': [{'apiGroups': [''], 'resources': ['persistentvolumeclaims'], 'verbs': ['get', 'list']}]}
    cluster_role = {'apiVersion': 'rbac.authorization.k8s.io/v1', 'kind': 'ClusterRole',
                    'metadata': {'name': NAME}, 'rules': [
                        {'apiGroups': [''], 'resources': ['persistentvolumes'], 'verbs': ['get']},
                        {'apiGroups': ['storage.k8s.io'], 'resources': ['storageclasses'],
                         'resourceNames': [CLASS], 'verbs': ['get']}]}
    subjects = [{'kind': 'ServiceAccount', 'name': NAME, 'namespace': ns}]
    bindings = [{'apiVersion': 'rbac.authorization.k8s.io/v1', 'kind': kind + 'Binding',
                 'metadata': {'name': NAME, **({'namespace': ns} if kind == 'Role' else {})},
                 'roleRef': {'apiGroup': 'rbac.authorization.k8s.io', 'kind': kind, 'name': NAME},
                 'subjects': subjects} for kind in ('Role', 'ClusterRole')]
    code = {'apiVersion': 'v1', 'kind': 'ConfigMap', 'immutable': True,
            'metadata': {'name': code_name, 'namespace': ns}, 'data': data}
    container = {'name': 'attach', 'image': image, 'imagePullPolicy': 'IfNotPresent',
                 'command': ['python3', '/app/reconcile.py'], 'args': ['--config', '/app/config.json'],
                 'env': [{'name': 'PYTHONDONTWRITEBYTECODE', 'value': '1'}],
                 'securityContext': {'runAsNonRoot': True, 'runAsUser': 65532,
                                     'allowPrivilegeEscalation': False, 'readOnlyRootFilesystem': True,
                                     'capabilities': {'drop': ['ALL']}},
                 'resources': {'requests': {'cpu': '25m', 'memory': '32Mi'},
                               'limits': {'cpu': '100m', 'memory': '96Mi'}},
                 'volumeMounts': [{'name': 'code', 'mountPath': '/app', 'readOnly': True}]}
    cron = {'apiVersion': 'batch/v1', 'kind': 'CronJob', 'metadata': {'name': NAME, 'namespace': ns},
            'spec': {'schedule': '* * * * *', 'suspend': True, 'concurrencyPolicy': 'Forbid',
                     'startingDeadlineSeconds': 60, 'successfulJobsHistoryLimit': 1, 'failedJobsHistoryLimit': 3,
                     'jobTemplate': {'spec': {'activeDeadlineSeconds': 270, 'backoffLimit': 0,
                        'template': {'metadata': {'labels': {'app': NAME}}, 'spec': {
                            'serviceAccountName': NAME, 'restartPolicy': 'Never',
                            'securityContext': {'seccompProfile': {'type': 'RuntimeDefault'}},
                            'nodeSelector': {'iam.gke.io/gke-metadata-server-enabled': 'true'},
                            'containers': [container], 'volumes': [{'name': 'code', 'configMap': {'name': code_name}}]}}}}}}
    iam = {'title': 'CMA new home snapshot attachment', 'stage': 'GA', 'includedPermissions': [
        'compute.disks.get', 'compute.disks.addResourcePolicies', 'compute.resourcePolicies.get',
        'compute.resourcePolicies.use', 'compute.zoneOperations.get', 'compute.regionOperations.get']}
    default_key = '/data/CHRONO_AGENT_SANDBOX_HOME_STORAGE_CLASS'
    return {
        'resources.json': {'apiVersion': 'v1', 'kind': 'List', 'items': [sc, sa, role, cluster_role, *bindings, code, cron]},
        'resource-policy.json': policy_body(config), 'iam-role.json': iam,
        'new-home-default.patch.json': [{'op': 'test', 'path': default_key, 'value': 'home-a'},
                                       {'op': 'replace', 'path': default_key, 'value': CLASS}],
        'rollback-default.patch.json': [{'op': 'test', 'path': default_key, 'value': CLASS},
                                       {'op': 'replace', 'path': default_key, 'value': 'home-a'}],
        'enable-attach.patch.json': [
            {'op': 'test', 'path': '/spec/suspend', 'value': True},
            {'op': 'test', 'path': '/spec/jobTemplate/spec/template/spec/containers/0/image', 'value': image},
            {'op': 'test', 'path': '/spec/jobTemplate/spec/template/spec/volumes/0/configMap/name', 'value': code_name},
            {'op': 'replace', 'path': '/spec/jobTemplate/spec/template/spec/containers/0/args',
             'value': ['--config', '/app/config.json', '--apply']},
            {'op': 'replace', 'path': '/spec/suspend', 'value': False}],
        'rollback-job.patch.json': [{'op': 'replace', 'path': '/spec/suspend', 'value': True}],
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--source-class', type=Path, required=True)
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--image', required=True)
    parser.add_argument('--google-service-account', required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    files = render(json.loads(args.source_class.read_text()), json.loads(args.config.read_text()),
                   args.image, args.google_service_account)
    args.out.mkdir(mode=0o700, parents=True, exist_ok=False)
    for name, data in files.items():
        path = args.out / name
        path.write_text(json.dumps(data, indent=2) + '\n')
        path.chmod(0o600)
        print(name, digest(data))


if __name__ == '__main__':
    main()
