import {realpathSync, mkdtempSync, mkdirSync, writeFileSync, readFileSync, readdirSync, copyFileSync, chmodSync, rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {spawnSync, execFileSync} from 'node:child_process';

const scripts = new URL('.', import.meta.url);
export const prefix = 'store::sandbox_sessions::exclusive::service_tests::endpoint_tests::';
export const parents = [
  'public_disabled_services_and_ownerless_receiver_refuse_new_streams',
  'retained_legacy_rows_block_until_close_or_expiry_and_cannot_release_successor',
  'public_replica_handoff_and_revocation_use_actual_receiver',
  'public_uncertain_confirm_and_receiver_restart_preserve_fencing',
  'delayed_receiver_renewal_ack_cannot_restore_public_authority',
].map(name => prefix + name);
export const measurement = prefix + 'measurements::matched_public_desktop_connect_windows';
const child = 'link::desktop::tests::child_fixture::desktop_receiver_child';
export const boundary = 'authenticated public discovery through first returned synthetic echo frame';
const line = (name, value) => `${name}=${JSON.stringify(value)}\n`;

export function cleanupOutput(index = 1) {
  const id = `receiver-${String(index).padStart(32, '0')}`;
  return line('DESKTOP_FIXTURE_ROOT', {schema: 1, event: 'created', id}) +
    line('DESKTOP_FIXTURE_CHILD', {schema: 1, event: 'started', id, pid: 111111}) +
    line('DESKTOP_FIXTURE_CHILD', {schema: 1, event: 'exited', id, pid: 111111, expected: 'success', success: true, code: 0}) +
    line('DESKTOP_FIXTURE_ROOT', {schema: 1, event: 'removed', id}) +
    line('DESKTOP_FIXTURE_CLEANUP', {schema: 1, id, before: ['a'.repeat(64)], after: ['a'.repeat(64)], owned: ['b'.repeat(64), 'c'.repeat(64)], owned_absent: true});
}

export function parentOutput(name, index) {
  return cleanupOutput(index + 1) + (name === parents[1] ? cleanupOutput(6) : '');
}

export function measurementOutput(mode = 'one', diagnostics = false, mutate = () => {}) {
  const samples = [];
  const starts = [];
  for (let window = 0; window < 3; window++) {
    for (let ordinal = 0; ordinal < (diagnostics ? 15 : 105); ordinal++) {
      starts.push({mode, window, ordinal});
      samples.push({window, ordinal, warmup: ordinal < 5, cold: window === 0 && ordinal === 0,
        ingress: Number(mode === 'two' && ordinal % 2 === 1), milliseconds: 3.5,
        outcome: 'ok', websocket_refusal_status: null, ...(diagnostics ? {discovery_original_resolver: null, websocket_original_resolver: null, provider: {}} : {})});
    }
  }
  const summary = {schema: 1, mode, diagnostics, boundary, replicas: 'synthetic routers',
    receiver: 'actual receiver with synthetic display', provider: 'synthetic IdP', samples};
  mutate({samples, starts, summary});
  return starts.map((start, index) => line('DESKTOP_PUBLIC_START', start) +
    (samples[index] ? line('DESKTOP_PUBLIC_SAMPLE', samples[index]) : '')).join('') +
    line('DESKTOP_PUBLIC_MEASUREMENT', summary) + cleanupOutput();
}

function xmlEscape(value) {
  return value.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;').replaceAll('"', '&quot;');
}
export function report(cases) {
  return `<testsuites tests="${cases.length}" failures="0" errors="0" skipped="0"><testsuite name="cma::bin/cma" tests="${cases.length}" failures="0" errors="0" skipped="0">` + cases.map(({name, output, failure = ''}) =>
    `<testcase name="${name}" classname="cma::bin/cma">${failure}<system-out>${xmlEscape(output)}</system-out></testcase>`).join('') + '</testsuite></testsuites>';
}

export function fixture({mode = 'one', diagnostics = false, output = measurementOutput(mode, diagnostics), xml, fault = ''} = {}) {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'desktop-receipt-')));
  const root = join(directory, 'repo');
  const scriptDir = join(root, '.github', 'scripts');
  const api = join(root, 'api');
  const bin = join(directory, 'bin');
  mkdirSync(scriptDir, {recursive: true}); mkdirSync(api); mkdirSync(bin);
  for (const name of readdirSync(scripts)) {
    if (name === 'desktop-replica-contract.py' || name.startsWith('desktop_replica_') && name.endsWith('.py') || name === 'backend-nextest.sh') {
      copyFileSync(new URL(name, scripts), join(scriptDir, name));
    }
  }
  const target = join(api, 'target'); mkdirSync(target);
  const binaries = {cma: join(target, 'cma-fixture'), runtime: join(target, 'receiver-fixture')};
  for (const [name, path] of Object.entries(binaries)) { writeFileSync(path, `synthetic ${name} binary\n`); chmodSync(path, 0o755); }
  const names = mode === 'contracts' ? parents : [measurement];
  const suite = (id, pkg, path, tests) => ({'binary-id': id, 'binary-name': pkg === 'cma' ? 'cma' : 'layer4_runtime',
    'package-id': pkg, 'package-name': pkg, kind: 'bin', 'build-platform': 'target', 'binary-path': path,
    testcases: Object.fromEntries(tests.map(name => [name, {ignored: true, 'filter-match': {status: 'matches'}}]))});
  writeFileSync(join(directory, 'inventory.json'), JSON.stringify({'rust-build-meta': {'target-directory': target},
    'rust-suites': {cma: suite('cma::bin/cma', 'cma', binaries.cma, names), runtime: suite('layer4-runtime::bin/layer4-runtime', 'layer4-runtime', binaries.runtime, [child])}}));
  writeFileSync(join(directory, 'cargo-metadata.json'), JSON.stringify({workspace_root: api, target_directory: target, packages: [], workspace_members: []}));
  writeFileSync(join(directory, 'results.xml'), xml ?? report(mode === 'contracts' ? parents.map((name, i) => ({name, output: parentOutput(name, i)})) : [{name: measurement, output}]));
  writeFileSync(join(directory, 'output.txt'), output);
  writeFileSync(join(root, '.gitignore'), '/api/target/\n');
  writeFileSync(join(api, 'Cargo.toml'), '[workspace]\n');
  writeFileSync(join(api, 'Cargo.lock'), '# synthetic lock\n');
  writeFileSync(join(api, 'rust-toolchain.toml'), '[toolchain]\nchannel = "1.96.1"\n');
  const cargo = join(bin, 'cargo');
  writeFileSync(cargo, `#!/usr/bin/env python3
import json, os, pathlib, shutil, sys
root=pathlib.Path(os.environ['DESKTOP_TEST_FIXTURE'])
a=sys.argv[1:]
with (root/'calls.jsonl').open('a') as f:f.write(json.dumps(a)+'\\n')
if a==['--version']:print('cargo 1.96.1')
elif a==['nextest','--version']:print('cargo-nextest 0.9.151')
elif a[0]=='metadata':print((root/'cargo-metadata.json').read_text())
elif a[:2]==['nextest','list']:print((root/'inventory.json').read_text())
elif a[:2]==['nextest','run']:
    print((root/'output.txt').read_text(),end='')
    if '--config-file' in a:
        text=pathlib.Path(a[a.index('--config-file')+1]).read_text()
        path=json.loads(next(line.split('=',1)[1].strip() for line in text.splitlines() if line.strip().startswith('path =')))
        pathlib.Path(path).write_text((root/'results.xml').read_text().replace('111111',str(os.getpid())))
    fault=os.environ.get('DESKTOP_TEST_FAULT','')
    if fault=='binary' and '--binaries-metadata' in a:
        meta=json.loads(pathlib.Path(a[a.index('--binaries-metadata')+1]).read_text())
        path=pathlib.Path(meta['rust-binaries']['cma::bin/cma']['binary-path'])
        path.write_bytes(path.read_bytes()+b'changed')
    if fault=='archive':pathlib.Path(os.environ['DESKTOP_TEST_ARCHIVE']).write_bytes(b'changed archive')
    if fault=='source':(root/'repo/api/Cargo.toml').write_text('[workspace]\\n# drift\\n')
    if fault=='root':(pathlib.Path(os.environ['CMA_DESKTOP_RECEIVER_ROOT'])/'leftover').mkdir()
    if fault=='exit':sys.exit(42)
else:raise SystemExit('unexpected fake cargo arguments '+repr(a))
`);
  chmodSync(cargo, 0o755);
  writeFileSync(join(bin, 'rustc'), '#!/bin/sh\nprintf "rustc 1.96.1\\nhost: synthetic-target\\n"\n'); chmodSync(join(bin, 'rustc'), 0o755);
  for (const args of [['init', '-q'], ['add', '.'], ['-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.test', 'commit', '-qm', 'synthetic fixture']]) {
    execFileSync('git', args, {cwd: root});
  }
  const evidence = join(directory, 'evidence');
  return {directory, root, evidence, binaries,
    run(extra = [], overrides = {}) { return spawnSync('python3', [join(scriptDir, 'desktop-replica-contract.py'), '--evidence', evidence, '--mode', mode, ...(diagnostics ? ['--diagnostics'] : []), ...extra],
      {encoding: 'utf8', env: {...process.env, PATH: `${bin}:${process.env.PATH}`, CHRONO_TEST_MONGODB_URI: 'mongodb://127.0.0.1:27017/?replicaSet=fixture', DESKTOP_TEST_FIXTURE: directory, DESKTOP_TEST_FAULT: fault, CARGO_INCREMENTAL: '0', CMA_NEXTEST_REUSE: undefined, CI: 'false', PYTHONDONTWRITEBYTECODE: '', ...overrides}}); },
    read(name) { return readFileSync(join(evidence, name), 'utf8'); },
    close() { rmSync(directory, {recursive: true, force: true}); },
  };
}
