import assert from 'node:assert/strict';
import test from 'node:test';
import { validateMetadata } from './release-runtime-input.mjs';

test('release input binds the exact component, commit, digest, archive, version and architecture', () => {
  const commit = 'a'.repeat(40), version = 'v1.2.3', repository = 'ghcr.io/chronoaiproject/cma-runtime';
  const value = {
    schemaVersion: 1, component: 'runtime', version, revision: commit.slice(0, 7), fullRevision: commit,
    sourceRepository: repository, versionReference: `${repository}:${version}`, revisionReference: `${repository}:${commit.slice(0, 7)}`,
    sourceDigest: `sha256:${'b'.repeat(64)}`, sourceReference: `${repository}@sha256:${'b'.repeat(64)}`,
    archive: `cma-runtime-${version}-${commit.slice(0, 7)}.oci.tar`, archiveReference: version,
    archiveSha256: `sha256:${'c'.repeat(64)}`, platforms: ['linux/amd64'],
  };
  assert.equal(validateMetadata(value, commit, version, repository), value.sourceReference);
  for (const key of Object.keys(value)) {
    const changed = structuredClone(value);
    changed[key] = 'wrong';
    assert.throws(() => validateMetadata(changed, commit, version, repository), key);
  }
  assert.throws(() => validateMetadata({ ...value, extra: true }, commit, version, repository));
});
