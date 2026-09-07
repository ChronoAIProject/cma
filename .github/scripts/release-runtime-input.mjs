import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createReadStream, lstatSync, readFileSync, readdirSync, appendFileSync } from 'node:fs';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

export function validateMetadata(value, commit, version, repository) {
  assert.match(commit, /^[0-9a-f]{40}$/);
  assert.match(version, /^v[0-9]+\.[0-9]+\.[0-9]+$/);
  assert.equal(repository, 'ghcr.io/chronoaiproject/cma-runtime');
  assert.deepEqual(Object.keys(value).sort(), ['archive', 'archiveReference', 'archiveSha256', 'component', 'fullRevision', 'platforms', 'revision', 'revisionReference', 'schemaVersion', 'sourceDigest', 'sourceReference', 'sourceRepository', 'version', 'versionReference'].sort());
  assert.equal(value.schemaVersion, 1);
  assert.equal(value.component, 'runtime');
  assert.equal(value.fullRevision, commit);
  assert.equal(value.revision, commit.slice(0, 7));
  assert.equal(value.version, version);
  assert.equal(value.sourceRepository, repository);
  assert.equal(value.versionReference, `${repository}:${version}`);
  assert.equal(value.revisionReference, `${repository}:${commit.slice(0, 7)}`);
  assert.equal(value.archive, `cma-runtime-${version}-${commit.slice(0, 7)}.oci.tar`);
  assert.equal(value.archiveReference, version);
  assert.match(value.sourceDigest, /^sha256:[0-9a-f]{64}$/);
  assert.match(value.archiveSha256, /^sha256:[0-9a-f]{64}$/);
  assert.equal(value.sourceReference, `${repository}@${value.sourceDigest}`);
  assert.deepEqual(value.platforms, ['linux/amd64']);
  return value.sourceReference;
}

async function verify(directory, commit, version) {
  const basename = `cma-runtime-${version}-${commit.slice(0, 7)}`;
  assert.deepEqual(readdirSync(directory).sort(), [`${basename}.json`, `${basename}.oci.tar`]);
  for (const name of readdirSync(directory)) {
    const stat = lstatSync(join(directory, name));
    assert.ok(stat.isFile() && !stat.isSymbolicLink() && stat.size > 0);
  }
  const value = JSON.parse(readFileSync(join(directory, `${basename}.json`), 'utf8'));
  const reference = validateMetadata(value, commit, version, 'ghcr.io/chronoaiproject/cma-runtime');
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(join(directory, value.archive))) hash.update(chunk);
  assert.equal(`sha256:${hash.digest('hex')}`, value.archiveSha256);
  assert.ok(process.env.GITHUB_OUTPUT);
  appendFileSync(process.env.GITHUB_OUTPUT, `reference=${reference}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try { assert.equal(process.argv.length, 5); await verify(...process.argv.slice(2)); }
  catch { process.stderr.write('Release runtime artifact provenance refused\n'); process.exitCode = 1; }
}
