import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { ensureVideoEncoder } from './install-contract-video.mjs';

test('a cached encoder is probed without downloading a browser or encoder', async () => {
  let probes = 0;
  await ensureVideoEncoder({
    exists: () => true,
    download: () => assert.fail('the cached encoder must be reused'),
    probe: async () => probes++,
  });
  assert.equal(probes, 1);
});

test('a missing encoder is installed and probed even when a browser is already available', async () => {
  let installed = false;
  let probes = 0;
  await ensureVideoEncoder({
    exists: () => installed,
    download: async () => {
      installed = true;
    },
    probe: async () => {
      assert.equal(installed, true);
      probes++;
    },
  });
  assert.equal(probes, 1);
});

test('missing, broken and failed downloads stop the gate', async () => {
  await assert.rejects(
    ensureVideoEncoder({
      exists: () => false,
      download: async () => {},
      probe: () => assert.fail('an absent encoder cannot be probed'),
    }),
    /absent after download/,
  );
  await assert.rejects(
    ensureVideoEncoder({
      exists: () => true,
      download: () => assert.fail('a broken cache must not silently switch binaries'),
      probe: async () => {
        throw new Error('encoder cannot execute');
      },
    }),
    /encoder cannot execute/,
  );
  await assert.rejects(
    ensureVideoEncoder({
      exists: () => false,
      download: async () => {
        throw new Error('download unavailable');
      },
      probe: () => assert.fail('a failed download cannot be probed'),
    }),
    /download unavailable/,
  );
});

test('Web CI prepares the encoder unconditionally before recorded design contracts', () => {
  const require = createRequire(new URL('../../web/package.json', import.meta.url));
  const yaml = require('js-yaml');
  const workflow = yaml.load(
    readFileSync(new URL('../workflows/ci-web.yml', import.meta.url), 'utf8'),
  );
  const steps = workflow.jobs.web.steps;
  const preparation = steps.filter((step) =>
    step.run?.includes('node ../.github/scripts/install-contract-video.mjs'),
  );
  assert.equal(preparation.length, 1);
  const step = preparation[0];
  assert.equal(step.if, undefined);
  assert.equal(step['continue-on-error'], undefined);
  assert.ok(step.run.includes('node --test ../.github/scripts/install-contract-video.test.mjs'));
  assert.ok(steps.indexOf(step) < steps.findIndex((entry) => entry.id === 'p1_design'));
});
