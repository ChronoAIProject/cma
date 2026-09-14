import { test, expect, type Launch } from './harness';
import { faultNames } from './environment';

for (const name of faultNames) {
  test(`FAULT-${name} records the real fault and preserves command identity`, async ({
    harness: h,
  }) => {
    const page = await h.open('visitor');
    const control = { name, fixtureOwner: h.fixture.fixtureOwner };
    await h.operators.controlFault({ ...control, phase: 'arm' });
    try {
      const launch = await h.launchTarget(page, h.fixture.faults[name]);
      if (name === 'worker_crash') {
        expect(['failed_retryable', 'failed_terminal', 'cancelled']).not.toContain(launch.state);
      } else if (name === 'first_message_ambiguity') {
        await expect
          .poll(async () => (await h.readLaunch(page, launch.id)).first_message, {
            timeout: 180_000,
          })
          .toBe('recovery_required');
        const ambiguous = await h.readLaunch(page, launch.id);
        expect(ambiguous.resolution.agent.id).toBeTruthy();
        await page.goto(`${h.config.baseURL}/agents/${ambiguous.resolution.agent.id}`);
        await expect(page.getByText('First message needs recovery', { exact: true })).toBeVisible();
        await expect(
          page.getByText('Delivery was interrupted before execution could be confirmed.', {
            exact: true,
          }),
        ).toBeVisible();
      } else {
        await expect
          .poll(async () => (await h.readLaunch(page, launch.id)).failure?.code, {
            timeout: 180_000,
          })
          .toBe(h.fixture.faults[name].expectedCode);
        await expect(
          page.getByRole('button', { name: 'Copy support code', exact: true }),
        ).toBeVisible();
      }
      await h.capture(page, `fault-${name}`);
      const original = h.commands.filter((row) => row.path.endsWith('/launches'));
      await page.reload();
      const current = await h.readLaunch(page, launch.id);
      expect(current.id).toBe(launch.id);
      expect(h.commands.filter((row) => row.path.endsWith('/launches'))).toEqual(original);
      await h.operators.controlFault({ ...control, phase: 'release', launchId: launch.id });
      if (name === 'first_message_ambiguity') {
        await expect(page.getByText('First message needs recovery', { exact: true })).toBeVisible();
        await expect(page.getByRole('button', { name: 'Stop response', exact: true })).toHaveCount(
          0,
        );
        expect((await h.readLaunch(page, launch.id)).first_message).toBe('recovery_required');
        expect(
          h.commands.filter(
            (row) =>
              row.path.endsWith('/input') ||
              row.path.endsWith('/agui/run') ||
              /\/responses(?:\/|$)/.test(row.path),
          ),
        ).toHaveLength(0);
        await h.capture(page, `fault-${name}-reloaded`);
      } else if (name === 'worker_crash' || current.failure?.retryable) {
        const ready = await h.ready(page, launch.id);
        const events = await h.get<{ items: { kind: { type: string } }[] }>(
          page,
          `/api/v1/agents/${ready.resolution.agent.id}/events`,
        );
        expect(events.items.filter((row) => row.kind.type === 'owner_message')).toHaveLength(1);
      }
    } finally {
      await h.operators.controlFault({ ...control, phase: 'release' });
    }
  });
}

test('FAULT-nyxid-requirements upstream outage never invents satisfied requirements', async ({
  harness: h,
}) => {
  const page = await h.open('visitor');
  const control = { name: 'nyxid_unavailable' as const, fixtureOwner: h.fixture.fixtureOwner };
  await h.operators.controlFault({ ...control, phase: 'arm' });
  try {
    const target = h.fixture.triggers.missingService;
    await page.goto(`${h.config.baseURL}/t/${target.publicId}/${target.placementId}`);
    const result = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname.endsWith(`/public/${target.publicId}/launches`) &&
        response.request().method() === 'POST',
    );
    await page.locator('.public-actions button').click();
    const response = await result;
    if (response.ok()) {
      const launch = await h.currentLaunch(page);
      const requirements = await h.get<{ services: { state: string }[] }>(
        page,
        `/api/v1/launches/${launch.id}/requirements`,
      );
      expect(requirements.services.some((service) => service.state === 'unknown')).toBe(true);
      expect(['ready', 'completed', 'failed_terminal']).not.toContain(launch.state);
    } else {
      expect(response.status()).toBeGreaterThanOrEqual(500);
      await expect(page.locator('[role="alert"]')).toBeVisible();
    }
    await h.capture(page, 'fault-nyxid-requirements');
    expect(h.commands.filter((row) => row.path.endsWith('/start'))).toHaveLength(0);
  } finally {
    await h.operators.controlFault({ ...control, phase: 'release' });
  }
});

test('FAULT-browser-close durable Launch completes while its browser is closed', async ({
  harness: h,
}) => {
  const page = await h.open('visitor');
  const launch = await h.launch(page, 'fresh');
  await page.close();
  const returning = await h.open('visitor');
  const resumed = await h.ready(returning, launch.id);
  expect(resumed.id).toBe(launch.id);
  expect(h.commands.filter((row) => row.path.endsWith('/launches'))).toHaveLength(1);
  await h.capture(returning, 'fault-browser-return');
});

test('FAULT-popup-blocked connection falls back to the same tab without duplicate handoff', async ({
  harness: h,
}) => {
  const page = await h.open('visitor');
  await page.addInitScript(() => {
    window.open = () => null;
  });
  const launch = await h.launch(page, 'missingService');
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await expect(
    page.getByText(
      'Your browser blocked the NyxID window. Continue in this tab, then return here.',
    ),
  ).toBeVisible();
  await h.capture(page, 'fault-popup-blocked');
  await h.identity(async () => {
    await page.getByRole('link', { name: 'Continue in NyxID', exact: true }).click();
    await h.operators.completeConnection(page, 'oauth');
  });
  await page.goto(`${h.config.baseURL}/launches/${launch.id}`);
  await h.ready(page, launch.id);
  expect(h.commands.filter((row) => row.path === '/api/v1/service-connections')).toHaveLength(1);
});

test('FAULT-handoff-response lost browser response keeps the same unsettled handoff command', async ({
  harness: h,
}) => {
  const page = await h.open('visitor');
  const launch = await h.launch(page, 'missingService');
  let intercepted = false;
  await page.route('**/api/v1/service-connections', async (route) => {
    if (!intercepted && route.request().method() === 'POST') {
      intercepted = true;
      // A lost browser response is local transport-fault evidence, not an upstream outage gate.
      const response = await route.fetch();
      expect(response.ok()).toBe(true);
      await route.abort('connectionfailed');
    } else await route.fallback();
  });
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await expect(page.locator('.public-connection [role="alert"]')).toBeVisible();
  const before = h.commands.filter((row) => row.path === '/api/v1/service-connections');
  await h.identity(async () => {
    const popup = page.waitForEvent('popup');
    await page.getByRole('button', { name: 'Connect', exact: true }).click();
    await h.operators.completeConnection(await popup, 'oauth');
  });
  await h.ready(page, launch.id);
  const after = h.commands.filter((row) => row.path === '/api/v1/service-connections');
  expect(after.at(-1)).toEqual(before.at(-1));
  expect((await h.get<Launch>(page, `/api/v1/launches/${launch.id}`)).id).toBe(launch.id);
});

test('FAULT-nyxid real upstream unavailability preserves handoff command identity', async ({
  harness: h,
}) => {
  const page = await h.open('visitor');
  const launch = await h.launch(page, 'missingService');
  const control = {
    name: 'nyxid_unavailable' as const,
    fixtureOwner: h.fixture.fixtureOwner,
    launchId: launch.id,
  };
  await h.operators.controlFault({ ...control, phase: 'arm' });
  try {
    const result = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === '/api/v1/service-connections' &&
        response.request().method() === 'POST',
    );
    await page.getByRole('button', { name: 'Connect', exact: true }).click();
    expect((await result).status()).toBeGreaterThanOrEqual(500);
    await expect(page.locator('.public-connection [role="alert"]')).toBeVisible();
    const original = h.commands.filter((row) => row.path === '/api/v1/service-connections').at(-1);
    await h.capture(page, 'fault-nyxid-unavailable');
    await h.operators.controlFault({ ...control, phase: 'release' });
    await h.identity(async () => {
      const popup = page.waitForEvent('popup');
      await page.getByRole('button', { name: 'Connect', exact: true }).click();
      await h.operators.completeConnection(await popup, 'oauth');
    });
    await h.ready(page, launch.id);
    expect(h.commands.filter((row) => row.path === '/api/v1/service-connections').at(-1)).toEqual(
      original,
    );
  } finally {
    await h.operators.controlFault({ ...control, phase: 'release' });
  }
});
