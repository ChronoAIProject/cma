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
const keys = {Name: 'name', Path: 'path', GitHubToken: 'github-token', Repository: 'repository',
  RunID: 'run-id', Pattern: 'pattern', MergeMultiple: 'merge-multiple', ArtifactIds: 'artifact-ids'};

async function exercise(merge, expectedAtRoot) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'cma-artifact-layout-'));
  try {
    const destination = config.inputs.path.replace('${{ runner.temp }}', directory);
    const received = [];
    const receipt = Buffer.from('[{"synthetic":"receipt"}]\n');
    const selected = {id: 7, name: 'selected-component-receipt', size: receipt.length,
      digest: 'sha256:' + crypto.createHash('sha256').update(receipt).digest('hex')};
    const inputs = {...config.inputs, path: destination, 'artifact-ids': '7', 'merge-multiple': merge};
    const artifactClient = {
      async listArtifacts() { return {artifacts: [selected, {id: 99, name: 'unrelated-artifact'}]}; },
      async downloadArtifact(id, options) {
        assert.equal(id, selected.id);
        assert.equal(options.expectedHash, selected.digest);
        received.push(id);
        fs.mkdirSync(options.path, {recursive: true});
        fs.writeFileSync(path.join(options.path, 'receipts.json'), receipt);
        return {digestMismatch: false};
      },
      async getArtifact() { throw new Error('Name selection must not replace exact artifact ID'); }
    };
    const core = {
      getInput(key) { return inputs[key] || ''; },
      getBooleanInput(key) { return inputs[key] === true; },
      debug() {}, info() {}, warning() {}, setOutput() {}
    };
    const context = {path, os, core, artifactClient, Inputs: keys, Outputs: {DownloadPath: 'download-path'}};
    vm.createContext(context);
    vm.runInContext(source, context, {timeout: 1000});
    await context.run();
    assert.deepEqual(received, [7]);
    const consumed = config.receiptPath.replace('${{ runner.temp }}', directory);
    assert.equal(fs.existsSync(consumed), expectedAtRoot);
    const actual = expectedAtRoot ? consumed : path.join(destination, selected.name, 'receipts.json');
    assert.deepEqual(fs.readFileSync(actual), receipt);
  } finally {
    fs.rmSync(directory, {recursive: true, force: true});
  }
}

(async () => {
  await exercise(false, false); // Reproduce the published workflow's failure.
  await exercise(config.inputs['merge-multiple'], true); // Exercise the actual successor configuration.
  console.log('Pinned download action layout: before misses, corrected matches, unrelated artifact excluded');
})().catch(() => { console.error('Pinned download action layout regression failed'); process.exitCode = 1; });
