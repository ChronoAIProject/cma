import unittest

from migration_native_process import NativeProcessError
from migration_native_readiness import image_id_matches, job, projected_boot_list, ready_pod, select_api
from test_migration_native_campaign import artifacts


def pod(expected=None, name='api', namespace='cma-migration', uid='pod-uid'):
    expected = expected or artifacts()['old']
    return {'metadata': {'name': name, 'namespace': namespace, 'uid': uid, 'resourceVersion': '1'},
            'spec': {'containers': [{'name': name, 'image': expected['image']}]},
            'status': {'phase': 'Running', 'conditions': [{'type': 'Ready', 'status': 'True'}],
                       'containerStatuses': [{'name': name, 'ready': True, 'restartCount': 0,
                          'imageID': 'containerd://'+expected['config'], 'containerID': 'containerd://'+'d'*64,
                          'state': {'running': {'startedAt': '2026-11-18T00:00:00Z'}}}]}}


class ReadinessTests(unittest.TestCase):
    def test_exact_manifest_or_config_identity_only(self):
        expected = artifacts()['old']
        for value in (expected['image'], expected['config'], expected['image'].split('@')[1],
                       'docker-pullable://'+expected['image'], 'containerd://'+expected['config']):
            self.assertTrue(image_id_matches(value, expected))
        for value in (None, '', 'tag', 'prefix'+expected['config'], expected['config']+'suffix'):
            self.assertFalse(image_id_matches(value, expected))
        self.assertEqual(ready_pod(pod(), expected)['uid'], 'pod-uid')

    def test_pod_identity_readiness_and_restarts_reject(self):
        for mutate in (lambda p:p['metadata'].update(deletionTimestamp='now'),
            lambda p:p['metadata'].update(namespace='foreign'),lambda p:p['status'].update(phase='Pending'),
            lambda p:p['status'].update(conditions=[]),lambda p:p['spec'].update(containers=[]),
            lambda p:p['status']['containerStatuses'][0].update(restartCount=True),
            lambda p:p['status']['containerStatuses'][0].update(restartCount=1),
            lambda p:p['status']['containerStatuses'][0].update(imageID='foreign'),
            lambda p:p['status']['containerStatuses'][0].update(state={})):
            current = pod();mutate(current)
            with self.assertRaises(NativeProcessError): ready_pod(current, artifacts()['old'])

    def test_single_reader_and_projected_failed_states_without_env_or_messages(self):
        current = pod()
        self.assertEqual(select_api({'items':[current]}, artifacts()['old'])['uid'], 'pod-uid')
        for document in ({'items':[]}, {'items':[current,current]}, {'items':[current],'metadata':{'continue':'more'}}, {}):
            with self.assertRaises(NativeProcessError): select_api(document, artifacts()['old'])
        current['spec']['containers'][0]['env'] = [{'name':'BEARER','value':'hidden'}]
        current['status']['containerStatuses'][0]['state'] = {'terminated':{'reason':'Error','exitCode':1,'message':'hidden'}}
        value = projected_boot_list({'metadata':{'resourceVersion':'1'},'items':[current]})
        self.assertNotIn('hidden', str(value))
        self.assertEqual(value['items'][0]['status']['containerStatuses'][0]['state']['terminated']['exitCode'], 1)

    def test_job_has_finite_deadline_no_token_and_optional_probe(self):
        value = job('probe', artifacts()['old']['image'], [], code='print(1)', env={'SCOPE':'fixture'})
        self.assertEqual(value['spec']['backoffLimit'], 0)
        self.assertEqual(value['spec']['activeDeadlineSeconds'], 120)
        self.assertFalse(value['spec']['template']['spec']['automountServiceAccountToken'])
        self.assertEqual(value['spec']['template']['spec']['containers'][0]['env'],[{'name':'SCOPE','value':'fixture'}])
        with self.assertRaises(NativeProcessError): job('../foreign','image',[])


if __name__ == '__main__':
    unittest.main()
