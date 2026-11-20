// @vitest-environment node
import type { FullConfig } from '@playwright/test';
import { afterEach, expect, it, vi } from 'vitest';
import * as environment from './environment';
import * as fixtures from './visitor-abuse-environment';
import * as security from './security';
import { TC117Client } from './visitor-abuse-client';
import preflight from './visitor-abuse-preflight';
import { loadAndControls } from './visitor-abuse-load';

afterEach(() => vi.restoreAllMocks());

it('blocks remote execution before reading credentials or opening clients without authority integration', async () => {
  const config = { projects: [{ name: 'tc117', retries: 0, repeatEach: 1 }] } as FullConfig;
  const settings = vi.spyOn(environment, 'settings').mockReturnValue({
    mode: 'mutation',
    baseURL: 'https://staging.example.test',
    publicId: 'synthetic-trigger',
    output: '/tmp/cma-tc117-unused',
  });
  const fixture = vi.spyOn(fixtures, 'tc117Fixture').mockReturnValue({} as fixtures.TC117Fixture);
  const prepare = vi.spyOn(security, 'prepareEvidenceDirectory').mockReturnValue(() => {});
  const open = vi.spyOn(TC117Client, 'open').mockRejectedValue(new Error('Remote client opened'));
  await expect(preflight(config)).rejects.toThrow(/^BLOCKED: TC-117 requires authority/);
  expect(settings).not.toHaveBeenCalled();
  expect(fixture).not.toHaveBeenCalled();
  expect(prepare).not.toHaveBeenCalled();
  expect(open).not.toHaveBeenCalled();
});

it.each([
  { instances: [] },
  { instances: ['https://one.example.test', 'https://two.example.test'] },
])(
  'blocks missing instance origins or load budget before load or control operations',
  async ({ instances }) => {
    const proxyContexts = vi.fn();
    const launch = vi.fn();
    const driver = { fixture: { instances }, proxyContexts, launch };
    await expect(
      loadAndControls(
        driver as unknown as TC117Client,
        {} as Parameters<typeof loadAndControls>[1],
      ),
    ).rejects.toThrow(/^BLOCKED:/);
    expect(proxyContexts).not.toHaveBeenCalled();
    expect(launch).not.toHaveBeenCalled();
  },
);
