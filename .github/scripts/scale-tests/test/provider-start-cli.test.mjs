import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { cp, mkdir, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const source = resolve(dirname(fileURLToPath(import.meta.url)), '..');

async function fixture(t) {
  const scratch = await mkdtemp(join(tmpdir(), 'cma-provider-start-cli-'));
  t.after(() => rm(scratch, { recursive: true, force: true }));
  const repository = join(scratch, 'repository');
  const tool = join(repository, '.github/scripts/scale-tests');
  const bin = join(scratch, 'bin');
  await mkdir(tool, { recursive: true });
  await mkdir(join(repository, 'api'));
  await mkdir(bin);
  await cp(join(source, 'provider-start.mjs'), join(tool, 'provider-start.mjs'));
  await cp(join(source, 'lib'), join(tool, 'lib'), { recursive: true });
  await writeFile(join(bin, 'git'), '#!/usr/bin/env node\n'
    + 'if (process.argv[2] === "rev-parse") console.log("a".repeat(40));\n', { mode: 0o700 });
  const marker = join(scratch, 'build-attempt');
  const cargo = join(bin, 'cargo');
  await writeFile(cargo, '#!/usr/bin/env node\n'
    + 'require("node:fs").writeFileSync(process.env.BUILD_PROBE, "called");\n'
    + 'process.exit(17);\n', { mode: 0o700 });
  const run = (output) => spawnSync(process.execPath, [join(tool, 'provider-start.mjs'), 'run', 'smoke', output], {
    encoding: 'utf8', env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, CARGO: cargo,
      CARGO_TARGET_DIR: join(scratch, 'target'), BUILD_PROBE: marker },
  });
  return { scratch, repository, marker, run };
}
const absent = (path) => assert.rejects(readFile(path), { code: 'ENOENT' });

for (const name of ['evidence', '..evidence']) {
  test(`CLI rejects repository child ${name} before creating it or invoking Cargo`, async (t) => {
    const f = await fixture(t);
    const output = join(f.repository, name);
    const result = f.run(output);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /evidence_must_be_outside_repository/);
    await absent(output);
    await absent(f.marker);
  });
}

test('CLI accepts a true sibling evidence directory and reaches the build', async (t) => {
  const f = await fixture(t);
  const result = f.run(join(f.scratch, 'outside', 'nested'));
  assert.equal(result.status, 1);
  assert.doesNotMatch(result.stderr, /evidence_must_be_outside_repository/);
  assert.equal(await readFile(f.marker, 'utf8'), 'called');
  assert.equal(await readFile(join(f.scratch, 'outside/nested/build.log'), 'utf8'), '');
});

test('CLI rejects an outside symlink resolving inside the repository', async (t) => {
  const f = await fixture(t);
  const link = join(f.scratch, 'outside-link');
  await symlink(f.repository, link, 'dir');
  const result = f.run(join(link, 'evidence'));
  assert.equal(result.status, 1);
  assert.match(result.stderr, /evidence_must_be_outside_repository/);
  await absent(join(f.repository, 'evidence'));
  await absent(f.marker);
});
