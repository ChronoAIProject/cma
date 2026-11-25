"""Offline control cases for candidate-bound regional recovery receipts."""
from datetime import datetime, timedelta, timezone
import json
from pathlib import Path
import tempfile
import unittest

from regional_recovery import SCOPES, DEPENDENCIES, CUSTODY, STAGES, HOME_CLASSES, main, verify
from release_candidate_packet import EvidenceFiles, digest
from staging_cd_receipts import canonical, sha256
from staging_cd_common import Refusal
from staging_cd_targets import TARGETS


def stamp(minutes, day=18):
    return (datetime(2026, 11, day, tzinfo=timezone.utc) + timedelta(minutes=minutes)).isoformat().replace('+00:00', 'Z')


class Campaign:
    def __init__(self, root, recheck=False):
        root.mkdir(exist_ok=True)
        self.root = root
        self.evidence = root / 'observations'
        self.evidence.mkdir()
        self.day = 26 if recheck else 18
        self.candidate = {'source': 'a' * 40, 'images': {name: 'sha256:' + sha256(name) for name in TARGETS},
                          **{key: sha256(key) for key in ('configuration_sha256', 'flags_sha256', 'environment_sha256')}}
        inventory = {}
        for i in range(12):
            key = sha256(f'home-{i}')
            inventory[key] = {'class': sorted(HOME_CLASSES)[i % 3], 'used_bytes': 4096 * (i + 1),
                              **{name: sha256(name + key) for name in ('owner_sha256', 'home_sha256', 'native_sha256', 'manifest_sha256',
                                                                       'receipts_sha256', 'source_storage_sha256')},
                              **{name: sha256(name + key + str(self.day)) for name in ('backup_sha256', 'backup_storage_sha256')},
                              'source_domain_sha256': sha256('source-runtime_region'), 'backup_domain_sha256': sha256('backup')}
        self.inventory = inventory
        self.scope = {'schema': 'cma-regional-recovery-v1', 'mode': 'synthetic', 'phase': 'frozen_recheck' if recheck else 'initial',
                      'candidate': self.candidate, 'window': {'start': self.time(0), 'end': self.time(720)},
                      'initial_report_sha256': None, 'frozen_at': stamp(0, 25) if recheck else None, 'inventory': inventory,
                      'home_sample': list(inventory)[:10], 'copy_changes': {key: {'steady': 10, 'peak': 100} for key in inventory},
                      'failure_scopes': {name: {'source_domain_sha256': sha256('source-' + name), 'recovery_domain_sha256': sha256('restore-' + name),
                                                'review_sha256': sha256('review-' + name), 'quota_bytes': 1000000,
                                                'consumer_order': [sha256('priority-1'), sha256('priority-2')]} for name in SCOPES},
                      'coverage_sha256': None, 'backup_campaign_sha256': None, 'commitments': {}}
        self.index = {'scope_sha256': '', 'coverage': None, 'backup_campaign': None, 'commitments': {}, 'regional': {},
                      'home_sample': {}, 'initial_report': None}
        self.backups()
        self.commitments()
        for i, name in enumerate(sorted(SCOPES)):
            self.index['regional'][name] = self.execution(name, inventory, 62 + i, regional=True)
        for i, key in enumerate(self.scope['home_sample']):
            self.index['home_sample'][key] = self.execution(key, {key: inventory[key]}, 70 + i, regional=False)

    def time(self, minute):
        return stamp(minute, self.day)

    def reference(self, name, value):
        path = self.evidence / (name + '.json')
        raw = canonical(value).encode()
        path.write_bytes(raw)
        return {'path': path.name, 'sha256': digest(raw)}

    def read(self, ref):
        return json.loads((self.evidence / ref['path']).read_bytes())

    def change(self, ref, update):
        row = self.read(ref)
        update(row)
        ref.update(self.reference(ref['path'][:-5], row))

    def backups(self):
        rows = [{'sandbox_sha256': key, 'owner_sha256': home['owner_sha256'], 'backup_sha256': home['backup_sha256'],
                 'reasons': []} for key, home in self.inventory.items()]
        coverage = {'mode': 'fixture', 'status': 'PASS', 'coverage': 1, 'retained_homes': len(rows), 'backed_homes': len(rows),
                    'orphan_count': 0, 'ownership_unknown_count': 0, 'homes': rows}
        self.index['coverage'] = self.reference('coverage', coverage)
        self.scope['coverage_sha256'] = self.index['coverage']['sha256']
        homes = {}
        for key, home in self.inventory.items():
            captures = []
            for i, kind in enumerate(('full', 'steady', 'peak')):
                capture = {'kind': kind, 'started_at': self.time(2 + i * 20), 'finished_at': self.time(12 + i * 20),
                           'bytes': home['used_bytes'] if kind == 'full' else self.scope['copy_changes'][key][kind], 'readable': True,
                           'receipt_sha256': sha256(key + kind + str(self.day)), 'backup_sha256': sha256(key + kind + 'backup'),
                           'storage_sha256': sha256(key + kind + 'copy'), 'domain_sha256': home['backup_domain_sha256'],
                           'manifest_sha256': sha256(key + kind + 'manifest'), 'captured_at': self.time(2 + i * 20),
                           'checkpoint_sha256': sha256('checkpoint-' + str(self.day) + kind)}
                if kind == 'peak':
                    capture.update(backup_sha256=home['backup_sha256'], storage_sha256=home['backup_storage_sha256'],
                                   manifest_sha256=home['manifest_sha256'])
                captures.append(capture)
            homes[key] = captures
        backup = {'mode': self.scope['mode'], 'candidate_sha256': sha256(self.candidate), 'started_at': self.time(1),
                  'finished_at': self.time(60), 'homes': homes,
                  **{field: sha256(field) for field in ('api_load_sha256', 'quota_sha256', 'cost_sha256')}}
        self.index['backup_campaign'] = self.reference('backup-campaign', backup)
        self.scope['backup_campaign_sha256'] = self.index['backup_campaign']['sha256']

    def commitments(self):
        for name in DEPENDENCIES:
            row = {'dependency': name, **{key: sha256(key + name) for key in ('approval_sha256', 'support_sha256', 'maintenance_sha256',
                                                                              'breaking_change_sha256', 'compatibility_sha256', 'upgrade_contact_sha256')},
                   'sla_sha256': sha256('sla-' + name) if name != 'model_gateway' else None,
                   'no_sla_risk_sha256': sha256('risk-' + name) if name == 'model_gateway' else None}
            reference = self.reference('commitment-' + name, row)
            self.index['commitments'][name] = reference
            self.scope['commitments'][name] = reference['sha256']

    def execution(self, name, inventory, minute, regional):
        run = sha256(name + str(self.day) + 'run')
        fault = sha256(name + str(self.day) + 'fault')
        source = self.scope['failure_scopes'][name]['source_domain_sha256'] if regional else self.inventory[name]['source_domain_sha256']
        target = self.scope['failure_scopes'][name]['recovery_domain_sha256'] if regional else sha256('sample-' + name)
        fault_ref = self.reference(name + '-fault', {'run_id': run, 'fault_sha256': fault, 'case': name, 'loss_at': self.time(minute),
                                                     'source_domain_sha256': source, 'recovery_domain_sha256': target, 'source_unavailable': True})
        checkpoint = sha256('checkpoint-' + str(self.day) + 'peak')
        cut = {'run_id': run, 'fault_sha256': fault, 'mongo_at': self.time(minute - 2), 'common_at': self.time(42),
               'checkpoint_sha256': checkpoint, 'data_classes': {kind: {'checkpoint_sha256': checkpoint, 'available_at': self.time(minute - 1),
                                                                          'independent_copy_sha256': sha256(run + kind), 'copy_domain_sha256': sha256('backup')} for kind in CUSTODY},
               'home_backups': {key: {**{field: home[field] for field in ('backup_sha256', 'backup_storage_sha256', 'backup_domain_sha256')},
                                          'checkpoint_sha256': checkpoint, 'available_at': self.time(52)} for key, home in inventory.items()}}
        cut_ref = self.reference(name + '-cut', cut)
        verified = minute + (100 if regional else 20)
        homes = {key: {**{field: home[field] for field in set(home) - {'class', 'source_storage_sha256', 'source_domain_sha256'}},
                       'restored_storage_sha256': sha256(run + key + 'restored'), 'restored_domain_sha256': target,
                       'started_at': self.time(minute + 4), 'verified_at': self.time(verified - 1)} for key, home in inventory.items()}
        readback = {'run_id': run, 'fault_sha256': fault, 'checkpoint_sha256': checkpoint, 'mongo_restored_at': self.time(minute + 3),
                    'service_verified_at': self.time(verified), 'homes': homes, 'old_writer_fenced': True, 'tombstones_replayed': True,
                    'key_custody_verified': True, 'object_inventory_verified': True, 'duplicate_effects': 0,
                    'unreadable_records': 0, 'secret_matches': 0, 'resurrected_records': 0}
        readback_ref = self.reference(name + '-readback', readback)
        stages = None
        if regional:
            stage_data = {'run_id': run, 'stages': [{'stage': stage, 'verified_at': self.time(minute + i + 1 if i < 7 else verified),
                                                   'receipt_sha256': sha256(run + stage)} for i, stage in enumerate(STAGES)],
                          'consumers': self.scope['failure_scopes'][name]['consumer_order'], 'available_quota_bytes': 1000000,
                          **{field: sha256(run + field) for field in ('capacity_receipt_sha256', 'throttle_receipt_sha256',
                                                                       'queue_receipt_sha256', 'manual_steps_receipt_sha256')}}
            stages = self.reference(name + '-stages', stage_data)
        row = {'case': name, 'mode': self.scope['mode'], 'candidate_sha256': sha256(self.candidate), 'run_id': run, 'attempt': 1,
               'cancelled': False, 'status': 'PASS', 'started_at': self.time(minute - 1), 'finished_at': self.time(verified + 1),
               'samples': len(inventory), 'fault': fault_ref, 'cut': cut_ref, 'readback': readback_ref, 'stages': stages}
        return self.reference(name + '-execution', row)

    def mutate_execution(self, name, artifact, mutation, sample=False):
        ref = self.index['home_sample' if sample else 'regional'][name]
        if artifact is None:
            self.change(ref, mutation)
            return
        row = self.read(ref)
        self.change(row[artifact], mutation)
        ref.update(self.reference(ref['path'][:-5], row))

    def verify(self):
        raw = canonical(self.scope).encode()
        self.index['scope_sha256'] = digest(raw)
        return verify(raw, digest(raw), self.index, EvidenceFiles(self.evidence))


class RecoveryTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.campaign = Campaign(Path(temp.name))

    def rejected(self, artifact, mutation, name='runtime_region'):
        self.campaign.mutate_execution(name, artifact, mutation)
        with self.assertRaises(Refusal):
            self.campaign.verify()

    def test_complete_cli_report(self):
        campaign = self.campaign
        raw = canonical(campaign.scope).encode()
        scope, index, output = campaign.root / 'scope.json', campaign.evidence / 'index.json', campaign.root / 'report.json'
        scope.write_bytes(raw)
        campaign.index['scope_sha256'] = digest(raw)
        index.write_text(canonical(campaign.index))
        self.assertEqual(main(['--scope', str(scope), '--scope-sha256', digest(raw), '--observation', str(index), '--output', str(output)]), 0)
        report = json.loads(output.read_text())
        self.assertEqual(report['status'], 'CONSISTENT')
        self.assertEqual(report['mode'], 'synthetic')
        self.assertEqual(set(report['regional']), SCOPES)
        self.assertEqual(len(report['sample']), 10)
        for row in report['regional'].values():
            self.assertEqual(row['restored_homes'], 12)
            self.assertEqual(row['used_bytes'], sum(home['used_bytes'] for home in campaign.inventory.values()))
            self.assertEqual(row['mongo_rpo_seconds'], 120)
            self.assertIn(row['integrated_rpo_seconds'], (1200, 1260, 1320))
        self.assertEqual(output.stat().st_mode & 0o777, 0o600)

    def test_missing_scope(self):
        self.campaign.index['regional'].pop('runtime_region')
        with self.assertRaises(Refusal):
            self.campaign.verify()

    def test_missing_full_inventory(self):
        self.rejected('readback', lambda row: row['homes'].pop(next(iter(row['homes']))))

    def test_bytes_mismatch(self):
        key = next(iter(self.campaign.inventory))
        self.rejected('readback', lambda row: row['homes'][key].update(used_bytes=1))

    def test_source_storage_is_not_restore(self):
        key, home = next(iter(self.campaign.inventory.items()))
        self.rejected('readback', lambda row: row['homes'][key].update(restored_storage_sha256=home['source_storage_sha256']))

    def test_mongo_rpo_exceeded(self):
        self.rejected('cut', lambda row: row.update(mongo_at=stamp(55)))

    def test_integrated_rpo_exceeded(self):
        self.rejected('cut', lambda row: row.update(common_at=stamp(0)))

    def test_recovery_rto_exceeded(self):
        self.rejected('readback', lambda row: row.update(service_verified_at=stamp(610)))

    def test_duplicate_fault(self):
        ref = self.campaign.index['regional']['control_plane']
        fault = self.campaign.read(self.campaign.read(ref)['fault'])['fault_sha256']
        self.rejected('fault', lambda row: row.update(fault_sha256=fault))

    def test_no_sla_risk_is_required(self):
        ref = self.campaign.index['commitments']['model_gateway']
        self.campaign.change(ref, lambda row: row.update(no_sla_risk_sha256=None))
        self.campaign.scope['commitments']['model_gateway'] = ref['sha256']
        with self.assertRaises(Refusal):
            self.campaign.verify()

    def test_missing_backup_cadence(self):
        ref = self.campaign.index['backup_campaign']
        self.campaign.change(ref, lambda row: next(iter(row['homes'].values())).pop())
        self.campaign.scope['backup_campaign_sha256'] = ref['sha256']
        with self.assertRaises(Refusal):
            self.campaign.verify()

    def test_common_cut_cannot_postdate_actual_backup(self):
        self.rejected('cut', lambda row: row.update(common_at=stamp(55)))

    def test_copy_in_failed_domain_is_refused(self):
        key = next(iter(self.campaign.inventory))
        self.rejected('cut', lambda row: row['home_backups'][key].update(
            backup_domain_sha256=self.campaign.scope['failure_scopes']['runtime_region']['source_domain_sha256']))

    def test_tombstone_and_old_writer_fence_required(self):
        self.rejected('readback', lambda row: row.update(tombstones_replayed=False))

    def test_candidate_mismatch(self):
        self.rejected(None, lambda row: row.update(candidate_sha256=sha256('other-build')))

    def test_late_initial_refused(self):
        self.campaign.scope['window']['end'] = stamp(720, 24)
        with self.assertRaises(Refusal):
            self.campaign.verify()

    def test_frozen_recheck_requires_independent_initial(self):
        first = self.campaign.verify()
        other = Campaign(self.campaign.root / 'recheck', recheck=True)
        reference = other.reference('initial', first)
        other.index['initial_report'] = reference
        other.scope['initial_report_sha256'] = reference['sha256']
        self.assertEqual(other.verify()['phase'], 'frozen_recheck')
        other.scope['candidate']['source'] = 'b' * 40
        with self.assertRaises(Refusal):
            other.verify()


if __name__ == '__main__':
    unittest.main()
