#!/usr/bin/env python3
"""Start the real shared-Mongo suite and publish only allowlisted summaries."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import sys
import time
import urllib.error

from desktop_policy import install as install_desktop_policy
from diagnostics import initialization_timeout
from fixture_evidence import summarize as summarize_fixture
from infrastructure import Infrastructure
from release_images import checked_packet
from stage_evidence import summarize as summarize_stages
from setup_evidence import capture as capture_setup
from tls_fixture import prepare as prepare_tls

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / 'owner'))
from common import utc, write
from fixture_no_secrets import LocalFixture


def run(args):
    os.umask(0o077)
    if not re.fullmatch(r'[0-9a-f]{40}', args.revision):
        raise ValueError('full_checkout_revision_required')
    release_packet = None
    if args.release_packet:
        if args.artifacts or args.run_id or args.attempt:
            raise ValueError('image_input_modes_are_exclusive')
        release_packet = checked_packet(args.release_packet, args.packet_sha256, args.revision)
        args.artifacts = args.release_packet.parent
    elif (args.packet_sha256 or not args.artifacts or not all(
            re.fullmatch(r'[1-9][0-9]*', x or '') for x in [args.run_id, args.attempt])):
        raise ValueError('ci_run_identity_required')
    args.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    private, summary = args.output / 'private', args.output / 'summary'
    private.mkdir(mode=0o700); summary.mkdir(mode=0o700)
    infrastructure = Infrastructure(private)
    fixture = None
    result = {'schema': 'multi-am-ci-run-v1', 'source': args.revision,
              'fixture': args.fixture, 'verdict': 'RUNNING', 'started_at': utc(), 'cleanup': None}
    result['image_input'] = {'mode': 'frozen_release_packet' if release_packet else 'same_ci_invocation',
                            'packet_sha256': args.packet_sha256, 'platform': 'linux/amd64',
                            'execution': 'native_amd64_required'}

    def interrupted(signum, _):
        raise RuntimeError('fixture_interrupted_' + str(signum))

    for sig in [signal.SIGTERM, signal.SIGINT]:
        signal.signal(sig, interrupted)
    try:
        result['stage'] = 'public_tls_fixture'
        tls_directory = prepare_tls(private / 'tls')
        result['stage'] = 'shared_cluster_and_images'
        ready = infrastructure.setup(args.artifacts, args.revision, args.run_id, args.attempt, release_packet)
        result['image_input']['manifests'] = {
            'api': ready['expected_api_image'].split('@', 1)[1],
            'runtime': ready['runtime_images']['base'].split('@', 1)[1],
            'runtime-omarchy': ready['runtime_images']['omarchy'].split('@', 1)[1],
        }
        ready['tls_directory'] = str(tls_directory)
        ready['fixture'] = args.fixture
        if args.fixture == 'features':
            ready['desktop_admission_policy'] = install_desktop_policy(
                infrastructure, ready['runtime_images']['omarchy'], ready['runtime_images']['base'])
        write(private / 'ready.private.json', ready)
        fixture = LocalFixture(ready, private)
        result['stage'] = 'two_am_initialization'
        fixture.initialize()
        config = {'am_origins': fixture.origins, 'bearer': fixture.secret['bearer'],
            'admin_bearer': fixture.secret['admin_bearer'], 'owner_id': 'native-am-fixture-owner',
            'outsider_bearer': fixture.secret['outsider_bearer'],
            'host': 'api.localhost.direct', 'public_origin': 'https://api.localhost.direct',
            'expected_api_image': ready['expected_api_image'], 'expected_api_source': args.revision,
            'runtime_images': ready['runtime_images'], 'runtime_sources': ready['runtime_sources'],
            'adapter_module': str(ROOT / 'owner/adapter.mjs'),
            'control_origin': fixture.serve(), 'control_token': fixture.control_token,
            'output_dir': str(private / 'journey'), 'matrix_output': str(private / 'matrix'),
            'summary_dir': str(summary), 'run_id': ready['run_id'], 'fixture': args.fixture,
            'browser_session_cookie': fixture.browser_cookie}
        if args.fixture == 'features':
            config['identity_origin'] = fixture.client.forward('identity', 8080)
        result['processes_ready_at'] = utc()
        result['stage'] = 'feature_journey'
        script = 'prepared_suite.mjs' if args.fixture == 'prepared' else 'suite.mjs'
        code, output = fixture.client.command(['node', str(ROOT / script)],
                                               data=json.dumps(config), timeout=2400, check=False)
        (private / 'suite.private.log').write_text(output)
        result['suite_exit_code'] = code
        coverage_file = summary / 'coverage.json'
        if not coverage_file.is_file():
            raise RuntimeError('suite_coverage_artifact_missing')
        verdict = json.loads(coverage_file.read_text())
        result['verdict'] = 'PASS' if code == 0 and verdict['suite_verdict'] == 'PASS' else 'FAIL'
    except Exception as error:
        result['verdict'] = 'FAIL'
        # Detailed exceptions are private; published output cannot include URLs,
        # credentials, resource identities or raw API/fixture payloads.
        (private / 'error.private.txt').write_text(type(error).__name__ + ': ' + str(error))
        result['failure_class'] = type(error).__name__
        if fixture and result['stage'] == 'two_am_initialization':
            result['initialization_step'] = fixture.initialization_step
        if initialization_timeout(error):
            result['failure_code'] = initialization_timeout(error)
        elif isinstance(error, urllib.error.HTTPError):
            result['http_status'] = error.code
        elif isinstance(error, (ValueError, RuntimeError)) and re.fullmatch('[a-z][a-z0-9_]{0,100}', str(error)):
            result['failure_code'] = str(error)
    finally:
        # Capture the initiating infrastructure stage before cleanup changes it.
        result['infrastructure_stage'] = infrastructure.stage
        result['infrastructure_failure'] = infrastructure.failed_command
        if infrastructure.created and infrastructure.node_id:
            try:
                result['setup_evidence'] = capture_setup(infrastructure)
            except Exception:
                result['setup_capture_failed'] = True
        if fixture:
            fixture.client.deadline = max(fixture.client.deadline, time.monotonic() + 120)
            try:
                if fixture.server:
                    try:
                        if fixture.thread and fixture.thread.is_alive():
                            fixture.server.shutdown()
                    finally:
                        fixture.server.server_close()
                if fixture.thread and fixture.thread.ident is not None:
                    fixture.thread.join(timeout=5)
                result['fixture_control_thread_joined'] = not fixture.thread or not fixture.thread.is_alive()
            except Exception as error:
                result['fixture_control_thread_joined'] = False
                result['fixture_control_cleanup_failure_class'] = type(error).__name__
            try:
                fixture.capture()
            except Exception:
                result['capture_failed'] = True
            try:
                result['fixture_evidence'] = summarize_fixture(private)
            except Exception:
                result['fixture_evidence_failed'] = True
            try:
                result['scheduler_stage_evidence'] = summarize_stages(private)
            except Exception:
                result['stage_evidence_failed'] = True
            try:
                result['fixture_children_joined'] = fixture.client.close()
            except Exception as error:
                result['fixture_children_joined'] = False
                result['fixture_child_cleanup_failure_class'] = type(error).__name__
        try:
            result['cleanup'] = infrastructure.cleanup()
        except Exception as error:
            result['cleanup'] = {'failed': True, 'failure_class': type(error).__name__}
            result['verdict'] = 'FAIL'
        if result.get('fixture_children_joined') is False or result.get('fixture_control_thread_joined') is False:
            result['verdict'] = 'FAIL'
        # Hashes only; raw files and paths are deliberately not uploaded.
        hashes = []
        for path in sorted(private.rglob('*')):
            if path.is_file() and not path.is_symlink():
                with path.open('rb') as source:
                    hashes.append(hashlib.file_digest(source, 'sha256').hexdigest())
        result['private_evidence_sha256'] = hashes
        result['infrastructure_commands'] = infrastructure.command_outcomes
        result['finished_at'] = utc()
        write(summary / 'result.json', result)
        print(json.dumps({k: result[k] for k in ['verdict', 'source', 'cleanup']}))
    return 0 if result['verdict'] == 'PASS' else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--artifacts', type=Path)
    parser.add_argument('--release-packet', type=Path)
    parser.add_argument('--packet-sha256')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--revision', required=True)
    parser.add_argument('--run-id')
    parser.add_argument('--attempt')
    parser.add_argument('--fixture', choices=('features', 'prepared'), default='features')
    raise SystemExit(run(parser.parse_args()))
