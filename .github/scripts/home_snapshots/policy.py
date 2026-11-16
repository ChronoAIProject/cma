"""D4 selection and Compute policy guards. No I/O or mutations."""
import datetime as dt
import hashlib
import json
import re
import uuid

DRIVER = 'pd.csi.storage.gke.io'
CLASS = 'home-a-hourly-v1'
LABEL = 'cma-home-backup'
COHORT = 'd4-hourly-v1'
ANNOTATION = 'cma.chrono.ai/home-snapshot-policy'


def require(value, code):
    if not value:
        raise ValueError(code)


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def timestamp(value):
    parsed = dt.datetime.fromisoformat(value.replace('Z', '+00:00'))
    require(parsed.tzinfo is not None, 'timestamp_timezone_required')
    return parsed


def resource(path):
    return path.removeprefix('https://www.googleapis.com/compute/v1/').removeprefix(
        'https://compute.googleapis.com/compute/v1/')


def disk_handle(value, project, region):
    match = re.fullmatch(r'projects/([a-z][a-z0-9-]+)/(?P<scope>zones|regions)/([a-z0-9-]+)/disks/([a-z][a-z0-9-]{0,62})', value)
    require(match, 'disk_handle_invalid')
    selected_project, scope, location, name = match.groups()
    require(selected_project == project, 'disk_project_mismatch')
    actual_region = location.rsplit('-', 1)[0] if scope == 'zones' else location
    require(actual_region == region, 'disk_region_mismatch')
    return scope, location, name


def policy_ref(config):
    return f"projects/{config['project']}/regions/{config['region']}/resourcePolicies/{config['policy']}"


def policy_body(config):
    return {'name': config['policy'], 'snapshotSchedulePolicy': {
        'schedule': {'hourlySchedule': {'hoursInCycle': 1, 'startTime': '00:00'}},
        'retentionPolicy': {'maxRetentionDays': 7, 'onSourceDiskDelete': 'KEEP_AUTO_SNAPSHOTS'},
        'snapshotProperties': {'storageLocations': [config['region']],
                               'labels': {LABEL: COHORT}, 'guestFlush': False}}}


def validate_policy(policy, config):
    require(resource(policy.get('selfLink', '')) == policy_ref(config), 'policy_identity_mismatch')
    require(policy.get('status') == 'READY', 'policy_not_ready')
    schedule = policy.get('snapshotSchedulePolicy', {})
    expected = policy_body(config)['snapshotSchedulePolicy']
    require(schedule.get('schedule') == expected['schedule'], 'policy_frequency_mismatch')
    require(schedule.get('retentionPolicy') == expected['retentionPolicy'], 'policy_retention_mismatch')
    props = schedule.get('snapshotProperties', {})
    require(props.get('storageLocations') == [config['region']], 'policy_location_mismatch')
    require(not props.get('guestFlush', False), 'guest_flush_not_qualified')
    require(props.get('labels', {}).get(LABEL) == COHORT, 'policy_label_mismatch')


def validate_class(storage_class, config):
    meta = storage_class['metadata']
    require(meta['name'] == CLASS, 'storage_class_mismatch')
    require(meta.get('annotations', {}).get(ANNOTATION) == policy_ref(config), 'class_policy_mismatch')
    require(storage_class.get('provisioner') == DRIVER, 'class_driver_mismatch')
    require(storage_class.get('parameters', {}).get('labels') == f'{LABEL}={COHORT}', 'class_cohort_missing')
    require(storage_class.get('parameters', {}).get('type') == 'hyperdisk-balanced', 'class_disk_type_mismatch')
    require(storage_class.get('parameters', {}).get('disk-encryption-kms-key') == config['kms_key'], 'class_cmek_mismatch')
    require(timestamp(meta['creationTimestamp']) >= timestamp(config['activation_time']), 'class_predates_activation')


def eligible(pvc):
    return pvc.get('spec', {}).get('storageClassName') == CLASS


def binding(pvc, pv, storage_class, config):
    """Reject adopted old/static/foreign homes; only CSI-created cohort disks qualify."""
    validate_class(storage_class, config)
    pm, ps = pvc['metadata'], pvc['spec']
    vm, vs = pv['metadata'], pv['spec']
    require(eligible(pvc) and vs.get('storageClassName') == CLASS, 'volume_class_mismatch')
    require(pm.get('namespace') == config['namespace'], 'volume_namespace_mismatch')
    require(not pm.get('deletionTimestamp') and not vm.get('deletionTimestamp'), 'volume_deleting')
    require(pvc.get('status', {}).get('phase') == pv.get('status', {}).get('phase') == 'Bound', 'volume_not_bound')
    require(pm.get('annotations', {}).get('cma/home-layout') == 'native-vct-full-home-v2', 'home_layout_mismatch')
    require(ps.get('dataSource') is None and ps.get('dataSourceRef') is None, 'restored_or_cloned_volume')
    require(ps.get('accessModes') == ['ReadWriteOncePod'], 'home_access_mode_mismatch')
    require(vs.get('csi', {}).get('driver') == DRIVER, 'volume_driver_mismatch')
    require(vm.get('annotations', {}).get('pv.kubernetes.io/provisioned-by') == DRIVER, 'volume_not_dynamic')
    require(ps.get('volumeName') == vm['name'] == 'pvc-' + pm['uid'], 'volume_name_mismatch')
    claim = vs.get('claimRef', {})
    require((claim.get('name'), claim.get('namespace'), claim.get('uid')) ==
            (pm['name'], pm['namespace'], pm['uid']), 'claim_identity_mismatch')
    require(timestamp(pm['creationTimestamp']) >= timestamp(storage_class['metadata']['creationTimestamp']), 'pvc_predates_cohort')
    require(timestamp(vm['creationTimestamp']) >= timestamp(pm['creationTimestamp']), 'pv_predates_claim')
    handle = vs['csi']['volumeHandle']
    _, _, name = disk_handle(handle, config['project'], config['region'])
    require(name == vm['name'], 'disk_name_mismatch')
    return handle


def validate_disk(disk, handle, pvc, config):
    require(resource(disk.get('selfLink', '')) == handle, 'disk_identity_mismatch')
    require(str(disk.get('id', '')).isdigit(), 'disk_id_missing')
    require(disk.get('status') == 'READY', 'disk_not_ready')
    require(disk.get('labels', {}).get(LABEL) == COHORT, 'disk_cohort_missing')
    require(timestamp(disk['creationTimestamp']) >= timestamp(pvc['metadata']['creationTimestamp']), 'disk_predates_claim')
    require(disk.get('diskEncryptionKey', {}).get('kmsKeyName', '').split('/cryptoKeyVersions/')[0] == config['kms_key'], 'disk_cmek_mismatch')
    require(disk.get('type', '').rsplit('/', 1)[-1] == 'hyperdisk-balanced', 'disk_type_mismatch')
    require(not disk.get('sourceSnapshot') and not disk.get('sourceImage') and not disk.get('sourceDisk'), 'disk_not_fresh')
    return [resource(p) for p in disk.get('resourcePolicies', [])]


def request_id(disk, config):
    return str(uuid.uuid5(uuid.NAMESPACE_URL, resource(disk['selfLink']) + ':' + str(disk['id']) + ':' + policy_ref(config)))
