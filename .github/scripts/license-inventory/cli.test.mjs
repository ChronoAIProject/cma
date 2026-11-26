import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile, writeFile, mkdir, mkdtemp, rm } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { generate, main, runCommand, writeDrafts } from '../license-inventory.mjs';
import { NPM_LOCKS } from './collectors.mjs';
import { canonicalJson, sha256 } from './model.mjs';

const directory = dirname(fileURLToPath(import.meta.url));
const repository = join(directory, '../../..');
const config = '.github/scripts/license-inventory';

async function fixture(t) {
  await mkdir(join(repository, '.claude'), { recursive: true });
  const root = await mkdtemp(join(repository, '.claude/license-test-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const data = {
    'api/rust-toolchain.toml': '[toolchain]\nchannel = "1.2.3"\n',
    'api/Cargo.toml': '[workspace]\n', 'api/Cargo.lock': '# synthetic lock\n',
    'vendor/fixture/LICENSE': 'Synthetic notice, not a license grant',
    'vendor/fixture/code.js': 'synthetic source', 'other/NOTICE.txt': 'unmapped synthetic notice',
    [`${config}/policy.json`]: await readFile(join(directory, 'policy.json'), 'utf8'),
    [`${config}/vendors.json`]: JSON.stringify([{ name: 'fixture', version: '1.0.0', license: 'MIT',
      prefixes: ['vendor/fixture'], evidence: ['vendor/fixture/LICENSE'] }]),
  };
  for (const lock of NPM_LOCKS) data[lock] = JSON.stringify({ lockfileVersion: 3, packages: {
    '': { name: 'fixture-root' }, 'node_modules/fixture': { version: '1.0.0', license: 'EPL-2.0' },
  } });
  for (const [path, bytes] of Object.entries(data)) {
    await mkdir(dirname(join(root, path)), { recursive: true });
    await writeFile(join(root, path), bytes);
  }
  const external = join(root, 'cache/Cargo.toml');
  await mkdir(dirname(external));
  await writeFile(external, '[package]\nname="fixture-dependency"\nlicense="MIT"\n');
  const metadata = { version: 1, workspace_members: ['own'], resolve: { nodes: [] }, packages: [
    { id: 'own', name: 'fixture-own', version: '0.1.0', source: null, manifest_path: join(root, 'api/Cargo.toml'), license: null },
    { id: 'dep', name: 'fixture-dependency', version: '1.0.0', source: 'registry+fixture', manifest_path: external, license: 'MIT' },
  ] };
  const state = { files: Object.keys(data), version: 'cargo 1.2.3 (fixture)', metadata, calls: [] };
  const run = (command, args) => {
    state.calls.push([command, args]);
    if (command === 'git') return `${state.files.join('\0')}\0`;
    if (args[0] === '--version') return state.version;
    return JSON.stringify(state.metadata);
  };
  return { root, run, state };
}

test('end-to-end draft generation is deterministic and binds every lock', async (t) => {
  const setup = await fixture(t);
  const first = await generate({ ...setup, offline: true });
  const second = await generate(setup);
  assert.equal(canonicalJson(first), canonicalJson(second));
  assert.deepEqual(first.counts, { workspace: 1, cargo: 1, npm: 5, vendored: 1 });
  assert.deepEqual(first.unmappedNotices, ['other/NOTICE.txt']);
  assert.ok(NPM_LOCKS.every((path) => first.inputHashes.some((input) => input.path === path && input.sha256)));
  assert.ok(setup.state.calls.some(([, args]) => args.includes('--offline')));
  assert.ok(setup.state.calls.some(([, args]) => args.join(' ') === 'metadata --locked --all-features --format-version 1'));
  assert.ok(!canonicalJson(first).includes(setup.root));
  const output = join(setup.root, 'draft');
  await main(['--output', output, '--offline'], setup);
  assert.equal(await readFile(join(output, 'inventory.json'), 'utf8'), canonicalJson(first));
  assert.match(await readFile(join(output, 'THIRD_PARTY_NOTICES.md'), 'utf8'), /DRAFT/);
  assert.match(await readFile(join(output, 'COPYLEFT_REVIEW.md'), 'utf8'), /EPL-2.0/);
  await assert.rejects(writeDrafts(output, first), { code: 'EEXIST' });
});

test('generation refuses missing/additional lockfiles and malformed JSON', async (t) => {
  const setup = await fixture(t);
  await mkdir(join(setup.root, 'extra'), { recursive: true });
  await writeFile(join(setup.root, 'extra/package-lock.json'), '{}');
  setup.state.files.push('extra/package-lock.json');
  await assert.rejects(generate(setup), /lock set changed/);
  setup.state.files.pop(); setup.state.files.pop();
  await assert.rejects(generate(setup), /lock set changed/);
  setup.state.files.push(NPM_LOCKS.at(-1));
  await writeFile(join(setup.root, NPM_LOCKS[0]), '{');
  await assert.rejects(generate(setup), /invalid JSON/);
});

test('generation refuses wrong toolchain or failed Cargo without a partial report', async (t) => {
  const setup = await fixture(t);
  setup.state.version = 'cargo 9.9.9 (fixture)';
  await assert.rejects(generate(setup), /differs/);
  setup.state.version = 'cargo 1.2.3 (fixture)';
  await writeFile(join(setup.root, 'api/rust-toolchain.toml'), '[toolchain]\nchannel="stable"\n');
  await assert.rejects(generate(setup), /exact Rust toolchain pin/);
  await writeFile(join(setup.root, 'api/rust-toolchain.toml'), '[toolchain]\nchannel="1.2.3"\n');
  await assert.rejects(generate({ ...setup, run: (command, args) => {
    if (args[0] === 'metadata') throw new Error('synthetic Cargo failure');
    return setup.run(command, args);
  } }), /synthetic Cargo failure/);
});

test('tracked missing notice files fail instead of disappearing from the inventory', async (t) => {
  for (const path of ['other/NOTICE.txt', 'vendor/fixture/LICENSE', 'NOTICE']) {
    const setup = await fixture(t);
    if (path === 'NOTICE') setup.state.files.push(path);
    else await rm(join(setup.root, path));
    await assert.rejects(generate(setup), { code: 'ENOENT' });
  }
});

test('policy and catalog bytes are captured once and checked before returning', async (t) => {
  for (const filename of ['policy.json', 'vendors.json']) {
    const setup = await fixture(t);
    const target = join(setup.root, config, filename);
    const original = await readFile(target);
    let reads = 0;
    await assert.rejects(generate({ ...setup, readSource: async (path) => {
      if (path !== target) return readFile(path);
      reads++;
      return reads === 1 ? original : Buffer.concat([original, Buffer.from(' ')]);
    } }), /input changed during collection/);
    assert.equal(reads, 2);
  }
});

test('draft rendering uses the captured policy after source files change', async (t) => {
  const setup = await fixture(t);
  const reads = new Map();
  const first = await generate({ ...setup, readSource: async (path) => {
    reads.set(path, (reads.get(path) ?? 0) + 1);
    return readFile(path);
  } });
  for (const filename of ['policy.json', 'vendors.json']) {
    const path = `${config}/${filename}`;
    assert.equal(reads.get(join(setup.root, path)), 2);
    assert.equal(first.inputHashes.find((item) => item.path === path).sha256,
      sha256(await readFile(join(setup.root, path))));
    await writeFile(join(setup.root, path), '{}');
  }
  assert.match(first.policy.obligations['EPL-2.0'].summary, /covered source/);
  const output = join(setup.root, 'captured');
  await writeDrafts(output, first);
  assert.match(await readFile(join(output, 'COPYLEFT_REVIEW.md'), 'utf8'), /covered source/);
  assert.equal(await readFile(join(output, 'inventory.json'), 'utf8'), canonicalJson(first));
});

test('subprocess adapter fails closed without exposing command diagnostics', () => {
  assert.equal(runCommand(process.execPath, ['-p', '"synthetic output"'], repository).trim(), 'synthetic output');
  assert.throws(() => runCommand(process.execPath, ['-e', 'console.error("private diagnostic");process.exit(2)'], repository),
    (error) => error.message.includes('failed') && !error.message.includes('private diagnostic'));
  assert.throws(() => runCommand('nonexistent-fixture-command', [], repository), /failed/);
});

test('CLI rejects ambiguous flags, documents usage and returns failure status', async () => {
  await main(['--help']);
  for (const args of [[], ['--output'], ['--output', '--offline'], ['--output', 'x', '--offline', '--offline'], ['--unknown', 'x']]) {
    await assert.rejects(main(args), /expected --output/);
  }
  const result = spawnSync(process.execPath, [join(directory, '../license-inventory.mjs'), '--bad'], { encoding: 'utf8' });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /expected --output/);
});
