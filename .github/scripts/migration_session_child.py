"""Finite private-stdio transport for one retained-Agent session.

Unknown command outcomes poison the connection. Closing stdin asks the same
session to perform public cleanup; it never starts another application request
sequence. The unreaped custody anchor reserves the process group until finality.
"""
from collections import deque
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import selectors
import subprocess
import time

from migration_redaction import EvidenceRedactor


_SPEC = importlib.util.spec_from_file_location(
    'migration_owned_process', Path(__file__).parent / 'scale-tests' / 'owned_process.py')
_CUSTODY = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(_CUSTODY)
STAGES = ('old-write', 'new-read-old-write', 'new-write', 'old-read-new-write')
LINE_LIMIT = 1024 * 1024
OUTPUT_LIMIT = 8 * 1024 * 1024
INPUT_LIMIT = 2 * 1024 * 1024


class SessionChildError(RuntimeError):
    """Fixed local error; child output and credentials never enter exceptions."""


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError('duplicate_response_key')
        result[key] = value
    return result


def reject_constant(_value):
    raise ValueError('non_json_constant')


def positive_seconds(value, maximum):
    if (isinstance(value, bool) or not isinstance(value, (int, float))
            or not math.isfinite(value) or not 0 < value <= maximum):
        raise SessionChildError('session_child_deadline_invalid')
    return value


class SessionChild:
    def __init__(self, *, node='node', protocol=None, timeout=600, cleanup_timeout=180,
                 group_factory=_CUSTODY.OwnedGroup, clock=time.monotonic, tick=lambda: None):
        self.argv = [node, str(protocol or Path(__file__).with_name('migration_agent_protocol.mjs'))]
        self.timeout = positive_seconds(timeout, 1800)
        self.cleanup_timeout = positive_seconds(cleanup_timeout, 300)
        self.clock = clock
        self.tick = tick
        self.group = group_factory()
        self.process = None
        self.selector = selectors.DefaultSelector()
        self.pending = bytearray()
        self.lines = deque()
        self.output_bytes = 0
        self.input_bytes = 0
        self.stderr_hash = hashlib.sha256()
        self.stderr_bytes = 0
        self.responses = []
        self.sequence = 0
        self.failed = False
        self.failure_code = None
        self.started = False
        self.closed = False
        self.finish_sent = False
        self.receipt = None
        self.redactor = EvidenceRedactor()

    def start(self):
        if self.started or self.closed:
            raise SessionChildError('session_child_start_once_required')
        self.started = True
        try:
            self.group.start()
            # Do not inherit deployment tokens or unrelated CLI credentials.
            environment = {key: os.environ[key] for key in ('PATH', 'TMPDIR', 'LANG') if key in os.environ}
            self.process = self.group.spawn(
                self.argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                env=environment, bufsize=0)
            for name in ('stdin', 'stdout', 'stderr'):
                os.set_blocking(getattr(self.process, name).fileno(), False)
            self.selector.register(self.process.stdout, selectors.EVENT_READ, 'stdout')
            self.selector.register(self.process.stderr, selectors.EVENT_READ, 'stderr')
        except Exception:
            self.failed = True
            self.close()
            raise SessionChildError('session_child_start_failed') from None
        return self

    def _consume(self, name, stream):
        data = os.read(stream.fileno(), 65536)
        if not data:
            self.selector.unregister(stream)
            return
        self.output_bytes += len(data)
        if self.output_bytes > OUTPUT_LIMIT:
            raise SessionChildError('session_child_output_bound')
        if name == 'stderr':
            self.stderr_hash.update(data)
            self.stderr_bytes += len(data)
            return
        self.pending.extend(data)
        while b'\n' in self.pending:
            line, _, rest = self.pending.partition(b'\n')
            self.pending = bytearray(rest)
            if not 0 < len(line) <= LINE_LIMIT:
                raise SessionChildError('session_child_line_bound')
            try:
                value = json.loads(line.decode('utf-8'), parse_constant=reject_constant,
                                   object_pairs_hook=unique_object)
            except (UnicodeDecodeError, ValueError):
                raise SessionChildError('session_child_invalid_json') from None
            if not isinstance(value, dict) or len(self.responses) + len(self.lines) >= 7:
                raise SessionChildError('session_child_response_bound')
            self.lines.append(value)
        if len(self.pending) > LINE_LIMIT:
            raise SessionChildError('session_child_line_bound')

    def _pump(self, deadline, output=None):
        if self.clock() >= deadline:
            raise SessionChildError('session_child_timeout')
        if not self.closed:
            self.tick()
        events = self.selector.select(min(.1, max(0, deadline - self.clock())))
        for key, _ in events:
            if key.data == 'stdin':
                if output:
                    count = os.write(key.fd, output[:65536])
                    del output[:count]
                if not output:
                    self.selector.unregister(key.fileobj)
            else:
                self._consume(key.data, key.fileobj)

    def _send(self, data, deadline, finish):
        output = bytearray(data)
        self.selector.register(self.process.stdin, selectors.EVENT_WRITE, 'stdin')
        while output:
            self._pump(deadline, output)
            if self.lines and output:
                # A child response before the complete command is transmitted
                # is unsolicited and must not become evidence for that command.
                raise SessionChildError('session_child_unsolicited_response')
        if finish:
            self.process.stdin.close()

    def command(self, op, payload):
        if (not self.started or self.process is None or self.failed or self.closed
                or self.finish_sent or self.sequence > 5):
            raise SessionChildError('session_child_command_refused')
        try:
            allowed = 'initialize' if self.sequence == 0 else ('finish' if self.sequence == 5 else 'stage')
            if op != allowed or not isinstance(payload, dict):
                raise SessionChildError('session_child_sequence_invalid')
            if op == 'stage' and payload.get('stage') != STAGES[self.sequence - 1]:
                raise SessionChildError('session_child_stage_order_invalid')
            if op == 'finish' and payload:
                raise SessionChildError('session_child_finish_payload_invalid')
            if op == 'initialize':
                for name in ('bearer', 'providerToken'):
                    self.redactor.remember(payload.get(name))
            # Drain already-available output before transmitting another command.
            for key, _ in self.selector.select(0):
                self._consume(key.data, key.fileobj)
            if self.lines or self.pending or self.process.poll() is not None:
                raise SessionChildError('session_child_not_idle')
            data = (json.dumps({'sequence': self.sequence, 'op': op, 'payload': payload},
                               separators=(',', ':'), allow_nan=False) + '\n').encode()
            if len(data) > 256 * 1024 or self.input_bytes + len(data) > INPUT_LIMIT:
                raise SessionChildError('session_child_input_bound')
            self.input_bytes += len(data)
            self.finish_sent = op == 'finish'
            deadline = self.clock() + self.timeout
            self._send(data, deadline, self.finish_sent)
            while not self.lines:
                self._pump(deadline)
                if not self.selector.get_map() and not self.lines:
                    raise SessionChildError('session_child_response_missing')
            response = self.lines.popleft()
            self.responses.append(response)
            expected = {'initialize': 'initialized', 'stage': 'stage', 'finish': 'finished'}[op]
            if response.get('op') == 'failed':
                raise SessionChildError('session_child_command_failed')
            if (type(response.get('sequence')) is not int or response['sequence'] != self.sequence
                    or response.get('op') != expected or self.lines):
                raise SessionChildError('session_child_response_mismatch')
            if op == 'initialize' and response != {
                    'sequence': 0, 'op': 'initialized', 'outcome': 'SESSION_INITIALIZED'}:
                raise SessionChildError('session_child_initialization_invalid')
            if op != 'initialize' and (set(response) != {'sequence', 'op', 'result'}
                                      or not isinstance(response['result'], dict)):
                raise SessionChildError('session_child_result_invalid')
            self.sequence += 1
            return response
        except Exception as error:
            self.failed = True
            self.failure_code = str(error) if type(error) is SessionChildError else 'session_child_io_failed'
            raise SessionChildError('session_child_command_unresolved') from None

    def close(self):
        """Keep external forwards alive until this returns, even after failure."""
        if self.closed:
            return self.receipt
        self.closed = True
        errors = []
        if self.process:
            if not self.process.stdin.closed:
                try:
                    self.selector.unregister(self.process.stdin)
                except KeyError:
                    pass  # stdin is registered only while a command is sending.
                self.process.stdin.close()
            deadline = self.clock() + self.cleanup_timeout
            try:
                while self.selector.get_map():
                    self._pump(deadline)
                self.process.wait(timeout=max(.001, deadline - self.clock()))
                if self.pending:
                    errors.append('session_child_truncated_output')
                self.responses.extend(self.lines)
                self.lines.clear()
            except (OSError, ValueError, SessionChildError, subprocess.TimeoutExpired):
                errors.append('session_child_finality_unconfirmed')
        group_receipt = self.group.cleanup(self.process, grace=2, kill_wait=2)
        if self.process:
            try:
                self.process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                errors.append('session_child_unjoined')
            for stream in (self.process.stdout, self.process.stderr):
                stream.close()
        self.selector.close()
        if not group_receipt.get('process_group_cleanup'):
            errors.append('session_child_group_unconfirmed')
        final = [row.get('result') for row in self.responses if row.get('op') == 'finished']
        # A late response after a timeout is evidence only. It never unpoisons
        # a command or turns an incomplete campaign into a successful journey.
        observed = (not errors and not self.failed and self.sequence == 6
                    and self.process is not None and self.process.returncode == 0
                    and len(self.responses) == 6
                    and len(final) == 1 and final[0].get('outcome') == 'SESSION_OBSERVED')
        self.receipt = {
            'schema': 'migration-session-child-finality-v1',
            'outcome': 'SESSION_OBSERVED' if observed else 'INCOMPLETE',
            'exit_code': self.process.returncode if self.process else None,
            'commands_completed': self.sequence, 'ambiguous_or_failed_command': self.failed,
            'failure_code': self.failure_code,
            'stderr_bytes': self.stderr_bytes, 'stderr_sha256': self.stderr_hash.hexdigest(),
            'responses': json.loads(self.redactor.text(json.dumps(self.responses))),
            'errors': errors, 'processes': group_receipt,
            'persisted_collection_coverage': 'NOT_PROVEN', 'full_reader_writer_gate': 'NOT_PROVEN',
        }
        return self.receipt
