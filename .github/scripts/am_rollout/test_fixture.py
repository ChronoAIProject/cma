"""The local credential fixture cannot weaken the real image-only operation."""
import copy
import unittest

import test_controller as t

c = t.c


class CredentialFixtureTests(unittest.TestCase):
    def setUp(self):
        self.h = t.RolloutTests()
        self.h.setUp()
        self.addCleanup(self.h.doCleanups)
        self.base = self.h.base
        pod = self.base['spec']['template']['spec']
        pod['volumes'] = [{'name': 'reference-' + str(i), 'configMap': {'name': 'reference-' + str(i)}}
                          for i in range(3)] + [copy.deepcopy(c.FIXTURE_RULE['production_volume'])]
        pod['securityContext'] = {'runAsUser': 10001, 'runAsGroup': 10001, 'fsGroup': 10001,
                                  'runAsNonRoot': True, 'seccompProfile': {'type': 'RuntimeDefault'}}
        api = c.container(self.base)[1]
        api['securityContext'] = {'runAsNonRoot': True, 'readOnlyRootFilesystem': True,
            'allowPrivilegeEscalation': False, 'privileged': False, 'capabilities': {'drop': ['ALL']}}
        api['volumeMounts'] = [copy.deepcopy(c.FIXTURE_RULE['mount'])]
        self.local = copy.deepcopy(self.base)
        self.local['metadata']['namespace'] = 'am-local-fixture'
        c.container(self.local)[1]['image'] = t.NEW
        self.local['spec']['template']['spec']['volumes'][3] = copy.deepcopy(c.FIXTURE_RULE['local_volume'])
        self.packet = self.h.packet
        self.packet['baseline']['deployment'] = self.h.ref('baseline', self.base)
        self.packet['candidate']['config_digest'] = 'sha256:' + 'd' * 64
        self.packet['candidate']['publication'] = self.h.ref('publication', {
            'status': 'PASS', 'image': t.NEW, 'source': 'c' * 40,
            'exact_config_and_layers_match': True, 'config_digest': 'sha256:' + 'd' * 64})
        identities = [{'pod_uid': 'candidate-' + str(i), 'container_id': 'containerd://process-' + str(i),
                       'image_id': t.NEW} for i in range(3)]
        self.secret = {'name': c.FIXTURE_NAME, 'namespace': 'am-local-fixture', 'uid': 'secret-uid',
            'resource_version': '100', 'type': 'Opaque', 'immutable': True,
            'data_keys': ['opensandbox-api-key'], 'content_sha256': 'e' * 64,
            'provenance': 'generated_for_this_local_run'}
        self.fake = {'origin': 'http://sandbox.am-local-fixture.svc', 'namespace_uid': 'namespace-local',
                     'service_uid': 'fake-service', 'isolated': True, 'production': False}
        self.receipt = {'status': 'PASS', 'source': 'c' * 40, 'image': t.NEW,
            'template_sha256': c.hash_json(self.local['spec']['template']),
            'checks': dict.fromkeys(c.REHEARSAL_CHECKS, 'PASS'), 'run_id': 'local-run',
            'local_cluster': {'context': 'kind-isolated', 'server': 'https://127.0.0.1:39443',
                              'kube_system_uid': 'kind-cluster-uid'},
            'local_namespace': {'name': 'am-local-fixture', 'uid': 'namespace-local'},
            'fixture_secret': {k: self.secret[k] for k in
                ('name', 'namespace', 'uid', 'resource_version', 'content_sha256')},
            'fake_provider': copy.deepcopy(self.fake), 'candidate_processes': identities,
            'final_pod_uids': ['candidate-0', 'candidate-1']}
        self.evidence = {'schema': 'am-local-credential-fixture-v1', 'status': 'PASS',
            'run_id': 'local-run', 'source': 'c' * 40, 'image': t.NEW, 'config_digest': 'sha256:' + 'd' * 64,
            'template_sha256': self.receipt['template_sha256'],
            'cluster': copy.deepcopy(self.receipt['local_cluster']),
            'namespace': copy.deepcopy(self.receipt['local_namespace']), 'secret': copy.deepcopy(self.secret),
            'fake_provider': copy.deepcopy(self.fake), 'processes': [{**identity,
                'namespace_uid': 'namespace-local', 'secret_uid': 'secret-uid', 'secret_resource_version': '100',
                'uid': 10001, 'gid': 10001, 'groups': [10001],
                'file': {'path': c.KEY_FILE, 'resolved_mode': 288, 'resolved_uid': 0, 'resolved_gid': 10001,
                         'sha256': 'e' * 64, 'readable': True, 'valid_nonempty_header': True,
                         'read_only_mount': True, 'write_refused': True, 'non_destructive_write_check': True},
                'effective_config': {'api_key_file': c.KEY_FILE, 'sandbox_provider': 'agent-sandbox',
                    'retain_opensandbox': True, 'sandbox_base_url': self.fake['origin']}}
                for identity in identities],
            'authentication': {
                'am_requests': [{'request_id': 'actual-am-request', 'pod_uid': 'candidate-0',
                    'caller': 'actual_am', 'credential_source': 'startup_file', 'status': 200,
                    'fake_service_uid': 'fake-service'}],
                'negative_checks': [{'credential': kind, 'request_id': kind + '-request',
                    'status': 401, 'fake_service_uid': 'fake-service'} for kind in ('missing', 'wrong')]}}
        self.packet['rehearsal']['credential_fixture'] = {'rule': copy.deepcopy(c.FIXTURE_RULE), 'evidence': None}
        self.h.io.base = self.base
        self.h.io.current = copy.deepcopy(self.base)
        self.save()

    def save(self, evidence=None, receipt=None, local=None):
        evidence = self.evidence if evidence is None else evidence
        receipt = self.receipt if receipt is None else receipt
        local = self.local if local is None else local
        self.packet['rehearsal']['deployment'] = self.h.ref('rehearsal', local)
        self.packet['rehearsal']['receipt'] = self.h.ref('receipt', receipt)
        self.packet['rehearsal']['credential_fixture']['evidence'] = self.h.ref('fixture-proof', evidence)

    def planned(self):
        planned = copy.deepcopy(self.base)
        c.container(planned)[1]['image'] = t.NEW
        return planned

    def test_exact_pair_and_actual_receipt_keep_production_CSI(self):
        before = copy.deepcopy(self.base)
        _, planned, rows = c.validate_packet(self.packet)
        self.assertEqual(self.base, before)
        self.assertEqual(planned, self.planned())
        self.assertEqual({r['path'] for r in rows}, {'/spec/volumes/3/csi', '/spec/volumes/3/secret'})
        self.assertIn('csi', planned['spec']['template']['spec']['volumes'][3])
        self.assertNotIn('secret', planned['spec']['template']['spec']['volumes'][3])

    def test_projection_needs_explicit_rule_and_execution_needs_real_evidence(self):
        with self.assertRaises(c.Refused): c.template_gate(self.planned(), self.local, [])
        self.assertEqual(len(c.template_gate(self.planned(), self.local, [], c.FIXTURE_RULE)), 2)
        self.packet['rehearsal']['credential_fixture']['evidence'] = None
        with self.assertRaisesRegex(c.Refused, 'unbound_evidence'): c.validate_packet(self.packet)
        self.assertEqual(self.h.io.calls, [])

    def test_malformed_or_partial_volume_pairs_cannot_be_excepted(self):
        for case in ('remove_only', 'add_only', 'other_name', 'other_index', 'configmap', 'hostpath',
                     'projected', 'optional', 'mode', 'key', 'path', 'extra_item', 'duplicate_volume'):
            with self.subTest(case=case):
                local = copy.deepcopy(self.local)
                volumes = local['spec']['template']['spec']['volumes']; volume = volumes[3]
                if case == 'remove_only': volume.pop('secret')
                if case == 'add_only': volume['csi'] = copy.deepcopy(c.FIXTURE_RULE['production_volume']['csi'])
                if case == 'other_name': volume['name'] = 'another-volume'
                if case == 'other_index': volumes[2], volumes[3] = volumes[3], volumes[2]
                if case in ('configmap', 'hostpath', 'projected'):
                    volume.pop('secret'); volume[{'configmap':'configMap','hostpath':'hostPath','projected':'projected'}[case]] = {}
                if case == 'optional': volume['secret']['optional'] = True
                if case == 'mode': volume['secret']['defaultMode'] = 292
                if case in ('key', 'path'): volume['secret']['items'][0][case] = 'other'
                if case == 'extra_item': volume['secret']['items'].append({'key': 'other', 'path': 'other'})
                if case == 'duplicate_volume': volumes.append(copy.deepcopy(volume))
                with self.assertRaises(c.Refused): c.template_gate(self.planned(), local, [], c.FIXTURE_RULE)

    def test_original_source_and_typed_rule_are_exact(self):
        for case in ('driver', 'provider_class', 'readonly', 'different_rule'):
            with self.subTest(case=case):
                planned = self.planned(); rule = copy.deepcopy(c.FIXTURE_RULE)
                source = planned['spec']['template']['spec']['volumes'][3]['csi']
                if case == 'driver': source['driver'] = 'different.csi'
                if case == 'provider_class': source['volumeAttributes']['secretProviderClass'] = 'other'
                if case == 'readonly': source['readOnly'] = False
                if case == 'different_rule': rule['local_volume']['secret']['secretName'] = 'other'
                with self.assertRaises(c.Refused): c.template_gate(planned, self.local, [], rule)

    def test_exact_JSON_types_cannot_coerce_boolean_or_mode(self):
        for case in ('rule_boolean', 'source_boolean', 'mount_boolean', 'float_mode'):
            with self.subTest(case=case):
                planned = self.planned(); local = copy.deepcopy(self.local); rule = copy.deepcopy(c.FIXTURE_RULE)
                if case == 'rule_boolean': rule['production_volume']['csi']['readOnly'] = 1
                if case == 'source_boolean': planned['spec']['template']['spec']['volumes'][3]['csi']['readOnly'] = 1
                if case == 'mount_boolean': c.container(local)[1]['volumeMounts'][0]['readOnly'] = 1
                if case == 'float_mode': local['spec']['template']['spec']['volumes'][3]['secret']['defaultMode'] = 288.0
                with self.assertRaises(c.Refused): c.template_gate(planned, local, [], rule)

    def test_no_generic_subtree_exception_even_with_fixture_enabled(self):
        rule = {'path': '/spec/volumes/3/secret', 'production_present': False, 'rehearsal_present': True,
                'production': None, 'rehearsal': c.FIXTURE_RULE['local_volume']['secret'], 'reason': 'cannot bypass'}
        with self.assertRaisesRegex(c.Refused, 'behavior_difference_cannot_be_excepted'):
            c.template_gate(self.planned(), self.local, [rule], c.FIXTURE_RULE)

    def test_mount_security_env_probes_and_shutdown_stay_strict(self):
        for case in ('writable', 'subpath', 'duplicate_mount', 'sidecar_mount', 'root_uid', 'root_group',
                     'capabilities', 'writable_root', 'env', 'readiness', 'lifecycle', 'grace'):
            with self.subTest(case=case):
                local = copy.deepcopy(self.local); pod = local['spec']['template']['spec']; api = c.container(local)[1]
                if case == 'writable': api['volumeMounts'][0]['readOnly'] = False
                if case == 'subpath': api['volumeMounts'][0]['subPath'] = 'opensandbox-api-key'
                if case == 'duplicate_mount': api['volumeMounts'].append(copy.deepcopy(api['volumeMounts'][0]))
                if case == 'sidecar_mount': pod['containers'].append({'name':'helper','volumeMounts':copy.deepcopy(api['volumeMounts'])})
                if case == 'root_uid': pod['securityContext']['runAsUser'] = 0
                if case == 'root_group': pod['securityContext']['fsGroup'] = 0
                if case == 'capabilities': api['securityContext']['capabilities']['add'] = ['SYS_ADMIN']
                if case == 'writable_root': api['securityContext']['readOnlyRootFilesystem'] = False
                if case == 'env': api['env'].append({'name':'CHRONO_RETAIN_OPENSANDBOX','value':'false'})
                if case == 'readiness': api['readinessProbe']['periodSeconds'] = 5
                if case == 'lifecycle': api['lifecycle']['preStop']['exec']['command'] = ['true']
                if case == 'grace': pod['terminationGracePeriodSeconds'] = 810
                with self.assertRaises(c.Refused): c.template_gate(self.planned(), local, [], c.FIXTURE_RULE)

    def test_missing_stale_or_nonlocal_runtime_identity_fails(self):
        for case in ('proposal', 'image', 'source', 'config', 'template', 'run', 'namespace', 'secret_uid',
                     'secret_version', 'mutable', 'keyset', 'value', 'cloud_server', 'production_context', 'cluster_uid',
                     'foreign_fake', 'missing_process', 'different_process', 'duplicate_process', 'final_pair'):
            with self.subTest(case=case):
                e = copy.deepcopy(self.evidence); receipt = copy.deepcopy(self.receipt)
                if case == 'proposal': e['status'] = 'PROPOSED'
                if case in ('image','source','config','template','run'):
                    e[{'config':'config_digest','template':'template_sha256','run':'run_id'}.get(case,case)] = 'wrong'
                if case == 'namespace': e['namespace']['uid'] = 'recreated'
                if case == 'secret_uid': e['secret']['uid'] = 'recreated'
                if case == 'secret_version': e['secret']['resource_version'] = '101'
                if case == 'mutable': e['secret']['immutable'] = False
                if case == 'keyset': e['secret']['data_keys'].append('extra')
                if case == 'value': e['secret']['value'] = 'must-not-be-in-receipt'
                if case == 'cloud_server': e['cluster']['server'] = 'https://production.example:443'
                if case == 'production_context': e['cluster']['context'] = c.CONTEXT
                if case == 'cluster_uid': e['cluster']['kube_system_uid'] = 'another-cluster'
                if case == 'foreign_fake': e['fake_provider']['origin'] = 'http://sandbox.chrono-bot.svc'
                if case == 'missing_process': e['processes'].pop()
                if case == 'different_process': e['processes'][0]['container_id'] = 'another-process'
                if case == 'duplicate_process': e['processes'].append(copy.deepcopy(e['processes'][0]))
                if case == 'final_pair': receipt['final_pod_uids'] = ['candidate-0','candidate-0']
                self.save(evidence=e, receipt=receipt)
                with self.assertRaises(c.Refused): c.validate_packet(self.packet)

    def test_every_process_needs_real_file_read_and_mount_proof(self):
        for case in ('uid','gid','groups','mode','file_digest','readable','valid_nonempty_header',
                     'read_only_mount','write_refused','non_destructive_write_check','read_branch','key_file'):
            with self.subTest(case=case):
                e = copy.deepcopy(self.evidence); p = e['processes'][2]
                if case in ('uid','gid'): p[case] = 0
                elif case == 'groups': p['groups'] = []
                elif case == 'mode': p['file']['resolved_mode'] = 292
                elif case == 'file_digest': p['file']['sha256'] = 'f' * 64
                elif case == 'read_branch': p['effective_config']['retain_opensandbox'] = False
                elif case == 'key_file': p['effective_config']['api_key_file'] = '/tmp/key'
                else: p['file'][case] = False
                self.save(evidence=e)
                with self.assertRaises(c.Refused): c.validate_packet(self.packet)

    def test_helper_only_or_accept_all_fake_is_not_authentication(self):
        for case in ('missing_am','helper','foreign_pod','environment_key','missing_negative','accept_all','wrong_service','duplicate_negative','reused_success'):
            with self.subTest(case=case):
                e = copy.deepcopy(self.evidence); auth = e['authentication']
                if case == 'missing_am': auth['am_requests'] = []
                if case == 'helper': auth['am_requests'][0]['caller'] = 'curl_helper'
                if case == 'foreign_pod': auth['am_requests'][0]['pod_uid'] = 'not-this-run'
                if case == 'environment_key': auth['am_requests'][0]['credential_source'] = 'env'
                if case == 'missing_negative': auth['negative_checks'].pop()
                if case == 'accept_all': auth['negative_checks'][0]['status'] = 200
                if case == 'wrong_service': auth['am_requests'][0]['fake_service_uid'] = 'other'
                if case == 'duplicate_negative': auth['negative_checks'][1]['request_id'] = auth['negative_checks'][0]['request_id']
                if case == 'reused_success': auth['negative_checks'][0]['request_id'] = auth['am_requests'][0]['request_id']
                self.save(evidence=e)
                with self.assertRaises(c.Refused): c.validate_packet(self.packet)

    def test_existing_rollout_rows_and_publication_cannot_be_replaced_by_file_proof(self):
        receipt = copy.deepcopy(self.receipt); receipt['checks']['drain_wake'] = 'NOT_PROVEN'; self.save(receipt=receipt)
        with self.assertRaisesRegex(c.Refused, 'exact_image_runtime'): c.validate_packet(self.packet)
        self.save()
        publication = c.bound(self.packet['candidate']['publication']); publication['config_digest'] = 'sha256:' + 'f' * 64
        self.packet['candidate']['publication'] = self.h.ref('publication', publication)
        with self.assertRaisesRegex(c.Refused, 'config_unbound'): c.validate_packet(self.packet)

    def test_real_operation_remains_image_only_and_retains_production_CSI(self):
        self.h.execute()
        for name, args in self.h.io.calls:
            if name in ('dry-run','mutation'):
                patch = c.json.loads(args[args.index('--patch') + 1])
                preimage = next(row['value'] for row in patch if row['path'] == '/spec')
                self.assertEqual(preimage['template']['spec']['volumes'][3], c.FIXTURE_RULE['production_volume'])
                self.assertEqual([row['path'] for row in patch if row['op'] != 'test'],
                                 ['/spec/template/spec/containers/0/image'])
        self.assertEqual(self.h.io.current, self.planned())


if __name__ == '__main__':
    unittest.main(verbosity=2)
