"""Verify all API pod attempts between a pre-boot list and readiness.

The watch starts at the list's opaque resourceVersion after rollout finishes.
Kubernetes replays retained changes, including pods already deleted. Expired
history or incomplete observations fail the campaign instead of implying a
clean boot from the last surviving pod alone.
"""
import json
from urllib.parse import urlencode

from migration_kind_pods import PodSelectionError, matches_image, matches_identity


class BootObservationError(PodSelectionError):
    pass


FAILED_WAITING = {'CrashLoopBackOff', 'ImagePullBackOff', 'ErrImagePull',
                  'CreateContainerConfigError', 'CreateContainerError', 'RunContainerError'}


def observe_boot(kubectl, namespace, before, selected, image, config):
    version = before.get('metadata', {}).get('resourceVersion')
    items = before.get('items')
    if not isinstance(version, str) or not version or not isinstance(items, list):
        raise BootObservationError('pre-boot pod list lacks a resourceVersion or items')
    existing = {row.get('metadata', {}).get('uid') for row in items}
    if None in existing or selected['uid'] in existing:
        raise BootObservationError('boot did not select a new pod identity')
    query = urlencode({'watch': 'true', 'labelSelector': 'app=api',
                       'resourceVersion': version, 'timeoutSeconds': '1',
                       'allowWatchBookmarks': 'true'})
    raw = kubectl(['--request-timeout=15s', 'get', '--raw',
                   f'/api/v1/namespaces/{namespace}/pods?{query}'], timeout=20)
    return evaluate(raw, existing, selected, image, config, version)


def evaluate(raw, existing, selected, image, config, version):
    if not isinstance(raw, str) or len(raw.encode('utf-8')) > 8 * 1024 * 1024:
        raise BootObservationError('startup watch output missing or excessive')
    lines = [line for line in raw.splitlines() if line.strip()]
    if not 1 <= len(lines) <= 2048:
        raise BootObservationError('startup watch event count missing or excessive')
    added, ready, attempted, observations = set(), set(), set(), []
    for line in lines:
        try:
            event = json.loads(line)
        except json.JSONDecodeError as error:
            raise BootObservationError('startup watch contains incomplete JSON') from error
        if not isinstance(event, dict) or not isinstance(event.get('object'), dict):
            raise BootObservationError('startup watch event contract mismatch')
        kind, pod = event.get('type'), event['object']
        if kind == 'ERROR':
            raise BootObservationError('startup watch history unavailable; no clean-boot proof')
        if kind == 'BOOKMARK':
            if not pod.get('metadata', {}).get('resourceVersion'):
                raise BootObservationError('startup watch bookmark lacks a resourceVersion')
            continue
        if kind not in ('ADDED', 'MODIFIED', 'DELETED'):
            raise BootObservationError('startup watch returned an unknown event')
        metadata, status = pod.get('metadata', {}), pod.get('status', {})
        uid, name = metadata.get('uid'), metadata.get('name')
        if not uid or not name or not metadata.get('resourceVersion'):
            raise BootObservationError('startup watch pod identity incomplete')
        if uid in existing:
            continue  # The previous serving pod is deliberately retired by Recreate.
        containers = pod.get('spec', {}).get('containers', [])
        if (len(containers) != 1 or containers[0].get('name') != 'api'
                or not matches_image(containers[0].get('image', ''), image)):
            raise BootObservationError('startup watch saw an unexpected API image')
        if kind == 'ADDED':
            added.add(uid)
        elif uid not in added:
            raise BootObservationError('startup watch missed pod creation')
        attempted.add(uid)
        statuses = status.get('containerStatuses', [])
        if len(statuses) > 1 or (statuses and statuses[0].get('name') != 'api'):
            raise BootObservationError('startup container status contract mismatch')
        container = statuses[0] if statuses else {}
        restarts = container.get('restartCount', 0)
        state = container.get('state', {})
        reason = state.get('waiting', {}).get('reason')
        if (type(restarts) is not int or restarts != 0
                or (container and 'restartCount' not in container)
                or status.get('phase') in ('Failed', 'Succeeded')
                or container.get('lastState', {}).get('terminated')
                or 'terminated' in state or reason in FAILED_WAITING):
            raise BootObservationError('failed API startup attempt observed')
        is_ready = (container.get('ready') is True and status.get('phase') == 'Running'
                    and any(c.get('type') == 'Ready' and c.get('status') == 'True'
                            for c in status.get('conditions', [])))
        if is_ready:
            if not matches_identity(container.get('imageID', ''), image, config):
                raise BootObservationError('startup watch image config differs from selected reader')
            ready.add(uid)
        if kind == 'DELETED' and uid not in ready:
            raise BootObservationError('API pod was replaced before becoming Ready')
        observations.append({'type': kind, 'uid': uid, 'name': name,
                             'resource_version': metadata['resourceVersion'],
                             'image': containers[0]['image'], 'phase': status.get('phase'),
                             'restart_count': restarts, 'ready': is_ready})
    if selected['uid'] not in ready:
        raise BootObservationError('startup watch did not observe the selected pod Ready')
    return {'schema': 'cma-migration-boot-observation-v1',
            'start_resource_version': version, 'selected_uid': selected['uid'],
            'attempted_uids': sorted(attempted), 'ready_uids': sorted(ready),
            'observations': observations, 'failed_boots': 0}
