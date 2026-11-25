#!/usr/bin/env python3
"""Run one reviewed, explicitly enabled native old/new/old migration campaign.

Default behavior validates private inputs only. Runtime use requires separate
tooling review and the lead's capacity release; --execute does not grant either.
"""
import argparse
import hashlib
import json
from pathlib import Path
import signal

from migration_native_campaign import NativeCampaign
from migration_native_config import load
from migration_native_kind import NativeKind
from migration_native_process import Journal, private_directory, require
from migration_session_child import SessionChild


def run(input_path, output, *, execute=False, kind_factory=NativeKind,
        session_factory=SessionChild, campaign_factory=NativeCampaign, duration=7200):
    config = load(input_path)
    plan = {'schema': 'migration-native-run-v1', 'input_sha256': config['input_sha256'],
            'artifacts': config['artifacts'], 'inventory_collections': config['inventory_collections'],
            'image_archives': len(config['archives']), 'runtime': 'NOT_RUN',
            'full_reader_writer_gate': 'NOT_PROVEN'}
    if not execute:
        return plan
    require(execute is True and type(duration) is int and 1 <= duration <= 7200,
            'native_execute_bound_invalid')
    output = Path(output).absolute()
    require(output.parent.resolve() == output.parent and not output.exists() and not output.is_symlink(),
            'native_fresh_campaign_directory_required')
    output.mkdir(mode=0o700)
    private_directory(output)
    journal = Journal(output)
    owner = None
    result = None
    interruption = None
    installed_timer = False
    finalizing = False
    installed_handlers = []
    previous_handlers = {name: signal.getsignal(name) for name in (signal.SIGALRM, signal.SIGTERM, signal.SIGINT)}
    def interrupt(_number, _frame):
        nonlocal interruption
        if finalizing:
            interruption = interruption or KeyboardInterrupt()
            return
        raise KeyboardInterrupt()
    def begin_cleanup():
        nonlocal finalizing
        finalizing = True
        if installed_timer: signal.setitimer(signal.ITIMER_REAL, 0)
    try:
        require(signal.getitimer(signal.ITIMER_REAL) == (0., 0.), 'native_existing_timer_forbidden')
        for name in previous_handlers:
            signal.signal(name, interrupt)
            installed_handlers.append(name)
        installed_timer = True
        signal.setitimer(signal.ITIMER_REAL, duration)
        journal.record({'type': 'run_intake', 'plan': plan})
        owner = kind_factory(config, output, journal)
        campaign = campaign_factory(config['artifacts'], owner, journal.record,
                                    session_factory=lambda: session_factory(tick=owner.pump),
                                    on_cleanup=begin_cleanup)
        result = campaign.run(enabled=True)
    except BaseException as error:
        interruption = error
    finally:
        # Defer further interrupts during bounded cleanup, then restore the
        # caller's handlers even if writing the final receipt fails.
        begin_cleanup()
        if owner is not None:
            try:
                cleanup = owner.teardown()
            except BaseException as error:
                if isinstance(error, (KeyboardInterrupt, SystemExit)):
                    interruption = interruption or error
                cleanup = {'outcome': 'INCOMPLETE', 'failures': ['native_final_cleanup_unconfirmed']}
        else:
            cleanup = {'outcome': 'NO_INFRASTRUCTURE_STARTED'}
        tail = {'poisoned': True}
        try:
            tail = journal.close()
        except Exception as error:
            interruption = interruption or error
        finally:
            # Only hashes/identities are public; the durable journal owns raw
            # application custody and protected document filenames.
            try:
                summary = {**plan, 'runtime': 'ATTEMPTED',
                    'outcome': result.get('outcome') if result is not None and not interruption
                        and not tail['poisoned'] and cleanup['outcome'] == 'OWNED_INFRASTRUCTURE_REMOVED' else 'INCOMPLETE',
                    'campaign': result, 'infrastructure_finality': cleanup,
                    'interrupted_or_failed': interruption is not None,
                    'journal_sha256': hashlib.sha256(journal.path.read_bytes()).hexdigest()}
                NativeKind.private_write(output/'FINAL-STATUS.json', (json.dumps(summary, indent=2)+'\n').encode())
            finally:
                for name in installed_handlers: signal.signal(name, previous_handlers[name])
    if interruption is not None:
        raise interruption
    require(not tail['poisoned'] and cleanup['outcome'] == 'OWNED_INFRASTRUCTURE_REMOVED',
            'native_run_finality_incomplete')
    return summary


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--execute', action='store_true')
    args = parser.parse_args(argv)
    if args.execute and args.output is None:
        parser.error('--output is required with --execute')
    try:
        result = run(args.input, args.output, execute=args.execute)
        public = {key: result[key] for key in ('schema', 'input_sha256', 'artifacts', 'runtime', 'full_reader_writer_gate')}
        public['outcome'] = result.get('outcome', 'INPUTS_VALIDATED_ONLY')
        print(json.dumps(public))
        return 0 if not args.execute or result.get('outcome') == 'NATIVE_CAMPAIGN_OBSERVED' else 1
    except (Exception, KeyboardInterrupt):
        print(json.dumps({'outcome': 'INCOMPLETE', 'reason': 'native_run_failed', 'full_reader_writer_gate': 'NOT_PROVEN'}))
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
