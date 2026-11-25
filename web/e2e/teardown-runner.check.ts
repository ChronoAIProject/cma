import { spawn } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test, expect } from '@playwright/test';

for (const mode of ['complete', 'held']) {
  test(`LOCAL exported fixture fits consumed runner allocation: ${mode}`, async ({
    browserName,
  }, info) => {
    expect(browserName).toBe('chromium');
    const directory = mkdtempSync(join(tmpdir(), 'cma-teardown-runner-'));
    const receipt = join(directory, 'receipt.json');
    const config = join(directory, 'runner.config.ts');
    writeFileSync(
      config,
      `export default ${JSON.stringify({
        testDir: resolve('e2e'),
        testMatch: 'teardown-runner.fixture.ts',
        workers: 1,
        retries: 0,
        timeout: 2_500,
        reporter: [['list'], [resolve('e2e/reporter.ts')]],
        outputDir: join(directory, 'output'),
        use: { launchOptions: { executablePath: process.env.CMA_E2E_BROWSER_PATH } },
      })};`,
    );
    try {
      const child = spawn(
        process.execPath,
        ['node_modules/@playwright/test/cli.js', 'test', '--config', config],
        {
          env: {
            ...process.env,
            CMA_TEARDOWN_CONTROL_MODE: mode,
            CMA_TEARDOWN_CONTROL_RECEIPT: receipt,
          },
          stdio: ['ignore', 'pipe', 'pipe'],
        },
      );
      const deadline = setTimeout(() => child.kill('SIGKILL'), 15_000);
      child.once('close', () => clearTimeout(deadline));
      let output = '';
      child.stdout.on('data', (chunk: Buffer) => {
        output += chunk.toString();
      });
      child.stderr.on('data', (chunk: Buffer) => {
        output += chunk.toString();
      });
      const exit = await new Promise<number | null>((resolve, reject) => {
        child.once('error', reject);
        child.once('close', resolve);
      });
      writeFileSync(info.outputPath('child.log'), output);
      expect(exit, output).toBe(mode === 'held' ? 1 : 0);
      const result = JSON.parse(readFileSync(receipt, 'utf8')) as {
        closed: boolean;
        remaining: number;
        requests: string[];
        rows: { action: string }[];
      };
      expect(result.closed).toBe(true);
      expect(result.remaining).toBeGreaterThan(500);
      expect(result.remaining).toBeLessThan(1_800);
      if (mode === 'held') {
        expect(output).toContain('Scoped fixture cleanup is incomplete');
        expect(output).not.toContain('exceeded the test timeout');
        expect(output).toContain('Staging harness status: failed');
        expect(output).toContain('failed: J4-oauth LOCAL');
        expect(result.requests).toEqual([]);
        expect(result.rows.some((r) => r.action === 'cleanup_blocked')).toBe(true);
        expect(result.rows.some((r) => r.action === 'cleanup_complete')).toBe(false);
      } else {
        expect(output).toContain('Staging harness status: passed');
        expect(result.requests).toEqual(['GET', 'DELETE', 'GET']);
        expect(result.rows.map((r) => r.action)).toEqual([
          'created',
          'cleanup_accepted',
          'cleanup_complete',
        ]);
      }
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });
}
