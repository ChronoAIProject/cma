"""Four-stage native campaign ordering; infrastructure is an explicit adapter.

The adapter owns image imports, boot observations, exact-Pod forwards, raw
document capture and infrastructure deletion. This engine never fabricates
those observations or infers collection compatibility from API success.
"""
from copy import deepcopy
import hashlib
import json
import re

from migration_session_child import SessionChild, STAGES


class NativeCampaignError(RuntimeError):
    pass


def require(condition, code):
    if not condition:
        raise NativeCampaignError(code)


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':'),
                                    allow_nan=False).encode()).hexdigest()


def artifact(value):
    require(isinstance(value, dict) and set(value) == {'image', 'source', 'config'},
            'campaign_artifact_shape_invalid')
    require(isinstance(value['image'], str) and re.fullmatch(
        r'(?:[a-z0-9][a-z0-9.:-]*/)+[a-z0-9][a-z0-9._/-]*@sha256:[a-f0-9]{64}', value['image'])
        and isinstance(value['source'], str) and re.fullmatch('[a-f0-9]{40}', value['source'])
        and isinstance(value['config'], str) and re.fullmatch('sha256:[a-f0-9]{64}', value['config']),
        'campaign_immutable_artifact_required')
    return deepcopy(value)


def stage_binding(value, stage, expected, previous):
    require(isinstance(value, dict) and set(value) == {'stage', 'origin', 'reader', 'boot'}
            and value['stage'] == stage, 'campaign_stage_binding_invalid')
    reader = value['reader']
    require(isinstance(reader, dict) and artifact(reader.get('artifact')) == expected,
            'campaign_reader_artifact_mismatch')
    require(isinstance(value['origin'], str)
            and re.fullmatch(r'http://127\.0\.0\.1:[1-9][0-9]{0,4}', value['origin']),
            'campaign_loopback_required')
    require(int(value['origin'].rsplit(':', 1)[1]) <= 65535, 'campaign_loopback_required')
    if stage == 'new-write':
        require(value['boot'] is None and previous is not None
                and reader == previous['reader'] and value['origin'] == previous['origin'],
                'campaign_reader_changed_without_boot')
    else:
        boot = value['boot']
        require(isinstance(boot, dict) and boot.get('schema') == 'cma-migration-boot-observation-v1'
                and boot.get('failed_boots') == 0 and boot.get('selected_uid') == reader.get('podUid')
                and isinstance(boot.get('observations'), list) and bool(boot['observations'])
                and reader.get('podUid') in boot.get('attempted_uids', [])
                and boot.get('attempted_uids') == boot.get('ready_uids'),
                'campaign_clean_boot_required')
        if previous:
            require(reader.get('podUid') != previous['reader'].get('podUid'),
                    'campaign_reader_uid_reused')
    # The session independently validates each boot event before any write.
    return deepcopy(value)


class NativeCampaign:
    """Explicit one-shot workflow; no constructor/startup side effects.

    Infrastructure methods:
      prepare(): verifies exclusive local custody and dependencies, no API writes
      bind_reader(role, stage): switches/boots only when required; binds forward
      session_config(binding): exact selected artifacts and private credentials
      observe_reader(binding): independently reads the bound ready Pod/container
      capture_documents(stage, binding, session_result): bounded real raw capture
      teardown(): disposes owned forwards/cluster/credentials, returns finality
    record(event) must durably retain each private event before the next action.
    """
    def __init__(self, artifacts, infrastructure, record, *, session_factory=SessionChild,
                 on_cleanup=lambda: None):
        require(isinstance(artifacts, dict) and set(artifacts) == {'old', 'new', 'runtime'},
                'campaign_artifacts_required')
        self.artifacts = {key: artifact(value) for key, value in artifacts.items()}
        require(self.artifacts['old']['image'] != self.artifacts['new']['image'],
                'campaign_distinct_readers_required')
        self.infrastructure = infrastructure
        self.record = record
        self.session_factory = session_factory
        self.on_cleanup = on_cleanup
        self.started = False

    def run(self, *, enabled=False):
        require(enabled is True, 'campaign_explicit_opt_in_required')
        require(not self.started, 'campaign_once_required')
        self.started = True
        session = None
        stages, failures = [], []
        session_finality = None
        infrastructure_finality = None
        last = None
        interruption = None
        try:
            self.record({'type': 'campaign_started', 'artifacts': self.artifacts})
            self.infrastructure.prepare()
            for index, stage in enumerate(STAGES):
                role = 'old' if index in (0, 3) else 'new'
                self.record({'type': 'reader_bind_intent', 'stage': stage,
                             'artifact': self.artifacts[role]})
                # Record even rejected boot observations: a failed rollback is
                # part of the matrix, not an exception to be hidden by teardown.
                binding = self.infrastructure.bind_reader(role, stage)
                self.record({'type': 'reader_binding_observed', 'stage': stage, 'binding': binding})
                binding = stage_binding(binding, stage, self.artifacts[role], last)
                before = self.infrastructure.observe_reader(binding)
                self.record({'type': 'reader_before_stage', 'stage': stage, 'reader': before})
                if session is None:
                    config = self.infrastructure.session_config(binding)
                    require(config.get('artifacts') == self.artifacts
                            and config.get('origin') == binding['origin'],
                            'campaign_session_binding_mismatch')
                    session = self.session_factory()
                    session.start()
                    session.command('initialize', config)
                self.record({'type': 'session_stage_intent', 'stage': stage})
                response = session.command('stage', binding)
                result = response.get('result', {})
                self.record({'type': 'session_stage_observed', 'stage': stage, 'response': response})
                require(result.get('schema') == 'migration-agent-session-stage-v1'
                        and result.get('stage') == stage
                        and result.get('result', {}).get('artifact') == self.artifacts[role]
                        and result['result'].get('outcome') == 'API_JOURNEY_OBSERVED'
                        and result['result'].get('persisted_collection_coverage') == 'NOT_PROVEN'
                        and result['result'].get('full_reader_writer_gate') == 'NOT_PROVEN',
                        'campaign_session_observation_invalid')
                after_session = self.infrastructure.observe_reader(binding)
                self.record({'type': 'reader_after_session', 'stage': stage, 'reader': after_session})
                require(before == after_session, 'campaign_reader_changed_during_session')
                documents = self.infrastructure.capture_documents(stage, binding, result)
                self.record({'type': 'documents_observed', 'stage': stage, 'documents': documents})
                require(isinstance(documents, dict) and documents.get('stage') == stage
                        and documents.get('artifact') == self.artifacts[role]
                        and documents.get('reader_sha256') == digest(before)
                        and documents.get('outcome') == 'DOCUMENTS_OBSERVED'
                        and documents.get('compatibility') == 'NOT_PROVEN',
                        'campaign_document_binding_invalid')
                after_documents = self.infrastructure.observe_reader(binding)
                self.record({'type': 'reader_after_documents', 'stage': stage, 'reader': after_documents})
                require(after_documents == before, 'campaign_reader_changed_during_capture')
                stages.append({'stage': stage, 'artifact': self.artifacts[role],
                               'reader_sha256': digest(before), 'session_sha256': digest(result),
                               'documents': documents})
                last = binding
            session.command('finish', {})
        except (KeyboardInterrupt, SystemExit) as error:
            interruption = error
            failures.append('campaign_interrupted')
        except Exception as error:
            # Infrastructure command errors may contain private API bodies;
            # only our own enumerated reason is eligible for public status.
            reason = str(error) if type(error) is NativeCampaignError else 'campaign_operation_failed'
            failures.append(reason)
        finally:
            # The CLI defers repeat interrupts during bounded public/process
            # cleanup. This hook also runs when a stage failed before a session.
            try:
                self.on_cleanup()
            except BaseException as error:
                if isinstance(error, (KeyboardInterrupt, SystemExit)): interruption = error
                failures.append('campaign_cleanup_hook_failed')
            if session is not None:
                try:
                    session_finality = session.close()
                    require(session_finality.get('outcome') == 'SESSION_OBSERVED'
                            and session_finality.get('exit_code') == 0
                            and session_finality.get('processes', {}).get('process_group_cleanup') is True,
                            'campaign_session_incomplete')
                except Exception:
                    failures.append('campaign_session_incomplete')
                try:
                    self.record({'type': 'session_finality', 'receipt': session_finality})
                except Exception:
                    failures.append('campaign_finality_record_failed')
            # Always run after session EOF/cleanup, never before. A dead reader
            # can prevent public cleanup; deleting its cluster does not fix that.
            try:
                infrastructure_finality = self.infrastructure.teardown()
                require(infrastructure_finality.get('outcome') == 'OWNED_INFRASTRUCTURE_REMOVED',
                        'campaign_infrastructure_incomplete')
            except Exception:
                failures.append('campaign_infrastructure_incomplete')
        result = {
            'schema': 'migration-native-campaign-v1', 'artifacts': self.artifacts, 'stages': stages,
            'outcome': 'NATIVE_CAMPAIGN_OBSERVED' if len(stages) == 4 and not failures else 'INCOMPLETE',
            'failures': failures, 'session': session_finality, 'infrastructure': infrastructure_finality,
            'persisted_collection_coverage': 'NOT_PROVEN', 'full_reader_writer_gate': 'NOT_PROVEN',
        }
        # If final evidence cannot be persisted, let the caller fail. It must
        # never print a successful result whose durable receipt does not exist.
        self.record({'type': 'campaign_finality', 'receipt': result})
        if interruption is not None:
            raise interruption
        return result
