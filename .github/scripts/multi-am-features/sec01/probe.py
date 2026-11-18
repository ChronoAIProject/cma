#!/usr/bin/env python3
"""Run inside an authorized tenant runtime. JSON stdin/output; no credentials.

This is a primitive, not an isolation verdict. The parent joins observations to
owner-side target existence, policy decisions, lifecycle and serving receipts.
Never invoke this with kubectl exec or substitute the host for the tenant.
"""
import errno
import hashlib
import json
import os
import socket
import stat
import sys
import time

MAX_INPUT = 65536
MAX_READ = 4096


def require(value, code):
    if not value:
        raise ValueError(code)


def file_probe(probe):
    path = probe['path']
    require(isinstance(path, str) and path and '\0' not in path, 'invalid_path')
    require(probe.get('synthetic') is True, 'synthetic_canary_required')
    rows = []
    # No create/truncate/write: opening for write is already an isolation failure.
    for operation in ['read', 'enumerate', 'write_open']:
        row = {'operation': operation, 'succeeded': False, 'errno': None}
        try:
            if operation == 'enumerate':
                with os.scandir(probe['directory']) as entries:
                    row['entry_count'] = sum(1 for _ in entries)
                row['succeeded'] = True
            else:
                flags = os.O_RDONLY if operation == 'read' else os.O_WRONLY
                # Nonblocking prevents a misplaced FIFO from hanging the run.
                fd = os.open(path, flags | os.O_NONBLOCK)
                row['succeeded'] = True
                try:
                    if operation == 'read':
                        require(stat.S_ISREG(os.fstat(fd).st_mode), 'canary_not_regular')
                        data = os.read(fd, MAX_READ + 1)
                        row.update(bytes=len(data), sha256=hashlib.sha256(data).hexdigest(),
                                   truncated=len(data) > MAX_READ)
                finally:
                    os.close(fd)
        except OSError as error:
            row['errno'] = error.errno
        except ValueError:
            row['invalid_canary_type'] = True
        rows.append(row)
    return {'kind': 'file', 'operations': rows}


def tcp_probe(probe):
    host, port = probe['host'], probe['port']
    require(isinstance(host, str) and host and len(host) <= 253, 'invalid_host')
    require(type(port) is int and 0 < port <= 65535, 'invalid_port')
    family = {'ipv4': socket.AF_INET, 'ipv6': socket.AF_INET6, 'dns': socket.AF_UNSPEC}[probe['family']]
    rows = []
    try:
        targets = socket.getaddrinfo(host, port, family, socket.SOCK_STREAM)
    except OSError as error:
        return {'kind': 'tcp', 'resolution_errno': error.errno, 'addresses': []}
    seen = set()
    for af, socktype, protocol, _, address in targets:
        if (af, address) in seen:
            continue
        seen.add((af, address))
        row = {'family': 'ipv6' if af == socket.AF_INET6 else 'ipv4',
               'address_sha256': hashlib.sha256(str(address).encode()).hexdigest(),
               'connected': False, 'errno': None}
        started = time.monotonic()
        with socket.socket(af, socktype, protocol) as connection:
            connection.settimeout(min(float(probe.get('timeout_seconds', 2)), 5))
            try:
                connection.connect(address)
                row['connected'] = True
                # No request or token endpoint is needed to prove TCP exposure.
            except OSError as error:
                row['errno'] = error.errno or errno.ETIMEDOUT
        row['elapsed_ms'] = (time.monotonic() - started) * 1000
        rows.append(row)
    return {'kind': 'tcp', 'addresses': rows}


def main():
    raw = sys.stdin.buffer.read(MAX_INPUT + 1)
    require(len(raw) <= MAX_INPUT, 'input_bound')
    packet = json.loads(raw)
    require(packet.get('schema') == 'sec01-probe-v1', 'schema')
    require(isinstance(packet.get('nonce'), str) and 16 <= len(packet['nonce']) <= 128, 'nonce')
    probes = packet['probes']
    require(isinstance(probes, list) and 0 < len(probes) <= 128, 'probe_count')
    require(len({p['id'] for p in probes}) == len(probes), 'duplicate_probe')
    results = []
    for probe in probes:
        require(isinstance(probe['id'], str) and len(probe['id']) <= 128, 'probe_id')
        started = time.time()
        if probe['kind'] == 'tcp':
            result = tcp_probe(probe)
        elif probe['kind'] == 'file':
            result = file_probe(probe)
        else:
            raise ValueError('unsupported_probe')
        results.append({'id': probe['id'], 'started_unix': started,
                        'ended_unix': time.time(), **result})
    print(json.dumps({'schema': 'sec01-observation-v1', 'nonce': packet['nonce'],
                      'probe_sha256': hashlib.sha256(raw).hexdigest(), 'observations': results}))


if __name__ == '__main__':
    try:
        main()
    except (ValueError, KeyError, TypeError, OverflowError):
        # Never echo paths, input JSON, tokens, environment or exception text.
        print(json.dumps({'error': 'invalid_probe_input'}))
        sys.exit(2)
