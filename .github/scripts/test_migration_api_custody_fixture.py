"""Exercise all fixture stages, credential loss, and independent snapshot failure."""
from copy import deepcopy
import json
import os
from pathlib import Path
import secrets
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from migration_api_custody_fixture import ApiCustodyFixture, fetch, manifests
from migration_api_custody_provider import issue, metadata
from migration_api_custody_documents import capture, job
from migration_browser_fixture import OWNER
from migration_fixture_dependencies import capture as dependency_capture
from migration_kind_manifests import manifests as base_manifests
from migration_sandbox_fixture import STAGES
from test_migration_sandbox_control_fixture import ControlAPI
from test_migration_sandbox_fixture import PROFILES
from test_migration_api_custody_validation import snapshot


class IndependentAPI(ControlAPI):
    def __init__(self, output):
        super().__init__(output)
        self.authority_calls=[]

    def request(self, method, route, body=None, key=None, status=200, bearer=None, use_cookies=True):
        if '/sandbox-commands/' in route or route.endswith('/commands'):
            if not bearer or use_cookies: raise RuntimeError('control used cookie or omitted bearer')
            self.authority_calls.append((method,route,use_cookies))
        elif bearer: raise RuntimeError('independent token leaked into browser workflow')
        return super().request(method,route,body,key,status)


class FixtureTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.api=IndependentAPI(self.temp.name);self.fixture=ApiCustodyFixture(self.api)
        self.issued={}
        def obtain(_fixture,slot):
            if slot not in self.issued:
                token=issue(OWNER,2000000000)
                self.issued[slot]=(token,{'slot':slot,**metadata(token,OWNER,2000000000)})
            return self.issued[slot]
        self.fetch=patch('migration_api_custody_fixture.fetch',side_effect=obtain)
        self.fetch.start();self.addCleanup(self.fetch.stop)

    def test_all_four_stages_keep_exact_custody_and_never_record_credentials(self):
        profiles=deepcopy(PROFILES)
        for stage in STAGES:
            result=self.fixture.exercise(stage,profiles)
            self.assertIn('auth_api_bearers',result['collections'])
            self.assertTrue(result['checks']['opposite-reader-custody-decryption'])
            self.assertEqual(self.fixture.by_sandbox,{})
            self.assertEqual(self.fixture.by_command,{})
        self.assertEqual(len(self.issued),4);self.assertTrue(self.api.authority_calls)
        for p in Path(self.temp.name).glob('*.json'):
            for token,_ in self.issued.values():self.assertNotIn(token,p.read_text())

    def test_changed_credential_fails_reader_and_clears_transient_state(self):
        profiles=deepcopy(PROFILES);self.fixture.exercise('old-write',profiles)
        token=issue(OWNER,2000000060)
        self.issued['old-write/0']=(token,{'slot':'old-write/0',**metadata(token,OWNER,2000000060)})
        with self.assertRaisesRegex(RuntimeError,'credential or expiry changed'):
            self.fixture.exercise('new-read-old-write',profiles)
        self.assertEqual(self.fixture.by_sandbox,{})
        self.assertEqual(self.fixture.by_command,{})

    def test_missing_custody_cannot_fall_back_to_browser_auth(self):
        with self.assertRaisesRegex(RuntimeError,'lacks independent'):
            self.fixture.request('POST','/api/v1/sandboxes/sbx_a/commands')
        for value in ([],[{},{}]):
            with self.assertRaises(RuntimeError):self.fixture.verify_controls(value)

    def test_overlay_preserves_find_only_boundary_and_separate_reader(self):
        credentials={k:secrets.token_urlsafe(24) for k in (
            'bearer','admin_bearer','outsider_bearer','client_id','client_secret','refresh_token')}
        credentials['expires_at']=2000000000
        objects=base_manifests('api','runtime','admin','key','mongo',{},identity_credentials=credentials)
        before=deepcopy(objects);rendered=manifests(objects)
        self.assertEqual(objects,before)
        self.assertFalse(any(o['kind']=='Secret' for o in rendered))
        for o in rendered:
            if o['kind']=='Role':self.assertTrue(all(r['verbs']==['get','list'] for r in o['rules']))
        init=next(o['data']['init.sh'] for o in rendered if o['kind']=='ConfigMap' and o['metadata']['name']=='mongo-init')
        own=init.split('role:"matrix_api_custody_reader",')[1].split('roles:[]')[0]
        self.assertEqual(own,'privileges:[{resource:{db:"matrix",collection:"auth_api_bearers"},actions:["find"]}],')
        manifest=job('matrix','mongo','old-write')
        self.assertFalse(manifest['spec']['template']['spec']['automountServiceAccountToken'])
        self.assertEqual(manifest['spec']['backoffLimit'],0)
        self.assertNotIn('MATRIX_API_CUSTODY_READ_PASSWORD',json.dumps(manifest))
        with self.assertRaises(RuntimeError):job('matrix','mongo','foreign')


class ClientResponseTests(unittest.TestCase):
    def test_malformed_and_oversized_provider_responses_do_not_enter_application_log(self):
        fixture=SimpleNamespace(admin_bearer=secrets.token_urlsafe(24),requests=[])
        responses=[b'x'*8193,b'{}',json.dumps({'slot':'old-write/0','access_token':'a b','metadata':{}}).encode(),
                   json.dumps({'slot':'old-write/0','access_token':secrets.token_urlsafe(24),'metadata':{'expires_at':1}}).encode()]
        for raw in responses:
            response=Mock(status=200);response.read.return_value=raw
            context=Mock();context.__enter__=Mock(return_value=response);context.__exit__=Mock(return_value=False)
            with patch.dict(os.environ,{'MATRIX_IDENTITY_URL':'http://127.0.0.1:1234'}),patch('migration_api_custody_fixture.urlopen',return_value=context),self.assertRaises(RuntimeError):
                fetch(fixture,'old-write/0')
        self.assertEqual(fixture.requests,[])


class CaptureTests(unittest.TestCase):
    def test_capture_joins_real_snapshot_shape_and_retains_exact_image_binding(self):
        with tempfile.TemporaryDirectory() as temp:
            output=Path(temp);value,records,controls=snapshot()
            (output/'old-write-sandbox-controls.private.json').write_text(json.dumps(records[0]))
            (output/'controls.json').write_text(json.dumps(controls))
            campaign=SimpleNamespace(output=output,kubectl=Mock(return_value=json.dumps(value)))
            control_result={'path':'controls.json','collections':['sandbox_controls'],'outcome':'PASS'}
            with patch('migration_api_custody_documents.capture_controls',return_value=control_result):
                for stage in STAGES[:2]:
                    result=capture(campaign,'matrix','mongo',stage,'source','pinned-image')
                    self.assertEqual(result['outcome'],'PASS');self.assertEqual(result['custodies'],2)
                    self.assertEqual(result['image'],'pinned-image')
            self.assertTrue((output/'api-custody-documents-new-read-old-write.private.json').exists())

    def test_first_capture_failure_is_retained_even_if_diagnostics_fail(self):
        with tempfile.TemporaryDirectory() as temp:
            campaign=SimpleNamespace(output=Path(temp),kubectl=Mock(side_effect=[None,TimeoutError('first failure'),RuntimeError('log unavailable')]))
            with patch('migration_api_custody_documents.capture_controls',return_value={}):
                with self.assertRaisesRegex(TimeoutError,'first failure'):
                    capture(campaign,'matrix','mongo','old-write','source','image')
            self.assertEqual(campaign.kubectl.call_count,3)
            self.assertTrue(list(Path(temp).glob('*.diagnostic-error.json')))

    def test_capture_invalid_documents_cannot_be_passed_as_http_success(self):
        with tempfile.TemporaryDirectory() as temp:
            output=Path(temp);value,records,controls=snapshot()
            value['collections']['auth_api_bearers'].clear()
            (output/'old-write-sandbox-controls.private.json').write_text(json.dumps(records[0]))
            (output/'controls.json').write_text(json.dumps(controls))
            campaign=SimpleNamespace(output=output,kubectl=Mock(return_value=json.dumps(value)))
            with patch('migration_api_custody_documents.capture_controls',return_value={'path':'controls.json'}),self.assertRaisesRegex(RuntimeError,'row count'):
                capture(campaign,'matrix','mongo','old-write','source','image')
            self.assertTrue((output/'api-custody-documents-old-write.private.json').exists())

    def test_opt_in_capture_calls_only_custody_observer(self):
        with patch('migration_api_custody_documents.capture',return_value={'outcome':'PASS'}) as called:
            result=dependency_capture('campaign',SimpleNamespace(api_custody_fixture=True),'ns','mongo','stage','source','api')
            self.assertEqual(result,{'outcome':'PASS'})
            called.assert_called_once_with('campaign','ns','mongo','stage','source','api')


if __name__=='__main__':unittest.main()
