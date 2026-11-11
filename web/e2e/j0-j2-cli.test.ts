// @vitest-environment node
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { afterEach, expect, it } from 'vitest';

const temporary: string[] = [];
afterEach(() => {
  temporary.splice(0).forEach((path) => rmSync(path, { recursive: true, force: true }));
});

function run(overrides: string[], authenticatedPreflight: boolean) {
  const directory = mkdtempSync(join(tmpdir(), 'cma-j0-j2-cli-'));
  temporary.push(directory);
  const counter = join(directory, 'mutations.txt');
  const source = resolve('e2e');
  const playwright = resolve('node_modules/@playwright/test');
  const config = join(directory, 'playwright.config.ts');
  writeFileSync(
    join(directory, 'cases.pw.ts'),
    `import { test as base } from ${JSON.stringify(playwright)};
import { appendFileSync } from 'node:fs';
const test = base.extend({ marker: [async ({}, use) => {
  appendFileSync(${JSON.stringify(counter)}, 'fixture\\n');
  await use();
}, { auto: true }] });
for (const id of ['J0', 'J1', 'J2']) test(id + ' local counter', async () => {
  appendFileSync(${JSON.stringify(counter)}, 'body\\n');
});`,
  );
  writeFileSync(
    config,
    `import slice from ${JSON.stringify(join(source, 'j0-j2.config.ts'))};
import { rmSync } from 'node:fs';
rmSync(slice.outputDir, { recursive: true, force: true });
export default {
  ...slice,
  testDir: ${JSON.stringify(directory)},
  outputDir: ${JSON.stringify(join(directory, 'output'))},
  globalSetup: ${authenticatedPreflight ? JSON.stringify(join(source, 'j0-j2-preflight.ts')) : 'undefined'},
  reporter: [[${JSON.stringify(join(source, 'j0-j2-reporter.ts'))}]],
  projects: [{ ...slice.projects[0], testMatch: 'cases.pw.ts' }],
};`,
  );
  const env = Object.fromEntries(
    Object.entries(process.env).filter(
      ([key]) => !key.startsWith('CMA_E2E_') && key !== 'GITHUB_OUTPUT',
    ),
  );
  const result = spawnSync(
    process.execPath,
    [join(playwright, 'cli.js'), 'test', '--config', config, ...overrides],
    { env, encoding: 'utf8', timeout: 25_000 },
  );
  return {
    result,
    output: result.stdout + result.stderr,
    mutations: existsSync(counter) ? readFileSync(counter, 'utf8') : '',
  };
}

it('executes the synthetic counter fixture and all three bodies in the control run', () => {
  // Only this local control omits authenticated setup; no browser or remote API is used.
  const { result, mutations } = run([], false);
  expect(result.error).toBeUndefined();
  expect(result.status).toBe(0);
  expect(mutations.match(/fixture/g)).toHaveLength(3);
  expect(mutations.match(/body/g)).toHaveLength(3);
}, 30_000);

it.each([['--repeat-each=2'], ['--repeat-each', '2'], ['--retries=1']])(
  'refuses %s in real preflight before credentials, fixtures or test bodies',
  (...overrides) => {
    const { result, output, mutations } = run(overrides, true);
    expect(result.error).toBeUndefined();
    expect(result.status).toBe(1);
    expect(output).toContain('Invalid E2E J0-J2 retry/repetition configuration');
    expect(output).not.toContain('Required E2E J0-J2 mode');
    expect(output).not.toContain('Required E2E prerequisite');
    expect(mutations).toBe('');
  },
  30_000,
);
