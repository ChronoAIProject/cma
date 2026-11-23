"""Bounded commands and exact-Pod forwards for an exclusive local campaign."""
import hashlib
import json
import os
from pathlib import Path
import re
import selectors
import subprocess
import time

from migration_redaction import EvidenceRedactor
from migration_session_child import _CUSTODY, positive_seconds


class NativeProcessError(RuntimeError):
    pass


def require(value, code):
    if not value:
        raise NativeProcessError(code)


def private_directory(path):
    path = Path(path).absolute()
    require(path.resolve() == path and path.is_dir() and path.stat().st_uid == os.getuid()
            and not path.stat().st_mode & 0o077, 'native_private_directory_required')
    return path


class Journal:
    def __init__(self, directory, name='native-campaign.private.jsonl'):
        path = private_directory(directory) / name
        require(path.name == name and re.fullmatch('[a-z0-9.-]+', name), 'native_journal_name_invalid')
        self.file = os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600), 'wb')
        self.path = path
        self.previous = None
        self.sequence = 0
        self.bytes = 0
        self.poisoned = False
        self.redactor = EvidenceRedactor()

    def record(self, value):
        require(not self.poisoned and not self.file.closed, 'native_journal_poisoned')
        try:
            value = json.loads(self.redactor.text(json.dumps(value, allow_nan=False)))
            row = {'sequence': self.sequence, 'previous': self.previous, 'event': value}
            raw = json.dumps(row, sort_keys=True, separators=(',', ':'), allow_nan=False).encode()
            require(len(raw) <= 4*1024**2, 'native_journal_event_bound')
            checksum = hashlib.sha256(raw).hexdigest()
            line = json.dumps({**row, 'sha256': checksum}, separators=(',', ':')).encode() + b'\n'
            require(self.bytes + len(line) <= 64*1024**2, 'native_journal_total_bound')
            self.file.write(line); self.file.flush(); os.fsync(self.file.fileno())
            self.previous = checksum; self.sequence += 1; self.bytes += len(line)
        except BaseException:
            self.poisoned = True
            raise

    def close(self):
        self.file.close()
        return {'records': self.sequence, 'tail_sha256': self.previous, 'bytes': self.bytes,
                'poisoned': self.poisoned}


def environment():
    # Docker context lookup needs its normal home; unrelated cloud credentials
    # and implicit Kubernetes/Docker endpoint overrides are not inherited.
    return {name: os.environ[name] for name in ('PATH', 'HOME', 'TMPDIR', 'LANG', 'DOCKER_CONFIG')
            if name in os.environ}


class Command:
    def __init__(self, record, *, tick=lambda: None, group_factory=_CUSTODY.OwnedGroup):
        self.record = record
        self.tick = tick
        self.group_factory = group_factory
        self.sequence = 0

    def __call__(self, argv, *, input_text=None, timeout=60, max_bytes=36*1024**2):
        positive_seconds(timeout, 600)
        require(isinstance(argv, list) and argv and all(isinstance(v, str) and '\0' not in v for v in argv),
                'native_command_argv_invalid')
        data = input_text.encode() if isinstance(input_text, str) else (input_text or b'')
        require(isinstance(data, bytes) and len(data) <= 16*1024**2, 'native_command_input_bound')
        require(type(max_bytes) is int and 0 < max_bytes <= 36*1024**2, 'native_command_output_bound')
        number = self.sequence; self.sequence += 1
        # Input can carry fixture Secret manifests; retain only its size.
        self.record({'type': 'command_intent', 'sequence': number, 'argv': argv,
                     'stdin_bytes': len(data)})
        group = self.group_factory(); process = None
        output, stderr = bytearray(), bytearray()
        error = None; cleanup = None
        selector = selectors.DefaultSelector()
        try:
            group.start()
            process = group.spawn(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, env=environment(), bufsize=0)
            pending = bytearray(data)
            for name in ('stdin', 'stdout', 'stderr'):
                os.set_blocking(getattr(process, name).fileno(), False)
            for name in ('stdout', 'stderr'):
                selector.register(getattr(process, name), selectors.EVENT_READ, name)
            if pending: selector.register(process.stdin, selectors.EVENT_WRITE, 'stdin')
            else: process.stdin.close()
            deadline = time.monotonic() + timeout
            while selector.get_map():
                self.tick()
                require(time.monotonic() < deadline, 'native_command_timeout')
                for key, _ in selector.select(min(.1, max(0, deadline-time.monotonic()))):
                    if key.data == 'stdin':
                        written = os.write(key.fd, pending[:65536]);del pending[:written]
                        if not pending: selector.unregister(key.fileobj);process.stdin.close()
                    else:
                        chunk = os.read(key.fd, 65536)
                        if not chunk: selector.unregister(key.fileobj)
                        else:
                            (output if key.data == 'stdout' else stderr).extend(chunk)
                            require(len(output)+len(stderr) <= max_bytes, 'native_command_output_bound')
            process.wait(timeout=max(.001, deadline-time.monotonic()))
            require(process.returncode == 0, 'native_command_nonzero')
        except BaseException as caught:
            error = caught
        finally:
            selector.close()
            cleanup = group.cleanup(process, grace=2, kill_wait=2)
            if process:
                try: process.wait(timeout=2)
                except subprocess.TimeoutExpired: error = NativeProcessError('native_command_unjoined')
                for stream in (process.stdin, process.stdout, process.stderr): stream.close()
        if not cleanup['process_group_cleanup']: error = NativeProcessError('native_command_group_unconfirmed')
        self.record({'type': 'command_finality', 'sequence': number,
                     'exit_code': process.returncode if process else None, 'processes': cleanup,
                     'stdout_bytes': len(output), 'stdout_sha256': hashlib.sha256(output).hexdigest(),
                     'stderr_bytes': len(stderr), 'stderr_sha256': hashlib.sha256(stderr).hexdigest(),
                     'outcome': 'FAILED' if error else 'EXIT_ZERO'})
        if isinstance(error, (KeyboardInterrupt, SystemExit)): raise error
        if error: raise NativeProcessError(str(error) if type(error) is NativeProcessError else 'native_command_failed') from None
        return bytes(output)


class Forward:
    def __init__(self, argv, remote_port, record, *, group_factory=_CUSTODY.OwnedGroup):
        require(type(remote_port) is int and 1 <= remote_port <= 65535, 'native_forward_port_invalid')
        self.argv = list(argv)
        self.remote_port = remote_port
        self.record = record
        self.group = group_factory()
        self.process = None
        self.selector = selectors.DefaultSelector()
        self.closed = False
        self.port = None
        self.bytes = 0
        self.checksum = hashlib.sha256()
        self.pending = bytearray()
        self.receipt = None

    def start(self):
        require(self.process is None and not self.closed, 'native_forward_once_required')
        self.record({'type': 'forward_intent', 'argv': self.argv, 'remote_port': self.remote_port})
        try:
            self.group.start()
            self.process = self.group.spawn(self.argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                           stderr=subprocess.PIPE, env=environment(), bufsize=0)
            for name in ('stdout', 'stderr'):
                stream = getattr(self.process, name);os.set_blocking(stream.fileno(), False)
                self.selector.register(stream, selectors.EVENT_READ, name)
            deadline = time.monotonic()+20
            while self.port is None:
                require(time.monotonic() < deadline, 'native_forward_timeout')
                self.pump(.05)
            self.record({'type': 'forward_ready', 'port': self.port, 'remote_port': self.remote_port})
            return self
        except BaseException:
            self.close()
            raise

    def pump(self, wait=0):
        if self.closed: return
        require(self.process is not None and self.process.poll() is None, 'native_forward_exited')
        for key, _ in self.selector.select(wait):
            data = os.read(key.fd, 65536)
            if not data:
                self.selector.unregister(key.fileobj)
                continue
            self.bytes += len(data);self.checksum.update(data)
            require(self.bytes <= 4*1024**2, 'native_forward_output_bound')
            if self.port is None and key.data == 'stdout':
                self.pending.extend(data)
                require(len(self.pending) <= 4096, 'native_forward_startup_bound')
                while b'\n' in self.pending:
                    line, _, remainder = self.pending.partition(b'\n');self.pending = bytearray(remainder)
                    match = re.fullmatch(rb'Forwarding from 127\.0\.0\.1:([0-9]+) -> ([0-9]+)', line)
                    require(match is not None and int(match[2]) == self.remote_port
                            and 1 <= int(match[1]) <= 65535, 'native_forward_binding_invalid')
                    self.port = int(match[1]);self.pending.clear();break
        require(self.process.poll() is None, 'native_forward_exited')

    def close(self):
        if self.closed: return self.receipt
        self.closed = True
        finality = self.group.cleanup(self.process, grace=2, kill_wait=2)
        if self.process:
            try: self.process.wait(timeout=2)
            except subprocess.TimeoutExpired: finality['process_group_cleanup'] = False
            self.process.stdout.close();self.process.stderr.close()
        self.selector.close()
        self.receipt = {'type': 'forward_finality', 'port': self.port, 'remote_port': self.remote_port,
                        'output_bytes': self.bytes, 'output_sha256': self.checksum.hexdigest(),
                        'processes': finality}
        self.record(self.receipt)
        return self.receipt
