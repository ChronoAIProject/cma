from contextlib import ExitStack
from copy import deepcopy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from migration_native_campaign import NativeCampaign
from migration_native_kind import NativeKind
from migration_native_kind_fixture import FakeCommands, FakeForward, FakeSession, fixture_config
from migration_native_process import Journal, NativeProcessError


class KindTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name).resolve()
        self.input_path,self.config,self.pins,self.runtime_bytes=fixture_config(self.root)
        self.directory=self.root/'campaign';self.directory.mkdir(mode=0o700)
        self.journal=Journal(self.directory);self.addCleanup(self.journal.close)
        self.commands=FakeCommands(self.config,self.directory,'cma-matrix-123456789a')
        FakeForward.instances=[]
        self.owner=NativeKind(self.config,self.directory,self.journal,cluster=self.commands.cluster,
                              command_factory=self.commands.factory,forward_factory=FakeForward)
        stack=ExitStack();self.addCleanup(stack.close)
        stack.enter_context(patch('migration_native_kind_setup.validate_tls'))
        stack.enter_context(patch('migration_native_endpoints.TLS_PINS',self.pins))
        self.imports=stack.enter_context(patch('migration_native_kind_setup.import_archive',
            side_effect=lambda _command,**kwargs:{'import_status':'EXACT_IMAGE_IMPORTED','artifact':kwargs['expected']}))
        stack.enter_context(patch('migration_native_kind_setup.metadata_from_archive',return_value=self.runtime_bytes))

    def campaign(self):
        session=FakeSession(self.owner.pump)
        result=NativeCampaign(self.config['artifacts'],self.owner,self.journal.record,
                              session_factory=lambda:session).run(enabled=True)
        return result,session

    def events(self):
        return [json.loads(line)['event'] for line in self.journal.path.read_text().splitlines()]

    def test_full_adapter_pipeline_switches_only_api_and_deletes_owned_resources(self):
        result,session=self.campaign()
        self.assertEqual(result['outcome'],'NATIVE_CAMPAIGN_OBSERVED',result['failures'])
        self.assertEqual(len(result['stages']),4)
        self.assertEqual(len(session.commands),6)
        self.assertEqual(self.commands.api_count,3)
        self.assertEqual(self.imports.call_count,len(self.config['images']))
        self.assertFalse(self.commands.created);self.assertFalse(self.commands.volume)
        self.assertTrue(all(f.closed for f in FakeForward.instances))
        self.assertFalse(self.owner.kubeconfig.exists())
        self.assertFalse((self.directory/'kind.private.json').exists())
        self.assertEqual(len(list(self.directory.glob('agent-documents-*.private.json'))),4)
        self.assertEqual(len(list(self.directory.glob('catalog-*.private.json'))),4)
        self.assertEqual(result['full_reader_writer_gate'],'NOT_PROVEN')
        self.assertEqual(self.commands.mongo['metadata']['uid'],'mongo-uid')
        self.assertEqual(sum('delete' in argv and argv[0]=='env' for argv,_ in self.commands.calls),1)
        for argv,_ in self.commands.calls:
            if argv[0]=='kubectl':
                self.assertNotIn('exec',argv);self.assertNotIn('secret',argv);self.assertNotIn('secrets',argv)
                self.assertNotIn('restart',argv)
        self.assertIs(self.owner.teardown(),self.owner.cleanup_receipt)

    def test_default_constructor_and_prepare_order(self):
        self.assertEqual(self.commands.calls,[])
        self.owner.prepare()
        self.assertIsNone(self.commands.api)
        self.assertIsNotNone(self.owner.mongo_pod)
        self.assertEqual(self.owner.api_template['spec']['replicas'],1)
        envs=[obj['data'] for argv,raw in self.commands.calls if raw for obj in json.loads(raw).get('items',[])
              if obj['kind']=='ConfigMap' and obj['metadata']['name']=='api-env']
        self.assertEqual(envs[0]['CHRONO_AGENT_MANAGER_REPLICAS_ENABLED'],'true')
        self.assertEqual(envs[0]['CHRONO_AGENT_CONTROL_WRITE_COORDINATION_ENABLED'],'true')
        self.assertEqual(envs[0]['CHRONO_NATIVE_PREPARED_TARGET'],'0')
        self.assertEqual(envs[0]['CHRONO_USAGE_REPORT_VIEWS_ENABLED'],'false')
        with self.assertRaises(NativeProcessError):self.owner.prepare()
        self.owner.teardown()

    def test_create_and_prepare_failures_keep_cleanup_and_do_not_start_session(self):
        self.commands.fail=lambda argv:'rollout' in argv
        result,session=self.campaign()
        self.assertEqual(result['outcome'],'INCOMPLETE')
        self.assertEqual(session.commands,[])
        self.assertFalse(self.commands.created);self.assertFalse(self.commands.volume)

    def test_failed_api_boot_has_failed_receipt_no_repair_or_later_stage(self):
        self.commands.fail=lambda argv:'rollout' in argv and 'deployment/api' in argv
        result,session=self.campaign()
        self.assertEqual(result['outcome'],'INCOMPLETE')
        self.assertEqual(self.commands.api_count,1)
        self.assertEqual(session.commands,[])
        self.assertTrue(any(e['type']=='boot_failed' for e in self.events()))
        self.assertTrue(any(e['type']=='boot_watch_event' for e in self.events()))
        self.assertFalse(self.commands.created)

    def test_rejected_document_join_stops_before_next_reader_and_keeps_snapshot(self):
        self.commands.fail=lambda argv:argv[0]=='node'
        result,session=self.campaign()
        self.assertEqual(result['outcome'],'INCOMPLETE')
        self.assertEqual(self.commands.api_count,1)
        self.assertEqual(len([op for op,_ in session.commands if op=='stage']),1)
        self.assertTrue((self.directory/'agent-documents-old-write.private.json').exists())
        self.assertFalse(self.commands.created)

    def test_mongo_provider_and_reader_replacements_are_rejected(self):
        self.owner.prepare();binding=self.owner.bind_reader('old','old-write')
        for target in (self.commands.mongo,self.commands.provider,self.commands.api):
            prior=deepcopy(target['status']['containerStatuses'][0])
            target['status']['containerStatuses'][0]['restartCount']=1
            with self.assertRaises(NativeProcessError):self.owner.observe_reader(binding)
            target['status']['containerStatuses'][0]=prior
        self.owner.teardown()

    def test_kubeconfig_changed_prevents_calls(self):
        self.owner.prepare();before=len(self.commands.calls)
        self.owner.kubeconfig.write_bytes(b'changed')
        with self.assertRaises(NativeProcessError):self.owner.get(['get','pods'])
        self.assertEqual(len(self.commands.calls),before)
        self.owner.teardown()

    def test_cleanup_refuses_foreign_node_and_preserves_failure(self):
        self.owner.prepare();self.commands.foreign_node=True
        result=self.owner.teardown()
        self.assertEqual(result['outcome'],'INCOMPLETE')
        self.assertTrue(self.commands.created)
        self.assertFalse(any(argv[0]=='env' and 'delete' in argv for argv,_ in self.commands.calls))

    def test_partial_create_failure_captures_own_node_before_cleanup(self):
        original=self.commands.__call__
        def failure(argv,**kwargs):
            value=original(argv,**kwargs)
            if argv[0]=='env' and 'create' in argv:raise RuntimeError('create command lost result')
            return value
        self.owner.command=failure
        result,session=self.campaign()
        self.assertEqual(result['outcome'],'INCOMPLETE')
        self.assertEqual(session.commands,[])
        self.assertEqual(self.owner.node_id,self.commands.node_id)
        self.assertFalse(self.commands.created);self.assertFalse(self.commands.volume)


if __name__=='__main__':unittest.main()
