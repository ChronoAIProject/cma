import test from 'node:test';
import assert from 'node:assert/strict';
import {spawnSync} from 'node:child_process';
import {mkdtempSync, writeFileSync, rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {runSec01} from './runner.mjs';
import {OPTIONS, passingFixture} from './test-support.mjs';

const script = new URL('./probe.py', import.meta.url).pathname;
function run(packet) {
  const child = spawnSync('python3', [script], {input: JSON.stringify(packet), encoding: 'utf8'});
  return {status: child.status, value: JSON.parse(child.stdout)};
}

test('bounded file primitive returns hash and no file content', () => {
  const directory = mkdtempSync(join(tmpdir(), 'sec01-'));
  try {
    const path = join(directory, 'owner-canary');
    writeFileSync(path, 'synthetic-private-marker');
    const value = run({schema: 'sec01-probe-v1', nonce: 'nonce-1234567890', probes: [{
      id: 'owner', kind: 'file', path, directory, synthetic: true,
    }]});
    assert.equal(value.status, 0);
    assert.equal(value.value.schema, 'sec01-observation-v1');
    const observation = value.value.observations[0];
    const read = observation.operations.find(row => row.operation === 'read');
    assert.equal(read.succeeded, true);
    assert.equal(read.bytes, 24);
    assert.equal(read.sha256.length, 64);
    assert.ok(!JSON.stringify(value.value).includes('synthetic-private-marker'));
  } finally { rmSync(directory, {recursive: true, force: true}); }
});

test('invalid and oversized packets fail closed without echoing input', () => {
  const invalid = spawnSync('python3', [script], {input: JSON.stringify({schema: 'wrong', secret: 'do-not-print'}), encoding: 'utf8'});
  assert.equal(invalid.status, 2);
  assert.equal(invalid.stdout.trim(), '{"error": "invalid_probe_input"}');
  const oversized = spawnSync('python3', [script], {input: 'x'.repeat(65537), encoding: 'utf8'});
  assert.equal(oversized.status, 2);
  assert.equal(oversized.stdout.trim(), '{"error": "invalid_probe_input"}');
});

test('the primitive classifies actual socket errors before the portable evidence boundary', () => {
  const child = spawnSync('python3', ['-B', '-c', `
import errno, importlib.util, json, socket, sys
from unittest.mock import MagicMock, patch
spec = importlib.util.spec_from_file_location('probe', sys.argv[1])
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)
results = []
target = (socket.AF_INET6, socket.SOCK_STREAM, 6, '', ('::1', 9, 0, 0))
request = {'host': 'synthetic.invalid', 'port': 9, 'family': 'ipv6'}
for number in [110, 60]:
    connection = MagicMock()
    connection.__enter__.return_value.connect.side_effect = OSError(number, 'synthetic timeout')
    with patch.object(probe.errno, 'ETIMEDOUT', number), \\
         patch.object(probe.socket, 'getaddrinfo', return_value=[target]), \\
         patch.object(probe.socket, 'socket', return_value=connection):
        results.append(probe.tcp_probe(request))
for error in [TimeoutError(), OSError(errno.EACCES, 'synthetic refusal'), OSError(errno.EIO, 'synthetic error')]:
    connection = MagicMock()
    connection.__enter__.return_value.connect.side_effect = error
    with patch.object(probe.socket, 'getaddrinfo', return_value=[target]), \\
         patch.object(probe.socket, 'socket', return_value=connection):
        results.append(probe.tcp_probe(request))
with patch.object(probe.socket, 'getaddrinfo', return_value=[target]), \\
     patch.object(probe.socket, 'socket', side_effect=OSError(errno.EAFNOSUPPORT, 'unsupported')):
    results.append(probe.tcp_probe(request))
with patch.object(probe.socket, 'getaddrinfo', side_effect=socket.gaierror(-2, 'unavailable')):
    results.append(probe.tcp_probe(request))
connection = MagicMock()
with patch.object(probe.socket, 'getaddrinfo', return_value=[target]), \\
     patch.object(probe.socket, 'socket', return_value=connection):
    results.append(probe.tcp_probe(request))
print(json.dumps(results))
`, script], {encoding: 'utf8'});
  assert.equal(child.status, 0, child.stderr);
  const rows = JSON.parse(child.stdout);
  assert.deepEqual(rows.slice(0, 6).map(row => row.addresses[0].failure_class),
    ['timeout', 'timeout', 'timeout', 'refused', 'error', 'unavailable']);
  assert(rows.every(row => row.requested_family === 'ipv6'));
  assert(rows.slice(0, 6).every(row => row.addresses[0].family === 'ipv6'));
  assert.equal(rows[6].failure_class, 'unavailable');
  assert.deepEqual(rows[6].addresses, []);
  assert.equal(rows[7].addresses[0].connected, true);
  assert(!child.stdout.includes('synthetic timeout'));
});

function fileErrorObservations() {
  const child = spawnSync('python3', ['-B', '-c', `
import errno, importlib.util, json, sys
from unittest.mock import patch
spec = importlib.util.spec_from_file_location('probe', sys.argv[1])
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)
request = {'path': '/synthetic/canary', 'directory': '/synthetic', 'synthetic': True}
cases = [(110, 'timeout'), (60, 'timeout'), (errno.EIO, 'error'), (0, 'error'),
         (errno.EACCES, 'permission_denied'), (errno.EPERM, 'permission_denied'),
         (errno.ENOENT, 'not_found'), (errno.ENODEV, 'unavailable'),
         (errno.ECONNREFUSED, 'error')]
results = []
for number, expected in cases:
    error = OSError(number, 'synthetic file failure')
    with patch.object(probe.errno, 'ETIMEDOUT', number if expected == 'timeout' else errno.ETIMEDOUT), \\
         patch.object(probe.os, 'open', side_effect=error), \\
         patch.object(probe.os, 'scandir', side_effect=error):
        results.append({'expected': expected, 'errno': number, 'observation': probe.file_probe(request)})
print(json.dumps(results))
`, script], {encoding: 'utf8'});
  assert.equal(child.status, 0, child.stderr);
  assert(!child.stdout.includes('synthetic file failure'));
  assert(!child.stdout.includes('/synthetic'));
  return JSON.parse(child.stdout);
}

test('file primitive distinguishes permission denial from timeout and storage failures', () => {
  for (const row of fileErrorObservations()) {
    assert.deepEqual(row.observation.operations.map(operation => operation.operation), ['read', 'enumerate', 'write_open']);
    assert(row.observation.operations.every(operation => operation.succeeded === false && operation.errno === row.errno));
    assert.deepEqual(row.observation.operations.map(operation => operation.failure_class), Array(3).fill(row.expected));
  }
});

test('actual file timeout and I/O observations leave twelve campaign cells BLOCKED', async () => {
  for (const row of fileErrorObservations().filter(row => ['timeout', 'error', 'unavailable'].includes(row.expected))) {
    const fixture = passingFixture(), probe = fixture.runtimeProbe;
    fixture.runtimeProbe = async args => {
      const value = await probe(args);
      return args.spec.kind === 'file' && args.spec.expect === 'deny'
        ? {...value, ...structuredClone(row.observation)} : value;
    };
    const packet = await runSec01({...OPTIONS, fixture});
    assert.equal(packet.verdict, 'BLOCKED', String(row.errno));
    assert.equal(packet.results.filter(row => row.status === 'BLOCKED').length, 12);
    assert.equal(packet.results.filter(row => row.status === 'PASS').length, 18);
    for (const category of ['file', 'kubernetes']) assert.equal(packet.by_category[category].BLOCKED, 6);
    assert(packet.results.every(row => row.cleanup === 'PASS'));
  }
});
