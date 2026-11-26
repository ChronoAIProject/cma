"""Opt-in coordinated AM boots; records are written only by the application."""
import secrets

from migration_kind_manifests import NAMESPACE
from migration_membership_documents import infrastructure


def manifests(objects):
    selected = {(obj['kind'], obj['metadata']['name']): obj for obj in objects}
    env = selected[('ConfigMap', 'api-env')]['data']
    if 'CHRONO_AGENT_MANAGER_REPLICAS_ENABLED' in env:
        raise ValueError('membership overlay requires an unconfigured base')
    env.update(CHRONO_AGENT_MANAGER_REPLICAS_ENABLED='true',
               CHRONO_AGENT_CONTROL_WRITE_COORDINATION_ENABLED='true',
               CHRONO_AGENT_MANAGER_MAX_OWNED_AGENTS='4')
    pod = selected[('Deployment', 'api')]['spec']['template']['spec']
    container, = pod['containers']
    if container['name'] != 'api' or container.get('env'):
        raise ValueError('membership overlay requires the api container without overrides')
    pod['automountServiceAccountToken'] = True
    container['env'] = [
        {'name': name, 'valueFrom': {'fieldRef': {'fieldPath': field}}}
        for name, field in (
            ('MATRIX_POD_IP', 'status.podIP'),
            ('CHRONO_AGENT_MANAGER_POD_NAME', 'metadata.name'),
            ('CHRONO_AGENT_MANAGER_POD_NAMESPACE', 'metadata.namespace'),
            ('CHRONO_AGENT_MANAGER_POD_UID', 'metadata.uid'))]
    # Ordered environment expansion binds the relay to this exact Pod, including
    # while a prior process incarnation remains in the retained membership set.
    container['env'].append({'name': 'CHRONO_AGENT_MANAGER_RELAY_ADDRESS',
                             'value': 'http://$(MATRIX_POD_IP):8080'})
    container['startupProbe'] = {'httpGet': {'path': '/livez', 'port': 8080},
                                 'periodSeconds': 2, 'failureThreshold': 90}
    # No native Agent is admitted by this fixture. Read-only discovery and own-
    # Pod inspection suffice; do not let a fixture accidentally provision one.
    role = selected[('Role', 'cma-runtime-sandbox-reclaimer')]
    for rule in role['rules']:
        rule['verbs'] = ['get', 'list']
    return infrastructure(objects, NAMESPACE, secrets.token_urlsafe(24))
