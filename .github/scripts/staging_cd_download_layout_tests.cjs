// Execute the pinned action's real selection/destination logic offline.
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const vm = require('node:vm');

const fixture = path.join(__dirname, 'fixtures/download-artifact');
const provenance = JSON.parse(fs.readFileSync(path.join(fixture, 'PROVENANCE.json')));
const source = fs.readFileSync(path.join(fixture, 'download-artifact.js'), 'utf8');
assert.equal(crypto.createHash('sha256').update(source).digest('hex'), provenance.fixture_sha256);
const config = JSON.parse(fs.readFileSync(0, 'utf8'));
assert.equal(config.action, `actions/download-artifact@${provenance.commit}`);
assert.equal(config.inputs['merge-multiple'], true);
const keys = {Name: 'name', Path: 'path', GitHubToken: 'github-token', Repository: 'repository',
  RunID: 'run-id', Pattern: 'pattern', MergeMultiple: 'merge-multiple', ArtifactIds: 'artifact-ids',
  SkipDecompress: 'skip-decompress', DigestMismatch: 'digest-mismatch'};
const DigestMismatchBehavior = {Ignore: 'ignore', Info: 'info', Warn: 'warn', Error: 'error'};

async function exercise(digestMismatch) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'cma-artifact-layout-'));
  try {
    const destination = config.inputs.path.replace('${{ runner.temp }}', directory);
    const received = [];
    const output = {};
    const receipt = Buffer.from('[{"synthetic":"receipt"}]\n');
    const selected = {id: 7, name: 'selected-component-receipt', size: receipt.length,
      digest: 'sha256:' + crypto.createHash('sha256').update(receipt).digest('hex')};
    const inputs = {...config.inputs, path: destination, 'artifact-ids': '7'};
    const artifactClient = {
      async listArtifacts() { return {artifacts: [selected, {id: 99, name: 'unrelated-artifact'}]}; },
      async downloadArtifact(id, options) {
        assert.equal(id, selected.id);
        assert.equal(options.expectedHash, selected.digest);
        assert.equal(options.skipDecompress, false);
        received.push(id);
        fs.mkdirSync(options.path, {recursive: true});
        fs.writeFileSync(path.join(options.path, 'receipts.json'), receipt);
        return {digestMismatch};
      },
      async getArtifact() { throw new Error('Name selection must not replace exact artifact ID'); }
    };
    const core = {
      getInput(key) { return inputs[key] || ''; },
      getBooleanInput(key) { return inputs[key] === true; },
      debug() {}, info() {}, warning() {}, setOutput(key, value) { output[key] = value; }
    };
    const context = {path, os, core, artifactClient, DigestMismatchBehavior, Inputs: keys,
      Outputs: {DownloadPath: 'download-path'}};
    vm.createContext(context);
    vm.runInContext(source, context, {timeout: 1000});
    if (digestMismatch) {
      await assert.rejects(context.run(), /Digest validation failed/);
      assert.deepEqual(output, {});
    } else {
      await context.run();
      assert.equal(output['download-path'], destination);
      const consumed = config.receiptPath.replace('${{ runner.temp }}', directory);
      assert.deepEqual(fs.readFileSync(consumed), receipt);
    }
    assert.deepEqual(received, [7]);
  } finally {
    fs.rmSync(directory, {recursive: true, force: true});
  }
}

(async () => {
  await exercise(false);
  await exercise(true);
  console.log('Pinned download action preserves receipt layout and rejects digest mismatches');
})().catch(() => { console.error('Pinned download action layout regression failed'); process.exitCode = 1; });
